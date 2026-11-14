//! Analytic blade-on-blade impacts.
//!
//! At full spin two tops' blades pass each other at ~150 units/s, while a
//! blade is under 0.15 thick. The engine checks contacts once per step, so it
//! either misses blade hits or finds them half-overlapped; and a rigid solver
//! resolving a real blade face locks the blades like gear teeth.
//!
//! So blade-vs-blade pairs are removed from the engine (see
//! [`crate::top::BladeHooks`]) and handled here instead:
//!
//! 1. **Exact detection.** Each physics step is split into sub-steps; both
//!    tops are extrapolated (position + velocity, rotation + angular velocity)
//!    and every blade pair is tested for overlap (top-down oriented
//!    rectangles + height range). The first overlap gives the hit time and
//!    point, so no hit is skipped however fast the tops spin.
//! 2. **Compliant impulse.** The impulse that would stop the blades along the
//!    hit direction is computed from both tops' real masses and inertia
//!    tensors, then scaled by `COMPLIANCE` (plastic blades flex and absorb
//!    most of the clash) and capped. The direction mixes the blade face
//!    normal (smash), the line between the centres (the tops separate) and
//!    the blade slopes (lift / press). Equal and opposite impulses at the
//!    contact point conserve momentum.
//! 3. **Blade abilities.** A special blade changes the split of the impulse
//!    (rebound throws more of it at the opponent, cushion takes less of it,
//!    see [`Ability::gains`]); an absorb blade also takes spin off the
//!    opponent and adds it to its own top.

use crate::parts::Ability;
use crate::top::{BladeGeom, Top};
use avian3d::prelude::*;
use bevy::prelude::*;
use std::collections::HashMap;

/// Share of the "rigid stop" impulse a blade clash actually delivers.
pub const COMPLIANCE: f32 = 0.3;
/// Restitution of a blade clash.
pub const RESTITUTION: f32 = 0.3;
/// Upper bound on one blade impulse.
pub const MAX_IMPULSE: f32 = 2.0;
/// Weight of the blade face normal in the hit direction (smash).
pub const FACE_WEIGHT: f32 = 0.6;
/// Weight of the line between centres in the hit direction (separation).
pub const CENTRE_WEIGHT: f32 = 1.0;
/// Sub-steps per physics step for the sweep.
const SWEEP_STEPS: usize = 12;
/// The same two blades can't hit again for this long (game seconds).
const PAIR_COOLDOWN: f32 = 0.03;
/// Absorb blade: spin (rad/s) taken off the opponent per unit of impulse...
const ABSORB_PER_IMPULSE: f32 = 3.0;
/// Spin taken by even the lightest touch (the rubber grips on contact).
const ABSORB_BASE: f32 = 1.2;
/// ...up to this much per hit, and never more than this share of its spin.
const ABSORB_MAX: f32 = 4.0;
const ABSORB_MAX_SHARE: f32 = 0.06;
/// Against an opponent turning the same way the rubber mostly just slips.
const ABSORB_SAME_DIR: f32 = 0.3;
/// Share of the stolen angular momentum the absorbing top keeps.
const ABSORB_EFFICIENCY: f32 = 0.8;
/// Absorbing never spins a top up beyond this (rad/s).
const ABSORB_SPIN_CAP: f32 = 215.0;

/// Multipliers on blade impulses (set by beast powers; 1.0 = plain physics).
/// `attack` scales the push this top deals, `defense` the push it receives.
#[derive(Component, Clone, Copy, Debug)]
pub struct ImpactScale {
    pub attack: f32,
    pub defense: f32,
}

impl Default for ImpactScale {
    fn default() -> Self {
        Self { attack: 1.0, defense: 1.0 }
    }
}

/// One blade impact, for effects (sparks, slow-motion markers, beast energy).
#[derive(Message, Clone, Copy, Debug)]
pub struct BladeHit {
    pub a: Entity,
    pub b: Entity,
    pub point: Vec3,
    /// Direction the second top was pushed.
    pub dir: Vec3,
    pub impulse: f32,
    /// Special blade that took part (if both did, the first top's).
    pub ability: Ability,
}

/// Running totals for the headless simulation.
#[derive(Resource, Default)]
pub struct BladeHitStats {
    pub hits: u32,
    pub max_impulse: f32,
}

pub fn plugin(app: &mut App) {
    app.add_message::<BladeHit>()
        .init_resource::<BladeHitStats>()
        .add_systems(FixedUpdate, blade_impacts);
}

/// A blade at some moment, in the horizontal plane plus its height range.
struct BladeNow {
    c: Vec2,
    u: Vec2,
    half_len: f32,
    half_thick: f32,
    y: f32,
    half_h: f32,
    slope: f32,
    ability: Ability,
}

#[derive(Clone, Copy)]
struct Body {
    e: Entity,
    pos: Vec3,
    rot: Quat,
    lin: Vec3,
    ang: Vec3,
    inv_mass: f32,
    com_local: Vec3,
}

impl Body {
    fn pose(&self, t: f32) -> (Vec3, Quat) {
        (self.pos + self.lin * t, (Quat::from_scaled_axis(self.ang * t) * self.rot).normalize())
    }
}

fn blades_at(geoms: &[BladeGeom], pos: Vec3, rot: Quat) -> Vec<BladeNow> {
    geoms
        .iter()
        .map(|g| {
            let c = pos + rot * g.center;
            let axis = rot * g.axis;
            BladeNow {
                c: Vec2::new(c.x, c.z),
                u: Vec2::new(axis.x, axis.z).normalize_or(Vec2::X),
                half_len: g.half_len,
                half_thick: g.half_thick,
                y: c.y,
                half_h: g.half_h,
                slope: g.slope,
                ability: g.ability,
            }
        })
        .collect()
}

fn corners(b: &BladeNow) -> [Vec2; 4] {
    let (u, v) = (b.u * b.half_len, b.u.perp() * b.half_thick);
    [b.c + u + v, b.c + u - v, b.c - u - v, b.c - u + v]
}

fn inside(p: Vec2, b: &BladeNow) -> bool {
    let d = p - b.c;
    d.dot(b.u).abs() <= b.half_len && d.dot(b.u.perp()).abs() <= b.half_thick
}

/// Separating-axis test for two oriented rectangles, plus height overlap.
fn overlap(a: &BladeNow, b: &BladeNow) -> bool {
    if (a.y - b.y).abs() > a.half_h + b.half_h {
        return false;
    }
    let (ca, cb) = (corners(a), corners(b));
    for axis in [a.u, a.u.perp(), b.u, b.u.perp()] {
        let pa = ca.iter().map(|p| p.dot(axis));
        let pb = cb.iter().map(|p| p.dot(axis));
        let (amin, amax) = pa.fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
        let (bmin, bmax) = pb.fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
        if amax < bmin || bmax < amin {
            return false;
        }
    }
    true
}

/// Approximate centre of the overlap region.
fn contact_point(a: &BladeNow, b: &BladeNow) -> Vec2 {
    let pts: Vec<Vec2> = corners(a)
        .into_iter()
        .filter(|p| inside(*p, b))
        .chain(corners(b).into_iter().filter(|p| inside(*p, a)))
        .collect();
    if pts.is_empty() {
        (a.c + b.c) * 0.5
    } else {
        pts.iter().sum::<Vec2>() / pts.len() as f32
    }
}

type BodyQuery<'a> = (
    Entity,
    &'a Top,
    &'a RigidBody,
    &'a Position,
    &'a Rotation,
    &'a mut LinearVelocity,
    &'a mut AngularVelocity,
    &'a ComputedMass,
    &'a ComputedAngularInertia,
    &'a ComputedCenterOfMass,
);

pub fn blade_impacts(
    mut tops: Query<BodyQuery>,
    scales: Query<&ImpactScale>,
    time: Res<Time>,
    mut hits: MessageWriter<BladeHit>,
    mut stats: ResMut<BladeHitStats>,
    mut cooldown: Local<HashMap<(Entity, usize, Entity, usize), f32>>,
) {
    let dt = time.delta_secs();
    let now = time.elapsed_secs();
    if dt <= 0.0 {
        return;
    }
    cooldown.retain(|_, t| now - *t < PAIR_COOLDOWN);

    let bodies: Vec<(Body, Vec<BladeGeom>, f32)> = tops
        .iter()
        .filter(|q| *q.2 == RigidBody::Dynamic && !q.1.blades.is_empty())
        .map(|(e, top, _, pos, rot, lin, ang, mass, _, com)| {
            let reach = top.blades.iter().map(|g| g.reach()).fold(0.0, f32::max);
            let body = Body {
                e,
                pos: pos.0,
                rot: rot.0,
                lin: lin.0,
                ang: ang.0,
                inv_mass: mass.inverse(),
                com_local: com.0,
            };
            (body, top.blades.clone(), reach)
        })
        .collect();

    for i in 0..bodies.len() {
        for j in (i + 1)..bodies.len() {
            let (a, ga, ra) = &bodies[i];
            let (b, gb, rb) = &bodies[j];
            let mut d = b.pos - a.pos;
            d.y = 0.0;
            let travel = (b.lin - a.lin).length() * dt;
            if d.length() > ra + rb + travel + 0.05 {
                continue;
            }
            if let Some(hit) = sweep(a, ga, b, gb, dt, &cooldown) {
                cooldown.insert((a.e, hit.ia, b.e, hit.ib), now);
                let scale = |e: Entity| scales.get(e).copied().unwrap_or_default();
                let (sa, sb) = (scale(a.e), scale(b.e));
                // Push on b, push on a.
                let gains = (sa.attack * sb.defense, sb.attack * sa.defense);
                if let Some(h) = resolve(&mut tops, a, b, &hit, gains) {
                    stats.hits += 1;
                    stats.max_impulse = stats.max_impulse.max(h.impulse);
                    hits.write(h);
                }
            }
        }
    }
}

struct Contact {
    t: f32,
    ia: usize,
    ib: usize,
    point: Vec3,
    face: Vec3,
    slope_a: f32,
    slope_b: f32,
    ability_a: Ability,
    ability_b: Ability,
}

/// First blade overlap between `a` and `b` within this step.
fn sweep(
    a: &Body,
    ga: &[BladeGeom],
    b: &Body,
    gb: &[BladeGeom],
    dt: f32,
    cooldown: &HashMap<(Entity, usize, Entity, usize), f32>,
) -> Option<Contact> {
    for s in 1..=SWEEP_STEPS {
        let t = dt * s as f32 / SWEEP_STEPS as f32;
        let (pa, qa) = a.pose(t);
        let (pb, qb) = b.pose(t);
        let ba = blades_at(ga, pa, qa);
        let bb = blades_at(gb, pb, qb);
        for (ia, x) in ba.iter().enumerate() {
            for (ib, y) in bb.iter().enumerate() {
                if cooldown.contains_key(&(a.e, ia, b.e, ib)) || !overlap(x, y) {
                    continue;
                }
                let p2 = contact_point(x, y);
                // Face normal of blade a, pointing towards blade b.
                let mut n = x.u.perp();
                if n.dot(y.c - x.c) < 0.0 {
                    n = -n;
                }
                return Some(Contact {
                    t,
                    ia,
                    ib,
                    point: Vec3::new(p2.x, 0.5 * (x.y + y.y), p2.y),
                    face: Vec3::new(n.x, 0.0, n.y),
                    slope_a: x.slope,
                    slope_b: y.slope,
                    ability_a: x.ability,
                    ability_b: y.ability,
                });
            }
        }
    }
    None
}

/// `gains` scale the impulse on (b, a). Both are 1.0 in plain physics, which
/// conserves momentum; beast powers make a hit lopsided.
fn resolve(
    tops: &mut Query<BodyQuery>,
    a: &Body,
    b: &Body,
    c: &Contact,
    gains: (f32, f32),
) -> Option<BladeHit> {
    let (pa, qa) = a.pose(c.t);
    let (pb, qb) = b.pose(c.t);
    let ra = c.point - (pa + qa * a.com_local);
    let rb = c.point - (pb + qb * b.com_local);
    let inv_ia = tops.get(a.e).ok()?.8.rotated(qa).inverse_tensor();
    let inv_ib = tops.get(b.e).ok()?.8.rotated(qb).inverse_tensor();

    let v_rel = (b.lin + b.ang.cross(rb)) - (a.lin + a.ang.cross(ra));
    if v_rel.dot(c.face) >= 0.0 {
        return None; // blades already moving apart
    }
    let mut centres = pb - pa;
    centres.y = 0.0;
    let centres = centres.normalize_or(c.face);
    let horizontal = (c.face * FACE_WEIGHT + centres * CENTRE_WEIGHT).normalize_or(centres);
    let tilt = 0.5 * (c.slope_a.cos() + c.slope_b.cos());
    let dir = (horizontal * tilt + Vec3::Y * (c.slope_a.sin() - c.slope_b.sin())).normalize();

    let approach = -v_rel.dot(dir);
    if approach <= 0.0 {
        return None;
    }
    let k = a.inv_mass
        + b.inv_mass
        + inv_ia.mul_vec3(ra.cross(dir)).cross(ra).dot(dir)
        + inv_ib.mul_vec3(rb.cross(dir)).cross(rb).dot(dir);
    let impulse = (COMPLIANCE * (1.0 + RESTITUTION) * approach / k).min(MAX_IMPULSE);
    let j = dir * impulse;

    // Blade abilities: (push its own top takes, push the opponent takes).
    let ((own_a, opp_a), (own_b, opp_b)) = (c.ability_a.gains(), c.ability_b.gains());
    let gains = (gains.0 * opp_a * own_b, gains.1 * own_a * opp_b);
    let (jb, ja) = (j * gains.0, j * gains.1);
    if let Ok(q) = tops.get_mut(b.e) {
        let (mut lin, mut ang) = (q.5, q.6);
        lin.0 += jb * b.inv_mass;
        ang.0 += inv_ib.mul_vec3(rb.cross(jb));
    }
    if let Ok(q) = tops.get_mut(a.e) {
        let (mut lin, mut ang) = (q.5, q.6);
        lin.0 -= ja * a.inv_mass;
        ang.0 -= inv_ia.mul_vec3(ra.cross(ja));
    }
    // Absorb: the rubber grips the other blade and drags spin across.
    let (axis_a, axis_b) = (qa * Vec3::Y, qb * Vec3::Y);
    let i_a = 1.0 / inv_ia.mul_vec3(axis_a).dot(axis_a).max(1e-6);
    let i_b = 1.0 / inv_ib.mul_vec3(axis_b).dot(axis_b).max(1e-6);
    for (thief, victim, axis_t, axis_v, i_t, i_v, active) in [
        (a.e, b.e, axis_a, axis_b, i_a, i_b, c.ability_a == Ability::Absorb),
        (b.e, a.e, axis_b, axis_a, i_b, i_a, c.ability_b == Ability::Absorb),
    ] {
        if !active {
            continue;
        }
        let Ok(spin_t) = tops.get(thief).map(|q| q.6.0.dot(axis_t)) else { continue };
        let Ok(mut q) = tops.get_mut(victim) else { continue };
        let spin_v = q.6.0.dot(axis_v);
        let grip = if spin_t * spin_v < 0.0 { 1.0 } else { ABSORB_SAME_DIR };
        let taken = (ABSORB_BASE + ABSORB_PER_IMPULSE * impulse).min(ABSORB_MAX).min(spin_v.abs() * ABSORB_MAX_SHARE) * grip;
        q.6.0 -= axis_v * spin_v.signum() * taken;
        let gained = (taken * ABSORB_EFFICIENCY * i_v / i_t).min((ABSORB_SPIN_CAP - spin_t.abs()).max(0.0));
        if let Ok(mut q) = tops.get_mut(thief) {
            q.6.0 += axis_t * spin_t.signum() * gained;
        }
    }
    let ability = if c.ability_a != Ability::None { c.ability_a } else { c.ability_b };
    Some(BladeHit { a: a.e, b: b.e, point: c.point, dir, impulse: impulse * gains.0.max(gains.1), ability })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blade(x: f32, z: f32, angle: f32) -> BladeNow {
        BladeNow {
            c: Vec2::new(x, z),
            u: Vec2::from_angle(angle),
            half_len: 0.1,
            half_thick: 0.03,
            y: 0.3,
            half_h: 0.05,
            slope: 0.0,
            ability: Ability::None,
        }
    }

    #[test]
    fn crossing_blades_overlap() {
        assert!(overlap(&blade(0.0, 0.0, 0.0), &blade(0.0, 0.0, 1.5)));
        assert!(overlap(&blade(0.0, 0.0, 0.0), &blade(0.15, 0.0, 0.0)));
    }

    #[test]
    fn separated_blades_do_not_overlap() {
        assert!(!overlap(&blade(0.0, 0.0, 0.0), &blade(0.0, 0.1, 0.0)));
        assert!(!overlap(&blade(0.0, 0.0, 0.0), &blade(0.25, 0.0, 0.0)));
        let mut high = blade(0.0, 0.0, 0.0);
        high.y = 0.5;
        assert!(!overlap(&blade(0.0, 0.0, 0.0), &high));
    }
}
