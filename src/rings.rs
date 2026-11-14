//! Data-driven attack rings, loaded from `assets/rings.ron`.
//!
//! A ring is a core (merged into the body's smooth convex outline) plus sets
//! of blades. Every blade is its own wedge-shaped convex collider whose side
//! faces can be sloped: the slope decides whether a hit lifts the opponent
//! (upper attack), shoves it sideways (smash) or presses it down. Inserts add
//! mass without touching anything.
//!
//! Colliders stay simple (core + wedges), but the ring is *drawn* as one
//! sculpted piece: the union of core and blades, each blade trailing off into
//! a curved back like the claws of a real attack blade (see [`RingDef::mesh`]).
//!
//! A blade set can also carry an [`Ability`]: absorb (steals spin), rebound
//! (throws the hit back) or cushion (soaks the hit up).

use crate::parts::{wedge_half_widths, Contact, Piece, Shape, METAL, PLASTIC, RUBBER};
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use serde::Deserialize;
use std::sync::OnceLock;

/// Bottom and height of the attack ring layer.
pub const RING_BOTTOM: f32 = 0.4;
pub const RING_HEIGHT: f32 = 0.12;
/// Height of the middle of the ring above the tip's bottom (where tops clash).
pub const RING_MID: f32 = RING_BOTTOM + RING_HEIGHT * 0.5;
/// Above this many blade colliders per ring the physics gets slow.
const MAX_BLADES: u32 = 12;

const BUILTIN: &str = include_str!("../assets/rings.ron");
const RING_FILE: &str = "assets/rings.ron";

/// Handle to an entry of the ring catalogue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Ring(pub usize);

#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub enum Material {
    #[default]
    Plastic,
    Metal,
    Rubber,
}

impl Material {
    fn density(self) -> f32 {
        match self {
            Material::Plastic => PLASTIC,
            Material::Metal => METAL,
            Material::Rubber => RUBBER,
        }
    }
}

/// Special function of a blade set.
#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ability {
    #[default]
    None,
    /// Rubber that grips the opponent: steals spin on every hit (most against
    /// an opponent spinning the other way).
    Absorb,
    /// Sprung blade: throws the hit back, the opponent flies further.
    Rebound,
    /// Soft damper: soaks up most of the push this top would take.
    Cushion,
}

impl Ability {
    pub fn name(self) -> &'static str {
        match self {
            Ability::None => "",
            Ability::Absorb => "吸收",
            Ability::Rebound => "反弹",
            Ability::Cushion => "缓冲",
        }
    }
    /// Colour of the blades (and of the sparks when the ability triggers).
    pub fn color(self) -> Option<Color> {
        match self {
            Ability::None => None,
            Ability::Absorb => Some(Color::srgb(0.62, 0.22, 0.95)),
            Ability::Rebound => Some(Color::srgb(0.75, 1.0, 0.2)),
            Ability::Cushion => Some(Color::srgb(1.0, 0.45, 0.7)),
        }
    }
    /// Multipliers on a blade impulse: (push its own top takes, push the
    /// opponent takes).
    pub fn gains(self) -> (f32, f32) {
        match self {
            Ability::None => (1.0, 1.0),
            Ability::Absorb => (0.75, 0.75),
            Ability::Rebound => (0.7, 1.45),
            Ability::Cushion => (0.4, 0.85),
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
pub enum CoreDef {
    Cylinder { radius: f32 },
    Frustum { top: f32, bottom: f32 },
}

fn one() -> f32 {
    1.0
}

#[derive(Deserialize, Clone, Debug)]
pub struct BladeSet {
    pub count: u32,
    pub radius: f32,
    pub length: f32,
    pub thickness: f32,
    #[serde(default = "one")]
    pub height: f32,
    /// Degrees. Positive: faces tilt upward (lifts the opponent).
    #[serde(default)]
    pub slope: f32,
    #[serde(default)]
    pub rake: f32,
    #[serde(default)]
    pub phase: f32,
    #[serde(default)]
    pub material: Material,
    /// Width of the blade tip as a share of its root (1 = rectangle).
    #[serde(default = "default_taper")]
    pub taper: f32,
    /// How far the blade's curved back trails behind its tip, as a share of
    /// the gap to the next blade (0 = a plain wedge). Visual only.
    #[serde(default = "default_sweep")]
    pub sweep: f32,
    #[serde(default)]
    pub ability: Ability,
}

fn default_sweep() -> f32 {
    0.55
}

fn default_taper() -> f32 {
    0.45
}

fn metal() -> Material {
    Material::Metal
}

#[derive(Deserialize, Clone, Debug)]
pub struct InsertSet {
    pub count: u32,
    pub radius: f32,
    pub size: (f32, f32, f32),
    #[serde(default)]
    pub phase: f32,
    #[serde(default = "metal")]
    pub material: Material,
}

#[derive(Deserialize, Clone, Debug)]
pub struct RingDef {
    pub name: String,
    pub desc: String,
    pub color: (f32, f32, f32),
    pub air_drag: f32,
    pub core: CoreDef,
    #[serde(default)]
    pub blades: Vec<BladeSet>,
    #[serde(default)]
    pub inserts: Vec<InsertSet>,
}

fn parse(text: &str) -> Result<Vec<RingDef>, String> {
    let defs: Vec<RingDef> = ron::from_str(text).map_err(|e| e.to_string())?;
    if defs.is_empty() {
        return Err("文件里没有任何攻击环".into());
    }
    for d in &defs {
        let n: u32 = d.blades.iter().map(|b| b.count).sum();
        if n > MAX_BLADES {
            return Err(format!("{}: 刃片共 {n} 片，超过上限 {MAX_BLADES}", d.name));
        }
        for b in &d.blades {
            if b.slope.abs() > 60.0 {
                return Err(format!("{}: 刃面倾角 {} 超出 ±60°", d.name, b.slope));
            }
        }
    }
    Ok(defs)
}

/// The ring catalogue: `assets/rings.ron` if it exists and parses, otherwise
/// the copy compiled into the game.
pub fn ring_defs() -> &'static [RingDef] {
    static DEFS: OnceLock<Vec<RingDef>> = OnceLock::new();
    DEFS.get_or_init(|| {
        if let Ok(text) = std::fs::read_to_string(RING_FILE) {
            match parse(&text) {
                Ok(defs) => return defs,
                Err(e) => eprintln!("{RING_FILE} 解析失败，改用内置数据：{e}"),
            }
        }
        parse(BUILTIN).expect("built-in rings.ron must parse")
    })
}

impl RingDef {
    pub fn color(&self) -> Color {
        Color::srgb(self.color.0, self.color.1, self.color.2)
    }

    pub fn core_radius(&self) -> f32 {
        match self.core {
            CoreDef::Cylinder { radius } => radius,
            CoreDef::Frustum { top, bottom } => top.max(bottom),
        }
    }

    /// Short summary of the blade geometry for the UI.
    pub fn blade_summary(&self) -> String {
        if self.blades.is_empty() {
            return "无刃片（圆环）".into();
        }
        self.blades
            .iter()
            .map(|b| {
                let kind = if b.slope > 8.0 {
                    "上挑"
                } else if b.slope < -8.0 {
                    "压制"
                } else {
                    "重击"
                };
                let special = match b.ability {
                    Ability::None => String::new(),
                    a => format!("【{}】", a.name()),
                };
                format!("{}片 倾角{:+.0}° {kind}{special}", b.count, b.slope)
            })
            .collect::<Vec<_>>()
            .join("，")
    }

    pub fn pieces(&self, accent: Color) -> Vec<Piece> {
        let h = RING_HEIGHT;
        let yc = RING_BOTTOM + h * 0.5;
        let core = match self.core {
            CoreDef::Cylinder { radius } => Shape::Cylinder { r: radius, h },
            CoreDef::Frustum { top, bottom } => Shape::Frustum { r_top: top, r_bottom: bottom, h },
        };
        // Core and blades are drawn together by `mesh`, not piece by piece.
        let mut v = vec![Piece::new(core, Vec3::Y * yc, PLASTIC, self.color()).hidden()];
        let steel = Color::srgb(0.7, 0.7, 0.75);
        for b in &self.blades {
            let bh = h * b.height.clamp(0.2, 1.0);
            let color = if b.material == Material::Metal { steel } else { accent };
            for i in 0..b.count {
                let a = (i as f32 / b.count as f32) * std::f32::consts::TAU + b.phase.to_radians();
                let around = Quat::from_rotation_y(-a);
                let shape = Shape::Wedge {
                    l: b.length,
                    t: b.thickness,
                    h: bh,
                    slope: b.slope.to_radians(),
                    taper: b.taper.clamp(0.1, 1.0),
                };
                // Blades sit on the ring's floor so shorter blades stay low.
                let y = RING_BOTTOM + bh * 0.5;
                v.push(
                    Piece::new(shape, around * Vec3::new(b.radius, y, 0.0), b.material.density(), color)
                        .rot(around * Quat::from_rotation_y(b.rake.to_radians()))
                        .contact(Contact::Own)
                        .ability(b.ability)
                        .hidden(),
                );
            }
        }
        for ins in &self.inserts {
            for i in 0..ins.count {
                let a = (i as f32 / ins.count as f32) * std::f32::consts::TAU + ins.phase.to_radians();
                let around = Quat::from_rotation_y(-a);
                let size = Vec3::new(ins.size.0, ins.size.1, ins.size.2);
                v.push(
                    Piece::new(
                        Shape::Cuboid(size),
                        around * Vec3::new(ins.radius, yc, 0.0),
                        ins.material.density(),
                        steel,
                    )
                    .rot(around)
                    .contact(Contact::MassOnly),
                );
            }
        }
        v
    }
}

/// A blade as seen from above, for the ring's outline.
struct Lobe {
    /// Footprint corners (x, z).
    quad: [Vec2; 4],
    tip_angle: f32,
    tip_radius: f32,
    /// Angle (radians) the curved back trails behind the tip.
    sweep: f32,
    /// Height of the blade's top above the ring floor.
    top: f32,
    slope: f32,
    color: Color,
}

/// Furthest point where the ray from the origin along `d` crosses the polygon.
fn ray_far(d: Vec2, poly: &[Vec2; 4]) -> Option<f32> {
    let mut far: Option<f32> = None;
    for i in 0..4 {
        let (a, e) = (poly[i], poly[(i + 1) % 4] - poly[i]);
        let denom = d.perp_dot(e);
        if denom.abs() < 1e-9 {
            continue;
        }
        let (s, u) = (a.perp_dot(e) / denom, a.perp_dot(d) / denom);
        if s > 0.0 && (0.0..=1.0).contains(&u) {
            far = Some(far.map_or(s, |f| f.max(s)));
        }
    }
    far
}

impl RingDef {
    fn lobes(&self, accent: Color) -> Vec<Lobe> {
        let steel = Color::srgb(0.72, 0.73, 0.78);
        let mut v = Vec::new();
        for b in &self.blades {
            let bh = RING_HEIGHT * b.height.clamp(0.2, 1.0);
            let slope = b.slope.to_radians();
            let (ib, it) = wedge_half_widths(b.thickness, bh, slope);
            let (ob, ot) = wedge_half_widths(b.thickness * b.taper.clamp(0.1, 1.0), bh, slope);
            let (wi, wo, x) = (ib.max(it), ob.max(ot), b.length * 0.5);
            let color = b.ability.color().unwrap_or(if b.material == Material::Metal { steel } else { accent });
            for i in 0..b.count {
                let a = (i as f32 / b.count as f32) * std::f32::consts::TAU + b.phase.to_radians();
                let around = Quat::from_rotation_y(-a);
                let pos = around * Vec3::new(b.radius, 0.0, 0.0);
                let rot = around * Quat::from_rotation_y(b.rake.to_radians());
                let at = |x: f32, z: f32| {
                    let p = pos + rot * Vec3::new(x, 0.0, z);
                    Vec2::new(p.x, p.z)
                };
                let tip = at(x, 0.0);
                v.push(Lobe {
                    quad: [at(-x, -wi), at(x, -wo), at(x, wo), at(-x, wi)],
                    tip_angle: tip.y.atan2(tip.x),
                    tip_radius: tip.length(),
                    sweep: b.sweep.clamp(0.0, 0.95) * std::f32::consts::TAU / b.count as f32,
                    top: bh,
                    slope,
                    color,
                });
            }
        }
        v
    }

    /// Outline radius at angle `theta`, and the blade that reaches furthest
    /// there (`None`: just the core). `lead` is +1 when the ring turns towards
    /// increasing angles (right spin), -1 otherwise: the curved back of every
    /// blade trails behind it.
    fn outline(&self, lobes: &[Lobe], theta: f32, lead: f32) -> (f32, Option<usize>) {
        let rc = self.core_radius();
        let d = Vec2::from_angle(theta);
        let (mut r, mut owner) = (rc, None);
        for (i, l) in lobes.iter().enumerate() {
            let mut reach = ray_far(d, &l.quad).unwrap_or(0.0);
            if l.sweep > 1e-3 {
                let behind = ((l.tip_angle - theta) * lead).rem_euclid(std::f32::consts::TAU);
                let x = behind / l.sweep;
                if x < 1.0 {
                    reach = reach.max(rc + (l.tip_radius * 0.97 - rc) * (1.0 - x.powf(1.7)));
                }
            }
            if reach > r {
                (r, owner) = (reach, Some(i));
            }
        }
        (r, owner)
    }

    /// The ring as one sculpted piece (origin at the ring floor, on the axis):
    /// a surface of revolution whose outer radius follows [`Self::outline`].
    /// Blades are lower than the core, chamfered on top (more for upward
    /// slopes) and undercut for downward slopes.
    pub fn mesh(&self, accent: Color, lead: f32) -> Mesh {
        const N: usize = 288;
        const ROWS: usize = 5;
        let h = RING_HEIGHT;
        let (r_top, r_bottom) = match self.core {
            CoreDef::Cylinder { radius } => (radius, radius),
            CoreDef::Frustum { top, bottom } => (top, bottom),
        };
        let rc = self.core_radius();
        let core_at = |y: f32| r_bottom + (r_top - r_bottom) * (y / h);
        let lobes = self.lobes(accent);
        let core_color = self.color().to_linear().to_f32_array();

        let mut pos: Vec<[f32; 3]> = vec![[0.0, h, 0.0], [0.0, 0.0, 0.0]];
        let mut col: Vec<[f32; 4]> = vec![core_color, core_color];
        for k in 0..N {
            let theta = k as f32 / N as f32 * std::f32::consts::TAU;
            let (ro, owner) = self.outline(&lobes, theta, lead);
            let e = ro - rc;
            let t = (e / 0.025).clamp(0.0, 1.0);
            let blend = t * t * (3.0 - 2.0 * t);
            let (top, slope, color) = match owner {
                Some(i) => (lobes[i].top, lobes[i].slope, lobes[i].color.to_linear().to_f32_array()),
                None => (h, 0.0, core_color),
            };
            let yb = h + (top - h) * blend;
            let up = (slope.to_degrees() / 45.0).clamp(0.0, 1.0);
            let down = (-slope.to_degrees() / 45.0).clamp(0.0, 1.0);
            let chamfer = yb * (0.25 + 0.5 * up);
            let r4 = ro.max(core_at(yb - chamfer));
            let r3 = r4 - (0.012 + 0.03 * up).min(0.012 + e * 0.6);
            let r2 = r_top.min(r3);
            let r5 = ro.max(core_at(0.0)) - (0.05 * down).min(e * 0.6);
            let blade = if blend > 0.5 { color } else { core_color };
            let rows: [(f32, f32, [f32; 4]); ROWS] = [
                (r2 - 0.03, h, core_color),
                (r2, yb, core_color),
                (r3, yb, blade),
                (r4, yb - chamfer, blade),
                (r5, 0.0, blade),
            ];
            let (sin, cos) = theta.sin_cos();
            for (r, y, c) in rows {
                pos.push([r * cos, y, r * sin]);
                col.push(c);
            }
        }
        let at = |k: usize, row: usize| (2 + (k % N) * ROWS + row) as u32;
        let mut idx: Vec<u32> = Vec::new();
        for k in 0..N {
            idx.extend([0, at(k + 1, 0), at(k, 0)]);
            for row in 0..ROWS - 1 {
                let (a, b) = (at(k, row), at(k, row + 1));
                let (c, d) = (at(k + 1, row), at(k + 1, row + 1));
                idx.extend([a, c, b, b, c, d]);
            }
            idx.extend([1, at(k, ROWS - 1), at(k + 1, ROWS - 1)]);
        }
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
        mesh.insert_indices(Indices::U32(idx));
        // Flat shading: crisp machined edges.
        mesh.duplicate_vertices();
        mesh.compute_flat_normals();
        mesh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outline_follows_blades_and_trails_behind_them() {
        let def = &parse(BUILTIN).unwrap()[0];
        let lobes = def.lobes(Color::WHITE);
        let rc = def.core_radius();
        let tip = &lobes[0];
        // At the tip the outline reaches the blade tip.
        let (r, owner) = def.outline(&lobes, tip.tip_angle, 1.0);
        assert!(owner.is_some() && r > rc + 0.05);
        // Right spin leads towards larger angles: the back is behind the tip.
        let behind = def.outline(&lobes, tip.tip_angle - tip.sweep * 0.5, 1.0).0;
        let ahead = def.outline(&lobes, tip.tip_angle + tip.sweep * 0.5, 1.0).0;
        assert!(behind > ahead);
        // Mirrored for left spin.
        let behind_l = def.outline(&lobes, tip.tip_angle + tip.sweep * 0.5, -1.0).0;
        assert!((behind_l - behind).abs() < 0.03);
    }

    #[test]
    fn catalogue_has_every_ability() {
        let defs = parse(BUILTIN).unwrap();
        for a in [Ability::Absorb, Ability::Rebound, Ability::Cushion] {
            assert!(defs.iter().any(|d| d.blades.iter().any(|b| b.ability == a)), "{a:?}");
        }
    }

    #[test]
    fn builtin_catalogue_parses() {
        let defs = parse(BUILTIN).unwrap();
        assert!(defs.len() >= 7);
        assert!(defs.iter().any(|d| d.blades.iter().any(|b| b.slope > 0.0)));
    }

    #[test]
    fn rejects_too_many_blades() {
        let text = r#"[(name: "x", desc: "", color: (1.0, 1.0, 1.0), air_drag: 1.0,
            core: Cylinder(radius: 0.2),
            blades: [(count: 13, radius: 0.3, length: 0.1, thickness: 0.05)])]"#;
        assert!(parse(text).is_err());
    }
}
