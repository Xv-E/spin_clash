//! Part catalogue for the spinning tops.
//!
//! Every part is described as a list of simple solid pieces (cylinder, box,
//! cone, sphere, frustum). The very same pieces are used to build the render
//! mesh *and* the physics collider, and every piece carries a material density,
//! so mass, centre of mass and the inertia tensor that the physics engine uses
//! all follow from the real geometry of the assembled top.

use bevy::prelude::*;
use std::f32::consts::TAU;

pub use crate::rings::{ring_defs, Ability, Ring, RingDef};

/// Densities (game units). Metal parts are ~6x denser than plastic ones.
pub const PLASTIC: f32 = 1.3;
pub const METAL: f32 = 7.8;
pub const RUBBER: f32 = 1.6;

#[derive(Clone, Copy, Debug)]
pub enum Shape {
    Cylinder { r: f32, h: f32 },
    Cuboid(Vec3),
    /// Apex points to +Y before rotation.
    Cone { r: f32, h: f32 },
    Sphere(f32),
    Frustum { r_top: f32, r_bottom: f32, h: f32 },
    /// Flat tip disc. Rendered as a disc; its contact surface is a shallow
    /// sphere of radius `contact` (bottom at the disc bottom): contact sits on
    /// the axis when upright and moves out to the rim when tilted, so the top
    /// rolls along its edge like a real flat tip.
    Disc { r: f32, h: f32, contact: f32 },
    /// Blade: length `l` (radial, x), base thickness `t` (z), height `h`.
    /// Both side faces lean by `slope` radians: positive narrows the top, so
    /// the faces point upward and a hit lifts the opponent; negative widens
    /// the top and presses it down. `taper` is the width of the outer end as
    /// a share of the inner end (1 = rectangle, < 1 = a wing narrowing to its
    /// tip).
    Wedge { l: f32, t: f32, h: f32, slope: f32, taper: f32 },
}

/// The 8 corners of a wedge, centred on the origin.
pub fn wedge_points(l: f32, t: f32, h: f32, slope: f32, taper: f32) -> [Vec3; 8] {
    // Inner end (-x) and outer end (+x) half-widths, bottom and top.
    let (ib, it) = wedge_half_widths(t, h, slope);
    let (ob, ot) = wedge_half_widths(t * taper, h, slope);
    let (x, y) = (l * 0.5, h * 0.5);
    [
        Vec3::new(-x, -y, -ib),
        Vec3::new(x, -y, -ob),
        Vec3::new(x, -y, ob),
        Vec3::new(-x, -y, ib),
        Vec3::new(-x, y, -it),
        Vec3::new(x, y, -ot),
        Vec3::new(x, y, ot),
        Vec3::new(-x, y, it),
    ]
}

/// Average half-width of a wedge over its length, top and bottom.
pub fn wedge_mean_half_width(t: f32, h: f32, slope: f32, taper: f32) -> f32 {
    let (ib, it) = wedge_half_widths(t, h, slope);
    let (ob, ot) = wedge_half_widths(t * taper, h, slope);
    0.25 * (ib + it + ob + ot)
}

/// Half-widths of a wedge's bottom and top faces.
pub fn wedge_half_widths(t: f32, h: f32, slope: f32) -> (f32, f32) {
    let bottom = t * 0.5;
    let top = (bottom - h * slope.tan()).max(0.004);
    (bottom, top)
}

impl Shape {
    pub fn volume(&self) -> f32 {
        use std::f32::consts::PI;
        match *self {
            Shape::Cylinder { r, h } | Shape::Disc { r, h, .. } => PI * r * r * h,
            Shape::Cuboid(s) => s.x * s.y * s.z,
            Shape::Cone { r, h } => PI * r * r * h / 3.0,
            Shape::Sphere(r) => 4.0 / 3.0 * PI * r * r * r,
            Shape::Frustum { r_top, r_bottom, h } => {
                PI * h / 3.0 * (r_top * r_top + r_top * r_bottom + r_bottom * r_bottom)
            }
            Shape::Wedge { l, t, h, slope, taper } => {
                l * h * 2.0 * wedge_mean_half_width(t, h, slope, taper)
            }
        }
    }

    /// Moment of inertia about the piece's own vertical axis, per unit mass.
    fn inertia_y_per_mass(&self) -> f32 {
        match *self {
            Shape::Cylinder { r, .. } | Shape::Disc { r, .. } => 0.5 * r * r,
            Shape::Cuboid(s) => (s.x * s.x + s.z * s.z) / 12.0,
            Shape::Cone { r, .. } => 0.3 * r * r,
            Shape::Sphere(r) => 0.4 * r * r,
            Shape::Frustum { r_top, r_bottom, .. } => {
                let r = 0.5 * (r_top + r_bottom);
                0.5 * r * r
            }
            Shape::Wedge { l, t, h, slope, taper } => {
                let w = 2.0 * wedge_mean_half_width(t, h, slope, taper);
                (l * l + w * w) / 12.0
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Piece {
    pub shape: Shape,
    pub pos: Vec3,
    pub rot: Quat,
    pub density: f32,
    pub color: Color,
    pub metallic: f32,
    pub contact: Contact,
    /// False for pieces that are drawn as part of a bigger mesh (the ring).
    pub visible: bool,
    /// Special function (blades only).
    pub ability: Ability,
}

/// How a body piece (ring / disk / cap) takes part in collisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contact {
    /// Part of the body's smooth convex outline.
    Hull,
    /// Its own collider (blades).
    Own,
    /// Mass only, never touches anything (inserts).
    MassOnly,
}

impl Piece {
    pub fn new(shape: Shape, pos: Vec3, density: f32, color: Color) -> Self {
        let metallic = if density >= METAL { 0.9 } else { 0.05 };
        Self {
            shape,
            pos,
            rot: Quat::IDENTITY,
            density,
            color,
            metallic,
            contact: Contact::Hull,
            visible: true,
            ability: Ability::None,
        }
    }
    pub fn hidden(mut self) -> Self {
        self.visible = false;
        self
    }
    pub fn ability(mut self, ability: Ability) -> Self {
        self.ability = ability;
        self
    }
    pub fn rot(mut self, rot: Quat) -> Self {
        self.rot = rot;
        self
    }
    pub fn contact(mut self, contact: Contact) -> Self {
        self.contact = contact;
        self
    }
    pub fn mass(&self) -> f32 {
        self.shape.volume() * self.density
    }
}

// ---------------------------------------------------------------------------
// Part kinds
// ---------------------------------------------------------------------------

/// Vertical layout (y = 0 is the very bottom of the tip).
const TIP_TOP: f32 = 0.2;
const TRACK_TOP: f32 = 0.31;
const DISK_TOP: f32 = crate::rings::RING_BOTTOM;
const RING_TOP: f32 = crate::rings::RING_BOTTOM + crate::rings::RING_HEIGHT;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Disk {
    Standard,
    RimHeavy,
    CoreHeavy,
    Light,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tip {
    Sharp,
    Ball,
    Flat,
    Rubber,
}

/// Shape of a tip's contact with the floor, as far as travel is concerned.
#[derive(Clone, Copy, Debug)]
pub struct TipDrive {
    /// Radius of the flat face's edge: once the top tilts off its face the
    /// contact jumps out to here.
    pub edge: f32,
    /// Tilt (radians, against the floor) where the contact starts to leave
    /// the face, and where it is fully on the edge.
    pub edge_tilt: (f32, f32),
    /// Radius of the rounded part: contact offset = ball * sin(tilt).
    pub ball: f32,
    /// Radius of the sphere the physics engine uses for this tip; the engine
    /// already produces that share of the drive by itself.
    pub collider: f32,
    /// How fast friction stands the top back up (1/s, at full spin).
    pub righting: f32,
    /// 0 = stands up vertical, 1 = stands square on the sloping floor.
    pub follow: f32,
    /// Seconds between two wobbles from the floor (shortest, longest).
    pub kick_every: (f32, f32),
    /// Tilt (radians) such a wobble adds (smallest, largest).
    pub kick_tilt: (f32, f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SpinDir {
    Right,
    Left,
}

impl Ring {
    pub fn all() -> Vec<Ring> {
        (0..ring_defs().len()).map(Ring).collect()
    }
    pub fn by_name(name: &str) -> Ring {
        let i = ring_defs().iter().position(|d| d.name == name);
        Ring(i.unwrap_or_else(|| panic!("no ring named {name}")))
    }
    pub fn def(self) -> &'static RingDef {
        &ring_defs()[self.0]
    }
    pub fn name(self) -> &'static str {
        &self.def().name
    }
    pub fn desc(self) -> &'static str {
        &self.def().desc
    }
    pub fn air_drag(self) -> f32 {
        self.def().air_drag
    }
    fn pieces(self, accent: Color) -> Vec<Piece> {
        self.def().pieces(accent)
    }
}

impl Disk {
    pub const ALL: [Disk; 4] = [Disk::Standard, Disk::RimHeavy, Disk::CoreHeavy, Disk::Light];
    pub fn name(self) -> &'static str {
        match self {
            Disk::Standard => "标准盘",
            Disk::RimHeavy => "外缘重盘",
            Disk::CoreHeavy => "中心重盘",
            Disk::Light => "轻量盘",
        }
    }
    pub fn desc(self) -> &'static str {
        match self {
            Disk::Standard => "实心金属圆盘，各项均衡",
            Disk::RimHeavy => "质量集中在外缘，转动惯量大，持久与防御强",
            Disk::CoreHeavy => "质量集中在中心，重而稳，但惯量小、掉速快",
            Disk::Light => "较轻，轴尖带得动、游走更快，但容易被撞飞",
        }
    }
    fn pieces(self) -> Vec<Piece> {
        let y0 = TRACK_TOP;
        let h = DISK_TOP - TRACK_TOP;
        let yc = y0 + h * 0.5;
        let steel = Color::srgb(0.72, 0.73, 0.78);
        let dark = Color::srgb(0.4, 0.4, 0.45);
        match self {
            Disk::Standard => {
                vec![Piece::new(Shape::Cylinder { r: 0.36, h }, Vec3::Y * yc, METAL, steel)]
            }
            Disk::RimHeavy => {
                let mut v = vec![Piece::new(
                    Shape::Cylinder { r: 0.32, h: h * 0.4 },
                    Vec3::Y * (y0 + h * 0.2),
                    METAL,
                    dark,
                )];
                let n = 16;
                for i in 0..n {
                    let a = i as f32 / n as f32 * TAU;
                    let rot = Quat::from_rotation_y(-a);
                    let pos = rot * Vec3::new(0.37, yc, 0.0);
                    let chord = 2.0 * 0.41 * (TAU / n as f32 / 2.0).tan();
                    v.push(
                        Piece::new(Shape::Cuboid(Vec3::new(0.09, h, chord)), pos, METAL, steel)
                            .rot(rot),
                    );
                }
                v
            }
            Disk::CoreHeavy => vec![
                Piece::new(Shape::Cylinder { r: 0.27, h }, Vec3::Y * yc, METAL * 1.4, steel),
                Piece::new(
                    Shape::Cylinder { r: 0.33, h: h * 0.35 },
                    Vec3::Y * (y0 + h * 0.175),
                    PLASTIC,
                    dark,
                ),
            ],
            Disk::Light => vec![Piece::new(
                Shape::Cylinder { r: 0.33, h: h * 0.8 },
                Vec3::Y * (y0 + h * 0.4),
                METAL * 0.85,
                Color::srgb(0.8, 0.7, 0.35),
            )],
        }
    }
}

impl Tip {
    pub const ALL: [Tip; 4] = [Tip::Sharp, Tip::Ball, Tip::Flat, Tip::Rubber];
    pub fn name(self) -> &'static str {
        match self {
            Tip::Sharp => "尖锐轴",
            Tip::Ball => "球形轴",
            Tip::Flat => "平底轴",
            Tip::Rubber => "橡胶平底轴",
        }
    }
    pub fn desc(self) -> &'static str {
        match self {
            Tip::Sharp => "点接触、摩擦小：几乎不移动，持久最强",
            Tip::Ball => "球面接触：倾斜时会绕圈，攻守兼备",
            Tip::Flat => "平面接触：抓地带动陀螺快速游走",
            Tip::Rubber => "橡胶高摩擦：疯狂冲撞，但转速掉得飞快",
        }
    }
    /// Coulomb friction coefficient of the tip material against the stadium.
    pub fn friction(self) -> f32 {
        match self {
            Tip::Sharp => 0.12,
            Tip::Ball => 0.3,
            Tip::Flat => 0.55,
            Tip::Rubber => 1.1,
        }
    }
    /// Effective radius of the contact patch: torsional (spin) friction torque
    /// is `friction * normal_force * contact_radius`. The physics engine only
    /// models point friction, so this part of the contact is added explicitly.
    pub fn contact_radius(self) -> f32 {
        match self {
            Tip::Sharp => 0.011,
            Tip::Ball => 0.018,
            Tip::Flat => 0.01,
            Tip::Rubber => 0.008,
        }
    }
    /// How the tip turns tilt into travel (see `top::tip_drive`).
    pub fn drive(self) -> TipDrive {
        let deg = |a: f32, b: f32| (a.to_radians(), b.to_radians());
        match self {
            // A point: the contact hardly leaves the axis, so it stays put.
            Tip::Sharp => TipDrive {
                edge: 0.004,
                edge_tilt: deg(3.0, 8.0),
                ball: 0.012,
                collider: 0.012,
                righting: 0.5,
                follow: 0.0,
                kick_every: (4.0, 9.0),
                kick_tilt: deg(1.0, 3.0),
            },
            // A ball: the contact creeps out smoothly as it tilts.
            Tip::Ball => TipDrive {
                edge: 0.0,
                edge_tilt: deg(0.0, 1.0),
                ball: 0.085,
                collider: 0.07,
                righting: 1.2,
                follow: 0.0,
                kick_every: (2.0, 5.0),
                kick_tilt: deg(3.0, 6.0),
            },
            // Flat: sits on its face, then snaps onto its edge and bolts.
            Tip::Flat => TipDrive {
                edge: 0.03,
                edge_tilt: deg(2.0, 5.0),
                ball: 0.12,
                collider: 0.12,
                righting: 4.0,
                follow: 1.0,
                kick_every: (1.0, 2.8),
                kick_tilt: deg(5.0, 9.0),
            },
            Tip::Rubber => TipDrive {
                edge: 0.03,
                edge_tilt: deg(1.5, 4.0),
                ball: 0.14,
                collider: 0.14,
                righting: 3.5,
                follow: 1.0,
                kick_every: (1.0, 2.6),
                kick_tilt: deg(5.0, 9.0),
            },
        }
    }
    fn pieces(self) -> Vec<Piece> {
        let col = match self {
            Tip::Rubber => Color::srgb(0.12, 0.12, 0.12),
            _ => Color::srgb(0.95, 0.85, 0.3),
        };
        let shaft = Color::srgb(0.2, 0.2, 0.22);
        let density = if self == Tip::Rubber { RUBBER } else { PLASTIC };
        let mut v = match self {
            Tip::Sharp => {
                // A real "sharp" tip is slightly rounded at the very point.
                let ball = 0.012;
                let h = TIP_TOP - ball;
                vec![
                    Piece::new(Shape::Sphere(ball), Vec3::Y * ball, density, col),
                    Piece::new(Shape::Cone { r: 0.09, h }, Vec3::Y * (ball + h * 0.5), density, col)
                        .rot(Quat::from_rotation_x(std::f32::consts::PI)),
                ]
            }
            Tip::Ball => vec![
                Piece::new(Shape::Sphere(0.07), Vec3::Y * 0.07, density, col),
                Piece::new(
                    Shape::Frustum { r_top: 0.1, r_bottom: 0.06, h: TIP_TOP - 0.07 },
                    Vec3::Y * (0.07 + (TIP_TOP - 0.07) * 0.5),
                    density,
                    col,
                ),
            ],
            Tip::Flat | Tip::Rubber => {
                let (r, contact) = if self == Tip::Flat { (0.065, 0.12) } else { (0.075, 0.14) };
                vec![
                    Piece::new(Shape::Disc { r, h: 0.03, contact }, Vec3::Y * 0.015, density, col),
                    Piece::new(
                        Shape::Frustum { r_top: 0.1, r_bottom: r, h: TIP_TOP - 0.03 },
                        Vec3::Y * (0.03 + (TIP_TOP - 0.03) * 0.5),
                        PLASTIC,
                        col,
                    ),
                ]
            }
        };
        // Track: the column that connects the tip to the disk.
        v.push(Piece::new(
            Shape::Cylinder { r: 0.16, h: TRACK_TOP - TIP_TOP },
            Vec3::Y * (TIP_TOP + (TRACK_TOP - TIP_TOP) * 0.5),
            PLASTIC,
            shaft,
        ));
        v
    }
}

impl SpinDir {
    pub fn name(self) -> &'static str {
        match self {
            SpinDir::Right => "右旋（顺时针）",
            SpinDir::Left => "左旋（逆时针）",
        }
    }
    /// Sign of the angular velocity around +Y (clockwise seen from above is -Y).
    pub fn sign(self) -> f32 {
        match self {
            SpinDir::Right => -1.0,
            SpinDir::Left => 1.0,
        }
    }
}

/// A fully assembled top.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Build {
    pub ring: Ring,
    pub disk: Disk,
    pub tip: Tip,
    pub dir: SpinDir,
}

impl Default for Build {
    fn default() -> Self {
        Self { ring: Ring(0), disk: Disk::Standard, tip: Tip::Ball, dir: SpinDir::Right }
    }
}

/// Which part a piece belongs to (the tip pieces get the tip's friction).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Ring,
    Disk,
    Tip,
    Cap,
}

pub struct Stats {
    pub mass: f32,
    pub spin_inertia: f32,
    pub com_height: f32,
    pub radius: f32,
}

impl Build {
    pub fn random(rng: &mut impl rand::Rng) -> Self {
        Self {
            ring: Ring(rng.random_range(0..ring_defs().len())),
            disk: Disk::ALL[rng.random_range(0..4)],
            tip: Tip::ALL[rng.random_range(0..4)],
            dir: if rng.random_bool(0.5) { SpinDir::Right } else { SpinDir::Left },
        }
    }

    pub fn pieces(&self, accent: Color) -> Vec<(Slot, Piece)> {
        let mut v = Vec::new();
        v.extend(self.tip.pieces().into_iter().map(|p| (Slot::Tip, p)));
        v.extend(self.disk.pieces().into_iter().map(|p| (Slot::Disk, p)));
        v.extend(self.ring.pieces(accent).into_iter().map(|p| (Slot::Ring, p)));
        // Cap with a bit-chip and a stripe that makes the rotation visible.
        v.push((
            Slot::Cap,
            Piece::new(
                Shape::Cylinder { r: 0.16, h: 0.07 },
                Vec3::Y * (RING_TOP + 0.035),
                PLASTIC,
                Color::srgb(0.95, 0.95, 0.95),
            ),
        ));
        v.push((
            Slot::Cap,
            Piece::new(
                Shape::Cuboid(Vec3::new(0.26, 0.012, 0.05)),
                Vec3::new(0.0, RING_TOP + 0.076, 0.0),
                PLASTIC,
                accent,
            ),
        ));
        v
    }

    pub fn stats(&self) -> Stats {
        let pieces = self.pieces(Color::WHITE);
        let mut mass = 0.0;
        let mut inertia = 0.0;
        let mut moment = 0.0;
        let mut radius: f32 = 0.0;
        for (_, p) in &pieces {
            let m = p.mass();
            let d2 = p.pos.x * p.pos.x + p.pos.z * p.pos.z;
            mass += m;
            inertia += m * (p.shape.inertia_y_per_mass() + d2);
            moment += m * p.pos.y;
            let extent = match p.shape {
                Shape::Cylinder { r, .. }
                | Shape::Disc { r, .. }
                | Shape::Cone { r, .. }
                | Shape::Sphere(r) => r,
                Shape::Frustum { r_top, r_bottom, .. } => r_top.max(r_bottom),
                Shape::Cuboid(s) => 0.5 * Vec2::new(s.x, s.z).length(),
                Shape::Wedge { l, t, h, slope, .. } => {
                    let (zb, zt) = wedge_half_widths(t, h, slope);
                    0.5 * Vec2::new(l, 2.0 * zb.max(zt)).length()
                }
            };
            radius = radius.max(d2.sqrt() + extent);
        }
        Stats { mass, spin_inertia: inertia, com_height: moment / mass, radius }
    }
}
