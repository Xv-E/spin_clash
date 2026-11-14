//! Spawning an assembled top as a rigid body, plus the contact physics that
//! the engine does not model on its own (torsional friction of the tip contact
//! patch and air drag on the spinning outline).

use crate::parts::{wedge_mean_half_width, wedge_points, Ability, Build, Contact, Piece, Shape, Slot};
use avian3d::prelude::*;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

/// Air drag constant: drag torque = AIR_K * outline_factor * radius^5 * omega^2.
pub const AIR_K: f32 = 1.3e-4;
/// A top is out once its smoothed total angular speed (rad/s) drops below this.
pub const STOP_SPIN: f32 = 4.0;
/// Time constant (game seconds) of that smoothing.
const SPIN_SMOOTHING: f32 = 0.5;
/// Global factor on spin losses (tip torsion + air drag): lower = longer spins.
pub const SPIN_LOSS_SCALE: f32 = 0.45;
/// Off balance: extra spin loss (rad/s²) per unit of sin(tilt) beyond
/// [`TILT_FREE`] while the tip is on the ground. A tilted top runs on the side
/// of its tip and wobbles: a bigger, sliding contact patch.
const TILT_LOSS: f32 = 18.0;
/// sin of the tilt (~4°) a healthy top precesses at without extra loss.
const TILT_FREE: f32 = 0.07;
/// Extra friction of the body (ring, blades, disk) scraping or banging into
/// the stadium: torque = SCRAPE_MU * N * (distance of the contact from the
/// spin axis). On top of the solver's own friction at those contacts (0.3):
/// the rough, edged body loses more than a smooth point contact would.
const SCRAPE_MU: f32 = 0.1;
/// Cap on that loss (rad/s²), so one hard bang into the wall costs a few
/// rad/s instead of a big chunk of the spin.
const SCRAPE_MAX: f32 = 40.0;
/// Smoothed total angular speed: rotation about *any* axis counts, so a top
/// knocked onto its edge that still whirls like a coin is not out yet.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct SpinMeter {
    pub smoothed: f32,
}

pub fn update_spin_meter(
    time: Res<Time>,
    mut q: Query<(&RigidBody, &AngularVelocity, &mut SpinMeter)>,
) {
    let k = (time.delta_secs() / SPIN_SMOOTHING).min(1.0);
    for (body, ang, mut meter) in &mut q {
        if *body == RigidBody::Dynamic {
            meter.smoothed += (ang.0.length() - meter.smoothed) * k;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Player,
    Ai,
}

impl Owner {
    pub fn accent(self) -> Color {
        match self {
            Owner::Player => Color::srgb(1.0, 0.55, 0.05),
            Owner::Ai => Color::srgb(0.1, 0.85, 0.95),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Owner::Player => "你",
            Owner::Ai => "电脑",
        }
    }
}

#[derive(Component)]
pub struct Top {
    pub owner: Owner,
    pub build: Build,
    pub tips: Vec<Entity>,
    /// Combined air drag constant of this top's outline.
    pub air_c: f32,
    /// Outer radius of the body outline.
    pub radius: f32,
    /// Blade geometry in body space, for analytic blade impacts.
    pub blades: Vec<BladeGeom>,
}

/// A blade in the top's local frame (see [`crate::blade_hits`]).
#[derive(Clone, Copy, Debug)]
pub struct BladeGeom {
    pub center: Vec3,
    /// Long (radial) axis of the blade.
    pub axis: Vec3,
    pub half_len: f32,
    /// Average half-thickness of the sloped cross-section.
    pub half_thick: f32,
    pub half_h: f32,
    pub slope: f32,
    pub ability: Ability,
}

impl BladeGeom {
    /// Furthest horizontal distance the blade reaches from the axis.
    pub fn reach(&self) -> f32 {
        Vec2::new(self.center.x, self.center.z).length() + self.half_len + self.half_thick
    }
}

/// Marks the collider of the attack ring (used for spark effects).
#[derive(Component)]
pub struct RingCollider;

/// A blade collider and the slope (radians) of its faces.
#[derive(Component)]
pub struct Blade {
    pub slope: f32,
    pub ability: Ability,
}

/// Contact hook for blade hits between two tops.
///
/// Two spinning blades meet at enormous relative surface speed. A rigid solver
/// treats their faces as interlocking gear teeth and stops that motion dead,
/// draining almost all spin in one hit. Real plastic blades flex and slide off
/// each other instead. So for blade contacts the normal is replaced by the line
/// between the two tops' centres, tilted by the blade slope:
///
/// - the engine still decides when and where blades touch and solves the
///   impulses (momentum is conserved);
/// - the spin is exchanged through Coulomb friction, which is capped;
/// - a sloped blade pushes the opponent up (positive) or down (negative), and
///   by reaction pushes its own top the other way.
#[derive(SystemParam)]
pub struct BladeHooks<'w, 's> {
    blades: Query<'w, 's, &'static Blade>,
    rings: Query<'w, 's, (), With<RingCollider>>,
    tops: Query<'w, 's, &'static Position, With<Top>>,
}

/// Friction of a top's body (ring, blades, disk) against the stadium: smooth
/// plastic on plastic, and it only slides. The body's own friction (tuned for
/// top-vs-top hits) made every wall contact brake the spin hard.
const WALL_FRICTION: f32 = 0.08;

impl CollisionHooks for BladeHooks<'_, '_> {
    /// Blade-vs-blade is handled analytically by [`crate::blade_hits`].
    fn filter_pairs(&self, collider1: Entity, collider2: Entity, _commands: &mut Commands) -> bool {
        !(self.blades.contains(collider1) && self.blades.contains(collider2))
    }

    fn modify_contacts(&self, contacts: &mut ContactPair, _commands: &mut Commands) -> bool {
        // Body against the stadium (anything that isn't another top).
        let is_top = |b: Option<Entity>| b.is_some_and(|b| self.tops.contains(b));
        let wall = (self.rings.contains(contacts.collider1) && !is_top(contacts.body2))
            || (self.rings.contains(contacts.collider2) && !is_top(contacts.body1));
        if wall {
            for manifold in &mut contacts.manifolds {
                manifold.friction = WALL_FRICTION;
            }
            return true;
        }
        let b1 = self.blades.get(contacts.collider1).ok();
        let b2 = self.blades.get(contacts.collider2).ok();
        let (s1, s2) = (b1.map(|b| b.slope), b2.map(|b| b.slope));
        if s1.is_none() && s2.is_none() {
            return true;
        }
        let (Some(e1), Some(e2)) = (contacts.body1, contacts.body2) else { return true };
        if e1 == e2 {
            return true;
        }
        // Only top-vs-top; blades against the stadium keep their real normal.
        let (Ok(p1), Ok(p2)) = (self.tops.get(e1), self.tops.get(e2)) else { return true };
        let mut d = p2.0 - p1.0;
        d.y = 0.0;
        let Some(centres) = d.try_normalize() else { return true };
        let (s1, s2) = (s1.unwrap_or(0.0), s2.unwrap_or(0.0));
        // The normal points from collider 1 to collider 2 and is the direction
        // collider 2 gets pushed: blade 1 lifts body 2, blade 2 lifts body 1.
        let horizontal = 0.5 * (s1.cos() + s2.cos());
        let normal = (centres * horizontal + Vec3::Y * (s1.sin() - s2.sin())).normalize();
        // A special blade against the opponent's body (blade-vs-blade never
        // gets here): the contact itself becomes springy, dead or grippy.
        let ability = [b1, b2].into_iter().flatten().map(|b| b.ability).find(|a| *a != Ability::None);
        for manifold in &mut contacts.manifolds {
            manifold.normal = normal;
            match ability {
                Some(Ability::Rebound) => manifold.restitution = 0.85,
                Some(Ability::Cushion) => {
                    manifold.restitution = 0.0;
                    manifold.friction *= 0.5;
                }
                // Rubber grip: friction evens out the two spins.
                Some(Ability::Absorb) => {
                    manifold.restitution = 0.05;
                    manifold.friction = 0.9;
                }
                _ => {}
            }
        }
        true
    }
}

/// Visual-only top used in the assembly screen.
#[derive(Component)]
pub struct PreviewTop;

pub struct Visuals<'a> {
    pub meshes: &'a mut Assets<Mesh>,
    pub materials: &'a mut Assets<StandardMaterial>,
}

pub fn piece_mesh(shape: Shape) -> Mesh {
    match shape {
        Shape::Cylinder { r, h } | Shape::Disc { r, h, .. } => {
            Cylinder::new(r, h).mesh().resolution(40).into()
        }
        Shape::Cuboid(s) => Cuboid::from_size(s).into(),
        Shape::Cone { r, h } => Cone { radius: r, height: h }.mesh().resolution(40).into(),
        Shape::Sphere(r) => Sphere::new(r).mesh().uv(32, 16),
        Shape::Frustum { r_top, r_bottom, h } => ConicalFrustum {
            radius_top: r_top,
            radius_bottom: r_bottom,
            height: h,
        }
        .mesh()
        .resolution(40)
        .into(),
        Shape::Wedge { l, t, h, slope, taper } => wedge_mesh(l, t, h, slope, taper),
    }
}

/// Flat-shaded mesh of a wedge (6 quads, each with its own normal).
fn wedge_mesh(l: f32, t: f32, h: f32, slope: f32, taper: f32) -> Mesh {
    use bevy::asset::RenderAssetUsages;
    use bevy::mesh::{Indices, PrimitiveTopology};
    let p = wedge_points(l, t, h, slope, taper);
    // Corner indices of each face, counter-clockwise seen from outside.
    let faces: [[usize; 4]; 6] = [
        [0, 1, 2, 3], // bottom
        [4, 7, 6, 5], // top
        [0, 4, 5, 1], // -z side
        [3, 2, 6, 7], // +z side
        [0, 3, 7, 4], // inner end
        [1, 5, 6, 2], // outer end
    ];
    let (mut pos, mut nor, mut idx) = (Vec::new(), Vec::new(), Vec::new());
    for f in faces {
        let n = (p[f[1]] - p[f[0]]).cross(p[f[2]] - p[f[0]]).normalize_or_zero();
        let base = pos.len() as u32;
        for &i in &f {
            pos.push(p[i].to_array());
            nor.push(n.to_array());
        }
        idx.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nor);
    mesh.insert_indices(Indices::U32(idx));
    mesh
}

/// Rounding of blade edges: smooth contact normals, no snagging on corners.
const BLADE_BORDER: f32 = 0.006;

/// Sample points on the surface of a shape (for building convex outlines).
fn shape_points(shape: Shape) -> Vec<Vec3> {
    let ring = |r: f32, y: f32, out: &mut Vec<Vec3>| {
        for i in 0..24 {
            let a = i as f32 / 24.0 * std::f32::consts::TAU;
            out.push(Vec3::new(r * a.cos(), y, r * a.sin()));
        }
    };
    let mut v = Vec::new();
    match shape {
        Shape::Cylinder { r, h } | Shape::Disc { r, h, .. } => {
            ring(r, h * 0.5, &mut v);
            ring(r, -h * 0.5, &mut v);
        }
        Shape::Frustum { r_top, r_bottom, h } => {
            ring(r_top, h * 0.5, &mut v);
            ring(r_bottom, -h * 0.5, &mut v);
        }
        Shape::Cone { r, h } => {
            ring(r, -h * 0.5, &mut v);
            v.push(Vec3::Y * h * 0.5);
        }
        Shape::Sphere(r) => {
            ring(r, 0.0, &mut v);
            v.push(Vec3::Y * r);
            v.push(Vec3::NEG_Y * r);
        }
        Shape::Wedge { l, t, h, slope, taper } => v.extend(wedge_points(l, t, h, slope, taper)),
        Shape::Cuboid(s) => {
            for x in [-0.5, 0.5] {
                for y in [-0.5, 0.5] {
                    for z in [-0.5, 0.5] {
                        v.push(Vec3::new(x, y, z) * s);
                    }
                }
            }
        }
    }
    v
}

/// Collider for a piece, plus its offset from the piece centre and the
/// collider density that gives the piece its intended mass.
pub fn piece_collider(piece: &Piece) -> (Collider, Vec3, f32) {
    if let Shape::Disc { h, contact, .. } = piece.shape {
        let vol = 4.0 / 3.0 * std::f32::consts::PI * contact.powi(3);
        let offset = Vec3::Y * (contact - h * 0.5);
        return (Collider::sphere(contact), offset, piece.mass() / vol);
    }
    (shape_collider(piece.shape), Vec3::ZERO, piece.density)
}

fn shape_collider(shape: Shape) -> Collider {
    match shape {
        Shape::Cylinder { r, h } => Collider::cylinder(r, h),
        Shape::Disc { .. } => unreachable!("handled in piece_collider"),
        Shape::Cuboid(s) => Collider::cuboid(s.x, s.y, s.z),
        Shape::Cone { r, h } => Collider::cone(r, h),
        Shape::Sphere(r) => Collider::sphere(r),
        Shape::Frustum { r_top, r_bottom, h } => {
            let n = 24;
            let mut pts = Vec::with_capacity(n * 2);
            for i in 0..n {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                let (s, c) = a.sin_cos();
                pts.push(Vec3::new(c * r_top, h * 0.5, s * r_top));
                pts.push(Vec3::new(c * r_bottom, -h * 0.5, s * r_bottom));
            }
            Collider::convex_hull(pts).expect("frustum hull")
        }
        Shape::Wedge { l, t, h, slope, taper } => {
            // Shrink by the border so the rounded hull matches the wedge.
            let b = BLADE_BORDER;
            let pts = wedge_points(l - 2.0 * b, t - 2.0 * b, h - 2.0 * b, slope, taper);
            Collider::from(
                avian3d::parry::shape::SharedShape::round_convex_hull(&pts, b)
                    .expect("wedge hull"),
            )
        }
    }
}

fn spawn_piece_visual(
    parent: &mut ChildSpawnerCommands,
    piece: &Piece,
    vis: &mut Visuals,
) -> Entity {
    let material = vis.materials.add(StandardMaterial {
        base_color: piece.color,
        metallic: piece.metallic,
        perceptual_roughness: if piece.metallic > 0.5 { 0.3 } else { 0.55 },
        ..default()
    });
    parent
        .spawn((
            Mesh3d(vis.meshes.add(piece_mesh(piece.shape))),
            MeshMaterial3d(material),
            Transform::from_translation(piece.pos).with_rotation(piece.rot),
        ))
        .id()
}

/// The attack ring, drawn as one sculpted piece (see [`crate::rings::RingDef::mesh`]).
/// Its claws trail behind the spin direction, so left-spin rings are mirrored.
fn spawn_ring_visual(parent: &mut ChildSpawnerCommands, build: &Build, accent: Color, vis: &mut Visuals) {
    let mesh = build.ring.def().mesh(accent, -build.dir.sign());
    parent.spawn((
        Mesh3d(vis.meshes.add(mesh)),
        MeshMaterial3d(vis.materials.add(StandardMaterial {
            base_color: Color::WHITE,
            metallic: 0.45,
            perceptual_roughness: 0.38,
            ..default()
        })),
        Transform::from_translation(Vec3::Y * crate::rings::RING_BOTTOM),
    ));
}

/// Spawn a physical top. It starts kinematic (held by the launcher) until
/// [`launch`] turns it into a dynamic body.
pub fn spawn_top(
    commands: &mut Commands,
    build: Build,
    owner: Owner,
    transform: Transform,
    mut vis: Option<Visuals>,
) -> Entity {
    let stats = build.stats();
    let air_c = AIR_K * build.ring.air_drag() * stats.radius.powi(5);
    let mut tips = Vec::new();
    let blades: Vec<BladeGeom> = build
        .pieces(Color::WHITE)
        .into_iter()
        .filter_map(|(_, p)| match (p.contact, p.shape) {
            (Contact::Own, Shape::Wedge { l, t, h, slope, taper }) => {
                Some(BladeGeom {
                    center: p.pos,
                    axis: p.rot * Vec3::X,
                    half_len: l * 0.5,
                    half_thick: wedge_mean_half_width(t, h, slope, taper),
                    half_h: h * 0.5,
                    slope,
                    ability: p.ability,
                })
            }
            _ => None,
        })
        .collect();
    let body = commands
        .spawn((
            Name::new(format!("Top {:?}", owner)),
            RigidBody::Kinematic,
            transform,
            Visibility::default(),
            SweptCcd::default(),
            LinearDamping(0.05),
            AngularDamping(0.0),
            CollisionMargin(0.002),
        ))
        .with_children(|parent| {
            let pieces = build.pieces(owner.accent());
            let (ring_f, ring_r) = ring_material();
            let mut ring_hull_points = Vec::new();
            for (slot, piece) in &pieces {
                let (collider, offset, density) = piece_collider(piece);
                let mut e = parent.spawn((
                    collider,
                    ColliderDensity(density),
                    Transform::from_translation(piece.pos + piece.rot * offset)
                        .with_rotation(piece.rot),
                ));
                match slot {
                    Slot::Tip if piece.contact != Contact::MassOnly => {
                        e.insert((
                            Friction::new(build.tip.friction())
                                .with_combine_rule(CoefficientCombine::Multiply),
                            Restitution::new(0.05).with_combine_rule(CoefficientCombine::Min),
                        ));
                        tips.push(e.id());
                    }
                    _ => match piece.contact {
                        Contact::Hull => {
                            // Mass only: contacts go through the convex outline below.
                            e.insert(CollisionLayers::NONE);
                            ring_hull_points.extend(
                                shape_points(piece.shape)
                                    .into_iter()
                                    .map(|p| piece.pos + piece.rot * p),
                            );
                        }
                        Contact::Own => {
                            // Blade: its own rounded wedge collider.
                            let slope = match piece.shape {
                                Shape::Wedge { slope, .. } => slope,
                                _ => 0.0,
                            };
                            e.insert((
                                Blade { slope, ability: piece.ability },
                                ActiveCollisionHooks::MODIFY_CONTACTS
                                    | ActiveCollisionHooks::FILTER_PAIRS,
                                RingCollider,
                                CollisionEventsEnabled,
                                Friction::new(ring_f)
                                    .with_combine_rule(CoefficientCombine::Multiply),
                                Restitution::new(ring_r)
                                    .with_combine_rule(CoefficientCombine::Max),
                            ));
                        }
                        Contact::MassOnly => {
                            e.insert(CollisionLayers::NONE);
                        }
                    },
                }
                if let Some(vis) = vis.as_mut().filter(|_| piece.visible) {
                    spawn_piece_visual(parent, piece, vis);
                }
            }
            if let Some(vis) = vis.as_mut() {
                spawn_ring_visual(parent, &build, owner.accent(), vis);
            }
            // Contact surface of the body (ring core + disk + cap): their
            // convex outline, so the disk's rim blocks can't snag like gear
            // teeth. Blades collide separately as rounded wedges whose sloped
            // faces make hits glance up or down. Mass and inertia still come
            // from the individual parts.
            parent.spawn((
                RingCollider,
                CollisionEventsEnabled,
                ActiveCollisionHooks::MODIFY_CONTACTS,
                Collider::convex_hull(ring_hull_points).expect("ring hull"),
                ColliderDensity(0.0),
                Friction::new(ring_f).with_combine_rule(CoefficientCombine::Multiply),
                Restitution::new(ring_r).with_combine_rule(CoefficientCombine::Max),
                Transform::default(),
            ));
        })
        .id();
    commands.entity(body).insert((
        Top { owner, build, tips, air_c, radius: stats.radius, blades },
        SpinDebt::default(),
        SteerState::default(),
        // A different rhythm every round in the game; repeatable in the sim.
        Wobble::new(if vis.is_some() { rand::random() } else { owner as u32 + 1 }),
    ));
    body
}

/// Visual-only model for the assembly screen.
pub fn spawn_preview(
    commands: &mut Commands,
    build: Build,
    transform: Transform,
    mut vis: Visuals,
) -> Entity {
    commands
        .spawn((PreviewTop, transform, Visibility::default()))
        .with_children(|parent| {
            for (_, piece) in build.pieces(Owner::Player.accent()) {
                if piece.visible {
                    spawn_piece_visual(parent, &piece, &mut vis);
                }
            }
            spawn_ring_visual(parent, &build, Owner::Player.accent(), &mut vis);
        })
        .id()
}

/// Friction and restitution of the body outline (top-vs-top and walls).
/// Tuned with `--sim` so one hit converts a realistic share of spin energy
/// into knock-back instead of almost all of it.
fn ring_material() -> (f32, f32) {
    (0.3, 0.3)
}

/// Release a top from the launcher.
pub fn launch(
    commands: &mut Commands,
    top: Entity,
    rotation: Quat,
    spin: f32,
    velocity: Vec3,
) {
    commands.entity(top).insert((
        RigidBody::Dynamic,
        Rotation(rotation),
        AngularVelocity(rotation * Vec3::Y * spin),
        LinearVelocity(velocity),
        SpinMeter { smoothed: spin.abs() },
        // Smooth rendering between physics steps (essential in slow motion).
        TransformInterpolation,
    ));
}

/// Spin rate around the top's own axis (signed).
pub fn axial_spin(rotation: &Rotation, angvel: &AngularVelocity) -> f32 {
    angvel.0.dot(rotation.0 * Vec3::Y)
}

/// Spin losses (rad/s) waiting to be taken off a top.
///
/// avian's gyroscopic integration swallows small changes to a fast spinning
/// body's angular velocity: measured with a top floating in empty space, any
/// change below about 0.01 rad/s per 1/240 s step (≈ 3 rad/s²) was undone by
/// the next physics step, while larger changes went through in full. Steady
/// losses such as tip torsion and air drag on a low-friction top (or the spin
/// cost of steering) sit far below that, so they vanished and those tops
/// never slowed down. Losses therefore build up here and are taken off in
/// chunks of [`DEBT_CHUNK`], which the integrator keeps.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct SpinDebt {
    /// Loss about the top's own axis (tip torsion, steering).
    pub axial: f32,
    /// Loss along the whole angular velocity (air drag).
    pub drag: f32,
}

/// Stagger state of a top.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct SteerState {
    /// Seconds left of stagger after a hit: the tip drive backs off, so the
    /// knock-back plays out instead of being driven straight back.
    pub stun: f32,
}

/// Smallest spin change (rad/s) applied at once: well above the ~0.01 rad/s
/// the gyroscopic integrator swallows.
const DEBT_CHUNK: f32 = 0.05;

/// Torsional friction of the tip contact patch + aerodynamic drag.
///
/// The solver handles normal forces and sliding friction at contact points.
/// What a point contact cannot express is the resisting torque of a finite
/// contact patch spinning in place (torque = mu * N * r_patch), so it is applied
/// here using the *actual* normal impulse the solver produced last step.
pub fn spin_losses(
    mut tops: Query<(
        Entity,
        &Position,
        &Top,
        &RigidBody,
        &Rotation,
        &mut AngularVelocity,
        &ComputedAngularInertia,
        &mut SpinDebt,
    )>,
    collisions: Collisions,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let top_bodies: Vec<Entity> = tops.iter().map(|t| t.0).collect();
    for (e, pos, top, body, rot, mut angvel, inertia, mut debt) in &mut tops {
        if *body != RigidBody::Dynamic {
            continue;
        }
        let mut normal_impulse = 0.0;
        for &tip in &top.tips {
            for pair in collisions.collisions_with(tip) {
                if pair.is_touching() {
                    normal_impulse += pair.total_normal_impulse_magnitude();
                }
            }
        }
        let normal_force = normal_impulse / dt;
        let world_inertia = inertia.rotated(rot.0).tensor();
        let axis = rot.0 * Vec3::Y;

        // Tip torsion: resists spin about the top's own axis.
        let i_axis = world_inertia.mul_vec3(axis).dot(axis).max(1e-6);
        let torque = SPIN_LOSS_SCALE * build_torsion(top) * normal_force;
        debt.axial += torque / i_axis * dt;
        // Off balance: tilted on its tip.
        if normal_force > 0.0 {
            let lean = axis.cross(Vec3::Y).length(); // sin(tilt)
            debt.axial += TILT_LOSS * (lean - TILT_FREE).max(0.0) * dt;        }

        // Body against the stadium (floor scrapes when tipped over, wall
        // hits): friction torque from the actual contact impulses. Tips have
        // their own model above; top-vs-top is handled by the blade hits and
        // the solver.
        let mut scrape = 0.0; // sum of normal impulse x lever arm
        for pair in collisions.iter() {
            if !pair.is_touching() {
                continue;
            }
            let (mine, other) = if pair.body1 == Some(e) {
                (pair.collider1, pair.body2)
            } else if pair.body2 == Some(e) {
                (pair.collider2, pair.body1)
            } else {
                continue;
            };
            if top.tips.contains(&mine) || other.is_some_and(|o| top_bodies.contains(&o)) {
                continue;
            }
            for m in &pair.manifolds {
                for c in &m.points {
                    let rel = c.point - pos.0;
                    scrape += c.normal_impulse.abs() * (rel - axis * rel.dot(axis)).length();
                }
            }
        }
        debt.axial += (SCRAPE_MU * scrape / i_axis).min(SCRAPE_MAX * dt);
        // Air drag: resists rotation about any axis (a toppled top whirling
        // on its edge slows down too).
        let w = angvel.0.length();
        if w > 1e-4 {
            let dir = angvel.0 / w;
            let i_dir = world_inertia.mul_vec3(dir).dot(dir).max(1e-6);
            debt.drag += SPIN_LOSS_SCALE * top.air_c * w * w / i_dir * dt;
        }

        if debt.axial >= DEBT_CHUNK {
            let spin = angvel.0.dot(axis);
            angvel.0 -= axis * spin.signum() * debt.axial.min(spin.abs());
            debt.axial = 0.0;
        }
        if debt.drag >= DEBT_CHUNK {
            let w = angvel.0.length();
            if w > 1e-4 {
                angvel.0 *= (w - debt.drag).max(0.0) / w;
            }
            debt.drag = 0.0;
        }
    }
}

/// How fast a wobble leans the top over (rad/s).
const KICK_RATE: f32 = 0.9;
/// Tilt (radians) a hit from the other top adds, smallest and largest.
const HIT_KICK: (f32, f32) = (0.09, 0.17);
/// Share of that braking torque actually charged (the engine's own tip
/// friction already takes spin off a moving top).
const DRIVE_SPIN_COST: f32 = 0.3;
/// Largest friction coefficient the tip drive makes use of.
const DRIVE_MAX_GRIP: f32 = 0.7;
/// Seconds a wobbled top rides on its edge before it starts to stand up.
const RIDE_TIME: (f32, f32) = (0.2, 0.45);
/// Below this spin (rad/s) the tip has no drive left worth modelling.
const DRIVE_MIN_SPIN: f32 = 15.0;
/// Share of the tip drive that still acts while reeling from a hit (the
/// knock-back comes first).
const DRIVE_WHEN_BUSY: f32 = 0.3;

/// What makes a top wander: the tilt of its axis against the floor.
///
/// Upright, the tip touches the floor on the spin axis and only drills on
/// the spot. Tilted, the contact moves off the axis by `r`; that point of the
/// tip runs over the floor at `spin * r`, and friction drags the whole top
/// along like a wheel, *across* the direction it leans in, until it rolls
/// (speed `spin * r`). A ball tip's `r` grows smoothly with the tilt; a flat
/// tip rests on its face (`r = 0`) and snaps onto its edge a few degrees
/// over, which is why it sits still and then bolts. Meanwhile the same
/// friction stands the top back up, so it settles again. The lean precesses,
/// so the path curves.
///
/// The engine's sphere tips give the smooth part of that by themselves. This
/// component adds what they can't: the face/edge jump, the righting, and the
/// small wobbles (floor, hits) that tip a settled top over again. Nothing
/// here steers: a wobble tips the top in a random direction.
#[derive(Component, Clone, Copy, Debug)]
pub struct Wobble {
    /// Tilt (radians) the current wobble still has to add, about `pivot`.
    kick_left: f32,
    pivot: Vec3,
    /// Seconds the top still rides on its edge before friction starts to
    /// stand it up again.
    ride: f32,
    /// Seconds until the floor wobbles the top again.
    next_kick: f32,
    /// Stagger timer last step (a jump means the top was just hit).
    prev_stun: f32,
    seed: u32,
}

impl Wobble {
    pub fn new(seed: u32) -> Self {
        let seed = seed.wrapping_mul(747_796_405).wrapping_add(2_891_336_453) | 1;
        Self { kick_left: 0.0, pivot: Vec3::X, ride: 0.0, next_kick: 0.8, prev_stun: 0.0, seed }
    }
    /// Uniform in 0..1 (xorshift).
    fn rand(&mut self) -> f32 {
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }
    fn between(&mut self, range: (f32, f32)) -> f32 {
        range.0 + (range.1 - range.0) * self.rand()
    }
}

type DriveQuery<'a> = (
    &'a Top,
    &'a RigidBody,
    &'a Position,
    &'a mut Rotation,
    &'a mut LinearVelocity,
    &'a mut AngularVelocity,
    &'a ComputedMass,
    &'a ComputedAngularInertia,
    &'a mut SpinDebt,
    &'a SteerState,
    &'a mut Wobble,
);

/// Tilt-driven travel of the tip: see [`Wobble`].
pub fn tip_drive(time: Res<Time>, mut tops: Query<DriveQuery>) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (top, body, pos, mut rot, mut lin, mut ang, mass, inertia, mut debt, steer, mut wob) in &mut tops {
        if *body != RigidBody::Dynamic {
            continue;
        }
        let hit = steer.stun > wob.prev_stun + 0.3;
        wob.prev_stun = steer.stun;
        let flat = Vec3::new(pos.0.x, 0.0, pos.0.z);
        let r = flat.length();
        let radial = if r > 1e-3 { flat / r } else { Vec3::ZERO };
        let mut axis = rot.0 * Vec3::Y;
        let spin = ang.0.dot(axis);
        let floor = crate::stadium::surface_height(r, pos.0.z.atan2(pos.0.x));
        // Only a top standing on its tip in the bowl.
        if (pos.0.y - floor).abs() > 0.1
            || r > crate::stadium::BOWL_RADIUS
            || axis.y < 0.6
            || spin.abs() < DRIVE_MIN_SPIN
        {
            continue;
        }
        let d = top.build.tip.drive();
        let normal = (Vec3::Y - radial * crate::stadium::bowl_slope(r)).normalize();
        let turn = |q: Quat, rot: &mut Rotation, ang: &mut AngularVelocity| {
            rot.0 = (q * rot.0).normalize();
            ang.0 = q * ang.0;
        };

        // Wobbles: a bump in the floor now and then, and every hit taken.
        wob.next_kick -= dt;
        let kick = if hit {
            Some(wob.between(HIT_KICK) * if d.edge < 0.01 { 0.5 } else { 1.0 })
        } else if wob.next_kick <= 0.0 {
            wob.next_kick = wob.between(d.kick_every);
            // A slower top is less stiff: the same bump tips it further.
            Some(wob.between(d.kick_tilt) * (150.0 / spin.abs()).sqrt().min(1.5))
        } else {
            None
        };
        if let Some(tilt) = kick {
            // Tipped over in whatever direction the bump happens to push it.
            let angle = wob.rand() * std::f32::consts::TAU;
            let lean = Vec3::new(angle.cos(), 0.0, angle.sin());
            wob.pivot = Vec3::Y.cross(lean);
            wob.kick_left = tilt;
            wob.ride = wob.between(RIDE_TIME);
        }
        if wob.kick_left > 0.0 {
            let step = (KICK_RATE * dt).min(wob.kick_left);
            wob.kick_left -= step;
            turn(Quat::from_axis_angle(wob.pivot, step), &mut rot, &mut ang);
        } else if wob.ride > 0.0 {
            wob.ride -= dt;
        } else {
            // Righting: friction at the tip stands the top up again (a flat
            // face mostly square to the floor, a point straight up).
            let upright = (Vec3::Y + (normal - Vec3::Y) * d.follow).normalize();
            let off = axis.angle_between(upright);
            if off > 1e-4 && off < 0.6 {
                if let Some(pivot) = axis.cross(upright).try_normalize() {
                    let step = (d.righting * (spin.abs() / 150.0).min(1.0) * off * dt).min(off);
                    turn(Quat::from_axis_angle(pivot, step), &mut rot, &mut ang);
                }
            }
        }

        // Drive: where the tip touches the floor, and what friction makes of it.
        axis = rot.0 * Vec3::Y;
        let rel = axis - normal * axis.dot(normal);
        let sin_tilt = rel.length().min(1.0);
        let Some(lean) = rel.with_y(0.0).try_normalize() else { continue };
        let tilt = sin_tilt.asin();
        let t = ((tilt - d.edge_tilt.0) / (d.edge_tilt.1 - d.edge_tilt.0)).clamp(0.0, 1.0);
        let (on_edge, on_ball) = (d.edge * t * t * (3.0 - 2.0 * t), d.ball * sin_tilt);
        let offset = on_edge.max(on_ball);
        if offset < 1e-5 {
            continue;
        }
        // On its edge the tip bites fully. On the rounded part the engine's
        // sphere tip already supplies its own share of the drive.
        let mut share =
            if on_edge >= on_ball { 1.0 } else { (1.0 - d.collider * sin_tilt / offset).clamp(0.0, 1.0) };
        if steer.stun > 0.0 {
            share *= DRIVE_WHEN_BUSY;
        }
        // High up the bowl the slope takes over and brings the top back.
        let high = ((r - 2.2) / 1.0).clamp(0.0, 1.0);
        share *= 1.0 - high * high * (3.0 - 2.0 * high);
        let rolling = lean.cross(Vec3::Y) * (spin * offset);
        let level = Vec3::new(lin.0.x, 0.0, lin.0.z);
        // (Capped: past this a grippier tip no longer bites any harder.)
        let grip = top.build.tip.friction().min(DRIVE_MAX_GRIP) * crate::GRAVITY * share;
        let dv = (rolling - level).clamp_length_max(grip * dt);
        lin.0 += dv;
        // The reaction of that friction force is a braking torque on the spin.
        let i_axis = inertia.rotated(rot.0).tensor().mul_vec3(axis).dot(axis).max(1e-6);
        debt.axial += DRIVE_SPIN_COST * dv.length() * offset / (mass.inverse().max(1e-6) * i_axis);
    }
}

fn build_torsion(top: &Top) -> f32 {
    top.build.tip.friction() * top.build.tip.contact_radius()
}

/// Why a top lost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finish {
    RingOut,
    SpinOut,
}

impl Finish {
    /// Points the opponent scores.
    pub fn points(self) -> u32 {
        match self {
            Finish::RingOut => 2,
            Finish::SpinOut => 1,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Finish::RingOut => "出界终结",
            Finish::SpinOut => "旋转终结",
        }
    }
}

/// Lose rules shared by the game and the headless simulation.
pub fn check_finish(pos: Vec3, meter: &SpinMeter) -> Option<Finish> {
    let r = Vec2::new(pos.x, pos.z).length();
    if r > crate::stadium::OUT_RADIUS || pos.y < -0.25 {
        return Some(Finish::RingOut);
    }
    // Out only when it has really stopped turning, whatever its orientation.
    if meter.smoothed < STOP_SPIN {
        return Some(Finish::SpinOut);
    }
    None
}
