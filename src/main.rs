mod assembly;
mod battle;
mod beast;
mod blade_hits;
mod parts;
mod rings;
mod sim;
mod slowmo;
mod stadium;
mod top;

use avian3d::prelude::*;
use bevy::prelude::*;
use parts::Build;

pub const GRAVITY: f32 = 19.6;
pub const PHYSICS_HZ: f64 = 240.0;
pub const SUBSTEPS: u32 = 40;

#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    #[default]
    Assembly,
    Battle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpponentMode {
    Random,
    Mirror,
}

/// The player's current loadout, kept between battles.
#[derive(Resource)]
pub struct Loadout {
    pub player: Build,
    pub opponent: OpponentMode,
    pub beast: beast::BeastKind,
}

#[derive(Resource)]
pub struct UiFont(pub Handle<Font>);

/// Simple orbit camera shared by both screens.
#[derive(Resource)]
pub struct OrbitCam {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub shake: f32,
    /// Point the camera moves in on (slow-motion close-ups).
    pub focus: Vec3,
    /// 0 = normal framing, 1 = fully zoomed in on `focus`.
    pub focus_blend: f32,
    /// Extra yaw swept around the focus point during a close-up.
    pub orbit: f32,
    /// Close-up framing.
    pub close_dist: f32,
    pub close_pitch: f32,
}

impl OrbitCam {
    pub fn new(target: Vec3, yaw: f32, pitch: f32, dist: f32) -> Self {
        Self {
            target,
            yaw,
            pitch,
            dist,
            shake: 0.0,
            focus: target,
            focus_blend: 0.0,
            orbit: 0.0,
            close_dist: 2.6,
            close_pitch: 0.55,
        }
    }
}

#[derive(Component)]
pub struct MainCamera;

/// Physics settings shared with the headless simulation.
pub fn configure_physics(app: &mut App) {
    app.insert_resource(Gravity(Vec3::NEG_Y * GRAVITY))
        .insert_resource(SubstepCount(SUBSTEPS))
        .insert_resource(Time::<Fixed>::from_hz(PHYSICS_HZ))
        .add_systems(FixedUpdate, (top::tip_drive, top::spin_losses, top::update_spin_meter).chain());
    blade_hits::plugin(app);
}

fn main() {
    if std::env::args().any(|a| a == "--sim") {
        sim::run();
        return;
    }

    let mut app = App::new();
    app.add_plugins((
        DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "陀螺对决 Spin Clash".into(),
                resolution: (1400, 860).into(),
                ..default()
            }),
            ..default()
        }),
        PhysicsPlugins::default().with_collision_hooks::<top::BladeHooks>(),
    ));
    configure_physics(&mut app);
    // Loaded up front: the first state's OnEnter runs before Startup commands apply.
    load_font(app.world_mut());
    app.init_state::<AppState>()
        .insert_resource(ClearColor(Color::srgb(0.05, 0.06, 0.09)))
        .insert_resource(GlobalAmbientLight { brightness: 350.0, ..default() })
        .insert_resource(Loadout {
            player: Build::default(),
            opponent: OpponentMode::Random,
            beast: beast::BeastKind::Hawk,
        })
        .insert_resource(OrbitCam::new(Vec3::ZERO, 0.0, 0.8, 9.0))
        .add_systems(Startup, setup_scene)
        .add_systems(Update, update_camera)
        .add_plugins((assembly::AssemblyPlugin, battle::BattlePlugin, slowmo::SlowMoPlugin, beast::BeastPlugin));
    // Dev aid: `SPIN_AUTOTEST=<dir>` plays a scripted session and saves screenshots.
    if let Ok(dir) = std::env::var("SPIN_AUTOTEST") {
        // Optional starting ring for the scripted session (catalogue index).
        if let Some(i) = std::env::var("SPIN_RING").ok().and_then(|v| v.parse().ok()) {
            app.world_mut().resource_mut::<Loadout>().player.ring = parts::Ring(i);
        }
        app.insert_resource(AutoTest { dir, step: 0, slow_shots: Vec::new(), hit_shots: Vec::new(), beast_shots: Vec::new() })
            .add_systems(PreUpdate, autotest.after(bevy::input::InputSystems));
    }
    app.run();
}

/// Bevy's built-in font has no CJK glyphs, so load a system Chinese font.
fn load_font(world: &mut World) {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let candidates = [
        format!("{windir}\\Fonts\\Deng.ttf"),
        format!("{windir}\\Fonts\\simhei.ttf"),
        format!("{windir}\\Fonts\\msyh.ttc"),
        "/System/Library/Fonts/PingFang.ttc".into(),
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc".into(),
    ];
    let font = candidates
        .iter()
        .find_map(|p| std::fs::read(p).ok().and_then(|b| Font::try_from_bytes(b).ok()));
    let handle = match font {
        Some(f) => world.resource_mut::<Assets<Font>>().add(f),
        None => {
            warn!("未找到中文字体，界面文字可能无法显示");
            Handle::default()
        }
    };
    world.insert_resource(UiFont(handle));
}

fn setup_scene(mut commands: Commands) {
    commands.spawn((MainCamera, Camera3d::default(), Transform::from_xyz(0.0, 7.0, 7.0)));
    commands.spawn((
        DirectionalLight { illuminance: 9000.0, shadows_enabled: true, ..default() },
        Transform::from_xyz(4.0, 10.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        PointLight { intensity: 400_000.0, range: 30.0, ..default() },
        Transform::from_xyz(-5.0, 6.0, -4.0),
    ));
}

fn update_camera(
    mut cam: ResMut<OrbitCam>,
    time: Res<Time<Real>>,
    mut q: Query<&mut Transform, With<MainCamera>>,
) {
    let Ok(mut tf) = q.single_mut() else { return };
    // Close-up: move in on the focus point and look down more steeply.
    let b = cam.focus_blend * cam.focus_blend * (3.0 - 2.0 * cam.focus_blend);
    let target = cam.target.lerp(cam.focus, b);
    let dist = cam.dist + (cam.close_dist - cam.dist) * b;
    let pitch = cam.pitch + (cam.close_pitch - cam.pitch) * b;
    let yaw = cam.yaw + cam.orbit * b;
    let offset = Vec3::new(
        dist * pitch.cos() * yaw.sin(),
        dist * pitch.sin(),
        dist * pitch.cos() * yaw.cos(),
    );
    let t = time.elapsed_secs();
    let shake = cam.shake * cam.shake;
    let jitter = Vec3::new((t * 71.0).sin(), (t * 53.0).cos(), (t * 67.0).sin()) * shake * 0.15;
    *tf = Transform::from_translation(target + offset + jitter)
        .looking_at(target + jitter * 0.5, Vec3::Y);
    cam.shake = (cam.shake - time.delta_secs() * 2.0).max(0.0);
}

#[derive(Resource)]
struct AutoTest {
    dir: String,
    step: usize,
    slow_shots: Vec<f32>,
    hit_shots: Vec<f32>,
    beast_shots: Vec<f32>,
}

fn autotest(
    mut commands: Commands,
    mut test: ResMut<AutoTest>,
    time: Res<Time<Real>>,
    slow: Res<slowmo::SlowMo>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut next: ResMut<NextState<AppState>>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::render::view::screenshot::{save_to_disk, Screenshot};
    let t = time.elapsed_secs();
    let script: &[(f32, &str)] = &[
        (2.5, "shot:1_assembly"),
        (3.0, "press:right"),
        (3.5, "shot:2_assembly_changed"),
        (4.0, "battle"),
        (6.0, "shot:3_aim"),
        (6.2, "press:space"),
        (6.8, "release:space"),
        (7.6, "shot:4_launch"),
        (10.0, "shot:5_fight"),
        (16.0, "shot:6_fight_later"),
        (40.0, "shot:7_end"),
        (41.0, "exit"),
    ];
    while test.step < script.len() && t >= script[test.step].0 {
        let action = script[test.step].1;
        test.step += 1;
        info!("autotest t={t:.1}: {action}");
        match action.split_once(':') {
            Some(("shot", name)) => {
                let path = format!("{}/{name}.png", test.dir);
                commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
            }
            Some(("press", "space")) => keys.press(KeyCode::Space),
            Some(("release", "space")) => keys.release(KeyCode::Space),
            Some(("press", "right")) => keys.press(KeyCode::ArrowRight),
            _ if action == "battle" => next.set(AppState::Battle),
            _ if action == "exit" => {
                exit.write(AppExit::Success);
            }
            _ => {}
        }
    }
    // Capture the first slow-motion impact: on entry, then 0.4 s and 0.9 s later.
    if slow.is_slow() && test.slow_shots.is_empty() {
        test.slow_shots = vec![t, t + 0.4, t + 0.9];
    }
    while let Some(&at) = test.slow_shots.first() {
        if at < 0.0 || t < at {
            break;
        }
        let n = 4 - test.slow_shots.len();
        let path = format!("{}/8_slowmo_{n}.png", test.dir);
        info!("autotest t={t:.1}: slow-mo shot {n}");
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
        test.slow_shots.remove(0);
        if test.slow_shots.is_empty() {
            test.slow_shots.push(-1.0);
        }
    }
    // And the first blade hit itself: at the hit, then 0.3 s and 0.8 s later.
    if slow.has_recent_hit() && test.hit_shots.is_empty() {
        test.hit_shots = vec![t, t + 0.3, t + 0.8];
    }
    while let Some(&at) = test.hit_shots.first() {
        if at < 0.0 || t < at {
            break;
        }
        let n = 4 - test.hit_shots.len();
        let path = format!("{}/9_bladehit_{n}.png", test.dir);
        info!("autotest t={t:.1}: blade-hit shot {n}");
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
        test.hit_shots.remove(0);
        if test.hit_shots.is_empty() {
            test.hit_shots.push(-1.0);
        }
    }
    // Mash Space during the fight: the beast fires once its energy is full.
    if (12.0..38.0).contains(&t) {
        if ((t * 2.0) as u32) % 2 == 0 {
            keys.press(KeyCode::Space);
        } else {
            keys.release(KeyCode::Space);
        }
    }
    // Capture the first beast cinematic.
    if slow.has_caption() && test.beast_shots.is_empty() {
        test.beast_shots = vec![t + 0.15, t + 0.7, t + 1.4];
    }
    while let Some(&at) = test.beast_shots.first() {
        if at < 0.0 || t < at {
            break;
        }
        let n = 4 - test.beast_shots.len();
        let path = format!("{}/10_beast_{n}.png", test.dir);
        info!("autotest t={t:.1}: beast shot {n}");
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
        test.beast_shots.remove(0);
        if test.beast_shots.is_empty() {
            test.beast_shots.push(-1.0);
        }
    }
    if test.step >= 3 {
        keys.release(KeyCode::ArrowRight);
    }
}
