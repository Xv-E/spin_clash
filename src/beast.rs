//! 圣兽 (spirit beasts): each top carries a beast whose power builds up from
//! blade hits and time, and is unleashed with Space (the computer decides on
//! its own). Every power works through the physics:
//!
//! | 圣兽   | 技能     | Effect                                                     |
//! |--------|----------|------------------------------------------------------------|
//! | 赤炎鹰 | 烈焰突袭 | dash toward the opponent; own blade hits ×1.8 for 2 s      |
//! | 苍岩龟 | 磐岩壁垒 | 4 s: incoming blade hits ×0.35, pulled to centre, upright  |
//! | 雷霆狼 | 雷霆追猎 | 3 s: hunts the opponent at no spin cost; blade hits ×1.3   |
//! | 月影狐 | 月华回旋 | another +45 rad/s of spin and 2.5 s of wobble damping      |
//!
//! Every activation also restores [`AWAKEN_SPIN`] of spin.
//!
//! Activation plays a bullet-time close-up and summons the beast's apparition.

use crate::blade_hits::{BladeHit, ImpactScale};
use crate::slowmo::SlowMo;
use crate::top::{axial_spin, piece_mesh, Owner, Top};
use crate::parts::Shape;
use crate::{AppState, UiFont};
use avian3d::prelude::*;
use bevy::prelude::*;

/// Energy needed to unleash a beast.
pub const FULL: f32 = 100.0;
/// Passive charge per game second.
const CHARGE_RATE: f32 = 4.0;
/// Charge per unit of blade impulse (dealt or received).
const HIT_CHARGE: f32 = 35.0;
const MAX_SPIN: f32 = 200.0;
/// Spin (rad/s) every beast restores when it awakens.
const AWAKEN_SPIN: f32 = 30.0;
/// Extra spin from the fox's 月华回旋 on top of that.
const FOX_SPIN: f32 = 45.0;
/// Height of the floating beast figure above its top.
const FIGURE_Y: f32 = 1.1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BeastKind {
    Hawk,
    Tortoise,
    Wolf,
    Fox,
}

impl BeastKind {
    pub const ALL: [BeastKind; 4] =
        [BeastKind::Hawk, BeastKind::Tortoise, BeastKind::Wolf, BeastKind::Fox];

    pub fn random(rng: &mut impl rand::Rng) -> Self {
        Self::ALL[rng.random_range(0..Self::ALL.len())]
    }
    pub fn name(self) -> &'static str {
        match self {
            BeastKind::Hawk => "赤炎鹰",
            BeastKind::Tortoise => "苍岩龟",
            BeastKind::Wolf => "雷霆狼",
            BeastKind::Fox => "月影狐",
        }
    }
    pub fn skill(self) -> &'static str {
        match self {
            BeastKind::Hawk => "烈焰突袭",
            BeastKind::Tortoise => "磐岩壁垒",
            BeastKind::Wolf => "雷霆追猎",
            BeastKind::Fox => "月华回旋",
        }
    }
    pub fn desc(self) -> &'static str {
        match self {
            BeastKind::Hawk => "烈焰突袭：猛冲向对手，2 秒内刃击威力 ×1.8",
            BeastKind::Tortoise => "磐岩壁垒：4 秒内受到的刃击 ×0.35，被拉回中心并保持直立",
            BeastKind::Wolf => "雷霆追猎：3 秒内追猎对手且不消耗转速，刃击 ×1.3",
            BeastKind::Fox => "月华回旋：转速再额外 +45 rad/s，2.5 秒内抑制晃动",
        }
    }
    pub fn color(self) -> Color {
        match self {
            BeastKind::Hawk => Color::srgb(1.0, 0.35, 0.1),
            BeastKind::Tortoise => Color::srgb(0.3, 0.85, 0.5),
            BeastKind::Wolf => Color::srgb(0.45, 0.6, 1.0),
            BeastKind::Fox => Color::srgb(0.85, 0.75, 1.0),
        }
    }
    /// How long the power lasts (game seconds).
    fn duration(self) -> f32 {
        match self {
            BeastKind::Hawk => 2.0,
            BeastKind::Tortoise => 4.0,
            BeastKind::Wolf => 3.0,
            BeastKind::Fox => 2.5,
        }
    }
}

/// The beast bound to a top.
#[derive(Component)]
pub struct Beast {
    pub kind: BeastKind,
    pub energy: f32,
    /// Game seconds of power left.
    pub active: f32,
}

impl Beast {
    pub fn new(kind: BeastKind) -> Self {
        Self { kind, energy: 0.0, active: 0.0 }
    }
}

/// Whether a fight is in progress (set by the battle module).
#[derive(Resource, Default)]
pub struct FightLive(pub bool);

/// Request to unleash a top's beast.
#[derive(Message, Clone, Copy)]
pub struct ActivateBeast(pub Entity);

/// HUD markers, spawned by the battle HUD and updated here.
#[derive(Component)]
pub struct HudEnergy(pub Owner);
#[derive(Component)]
pub struct HudBeastText(pub Owner);

#[derive(Component)]
struct Apparition {
    top: Entity,
    life: f32,
    max_life: f32,
    material: Handle<StandardMaterial>,
    color: Color,
}

#[derive(Component)]
struct Banner {
    life: f32,
}

pub struct BeastPlugin;

impl Plugin for BeastPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FightLive>()
            .add_message::<ActivateBeast>()
            .add_systems(
                Update,
                (player_input, ai_decide, activate, charge_from_hits, animate, hud, banner)
                    .chain()
                    .run_if(in_state(AppState::Battle)),
            )
            .add_systems(FixedUpdate, powers.run_if(in_state(AppState::Battle)));
    }
}

fn player_input(
    keys: Res<ButtonInput<KeyCode>>,
    live: Res<FightLive>,
    tops: Query<(Entity, &Top, &Beast)>,
    mut out: MessageWriter<ActivateBeast>,
) {
    if !live.0 || !keys.just_pressed(KeyCode::Space) {
        return;
    }
    for (e, top, beast) in &tops {
        if top.owner == Owner::Player && beast.energy >= FULL && beast.active <= 0.0 {
            out.write(ActivateBeast(e));
        }
    }
}

/// The computer unleashes its beast when the moment suits that beast.
fn ai_decide(
    live: Res<FightLive>,
    tops: Query<(Entity, &Top, &Beast, &Position, &Rotation, &AngularVelocity)>,
    mut out: MessageWriter<ActivateBeast>,
) {
    if !live.0 {
        return;
    }
    let Some(player) = tops.iter().find(|t| t.1.owner == Owner::Player) else { return };
    let player_spin = axial_spin(player.4, player.5).abs();
    for (e, top, beast, pos, rot, ang) in &tops {
        if top.owner != Owner::Ai || beast.energy < FULL || beast.active > 0.0 {
            continue;
        }
        let dist = (player.3.0 - pos.0).with_y(0.0).length();
        let spin = axial_spin(rot, ang).abs();
        let tilt = (rot.0 * Vec3::Y).angle_between(Vec3::Y);
        let go = match beast.kind {
            BeastKind::Hawk | BeastKind::Wolf => dist < 2.5,
            BeastKind::Tortoise => dist < 1.4 || spin < player_spin * 0.85,
            BeastKind::Fox => spin < 80.0 || tilt > 0.25,
        };
        if go {
            out.write(ActivateBeast(e));
        }
    }
}

fn activate(
    mut commands: Commands,
    mut requests: MessageReader<ActivateBeast>,
    mut tops: Query<(
        Entity,
        &Top,
        &mut Beast,
        &Position,
        &Rotation,
        &mut LinearVelocity,
        &mut AngularVelocity,
    )>,
    mut slow: ResMut<SlowMo>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    font: Res<UiFont>,
    banners: Query<(), With<Banner>>,
) {
    // Stack captions when both beasts awaken at once.
    let mut banner_slot = banners.iter().count();
    for &ActivateBeast(e) in requests.read() {
        let others: Vec<Vec3> =
            tops.iter().filter(|t| t.0 != e).map(|t| t.3.0).collect();
        let Ok((_, top, mut beast, pos, rot, mut lin, mut ang)) = tops.get_mut(e) else { continue };
        if beast.energy < FULL || beast.active > 0.0 {
            continue;
        }
        beast.energy = 0.0;
        beast.active = beast.kind.duration();
        let to_opponent = others
            .first()
            .map(|o| (*o - pos.0).with_y(0.0).normalize_or_zero())
            .unwrap_or(Vec3::ZERO);
        if beast.kind == BeastKind::Hawk {
            lin.0 += to_opponent * 7.0;
        }
        // Spin restore: about the top's own axis, in its own direction.
        let boost = AWAKEN_SPIN + if beast.kind == BeastKind::Fox { FOX_SPIN } else { 0.0 };
        let axis = rot.0 * Vec3::Y;
        let spin = axial_spin(rot, &ang);
        let sign = if spin < 0.0 { -1.0 } else { 1.0 };
        let boosted = (spin.abs() + boost).min(MAX_SPIN.max(spin.abs())) * sign;
        let gained = (boosted - spin).abs();
        ang.0 += axis * (boosted - spin);
        let kind = beast.kind;
        slow.cinematic(
            e,
            1.6,
            format!(
                "{} 的圣兽「{}」发动 —— {}   回旋力 +{:.0}",
                top.owner.name(),
                kind.name(),
                kind.desc(),
                gained
            ),
        );
        spawn_apparition(&mut commands, &mut meshes, &mut materials, e, kind, pos.0);
        spawn_banner(&mut commands, &font.0, top.owner, kind, banner_slot);
        banner_slot += 1;
    }
}

/// Both tops in a blade hit gain energy (unless their power is running).
fn charge_from_hits(mut hits: MessageReader<BladeHit>, mut beasts: Query<&mut Beast>) {
    for h in hits.read() {
        for e in [h.a, h.b] {
            if let Ok(mut b) = beasts.get_mut(e) {
                if b.active <= 0.0 {
                    b.energy = (b.energy + h.impulse * HIT_CHARGE).min(FULL);
                }
            }
        }
    }
}

/// Passive charge and the running powers (fixed physics rate).
fn powers(
    live: Res<FightLive>,
    time: Res<Time>,
    mut tops: Query<(
        Entity,
        &mut Beast,
        &mut ImpactScale,
        &RigidBody,
        &Position,
        &Rotation,
        &mut LinearVelocity,
        &mut AngularVelocity,
    )>,
) {
    let dt = time.delta_secs();
    let positions: Vec<(Entity, Vec3)> = tops.iter().map(|t| (t.0, t.4.0)).collect();
    for (e, mut beast, mut scale, body, pos, rot, mut lin, mut ang) in &mut tops {
        *scale = ImpactScale::default();
        if *body != RigidBody::Dynamic {
            continue;
        }
        if beast.active <= 0.0 {
            if live.0 {
                beast.energy = (beast.energy + CHARGE_RATE * dt).min(FULL);
            }
            continue;
        }
        beast.active -= dt;
        let opponent = positions.iter().find(|p| p.0 != e).map(|p| p.1);
        match beast.kind {
            BeastKind::Hawk => scale.attack = 1.8,
            BeastKind::Tortoise => {
                scale.defense = 0.35;
                let to_centre = (-pos.0).with_y(0.0);
                if to_centre.length() > 0.2 {
                    lin.0 += to_centre.normalize() * 3.0 * dt;
                }
                steady(rot, &mut ang, dt);
            }
            BeastKind::Wolf => {
                scale.attack = 1.3;
                if let Some(o) = opponent {
                    let d = (o - pos.0).with_y(0.0);
                    if d.length() > 0.3 {
                        lin.0 += d.normalize() * 9.0 * dt;
                    }
                }
            }
            BeastKind::Fox => steady(rot, &mut ang, dt),
        }
    }
}

/// Damp the wobble (angular velocity off the spin axis) and lean back upright.
fn steady(rot: &Rotation, ang: &mut AngularVelocity, dt: f32) {
    let axis = rot.0 * Vec3::Y;
    let spin = ang.0.dot(axis);
    let wobble = ang.0 - axis * spin;
    ang.0 = axis * spin + wobble * (1.0 - 6.0 * dt).max(0.0);
    let tilt = axis.angle_between(Vec3::Y);
    if tilt > 0.01 {
        ang.0 += axis.cross(Vec3::Y).normalize_or_zero() * tilt * 3.0 * dt;
    }
}

// ---------------------------------------------------------------------------
// Apparition: a glowing beast figure above the top, plus an aura column.
// ---------------------------------------------------------------------------

/// Primitive pieces of each beast's figure (shape, position, rotation, scale).
fn figure(kind: BeastKind) -> Vec<(Shape, Vec3, Quat, Vec3)> {
    let q = Quat::IDENTITY;
    let one = Vec3::ONE;
    let s = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    match kind {
        BeastKind::Hawk => vec![
            (Shape::Sphere(1.0), s(0.0, 0.0, 0.0), q, s(0.22, 0.18, 0.45)),
            (Shape::Sphere(0.14), s(0.0, 0.1, 0.45), q, one),
            (Shape::Cone { r: 0.06, h: 0.16 }, s(0.0, 0.08, 0.62), Quat::from_rotation_x(1.57), one),
            (Shape::Cuboid(s(0.95, 0.04, 0.35)), s(0.55, 0.12, -0.05), Quat::from_rotation_z(0.35), one),
            (Shape::Cuboid(s(0.95, 0.04, 0.35)), s(-0.55, 0.12, -0.05), Quat::from_rotation_z(-0.35), one),
            (Shape::Cuboid(s(0.3, 0.03, 0.35)), s(0.0, 0.0, -0.55), q, one),
        ],
        BeastKind::Tortoise => vec![
            (Shape::Sphere(1.0), s(0.0, 0.0, 0.0), q, s(0.6, 0.32, 0.7)),
            (Shape::Sphere(0.16), s(0.0, 0.0, 0.78), q, one),
            (Shape::Cylinder { r: 0.09, h: 0.25 }, s(0.4, -0.2, 0.4), q, one),
            (Shape::Cylinder { r: 0.09, h: 0.25 }, s(-0.4, -0.2, 0.4), q, one),
            (Shape::Cylinder { r: 0.09, h: 0.25 }, s(0.4, -0.2, -0.4), q, one),
            (Shape::Cylinder { r: 0.09, h: 0.25 }, s(-0.4, -0.2, -0.4), q, one),
        ],
        BeastKind::Wolf => vec![
            (Shape::Cuboid(s(0.32, 0.32, 0.85)), s(0.0, 0.0, 0.0), q, one),
            (Shape::Cuboid(s(0.3, 0.28, 0.32)), s(0.0, 0.2, 0.55), q, one),
            (Shape::Cuboid(s(0.16, 0.14, 0.22)), s(0.0, 0.14, 0.8), q, one),
            (Shape::Cone { r: 0.07, h: 0.18 }, s(0.09, 0.42, 0.55), q, one),
            (Shape::Cone { r: 0.07, h: 0.18 }, s(-0.09, 0.42, 0.55), q, one),
            (Shape::Cylinder { r: 0.05, h: 0.5 }, s(0.0, 0.15, -0.6), Quat::from_rotation_x(-0.9), one),
            (Shape::Cylinder { r: 0.06, h: 0.35 }, s(0.12, -0.3, 0.3), q, one),
            (Shape::Cylinder { r: 0.06, h: 0.35 }, s(-0.12, -0.3, 0.3), q, one),
            (Shape::Cylinder { r: 0.06, h: 0.35 }, s(0.12, -0.3, -0.3), q, one),
            (Shape::Cylinder { r: 0.06, h: 0.35 }, s(-0.12, -0.3, -0.3), q, one),
        ],
        BeastKind::Fox => vec![
            (Shape::Sphere(1.0), s(0.0, 0.0, 0.0), q, s(0.22, 0.22, 0.48)),
            (Shape::Sphere(0.17), s(0.0, 0.16, 0.45), q, one),
            (Shape::Cone { r: 0.06, h: 0.22 }, s(0.09, 0.38, 0.45), q, one),
            (Shape::Cone { r: 0.06, h: 0.22 }, s(-0.09, 0.38, 0.45), q, one),
            (Shape::Cone { r: 0.07, h: 0.18 }, s(0.0, 0.12, 0.66), Quat::from_rotation_x(1.57), one),
            (Shape::Cone { r: 0.22, h: 0.75 }, s(0.0, 0.3, -0.65), Quat::from_rotation_x(-2.2), one),
        ],
    }
}

fn spawn_apparition(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    top: Entity,
    kind: BeastKind,
    at: Vec3,
) {
    let color = kind.color();
    let material = materials.add(StandardMaterial {
        base_color: color.with_alpha(0.0),
        emissive: (color.to_linear() * 6.0),
        alpha_mode: AlphaMode::Add,
        unlit: true,
        double_sided: true,
        cull_mode: None,
        ..default()
    });
    let life = kind.duration() + 0.6;
    commands
        .spawn((
            Apparition { top, life, max_life: life, material: material.clone(), color },
            Transform::from_translation(at),
            Visibility::default(),
            DespawnOnExit(AppState::Battle),
        ))
        .with_children(|p| {
            // Aura column and rune ring around the top.
            p.spawn((
                Mesh3d(meshes.add(Cylinder::new(0.62, 1.6))),
                MeshMaterial3d(material.clone()),
                Transform::from_xyz(0.0, 0.8, 0.0).with_scale(Vec3::new(1.0, 1.0, 1.0)),
            ));
            p.spawn((
                Mesh3d(meshes.add(Torus::new(0.66, 0.74))),
                MeshMaterial3d(material.clone()),
                Transform::from_xyz(0.0, 0.25, 0.0),
                AuraRing,
            ));
            // The beast figure, floating above.
            p.spawn((Transform::from_xyz(0.0, FIGURE_Y, 0.0).with_scale(Vec3::splat(0.95)), Visibility::default(), Figure))
                .with_children(|f| {
                    for (shape, pos, rot, scale) in figure(kind) {
                        f.spawn((
                            Mesh3d(meshes.add(piece_mesh(shape))),
                            MeshMaterial3d(material.clone()),
                            Transform::from_translation(pos).with_rotation(rot).with_scale(scale),
                        ));
                    }
                });
        });
}

#[derive(Component)]
struct AuraRing;
#[derive(Component)]
struct Figure;

/// Follow the top, fade in/out, bob the figure, spin the ring (real time, so
/// it stays lively during bullet time).
fn animate(
    mut commands: Commands,
    real: Res<Time<Real>>,
    time: Res<Time>,
    tops: Query<&Transform, (With<Top>, Without<Apparition>)>,
    mut apps: Query<(Entity, &mut Apparition, &mut Transform, &Children), Without<Top>>,
    mut rings: Query<&mut Transform, (With<AuraRing>, Without<Apparition>, Without<Top>, Without<Figure>)>,
    mut figures: Query<&mut Transform, (With<Figure>, Without<Apparition>, Without<Top>, Without<AuraRing>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let t = real.elapsed_secs();
    for (e, mut app, mut tf, children) in &mut apps {
        app.life -= time.delta_secs();
        if app.life <= 0.0 {
            commands.entity(e).despawn();
            continue;
        }
        if let Ok(top) = tops.get(app.top) {
            tf.translation = top.translation;
        }
        // Fade in over 0.3 s, out over the last 0.6 s.
        let age = app.max_life - app.life;
        let a = (age / 0.3).min(1.0).min(app.life / 0.6).clamp(0.0, 1.0);
        if let Some(m) = materials.get_mut(&app.material) {
            m.base_color = app.color.with_alpha(0.35 * a);
            m.emissive = app.color.to_linear() * (9.0 * a);
        }
        for &c in children {
            if let Ok(mut r) = rings.get_mut(c) {
                r.rotation = Quat::from_rotation_y(t * 2.5) * Quat::from_rotation_x(0.15 * (t * 3.0).sin());
            }
            if let Ok(mut f) = figures.get_mut(c) {
                f.translation.y = FIGURE_Y + 0.08 * (t * 2.0).sin() + 0.4 * (1.0 - a);
                f.rotation = Quat::from_rotation_y(t * 0.6);
            }
        }
    }
}

fn spawn_banner(commands: &mut Commands, font: &Handle<Font>, owner: Owner, kind: BeastKind, slot: usize) {
    commands
        .spawn((
            Banner { life: 2.2 },
            DespawnOnExit(AppState::Battle),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Percent(22.0 + 15.0 * slot as f32),
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                ..default()
            },
        ))
        .with_children(|p| {
            p.spawn((
                Text::new(format!("{} · {}", kind.name(), kind.skill())),
                TextFont { font: font.clone(), font_size: 64.0, ..default() },
                TextColor(kind.color()),
                TextShadow::default(),
            ));
            p.spawn((
                Text::new(format!("{}的圣兽觉醒！", owner.name())),
                TextFont { font: font.clone(), font_size: 24.0, ..default() },
                TextColor(owner.accent()),
                TextShadow::default(),
            ));
        });
}

fn banner(
    mut commands: Commands,
    real: Res<Time<Real>>,
    mut banners: Query<(Entity, &mut Banner, &Children)>,
    mut texts: Query<&mut TextColor>,
) {
    for (e, mut b, children) in &mut banners {
        b.life -= real.delta_secs();
        if b.life <= 0.0 {
            commands.entity(e).despawn();
            continue;
        }
        let a = (b.life / 0.5).min(1.0);
        for &c in children {
            if let Ok(mut col) = texts.get_mut(c) {
                col.0 = col.0.with_alpha(a);
            }
        }
    }
}

fn hud(
    tops: Query<(&Top, &Beast)>,
    mut bars: Query<(&HudEnergy, &mut Node, &mut BackgroundColor)>,
    mut texts: Query<(&HudBeastText, &mut Text)>,
    real: Res<Time<Real>>,
) {
    let pulse = 0.6 + 0.4 * (real.elapsed_secs() * 6.0).sin().abs();
    for (top, beast) in &tops {
        for (h, mut node, mut bg) in &mut bars {
            if h.0 == top.owner {
                let share = if beast.active > 0.0 {
                    beast.active / beast.kind.duration()
                } else {
                    beast.energy / FULL
                };
                node.width = Val::Percent(share * 100.0);
                let full = beast.energy >= FULL && beast.active <= 0.0;
                bg.0 = beast.kind.color().with_alpha(if full { pulse } else { 0.85 });
            }
        }
        for (h, mut t) in &mut texts {
            if h.0 == top.owner {
                let state = if beast.active > 0.0 {
                    format!("「{}」发动中", beast.kind.skill())
                } else if beast.energy >= FULL {
                    if top.owner == Owner::Player {
                        "能量已满 — 按空格发动！".to_string()
                    } else {
                        "能量已满".to_string()
                    }
                } else {
                    format!("能量 {:.0}%", beast.energy)
                };
                t.0 = format!("圣兽 {}   {}", beast.kind.name(), state);
            }
        }
    }
}
