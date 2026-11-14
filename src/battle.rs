//! Battle: aim + charge the launcher, release both tops, let the physics play
//! out, detect ring-outs and spin-outs.

use crate::parts::Build;
use crate::parts::Ability;
use crate::stadium::{spawn_stadium, surface_height};
use crate::top::{
    check_finish, launch, spawn_top, Finish, Owner, RingCollider, SpinMeter, SteerState, Top,
    Visuals,
};
use crate::beast::{Beast, BeastKind, FightLive, HudBeastText, HudEnergy};
use crate::blade_hits::ImpactScale;
use crate::{AppState, Loadout, OpponentMode, OrbitCam, UiFont};
use avian3d::prelude::*;
use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use rand::Rng;

pub struct BattlePlugin;

impl Plugin for BattlePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Battle), (enter, start_round).chain())
            .add_systems(
                Update,
                (
                    camera_controls,
                    aim,
                    judge,
                    sparks,
                    update_sparks,
                    hud,
                    round_over_input,
                    sync_live,
                )
                    .chain()
                    .run_if(in_state(AppState::Battle)),
            )
            .add_systems(
                FixedUpdate,
                stagger_on_hits
                    .after(crate::blade_hits::blade_impacts)
                    .run_if(in_state(AppState::Battle)),
            );
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Phase {
    Aim,
    Fight,
    Over,
}

#[derive(Resource)]
struct Round {
    phase: Phase,
    player_build: Build,
    ai_build: Build,
    player_beast: BeastKind,
    ai_beast: BeastKind,
    /// Launch position angle around the stadium.
    aim_angle: f32,
    /// -1..1: launch direction from "towards centre" to "along the rim".
    aim_side: f32,
    charging: bool,
    meter: f32,
    meter_up: bool,
    time: f32,
    result: String,
    player_top: Option<Entity>,
    ai_top: Option<Entity>,
    ai_aim_side: f32,
    hits: u32,
    /// Match points, indexed by `Owner as usize`. First to [`WIN_POINTS`] wins.
    score: [u32; 2],
}

/// Points needed to win the match (ring-out 2, spin finish 1).
const WIN_POINTS: u32 = 4;

#[derive(Component)]
struct Spark {
    vel: Vec3,
    life: f32,
}

#[derive(Component)]
struct HudSpin(Owner);
#[derive(Component)]
struct HudBar(Owner);
#[derive(Component)]
struct HudCenter;
#[derive(Component)]
struct HudCenterBox;
#[derive(Component)]
struct HudTimer;
#[derive(Component)]
struct HudHelp;
#[derive(Component)]
struct HudMeter;
#[derive(Component)]
struct HudMeterBox;
#[derive(Component)]
struct AimArrow;

#[derive(Resource)]
struct SparkAssets {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
    /// Sparks of the blade abilities: absorb, rebound, cushion.
    special: [Handle<StandardMaterial>; 3],
}

impl SparkAssets {
    fn for_ability(&self, ability: Ability) -> Handle<StandardMaterial> {
        match ability {
            Ability::None => self.material.clone(),
            Ability::Absorb => self.special[0].clone(),
            Ability::Rebound => self.special[1].clone(),
            Ability::Cushion => self.special[2].clone(),
        }
    }
}

const LAUNCH_RADIUS: f32 = 2.4;
const MAX_SPIN: f32 = 200.0;
/// Launch spin at zero charge (a weak launch still makes a usable top).
const MIN_SPIN: f32 = 110.0;
const ROUND_TIME: f32 = 180.0;

fn enter(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    font: Res<UiFont>,
    loadout: Res<Loadout>,
    mut cam: ResMut<OrbitCam>,
) {
    *cam = OrbitCam::new(Vec3::new(0.0, 0.3, 0.0), 0.0, 0.85, 8.5);
    let scope = DespawnOnExit(AppState::Battle);
    spawn_stadium(&mut commands, &mut meshes, &mut materials, scope.clone());

    let special = [Ability::Absorb, Ability::Rebound, Ability::Cushion].map(|a| {
        let c = a.color().unwrap_or(Color::WHITE).to_linear();
        materials.add(StandardMaterial {
            base_color: a.color().unwrap_or(Color::WHITE),
            emissive: LinearRgba::rgb(c.red * 30.0, c.green * 30.0, c.blue * 30.0),
            unlit: true,
            ..default()
        })
    });
    commands.insert_resource(SparkAssets {
        mesh: meshes.add(Sphere::new(0.025).mesh().ico(1).unwrap()),
        material: materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.8, 0.3),
            emissive: LinearRgba::rgb(30.0, 18.0, 4.0),
            unlit: true,
            ..default()
        }),
        special,
    });

    // Aim arrow (shown during the aim phase).
    commands.spawn((
        AimArrow,
        scope.clone(),
        Mesh3d(meshes.add(Cuboid::new(0.08, 0.03, 1.2))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Owner::Player.accent(),
            emissive: LinearRgba::rgb(4.0, 1.5, 0.0),
            ..default()
        })),
        Transform::default(),
    ));

    let mut rng = rand::rng();
    let ai_build = match loadout.opponent {
        OpponentMode::Random => Build::random(&mut rng),
        OpponentMode::Mirror => Build { dir: loadout.player.dir, ..loadout.player },
    };
    let ai_beast = BeastKind::random(&mut rng);
    commands.insert_resource(Round {
        phase: Phase::Aim,
        player_build: loadout.player,
        ai_build,
        player_beast: loadout.beast,
        ai_beast,
        aim_angle: 0.0,
        aim_side: 0.3,
        charging: false,
        meter: 0.0,
        meter_up: true,
        time: 0.0,
        result: String::new(),
        player_top: None,
        ai_top: None,
        ai_aim_side: 0.0,
        hits: 0,
        score: [0, 0],
    });

    spawn_hud(&mut commands, &font.0, &loadout.player, &ai_build);
}

fn text(font: &Handle<Font>, s: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (Text::new(s), TextFont { font: font.clone(), font_size: size, ..default() }, TextColor(color))
}

fn build_line(b: &Build) -> String {
    format!("{} / {} / {} / {}", b.ring.name(), b.disk.name(), b.tip.name(), b.dir.name())
}

fn spawn_hud(commands: &mut Commands, font: &Handle<Font>, player: &Build, ai: &Build) {
    let scope = DespawnOnExit(AppState::Battle);
    commands
        .spawn((
            scope,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::SpaceBetween,
                padding: UiRect::all(Val::Px(18.0)),
                ..default()
            },
        ))
        .with_children(|root| {
            // Top row: two spin panels.
            root.spawn(Node { justify_content: JustifyContent::SpaceBetween, ..default() })
                .with_children(|row| {
                    for (i, (owner, build)) in
                        [(Owner::Player, player), (Owner::Ai, ai)].into_iter().enumerate()
                    {
                        if i == 1 {
                            row.spawn((
                                HudTimer,
                                text(font, "", 36.0, Color::WHITE),
                                TextLayout::new_with_justify(Justify::Center),
                                TextShadow::default(),
                            ));
                        }
                        row.spawn((
                            Node {
                                flex_direction: FlexDirection::Column,
                                row_gap: Val::Px(6.0),
                                padding: UiRect::all(Val::Px(12.0)),
                                width: Val::Px(430.0),
                                border_radius: BorderRadius::all(Val::Px(8.0)),
                                ..default()
                            },
                            BackgroundColor(Color::srgba(0.05, 0.06, 0.1, 0.75)),
                        ))
                        .with_children(|p| {
                            p.spawn(text(font, owner.name(), 22.0, owner.accent()));
                            p.spawn(text(font, build_line(build), 14.0, Color::srgb(0.75, 0.77, 0.85)));
                            p.spawn((
                                Node { width: Val::Percent(100.0), height: Val::Px(12.0), ..default() },
                                BackgroundColor(Color::srgb(0.18, 0.19, 0.26)),
                            ))
                            .with_children(|bar| {
                                bar.spawn((
                                    HudBar(owner),
                                    Node { width: Val::Percent(0.0), height: Val::Percent(100.0), ..default() },
                                    BackgroundColor(owner.accent()),
                                ));
                            });
                            p.spawn((HudSpin(owner), text(font, "待发射", 16.0, Color::WHITE)));
                            p.spawn((HudBeastText(owner), text(font, "", 15.0, Color::srgb(0.9, 0.9, 1.0))));
                            p.spawn((
                                Node { width: Val::Percent(100.0), height: Val::Px(8.0), ..default() },
                                BackgroundColor(Color::srgb(0.15, 0.15, 0.22)),
                            ))
                            .with_children(|bar| {
                                bar.spawn((
                                    HudEnergy(owner),
                                    Node { width: Val::Percent(0.0), height: Val::Percent(100.0), ..default() },
                                    BackgroundColor(Color::WHITE),
                                ));
                            });
                        });
                    }
                });

            // Centre message.
            root.spawn(Node { justify_content: JustifyContent::Center, ..default() })
                .with_children(|c| {
                    c.spawn((
                        HudCenterBox,
                        Node {
                            padding: UiRect::axes(Val::Px(36.0), Val::Px(18.0)),
                            border_radius: BorderRadius::all(Val::Px(12.0)),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.03, 0.04, 0.08, 0.8)),
                        Visibility::Hidden,
                    ))
                    .with_children(|b| {
                        b.spawn((
                            HudCenter,
                            text(font, "", 52.0, Color::WHITE),
                            TextLayout::new_with_justify(Justify::Center),
                        ));
                    });
                });

            // Bottom: launch meter + help.
            root.spawn(Node {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: Val::Px(8.0),
                ..default()
            })
            .with_children(|b| {
                b.spawn((
                    HudMeterBox,
                    Node { width: Val::Px(420.0), height: Val::Px(22.0), ..default() },
                    BackgroundColor(Color::srgb(0.15, 0.16, 0.22)),
                ))
                .with_children(|m| {
                    m.spawn((
                        HudMeter,
                        Node { width: Val::Percent(0.0), height: Val::Percent(100.0), ..default() },
                        BackgroundColor(Color::srgb(1.0, 0.6, 0.1)),
                    ));
                });
                b.spawn((
                    Node {
                        padding: UiRect::axes(Val::Px(16.0), Val::Px(8.0)),
                        border_radius: BorderRadius::all(Val::Px(8.0)),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.03, 0.04, 0.08, 0.75)),
                ))
                .with_children(|h| {
                    h.spawn((
                        HudHelp,
                        text(font, "", 16.0, Color::srgb(0.88, 0.9, 0.96)),
                        TextLayout::new_with_justify(Justify::Center),
                    ));
                });
            });
        });
}

fn launch_transform(angle: f32) -> Transform {
    let p = Vec3::new(LAUNCH_RADIUS * angle.sin(), 0.0, LAUNCH_RADIUS * angle.cos());
    let y = surface_height(LAUNCH_RADIUS, p.z.atan2(p.x)) + 0.5;
    Transform::from_xyz(p.x, y, p.z)
}

fn launch_velocity(angle: f32, side: f32, power: f32) -> Vec3 {
    let radial_in = -Vec3::new(angle.sin(), 0.0, angle.cos());
    let tangent = Vec3::new(angle.cos(), 0.0, -angle.sin());
    (radial_in * (1.0 - side.abs()) + tangent * side).normalize() * (1.5 + 3.0 * power)
}

/// (Re)spawn both tops on the launchers.
fn start_round(
    mut commands: Commands,
    mut round: ResMut<Round>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut slow: ResMut<crate::slowmo::SlowMo>,
    mut vtime: ResMut<Time<Virtual>>,
) {
    slow.reset(&mut vtime);
    for e in [round.player_top.take(), round.ai_top.take()].into_iter().flatten() {
        commands.entity(e).despawn();
    }
    let mut rng = rand::rng();
    round.phase = Phase::Aim;
    round.time = 0.0;
    round.meter = 0.0;
    round.charging = false;
    round.hits = 0;
    if round.score.iter().any(|s| *s >= WIN_POINTS) {
        round.score = [0, 0];
    }
    round.ai_aim_side = rng.random_range(-0.6..0.6);
    let p = spawn_top(
        &mut commands,
        round.player_build,
        Owner::Player,
        launch_transform(round.aim_angle),
        Some(Visuals { meshes: &mut meshes, materials: &mut materials }),
    );
    let a = spawn_top(
        &mut commands,
        round.ai_build,
        Owner::Ai,
        launch_transform(round.aim_angle + std::f32::consts::PI),
        Some(Visuals { meshes: &mut meshes, materials: &mut materials }),
    );
    commands.entity(p).insert((
        DespawnOnExit(AppState::Battle),
        Beast::new(round.player_beast),
        ImpactScale::default(),
    ));
    commands.entity(a).insert((
        DespawnOnExit(AppState::Battle),
        Beast::new(round.ai_beast),
        ImpactScale::default(),
    ));
    round.player_top = Some(p);
    round.ai_top = Some(a);
}

fn camera_controls(
    keys: Res<ButtonInput<KeyCode>>,
    mut wheel: MessageReader<MouseWheel>,
    mut cam: ResMut<OrbitCam>,
    time: Res<Time<Real>>,
) {
    let dt = time.delta_secs();
    if keys.pressed(KeyCode::KeyQ) {
        cam.yaw -= 1.5 * dt;
    }
    if keys.pressed(KeyCode::KeyE) {
        cam.yaw += 1.5 * dt;
    }
    for ev in wheel.read() {
        cam.dist = (cam.dist - ev.y * 0.6).clamp(4.0, 18.0);
    }
}

/// Aim phase: move the launcher, charge with Space, release to launch.
fn aim(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut round: ResMut<Round>,
    time: Res<Time>,
    mut tops: Query<&mut Transform, (With<Top>, Without<AimArrow>)>,
    mut arrow: Query<(&mut Transform, &mut Visibility), With<AimArrow>>,
    mut cam: ResMut<OrbitCam>,
) {
    let dt = time.delta_secs();
    let Ok((mut arrow_tf, mut arrow_vis)) = arrow.single_mut() else { return };
    if round.phase != Phase::Aim {
        *arrow_vis = Visibility::Hidden;
        return;
    }
    *arrow_vis = Visibility::Visible;
    if keys.pressed(KeyCode::KeyA) {
        round.aim_angle -= 1.2 * dt;
    }
    if keys.pressed(KeyCode::KeyD) {
        round.aim_angle += 1.2 * dt;
    }
    if keys.pressed(KeyCode::KeyW) {
        round.aim_side = (round.aim_side + 1.2 * dt).min(0.95);
    }
    if keys.pressed(KeyCode::KeyS) {
        round.aim_side = (round.aim_side - 1.2 * dt).max(-0.95);
    }
    // Camera sits behind the player's launcher.
    cam.yaw = round.aim_angle;

    let (Some(p), Some(a)) = (round.player_top, round.ai_top) else { return };
    let ptf = launch_transform(round.aim_angle);
    let atf = launch_transform(round.aim_angle + std::f32::consts::PI);
    let spin_vis = time.elapsed_secs() * 6.0;
    if let Ok(mut tf) = tops.get_mut(p) {
        *tf = ptf.with_rotation(Quat::from_rotation_y(spin_vis * round.player_build.dir.sign()));
    }
    if let Ok(mut tf) = tops.get_mut(a) {
        *tf = atf.with_rotation(Quat::from_rotation_y(spin_vis * round.ai_build.dir.sign()));
    }
    let dir = launch_velocity(round.aim_angle, round.aim_side, 0.5).normalize();
    *arrow_tf = Transform::from_translation(ptf.translation + dir * 0.9 - Vec3::Y * 0.3)
        .looking_to(dir, Vec3::Y);

    if keys.just_pressed(KeyCode::Space) {
        round.charging = true;
        round.meter = 0.0;
        round.meter_up = true;
    }
    if round.charging {
        let speed = 1.4;
        if round.meter_up {
            round.meter += speed * dt;
            if round.meter >= 1.0 {
                round.meter = 1.0;
                round.meter_up = false;
            }
        } else {
            round.meter -= speed * dt;
            if round.meter <= 0.0 {
                round.meter = 0.0;
                round.meter_up = true;
            }
        }
    }
    if round.charging && keys.just_released(KeyCode::Space) {
        let mut rng = rand::rng();
        let power = round.meter;
        // Player.
        let tilt = Quat::from_rotation_x(rng.random_range(-0.04..0.04))
            * Quat::from_rotation_z(rng.random_range(-0.04..0.04));
        let spin = (MIN_SPIN + (MAX_SPIN - MIN_SPIN) * power) * round.player_build.dir.sign();
        launch(&mut commands, p, tilt, spin, launch_velocity(round.aim_angle, round.aim_side, power));
        // AI.
        let ai_power = rng.random_range(0.6..0.97);
        let ai_spin = (MIN_SPIN + (MAX_SPIN - MIN_SPIN) * ai_power) * round.ai_build.dir.sign();
        let ai_angle = round.aim_angle + std::f32::consts::PI;
        let tilt = Quat::from_rotation_x(rng.random_range(-0.04..0.04))
            * Quat::from_rotation_z(rng.random_range(-0.04..0.04));
        launch(&mut commands, a, tilt, ai_spin, launch_velocity(ai_angle, round.ai_aim_side, ai_power));
        round.charging = false;
        round.phase = Phase::Fight;
    }
}

/// Game seconds a top reels after being hit.
const STAGGER: f32 = 0.8;

/// Stagger both tops whenever they hit each other (blade hit or body
/// contact): the tip drive backs off so the knock-back plays out.
pub(crate) fn stagger_on_hits(
    time: Res<Time>,
    mut hits: MessageReader<crate::blade_hits::BladeHit>,
    graph: Res<ContactGraph>,
    mut tops: Query<(Entity, &mut SteerState), With<Top>>,
) {
    let dt = time.delta_secs();
    let mut struck: Vec<Entity> = hits.read().flat_map(|h| [h.a, h.b]).collect();
    for pair in graph.iter_active_touching() {
        let (Some(b1), Some(b2)) = (pair.body1, pair.body2) else { continue };
        if b1 != b2 && tops.contains(b1) && tops.contains(b2) {
            struck.extend([b1, b2]);
        }
    }
    for (e, mut state) in &mut tops {
        state.stun = if struck.contains(&e) { STAGGER } else { (state.stun - dt).max(0.0) };
    }
}

fn judge(
    mut round: ResMut<Round>,
    time: Res<Time>,
    tops: Query<(&Top, &Position, Option<&SpinMeter>)>,
) {
    if round.phase != Phase::Fight {
        return;
    }
    round.time += time.delta_secs();
    // Give the tops a moment to land before judging.
    if round.time < 1.0 {
        return;
    }
    let mut results: Vec<(Owner, Option<Finish>, f32)> = Vec::new();
    for (top, pos, meter) in &tops {
        let Some(meter) = meter else { continue };
        results.push((top.owner, check_finish(pos.0, meter), meter.smoothed));
    }
    let lost: Vec<_> = results.iter().filter(|r| r.1.is_some()).collect();
    let time_up = round.time >= ROUND_TIME;
    if lost.is_empty() && !time_up {
        return;
    }
    round.phase = Phase::Over;
    let mut result = if lost.len() >= 2 {
        "平局！双方同时倒下".to_string()
    } else if let Some((owner, Some(finish), _)) = lost.first().copied().copied() {
        let winner = if owner == Owner::Player { Owner::Ai } else { Owner::Player };
        round.score[winner as usize] += finish.points();
        let headline = if winner == Owner::Player { "得分！" } else { "失分…" };
        let how = match finish {
            Finish::RingOut => "被撞出界",
            Finish::SpinOut => "停止旋转",
        };
        format!("{headline}\n{}{how} — {} +{} 分", owner.name(), finish.name(), finish.points())
    } else {
        // Time up: higher spin takes one point.
        let p = results.iter().find(|r| r.0 == Owner::Player).map(|r| r.2).unwrap_or(0.0);
        let a = results.iter().find(|r| r.0 == Owner::Ai).map(|r| r.2).unwrap_or(0.0);
        let winner = if p > a { Owner::Player } else { Owner::Ai };
        round.score[winner as usize] += 1;
        if p > a { "时间到 — 转速领先 +1 分".into() } else { "时间到 — 转速落后，对手 +1 分".into() }
    };
    let [p, a] = round.score;
    if p >= WIN_POINTS || a >= WIN_POINTS {
        let verdict = if p > a { "你赢得比赛！" } else { "电脑赢得比赛…" };
        result += &format!("\n{p} : {a}  {verdict}");
    }
    round.result = result;
}

/// Sparks + camera shake when two attack rings collide.
fn sparks(
    mut commands: Commands,
    mut events: MessageReader<CollisionStart>,
    mut blade_hits: MessageReader<crate::blade_hits::BladeHit>,
    rings: Query<&ColliderOf, With<RingCollider>>,
    bodies: Query<(&Position, &LinearVelocity), With<Top>>,
    assets: Res<SparkAssets>,
    mut cam: ResMut<OrbitCam>,
    mut round: ResMut<Round>,
) {
    let mut rng = rand::rng();
    // Blade-on-blade: sparks fly from the exact contact point, along the hit.
    for hit in blade_hits.read() {
        round.hits += 1;
        let strength = hit.impulse * 6.0;
        cam.shake = (cam.shake + strength * 0.12).min(1.0);
        let n = (8.0 + strength * 6.0).min(48.0) as usize;
        for _ in 0..n {
            let side = if rng.random_bool(0.5) { 1.0 } else { -1.0 };
            let spread = Vec3::new(
                rng.random_range(-0.6..0.6),
                rng.random_range(0.1..1.2),
                rng.random_range(-0.6..0.6),
            );
            let v = (hit.dir * side + spread) * (2.0 + strength);
            commands.spawn((
                Spark { vel: v, life: rng.random_range(0.15..0.45) },
                Mesh3d(assets.mesh.clone()),
                MeshMaterial3d(assets.for_ability(hit.ability)),
                Transform::from_translation(hit.point),
                DespawnOnExit(AppState::Battle),
            ));
        }
    }
    for ev in events.read() {
        let (Ok(a), Ok(b)) = (rings.get(ev.collider1), rings.get(ev.collider2)) else { continue };
        if a.body == b.body {
            continue;
        }
        let (Ok((pa, va)), Ok((pb, vb))) = (bodies.get(a.body), bodies.get(b.body)) else { continue };
        let strength = (va.0 - vb.0).length();
        let point = (pa.0 + pb.0) * 0.5 + Vec3::Y * crate::rings::RING_MID;
        round.hits += 1;
        cam.shake = (cam.shake + strength * 0.15).min(1.0);
        let n = (6.0 + strength * 6.0).min(40.0) as usize;
        for _ in 0..n {
            let v = Vec3::new(
                rng.random_range(-1.0..1.0),
                rng.random_range(0.2..1.5),
                rng.random_range(-1.0..1.0),
            ) * (2.0 + strength);
            commands.spawn((
                Spark { vel: v, life: rng.random_range(0.2..0.5) },
                Mesh3d(assets.mesh.clone()),
                MeshMaterial3d(assets.material.clone()),
                Transform::from_translation(point),
                DespawnOnExit(AppState::Battle),
            ));
        }
    }
}

fn update_sparks(
    mut commands: Commands,
    time: Res<Time>,
    mut q: Query<(Entity, &mut Spark, &mut Transform)>,
) {
    let dt = time.delta_secs();
    for (e, mut s, mut tf) in &mut q {
        s.life -= dt;
        if s.life <= 0.0 {
            commands.entity(e).despawn();
            continue;
        }
        s.vel.y -= 15.0 * dt;
        tf.translation += s.vel * dt;
        tf.scale = Vec3::splat((s.life * 3.0).min(1.0));
    }
}

fn hud(
    round: Res<Round>,
    tops: Query<(&Top, &Rotation, &AngularVelocity, &RigidBody)>,
    mut spins: Query<(&HudSpin, &mut Text), (Without<HudCenter>, Without<HudHelp>, Without<HudTimer>)>,
    mut bars: Query<(&HudBar, &mut Node), Without<HudMeter>>,
    mut center: Query<&mut Text, (With<HudCenter>, Without<HudSpin>, Without<HudHelp>, Without<HudTimer>)>,
    mut center_box: Query<&mut Visibility, (With<HudCenterBox>, Without<HudMeterBox>)>,
    mut timer: Query<&mut Text, (With<HudTimer>, Without<HudSpin>, Without<HudHelp>, Without<HudCenter>)>,
    mut help: Query<&mut Text, (With<HudHelp>, Without<HudSpin>, Without<HudCenter>, Without<HudTimer>)>,
    mut meter: Query<&mut Node, (With<HudMeter>, Without<HudBar>)>,
    mut meter_box: Query<&mut Visibility, (With<HudMeterBox>, Without<HudCenterBox>)>,
    slow: Res<crate::slowmo::SlowMo>,
) {
    for (top, rot, ang, body) in &tops {
        // Total angular speed (rotation about any axis), as used for the judgement.
        let spin = if *body == RigidBody::Dynamic { ang.0.length() } else { 0.0 };
        let tilt = (rot.0 * Vec3::Y).angle_between(Vec3::Y).to_degrees();
        for (h, mut t) in &mut spins {
            if h.0 == top.owner {
                t.0 = if *body == RigidBody::Dynamic {
                    format!("转速 {:>5.0} rpm    倾角 {:>4.1}°", spin * 60.0 / std::f32::consts::TAU, tilt)
                } else {
                    "待发射".into()
                };
            }
        }
        for (h, mut n) in &mut bars {
            if h.0 == top.owner {
                n.width = Val::Percent((spin / MAX_SPIN * 100.0).min(100.0));
            }
        }
    }
    let message = match round.phase {
        // Hidden in slow motion so it doesn't cover the close-up.
        Phase::Fight if round.time < 1.0 && !slow.is_slow() => "发射！".to_string(),
        Phase::Over => round.result.clone(),
        _ => String::new(),
    };
    if let Ok(mut v) = center_box.single_mut() {
        *v = if message.is_empty() { Visibility::Hidden } else { Visibility::Inherited };
    }
    if let Ok(mut t) = center.single_mut() {
        t.0 = message;
    }
    if let Ok(mut t) = timer.single_mut() {
        let [p, a] = round.score;
        t.0 = match round.phase {
            Phase::Aim => format!("准备\n{p} : {a}"),
            _ => format!("{:.0}\n{p} : {a}", (ROUND_TIME - round.time).max(0.0)),
        };
    }
    if let Ok(mut t) = help.single_mut() {
        t.0 = match round.phase {
            Phase::Aim => format!("A/D 移动发射位置   W/S 调整发射方向   按住空格蓄力，松开发射（力量条往返摆动，看准时机）\n先得 {WIN_POINTS} 分获胜：撞出界 2 分 / 停转 1 分"),
            Phase::Fight => format!("空格 圣兽技能（能量满时）   Q/E 视角   滚轮缩放   按住 F 慢动作   P 暂停   T 自动慢动作   撞击 {} 次", round.hits),
            Phase::Over if round.score.iter().any(|s| *s >= WIN_POINTS) => {
                "R 新的比赛（相同配置）    Enter / Esc 返回组装台".into()
            }
            Phase::Over => "R 下一回合    Enter / Esc 返回组装台".into(),
        };
    }
    if let Ok(mut n) = meter.single_mut() {
        n.width = Val::Percent(round.meter * 100.0);
    }
    if let Ok(mut v) = meter_box.single_mut() {
        *v = if round.phase == Phase::Aim { Visibility::Inherited } else { Visibility::Hidden };
    }
}

/// Beasts charge and act only while the fight is on.
fn sync_live(round: Res<Round>, mut live: ResMut<FightLive>) {
    live.0 = round.phase == Phase::Fight && round.time > 1.0;
}

fn round_over_input(
    keys: Res<ButtonInput<KeyCode>>,
    round: Res<Round>,
    mut next: ResMut<NextState<AppState>>,
    mut commands: Commands,
) {
    if keys.just_pressed(KeyCode::Escape) {
        next.set(AppState::Assembly);
        return;
    }
    if round.phase != Phase::Over {
        return;
    }
    if keys.just_pressed(KeyCode::Enter) {
        next.set(AppState::Assembly);
    }
    if keys.just_pressed(KeyCode::KeyR) {
        commands.run_system_cached(start_round);
    }
}
