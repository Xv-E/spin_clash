//! The battle stadium: a bowl with a raised wall and three low "exit" gaps.
//! The same triangle data is used for rendering and as the static collider.

use avian3d::prelude::*;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use std::f32::consts::TAU;

pub const BOWL_RADIUS: f32 = 3.6;
pub const WALL_RADIUS: f32 = 4.0;
pub const LIP_RADIUS: f32 = 4.35;
/// A top whose centre passes this radius has left the stadium (it is over
/// the crest of the wall).
pub const OUT_RADIUS: f32 = WALL_RADIUS + 0.2;
/// Bowl profile: height = K2 r² + K4 r⁴. The r⁴ term makes it curl up
/// towards the rim, so tops are thrown back at the middle.
const BOWL_K2: f32 = 0.045;
const BOWL_K4: f32 = 0.0009;
/// How fast the top of the wall falls away outwards.
const LIP_FALL: f32 = 0.5;
const EXITS: usize = 3;

#[derive(Component)]
pub struct Stadium;

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Wall height at angle `theta`: tall, dipping low at the exits.
fn wall_height(theta: f32) -> f32 {
    let sector = TAU / EXITS as f32;
    let local = (theta.rem_euclid(sector) - sector * 0.5).abs(); // 0 at an exit centre
    let gap = 1.0 - smoothstep((local - 0.12) / 0.2);
    0.85 - 0.62 * gap
}

fn bowl_height(r: f32) -> f32 {
    BOWL_K2 * r * r + BOWL_K4 * r.powi(4)
}

/// Slope (rise per unit of radius) of the bowl at radius `r`.
pub fn bowl_slope(r: f32) -> f32 {
    let r = r.min(BOWL_RADIUS);
    2.0 * BOWL_K2 * r + 4.0 * BOWL_K4 * r.powi(3)
}

/// Height of the stadium surface. Continuous everywhere: the wall grows out
/// of the bowl's rim, and its top slopes away outwards so nothing can come to
/// rest on it.
pub fn surface_height(r: f32, theta: f32) -> f32 {
    if r <= BOWL_RADIUS {
        return bowl_height(r);
    }
    let t = (r - BOWL_RADIUS) / (WALL_RADIUS - BOWL_RADIUS);
    let wall = bowl_height(BOWL_RADIUS)
        + bowl_slope(BOWL_RADIUS) * (r.min(WALL_RADIUS) - BOWL_RADIUS)
        + wall_height(theta) * smoothstep(t);
    if r <= WALL_RADIUS {
        wall
    } else if r <= LIP_RADIUS {
        wall - (r - WALL_RADIUS) * LIP_FALL
    } else {
        // Outer edge drops away.
        wall - (LIP_RADIUS - WALL_RADIUS) * LIP_FALL - (r - LIP_RADIUS) * 6.0
    }
}

fn build_geometry() -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let rings = 56;
    let segs = 120;
    let r_max = LIP_RADIUS + 0.12;
    let mut verts = vec![Vec3::ZERO];
    for i in 1..=rings {
        let t = i as f32 / rings as f32;
        // Denser rings near the wall.
        let r = r_max * t.powf(0.8);
        for j in 0..segs {
            let theta = j as f32 / segs as f32 * TAU;
            verts.push(Vec3::new(r * theta.cos(), surface_height(r, theta), r * theta.sin()));
        }
    }
    let idx = |i: u32, j: u32| 1 + (i - 1) * segs + (j % segs);
    let mut tris = Vec::new();
    for j in 0..segs {
        tris.push([0, idx(1, j + 1), idx(1, j)]);
    }
    for i in 1..rings {
        for j in 0..segs {
            let (a, b) = (idx(i, j), idx(i, j + 1));
            let (c, d) = (idx(i + 1, j), idx(i + 1, j + 1));
            tris.push([a, b, c]);
            tris.push([b, d, c]);
        }
    }
    (verts, tris)
}

pub fn stadium_collider() -> Collider {
    let (verts, tris) = build_geometry();
    Collider::trimesh_with_config(verts, tris, TrimeshFlags::FIX_INTERNAL_EDGES)
}

fn stadium_mesh() -> Mesh {
    let (verts, tris) = build_geometry();
    let colors: Vec<[f32; 4]> = verts
        .iter()
        .map(|v| {
            let r = Vec2::new(v.x, v.z).length();
            let theta = v.z.atan2(v.x);
            let ring = ((r * 2.0).fract() < 0.04) as u8 as f32;
            let base = if r < BOWL_RADIUS {
                Vec3::new(0.32, 0.36, 0.46)
            } else if wall_height(theta) < 0.5 && r > BOWL_RADIUS + 0.2 {
                Vec3::new(0.8, 0.12, 0.08)
            } else {
                Vec3::new(0.08, 0.1, 0.2)
            };
            let c = base * (1.0 - 0.25 * ring);
            [c.x, c.y, c.z, 1.0]
        })
        .collect();
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        verts.iter().map(|v| v.to_array()).collect::<Vec<_>>(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(tris.into_iter().flatten().collect()));
    mesh.compute_smooth_normals();
    mesh
}

/// Physics-only stadium (for the headless simulation).
pub fn spawn_stadium_physics(commands: &mut Commands) -> Entity {
    commands
        .spawn((
            Stadium,
            RigidBody::Static,
            stadium_collider(),
            Friction::new(1.0).with_combine_rule(CoefficientCombine::Multiply),
            Restitution::new(0.1),
            Transform::default(),
        ))
        .id()
}

/// Ground plane outside the stadium where knocked-out tops land.
fn ground_bundle() -> impl Bundle {
    (
        RigidBody::Static,
        Collider::cuboid(40.0, 1.0, 40.0),
        Transform::from_xyz(0.0, -1.1, 0.0),
        Friction::new(1.0).with_combine_rule(CoefficientCombine::Multiply),
    )
}

pub fn spawn_stadium(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    scope: impl Bundle + Clone,
) {
    let e = spawn_stadium_physics(commands);
    commands.entity(e).insert((
        Mesh3d(meshes.add(stadium_mesh())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 0.4,
            reflectance: 0.4,
            ..default()
        })),
        scope.clone(),
    ));
    commands.spawn((
        ground_bundle(),
        Mesh3d(meshes.add(Cuboid::new(40.0, 1.0, 40.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.08, 0.09, 0.12),
            perceptual_roughness: 0.9,
            ..default()
        })),
        scope.clone(),
    ));
    // Support column under the bowl.
    commands.spawn((
        Mesh3d(meshes.add(ConicalFrustum {
            radius_top: LIP_RADIUS + 0.1,
            radius_bottom: LIP_RADIUS - 0.3,
            height: 0.6,
        })),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.15, 0.17, 0.25),
            ..default()
        })),
        Transform::from_xyz(0.0, -0.32, 0.0),
        scope,
    ));
}

pub fn spawn_stadium_headless(commands: &mut Commands) {
    spawn_stadium_physics(commands);
    commands.spawn(ground_bundle());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No step or groove anywhere a top can get to.
    #[test]
    fn surface_is_continuous() {
        for j in 0..24 {
            let theta = j as f32 / 24.0 * TAU;
            let mut prev = surface_height(0.0, theta);
            let mut r = 0.0;
            while r < LIP_RADIUS {
                r += 0.01;
                let h = surface_height(r, theta);
                assert!((h - prev).abs() < 0.04, "step of {} at r={r}", h - prev);
                prev = h;
            }
        }
    }

    #[test]
    fn bowl_rises_all_the_way_to_the_wall() {
        let mut prev = 0.0;
        for i in 1..=40 {
            let h = surface_height(i as f32 * 0.1, 0.3);
            assert!(h > prev);
            prev = h;
        }
    }
}
