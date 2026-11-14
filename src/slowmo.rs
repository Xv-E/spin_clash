//! Slow motion for collisions.
//!
//! Slowing down `Time<Virtual>` slows the fixed physics schedule with it, so
//! the physics is exactly the same — the same 1/240 s steps — just shown over
//! more real time. Transform interpolation keeps the motion smooth.
//!
//! - Auto: predicts an impact a moment before the tops touch, slows down and
//!   moves the camera in on it, looking from the side square to the knock-back
//!   direction so the tops visibly fly apart across the screen.
//! - Hold F for manual slow motion, P to pause, T to toggle auto mode.
//! - While slowed or paused, contact points, contact normals, velocities and
//!   spin axes are drawn.

use crate::top::{axial_spin, Top};
use crate::{AppState, OrbitCam, UiFont};
use avian3d::prelude::*;
use bevy::prelude::*;

/// Playback speed during slow motion (1/20 of real time).
pub const SLOW_SPEED: f32 = 0.05;
/// How far ahead (in game seconds) an impact is predicted.
const LOOKAHEAD: f32 = 0.06;
/// Minimum closing speed for an auto slow-motion trigger.
const MIN_CLOSING: f32 = 1.5;
/// Real seconds an auto slow motion lasts.
const HOLD: f32 = 2.2;
/// Real seconds before auto mode can trigger again.
const COOLDOWN: f32 = 1.5;
/// Both tops must still spin at least this fast (rad/s) for auto mode.
const MIN_SPIN: f32 = 25.0;
/// Impact bullet-time camera: how close (while the tops close in, then once
/// they touch) and how low. It looks from the side, square to the knock-back
/// direction, so the tops fly apart across the screen and a lift shows as
/// rising off the floor.
const IMPACT_DIST: f32 = 1.7;
const IMPACT_CONTACT_DIST: f32 = 1.25;
const IMPACT_PITCH: f32 = 0.22;
/// How fast (1/real s) the camera swings round to that side view.
const SWING_RATE: f32 = 4.0;
/// How fast the beast cinematic circles its top (rad/s).
const ORBIT_SPEED: f32 = 0.5;
/// How fast (1/real s) the close-up distance eases to its new value.
const ZOOM_RATE: f32 = 3.0;

/// What the bullet-time camera looks at.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Focus {
    /// Midpoint of the two tops (manual slow motion / pause).
    Midpoint,
    /// A predicted or actual impact point.
    Point(Vec3),
    /// Follow one top (beast activation).
    Top(Entity),
}

pub struct SlowMoPlugin;

impl Plugin for SlowMoPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(SlowMo::default())
            .add_systems(OnEnter(AppState::Battle), spawn_indicator)
            .add_systems(OnExit(AppState::Battle), reset_on_exit)
            .add_systems(
                Update,
                (controls, detect_impact, record_hits, apply_speed, indicator)
                    .chain()
                    .run_if(in_state(AppState::Battle)),
            );
    }
}

#[derive(Resource)]
pub struct SlowMo {
    pub auto: bool,
    manual: bool,
    paused: bool,
    /// Real seconds of auto slow motion left.
    hold: f32,
    cooldown: f32,
    /// Current playback speed (eased).
    speed: f32,
    /// Recent blade hits: (point, push direction, impulse, game time).
    recent_hits: Vec<(Vec3, Vec3, f32, f32)>,
    focus: Focus,
    /// Horizontal knock-back direction of the current impact (its sign does
    /// not matter: both tops fly apart along it).
    knock: Vec3,
    /// The tops have actually touched during this bullet time: the camera
    /// has moved from the predicted point to the real contact and zoomed in.
    contact_seen: bool,
    /// Caption shown while a beast cinematic plays.
    caption: Option<String>,
}

impl Default for SlowMo {
    fn default() -> Self {
        Self {
            auto: true,
            manual: false,
            paused: false,
            hold: 0.0,
            cooldown: 0.0,
            speed: 1.0,
            recent_hits: Vec::new(),
            focus: Focus::Midpoint,
            knock: Vec3::X,
            contact_seen: false,
            caption: None,
        }
    }
}

impl SlowMo {
    pub fn reset(&mut self, vtime: &mut Time<Virtual>) {
        *self = SlowMo { auto: self.auto, ..default() };
        vtime.set_relative_speed(1.0);
        vtime.unpause();
    }
    /// Bullet time on one top (beast activation), with a caption.
    pub fn cinematic(&mut self, top: Entity, real_secs: f32, caption: String) {
        self.hold = self.hold.max(real_secs);
        self.cooldown = self.cooldown.max(real_secs + 0.5);
        self.focus = Focus::Top(top);
        self.caption = Some(caption);
    }
    /// A beast cinematic is playing (autotest).
    pub fn has_caption(&self) -> bool {
        self.caption.is_some()
    }
    /// A blade hit happened in the last 0.25 game seconds (autotest).
    pub fn has_recent_hit(&self) -> bool {
        !self.recent_hits.is_empty()
    }
    /// Fully in slow motion (used by the screenshot autotest).
    pub fn is_slow(&self) -> bool {
        self.speed < SLOW_SPEED * 2.0
    }
}

#[derive(Component)]
struct Indicator;

fn spawn_indicator(mut commands: Commands, font: Res<UiFont>) {
    commands
        .spawn((
            DespawnOnExit(AppState::Battle),
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(64.0),
                width: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .with_children(|p| {
            p.spawn((
                Node {
                    padding: UiRect::axes(Val::Px(14.0), Val::Px(6.0)),
                    border_radius: BorderRadius::all(Val::Px(8.0)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.03, 0.04, 0.08, 0.75)),
            ))
            .with_children(|p| {
                p.spawn((
                Indicator,
                Text::new(""),
                TextFont { font: font.0.clone(), font_size: 18.0, ..default() },
                TextColor(Color::srgb(1.0, 0.85, 0.3)),
                TextLayout::new_with_justify(Justify::Center),
            ));
            });
        });
}

fn reset_on_exit(mut slow: ResMut<SlowMo>, mut vtime: ResMut<Time<Virtual>>) {
    slow.reset(&mut vtime);
}

fn controls(keys: Res<ButtonInput<KeyCode>>, mut slow: ResMut<SlowMo>) {
    if keys.just_pressed(KeyCode::KeyT) {
        slow.auto = !slow.auto;
        if !slow.auto {
            slow.hold = 0.0;
        }
    }
    if keys.just_pressed(KeyCode::KeyP) {
        slow.paused = !slow.paused;
    }
    slow.manual = keys.pressed(KeyCode::KeyF);
}

/// Predict an impact: the gap between the two outlines closes within LOOKAHEAD.
fn detect_impact(
    mut slow: ResMut<SlowMo>,
    real: Res<Time<Real>>,
    tops: Query<(Entity, &Top, &RigidBody, &Position, &LinearVelocity, &Rotation, &AngularVelocity)>,
    mut cam: ResMut<OrbitCam>,
    graph: Res<ContactGraph>,
) {
    let dt = real.delta_secs();
    if !slow.paused {
        slow.hold = (slow.hold - dt).max(0.0);
        slow.cooldown = (slow.cooldown - dt).max(0.0);
        if slow.hold <= 0.0 {
            slow.caption = None;
        }
    }
    let live: Vec<_> = tops.iter().filter(|t| *t.2 == RigidBody::Dynamic).collect();
    let [(ea, ta, _, pa, va, ra, wa), (eb, tb, _, pb, vb, rb, wb)] = live.as_slice() else {
        cam.focus_blend = (cam.focus_blend - dt * 2.0).max(0.0);
        return;
    };
    let mut d = pb.0 - pa.0;
    d.y = 0.0;
    let dist = d.length().max(1e-4);
    let n = d / dist;
    let gap = dist - (ta.radius + tb.radius);
    let closing = (va.0 - vb.0).dot(n);

    // Where the outer edges meet along the line between the centres.
    let reach = |t: &Top| t.blades.iter().map(|g| g.reach()).fold(t.radius, f32::max);
    let edge_point = (pa.0 + n * reach(ta) + pb.0 - n * reach(tb)) * 0.5 + Vec3::Y * crate::rings::RING_MID;

    // Only real clashes: skip tops that are already toppling or out.
    let spinning = axial_spin(ra, wa).abs() > MIN_SPIN && axial_spin(rb, wb).abs() > MIN_SPIN;
    if slow.auto
        && spinning
        && slow.cooldown <= 0.0
        && closing > MIN_CLOSING
        && gap < closing * LOOKAHEAD + 0.02
    {
        slow.hold = HOLD;
        slow.cooldown = HOLD + COOLDOWN;
        slow.focus = Focus::Point(edge_point);
        slow.knock = n;
        slow.contact_seen = false;
        slow.caption = None;
    }

    // Impact bullet time: aim at the real contact between the two bodies
    // while they touch; until the first touch, keep re-predicting from the
    // current positions; after they part, stay on the last contact.
    if slow.hold > 0.0 && matches!(slow.focus, Focus::Point(_)) {
        if let Some((p, normal)) = body_contact(&graph, *ea, *eb) {
            slow.focus = Focus::Point(p);
            slow.set_knock(normal);
            slow.contact_seen = true;
        } else if !slow.contact_seen {
            slow.focus = Focus::Point(edge_point);
            slow.knock = n;
        }
    }

    // Bullet-time camera.
    let close_up = slow.hold > 0.0 || slow.manual || slow.paused;
    if !close_up && slow.hold <= 0.0 {
        slow.focus = Focus::Midpoint;
    }
    let goal = match slow.focus {
        Focus::Point(p) => p,
        // Beast cinematic: frame the top and the beast rising above it.
        Focus::Top(e) if e == *ea => pa.0 + Vec3::Y * 0.9,
        Focus::Top(e) if e == *eb => pb.0 + Vec3::Y * 0.9,
        _ => (pa.0 + pb.0) * 0.5 + Vec3::Y * crate::rings::RING_MID,
    };
    let follow = if matches!(slow.focus, Focus::Top(_)) { 14.0 } else { 6.0 };
    cam.focus = cam.focus.lerp(goal, (dt * follow).min(1.0));
    let impact = matches!(slow.focus, Focus::Point(_) | Focus::Top(_));
    let touched = slow.contact_seen;
    let (dist, pitch) = match slow.focus {
        Focus::Point(_) if touched => (IMPACT_CONTACT_DIST, IMPACT_PITCH),
        Focus::Point(_) => (IMPACT_DIST, IMPACT_PITCH),
        Focus::Top(_) => (3.9, 0.2),
        Focus::Midpoint => (2.6, 0.55),
    };
    // Ease the zoom while in a close-up (a push-in, not a jump); snap it
    // while the close-up is faded out.
    let k = if cam.focus_blend > 0.0 { (dt * ZOOM_RATE).min(1.0) } else { 1.0 };
    cam.close_dist += (dist - cam.close_dist) * k;
    cam.close_pitch = pitch;
    let target = if close_up { 1.0 } else { 0.0 };
    let rate = if close_up { 3.0 } else { 1.5 };
    cam.focus_blend += (target - cam.focus_blend).clamp(-rate * dt, rate * dt);
    if close_up && matches!(slow.focus, Focus::Point(_)) {
        // Swing to the side view square to the knock-back (on whichever side
        // is closer to where the camera already is).
        let goal = side_yaw(slow.knock, cam.yaw + cam.orbit) - cam.yaw;
        cam.orbit += wrap_angle(goal - cam.orbit) * (dt * SWING_RATE).min(1.0);
    } else if close_up && impact && !slow.paused {
        // Beast cinematic: circle the top.
        cam.orbit += ORBIT_SPEED * dt;
    } else if cam.focus_blend <= 0.0 {
        cam.orbit = 0.0;
    }
}

/// Camera yaw looking square onto `knock` (horizontal), picking of the two
/// sides the one nearer to `current`.
fn side_yaw(knock: Vec3, current: f32) -> f32 {
    // The camera sits at (sin yaw, cos yaw) from its target: put it on the
    // line perpendicular to the knock-back.
    let yaw = (-knock.z).atan2(knock.x);
    let other = yaw + std::f32::consts::PI;
    if wrap_angle(yaw - current).abs() <= wrap_angle(other - current).abs() { yaw } else { other }
}

fn wrap_angle(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (a + PI).rem_euclid(TAU) - PI
}

impl SlowMo {
    /// Take the horizontal part of a push direction as the knock-back axis
    /// (a nearly vertical push keeps the previous one).
    fn set_knock(&mut self, dir: Vec3) {
        if let Some(k) = dir.with_y(0.0).try_normalize() {
            self.knock = k;
        }
    }
}

/// Average contact point and normal between two top bodies, if they are
/// touching now.
fn body_contact(graph: &ContactGraph, a: Entity, b: Entity) -> Option<(Vec3, Vec3)> {
    let (mut sum, mut normal, mut n) = (Vec3::ZERO, Vec3::ZERO, 0.0);
    for pair in graph.iter_active_touching() {
        let (Some(b1), Some(b2)) = (pair.body1, pair.body2) else { continue };
        if !((b1 == a && b2 == b) || (b1 == b && b2 == a)) {
            continue;
        }
        // Normals point from body 1 to body 2: orient them all from a to b.
        let sign = if b1 == a { 1.0 } else { -1.0 };
        for m in &pair.manifolds {
            for c in &m.points {
                sum += c.point;
                normal += m.normal * sign;
                n += 1.0;
            }
        }
    }
    (n > 0.0).then(|| (sum / n, normal))
}

/// Keep blade hits around for 0.25 game seconds so slow motion can show them.
fn record_hits(
    mut slow: ResMut<SlowMo>,
    time: Res<Time>,
    mut hits: MessageReader<crate::blade_hits::BladeHit>,
) {
    let now = time.elapsed_secs();
    for h in hits.read() {
        slow.recent_hits.push((h.point, h.dir, h.impulse, now));
        // Snap the camera to the real hit point (unless a beast cinematic
        // is playing). A hit the prediction missed starts bullet time too.
        if !matches!(slow.focus, Focus::Top(_)) {
            if slow.hold > 0.0 {
                slow.focus = Focus::Point(h.point);
                slow.set_knock(h.dir);
                slow.contact_seen = true;
            } else if slow.auto && slow.cooldown <= 0.0 {
                slow.hold = HOLD;
                slow.cooldown = HOLD + COOLDOWN;
                slow.focus = Focus::Point(h.point);
                slow.set_knock(h.dir);
                slow.contact_seen = true;
            }
        }
    }
    slow.recent_hits.retain(|h| now - h.3 < 0.25);
}

fn apply_speed(mut slow: ResMut<SlowMo>, real: Res<Time<Real>>, mut vtime: ResMut<Time<Virtual>>) {
    if slow.paused {
        vtime.pause();
        return;
    }
    vtime.unpause();
    let want = if slow.manual || slow.hold > 0.0 { SLOW_SPEED } else { 1.0 };
    let dt = real.delta_secs();
    // Drop into slow motion almost instantly, ramp back out gently.
    let rate = if want < slow.speed { 25.0 } else { 2.5 };
    slow.speed += (want - slow.speed) * (rate * dt).min(1.0);
    if (slow.speed - want).abs() < 0.002 {
        slow.speed = want;
    }
    vtime.set_relative_speed(slow.speed);
}

fn indicator(slow: Res<SlowMo>, mut q: Query<&mut Text, With<Indicator>>) {
    let Ok(mut t) = q.single_mut() else { return };
    let auto = if slow.auto { "开" } else { "关" };
    t.0 = if slow.paused {
        format!("暂停中（P 继续）   自动慢动作：{auto}")
    } else if let Some(caption) = &slow.caption {
        caption.clone()
    } else if slow.speed < 0.99 {
        "慢动作".to_string()
    } else {
        format!("自动慢动作：{auto}（T 切换）")
    };
}
