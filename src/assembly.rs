//! Assembly screen: pick ring / disk / tip / spin direction, see the physical
//! properties of the result, then start a battle.

use crate::parts::{Build, Disk, Ring, SpinDir, Tip};
use crate::top::{spawn_preview, PreviewTop, Visuals};
use crate::beast::BeastKind;
use crate::{AppState, Loadout, OpponentMode, OrbitCam, UiFont};
use bevy::prelude::*;

pub struct AssemblyPlugin;

impl Plugin for AssemblyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Assembly), enter)
            .add_systems(
                Update,
                (buttons, keyboard, refresh, spin_preview, button_hover)
                    .chain()
                    .run_if(in_state(AppState::Assembly)),
            );
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    Ring,
    Disk,
    Tip,
    Dir,
    Beast,
    Opponent,
}

#[derive(Component)]
struct CycleButton {
    field: Field,
    delta: i32,
}

#[derive(Component)]
struct StartButton;

#[derive(Component)]
struct FieldName(Field);

#[derive(Component)]
struct FieldDesc(Field);

#[derive(Component)]
struct StatsText;

#[derive(Component)]
struct StatBar(usize);

/// Set when the loadout changed and the preview / texts must be rebuilt.
#[derive(Resource)]
struct Dirty(bool);

const PANEL: Color = Color::srgba(0.08, 0.09, 0.14, 0.92);
const BUTTON: Color = Color::srgb(0.2, 0.22, 0.32);
const BUTTON_HOVER: Color = Color::srgb(0.3, 0.33, 0.48);
const ACCENT: Color = Color::srgb(1.0, 0.55, 0.05);

fn cycle<T: Copy + PartialEq>(all: &[T], cur: T, delta: i32) -> T {
    let i = all.iter().position(|x| *x == cur).unwrap_or(0) as i32;
    all[(i + delta).rem_euclid(all.len() as i32) as usize]
}

fn apply(loadout: &mut Loadout, field: Field, delta: i32) {
    let b = &mut loadout.player;
    match field {
        Field::Ring => b.ring = cycle(&Ring::all(), b.ring, delta),
        Field::Disk => b.disk = cycle(&Disk::ALL, b.disk, delta),
        Field::Tip => b.tip = cycle(&Tip::ALL, b.tip, delta),
        Field::Dir => b.dir = cycle(&[SpinDir::Right, SpinDir::Left], b.dir, delta),
        Field::Beast => loadout.beast = cycle(&BeastKind::ALL, loadout.beast, delta),
        Field::Opponent => {
            loadout.opponent =
                cycle(&[OpponentMode::Random, OpponentMode::Mirror], loadout.opponent, delta)
        }
    }
}

fn field_texts(loadout: &Loadout, field: Field) -> (&'static str, &'static str) {
    let b = loadout.player;
    match field {
        Field::Ring => (b.ring.name(), b.ring.desc()),
        Field::Disk => (b.disk.name(), b.disk.desc()),
        Field::Tip => (b.tip.name(), b.tip.desc()),
        Field::Dir => (
            b.dir.name(),
            "相反旋转方向的陀螺相撞时接触面相对速度更大，撞击更猛烈",
        ),
        Field::Beast => (loadout.beast.name(), loadout.beast.desc()),
        Field::Opponent => match loadout.opponent {
            OpponentMode::Random => ("随机配置", "电脑每局随机组装一个陀螺"),
            OpponentMode::Mirror => ("镜像配置", "电脑使用与你完全相同的零件"),
        },
    }
}

fn label(font: &Handle<Font>, text: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (
        Text::new(text),
        TextFont { font: font.clone(), font_size: size, ..default() },
        TextColor(color),
    )
}

fn enter(
    mut commands: Commands,
    font: Res<UiFont>,
    mut cam: ResMut<OrbitCam>,
) {
    *cam = OrbitCam::new(Vec3::new(0.55, 0.15, 0.0), 0.0, 0.42, 2.4);
    commands.insert_resource(Dirty(true));
    let font = font.0.clone();
    let scope = DespawnOnExit(AppState::Assembly);

    commands
        .spawn((
            scope,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::FlexEnd,
                ..default()
            },
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    width: Val::Px(560.0),
                    height: Val::Percent(100.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(Val::Px(24.0)),
                    row_gap: Val::Px(10.0),
                    ..default()
                },
                BackgroundColor(PANEL),
            ))
            .with_children(|panel| {
                panel.spawn(label(&font, "陀螺组装台", 38.0, ACCENT));
                panel.spawn(label(
                    &font,
                    "每个零件的形状和材质决定质量、重心和转动惯量，物理引擎据此模拟对战",
                    15.0,
                    Color::srgb(0.7, 0.72, 0.8),
                ));
                for (field, title) in [
                    (Field::Ring, "攻击环"),
                    (Field::Disk, "配重盘"),
                    (Field::Tip, "轴尖"),
                    (Field::Dir, "旋转方向"),
                    (Field::Beast, "圣兽"),
                    (Field::Opponent, "对手"),
                ] {
                    field_row(panel, &font, field, title);
                }

                panel.spawn((
                    label(&font, "", 17.0, Color::srgb(0.85, 0.87, 0.95)),
                    StatsText,
                    Node { margin: UiRect::top(Val::Px(8.0)), ..default() },
                ));
                for (i, name) in ["攻击", "防御", "持久"].into_iter().enumerate() {
                    panel
                        .spawn(Node {
                            align_items: AlignItems::Center,
                            column_gap: Val::Px(10.0),
                            ..default()
                        })
                        .with_children(|row| {
                            row.spawn((
                                label(&font, name, 17.0, Color::WHITE),
                                Node { width: Val::Px(48.0), ..default() },
                            ));
                            row.spawn((
                                Node { width: Val::Px(380.0), height: Val::Px(14.0), ..default() },
                                BackgroundColor(Color::srgb(0.18, 0.19, 0.26)),
                            ))
                            .with_children(|bar| {
                                bar.spawn((
                                    StatBar(i),
                                    Node {
                                        width: Val::Percent(50.0),
                                        height: Val::Percent(100.0),
                                        ..default()
                                    },
                                    BackgroundColor(
                                        [
                                            Color::srgb(0.95, 0.3, 0.2),
                                            Color::srgb(0.25, 0.55, 0.95),
                                            Color::srgb(0.2, 0.85, 0.45),
                                        ][i],
                                    ),
                                ));
                            });
                        });
                }

                panel.spawn(Node { flex_grow: 1.0, ..default() });
                panel
                    .spawn((
                        StartButton,
                        Button,
                        Node {
                            height: Val::Px(64.0),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            border_radius: BorderRadius::all(Val::Px(10.0)),
                            ..default()
                        },
                        BackgroundColor(Color::srgb(0.85, 0.4, 0.02)),
                    ))
                    .with_children(|b| {
                        b.spawn(label(&font, "开始对战  (Enter)", 26.0, Color::WHITE));
                    });
                panel.spawn(label(
                    &font,
                    "快捷键：1~6 或 ↑↓ 选择行，←→ 切换零件，Enter 开始",
                    14.0,
                    Color::srgb(0.6, 0.62, 0.7),
                ));
            });
        });
}

#[derive(Component, Clone)]
struct PedestalMarker;

fn field_row(panel: &mut ChildSpawnerCommands, font: &Handle<Font>, field: Field, title: &str) {
    panel
        .spawn(Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(4.0), ..default() })
        .with_children(|col| {
            col.spawn(
                Node { align_items: AlignItems::Center, column_gap: Val::Px(10.0), ..default() },
            )
            .with_children(|row| {
                row.spawn((
                    label(font, title, 18.0, Color::srgb(0.7, 0.72, 0.8)),
                    Node { width: Val::Px(84.0), ..default() },
                ));
                for (delta, glyph) in [(-1, "<"), (1, ">")] {
                    if delta == 1 {
                        row.spawn((
                            FieldName(field),
                            label(font, "", 22.0, Color::WHITE),
                            Node { width: Val::Px(220.0), justify_content: JustifyContent::Center, ..default() },
                            TextLayout::new_with_justify(Justify::Center),
                        ));
                    }
                    row.spawn((
                        CycleButton { field, delta },
                        Button,
                        Node {
                            width: Val::Px(40.0),
                            height: Val::Px(34.0),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            border_radius: BorderRadius::all(Val::Px(6.0)),
                            ..default()
                        },
                        BackgroundColor(BUTTON),
                    ))
                    .with_children(|b| {
                        b.spawn(label(font, glyph, 18.0, Color::WHITE));
                    });
                }
            });
            col.spawn((
                FieldDesc(field),
                label(font, "", 14.0, Color::srgb(0.62, 0.64, 0.72)),
                Node { margin: UiRect::left(Val::Px(94.0)), ..default() },
            ));
        });
}

fn buttons(
    q: Query<(&Interaction, Option<&CycleButton>, Option<&StartButton>), Changed<Interaction>>,
    mut loadout: ResMut<Loadout>,
    mut dirty: ResMut<Dirty>,
    mut next: ResMut<NextState<AppState>>,
) {
    for (interaction, cycle, start) in &q {
        if *interaction != Interaction::Pressed {
            continue;
        }
        if let Some(c) = cycle {
            apply(&mut loadout, c.field, c.delta);
            dirty.0 = true;
        }
        if start.is_some() {
            next.set(AppState::Battle);
        }
    }
}

fn button_hover(
    mut q: Query<(&Interaction, &mut BackgroundColor), (With<CycleButton>, Changed<Interaction>)>,
) {
    for (i, mut bg) in &mut q {
        bg.0 = if *i == Interaction::None { BUTTON } else { BUTTON_HOVER };
    }
}

fn keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    mut loadout: ResMut<Loadout>,
    mut dirty: ResMut<Dirty>,
    mut next: ResMut<NextState<AppState>>,
    mut row: Local<usize>,
) {
    const FIELDS: [Field; 6] =
        [Field::Ring, Field::Disk, Field::Tip, Field::Dir, Field::Beast, Field::Opponent];
    for (i, k) in [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4, KeyCode::Digit5, KeyCode::Digit6]
        .into_iter()
        .enumerate()
    {
        if keys.just_pressed(k) {
            *row = i;
        }
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        *row = (*row + 5) % 6;
    }
    if keys.just_pressed(KeyCode::ArrowDown) {
        *row = (*row + 1) % 6;
    }
    for (k, d) in [(KeyCode::ArrowLeft, -1), (KeyCode::ArrowRight, 1)] {
        if keys.just_pressed(k) {
            apply(&mut loadout, FIELDS[*row], d);
            dirty.0 = true;
        }
    }
    if keys.just_pressed(KeyCode::Enter) {
        next.set(AppState::Battle);
    }
}

/// Rough 0..1 ratings derived from the physical properties.
fn ratings(b: Build) -> [f32; 3] {
    let s = b.stats();
    // Attack from blade geometry: how far blades stick out past the core,
    // plus a bonus for sloped faces (lifting / pressing the opponent).
    let def = b.ring.def();
    let bladeless = def.blades.is_empty();
    let ring_attack = if bladeless {
        0.25
    } else {
        let protrusion = (s.radius - def.core_radius()) / 0.2;
        let slope = def.blades.iter().map(|bl| bl.slope.abs()).fold(0.0, f32::max) / 40.0;
        (0.45 + 0.4 * protrusion.min(1.0) + 0.15 * slope.min(1.0)).min(1.0)
    };
    let mobility = (b.tip.friction() / 1.1).sqrt();
    let attack = (0.55 * ring_attack + 0.45 * mobility).clamp(0.05, 1.0);
    let defense = ((s.mass - 0.15) / 0.3 * 0.7
        + if bladeless { 0.3 } else { 0.0 }
        + if b.tip == Tip::Ball { 0.1 } else { 0.0 })
    .clamp(0.05, 1.0);
    // Spin-down rate at 100 rad/s relative to stored angular momentum.
    let torsion = b.tip.friction() * b.tip.contact_radius() * s.mass * crate::GRAVITY;
    let air = crate::top::AIR_K * b.ring.air_drag() * s.radius.powi(5) * 100.0 * 100.0;
    let decel = crate::top::SPIN_LOSS_SCALE * (torsion + air) / s.spin_inertia;
    let stamina = (1.0 - (decel / (14.0 * crate::top::SPIN_LOSS_SCALE))).clamp(0.05, 1.0);
    [attack, defense, stamina]
}

fn refresh(
    mut commands: Commands,
    mut dirty: ResMut<Dirty>,
    loadout: Res<Loadout>,
    previews: Query<Entity, Or<(With<PreviewTop>, With<PedestalMarker>)>>,
    mut names: Query<(&FieldName, &mut Text), (Without<FieldDesc>, Without<StatsText>)>,
    mut descs: Query<(&FieldDesc, &mut Text), (Without<FieldName>, Without<StatsText>)>,
    mut stats_text: Query<&mut Text, (With<StatsText>, Without<FieldName>, Without<FieldDesc>)>,
    mut bars: Query<(&StatBar, &mut Node)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !dirty.0 {
        return;
    }
    dirty.0 = false;
    for e in &previews {
        commands.entity(e).despawn();
    }
    let build = loadout.player;
    let preview = spawn_preview(
        &mut commands,
        build,
        Transform::from_xyz(0.0, 0.0, 0.0),
        Visuals { meshes: &mut meshes, materials: &mut materials },
    );
    commands.entity(preview).insert(DespawnOnExit(AppState::Assembly));
    commands.spawn((
        PedestalMarker,
        DespawnOnExit(AppState::Assembly),
        Mesh3d(meshes.add(Cylinder::new(0.75, 0.06))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.2, 0.22, 0.3),
            metallic: 0.6,
            perceptual_roughness: 0.3,
            ..default()
        })),
        Transform::from_xyz(0.0, -0.03, 0.0),
    ));

    for (f, mut t) in &mut names {
        t.0 = field_texts(&loadout, f.0).0.to_string();
    }
    for (f, mut t) in &mut descs {
        t.0 = field_texts(&loadout, f.0).1.to_string();
    }
    let s = build.stats();
    if let Ok(mut t) = stats_text.single_mut() {
        t.0 = format!(
            "刃片：{}\n质量 {:.3}   自转惯量 {:.4}\n重心高度 {:.3}   最大半径 {:.3}\n轴尖摩擦系数 {:.2}   接触半径 {:.3}",
            build.ring.def().blade_summary(),
            s.mass,
            s.spin_inertia,
            s.com_height,
            s.radius,
            build.tip.friction(),
            build.tip.contact_radius()
        );
    }
    let r = ratings(build);
    for (b, mut node) in &mut bars {
        node.width = Val::Percent(r[b.0] * 100.0);
    }
}

fn spin_preview(time: Res<Time>, loadout: Res<Loadout>, mut q: Query<&mut Transform, With<PreviewTop>>) {
    for mut tf in &mut q {
        tf.rotate_y(loadout.player.dir.sign() * time.delta_secs() * 1.2);
    }
}
