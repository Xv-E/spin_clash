//! Headless physics simulation: `cargo run --release -- --sim`.
//! Runs solo spin tests and a few duels with the real game physics, without a
//! window, to check that the parts behave as intended.

use crate::parts::{Build, Disk, Ring, SpinDir, Tip};
use crate::stadium::spawn_stadium_headless;
use crate::top::{
    Blade, RingCollider, SpinMeter, axial_spin, check_finish, launch, spawn_top, Finish, Owner, Top,
};
use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use std::time::Duration;

const MAX_SECS: f32 = 150.0;

fn new_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        TransformPlugin,
        bevy::asset::AssetPlugin::default(),
        bevy::mesh::MeshPlugin,
        PhysicsPlugins::default().with_collision_hooks::<crate::top::BladeHooks>(),
    ));
    crate::configure_physics(&mut app);
    if let Ok(n) = std::env::var("SIM_SUB") { app.insert_resource(SubstepCount(n.parse().unwrap())); }
    if let Ok(n) = std::env::var("SIM_HZ") { app.insert_resource(Time::<Fixed>::from_hz(n.parse().unwrap())); }
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / crate::PHYSICS_HZ,
    )));
    // Driving the app by hand: run the plugin lifecycle hooks `App::run` would.
    app.finish();
    app.cleanup();
    {
        let world = app.world_mut();
        let mut commands = world.commands();
        if std::env::var("SIM_FLAT").is_ok() {
            commands.spawn((RigidBody::Static, Collider::cuboid(20.0, 1.0, 20.0), Transform::from_xyz(0.0, -0.5, 0.0), Friction::new(1.0).with_combine_rule(CoefficientCombine::Multiply)));
        } else {
            spawn_stadium_headless(&mut commands);
        }
        world.flush();
    }
    app.update();
    app
}

struct Launch {
    build: Build,
    owner: Owner,
    pos: Vec3,
    vel: Vec3,
    spin: f32,
    tilt: f32,
}

/// What happened in the first 0.15 s after the tops first touched.
#[derive(Default, Clone, Copy)]
struct FirstHit {
    /// Share of spin each top kept.
    kept: [f32; 2],
    /// Peak upward speed of each top (lift).
    lift: [f32; 2],
    /// Peak downward speed of each top (pressed down).
    press: [f32; 2],
    /// Peak horizontal speed of each top (knock-back).
    knock: [f32; 2],
}

struct Outcome {
    owner: Owner,
    finish: Finish,
    time: f32,
    max_r: f32,
}

fn simulate(launches: &[Launch], stop_on_first: bool) -> (Vec<Outcome>, u32, Option<FirstHit>) {
    let mut app = new_app();
    let mut ents = Vec::new();
    {
        let world = app.world_mut();
        let mut commands = world.commands();
        for l in launches {
            let e = spawn_top(&mut commands, l.build, l.owner, Transform::from_translation(l.pos), None);
            if std::env::var("SIM_NOCCD").is_ok() { commands.entity(e).remove::<SweptCcd>(); }
            ents.push(e);
        }
        world.flush();
    }
    app.update();
    {
        let world = app.world_mut();
        let mut commands = world.commands();
        for (l, &e) in launches.iter().zip(&ents) {
            launch(
                &mut commands,
                e,
                Quat::from_rotation_z(l.tilt),
                l.spin * l.build.dir.sign(),
                l.vel,
            );
            // SIM_COIN: whirl about the world vertical whatever the tilt
            // (a top knocked onto its edge, spinning like a coin).
            if std::env::var("SIM_COIN").is_ok() {
                commands.entity(e).insert((
                    AngularVelocity(Vec3::Y * l.spin),
                    SpinMeter { smoothed: l.spin },
                ));
            }
        }
        world.flush();
    }
    let dt = 1.0 / crate::PHYSICS_HZ as f32;
    let mut t = 0.0;
    let mut outcomes: Vec<Outcome> = Vec::new();
    let mut max_r = vec![0.0f32; ents.len()];
    let mut contacts = 0u32;
    let mut touching_prev = false;
    // (time of first touch, spin of each top just before it)
    let mut first: Option<(f32, [f32; 2])> = None;
    let mut hit = FirstHit::default();
    let mut prev_spin = [0.0f32; 2];
    // SIM_CONTACTS: per contact kind -> (steps seen, deepest penetration)
    let mut kinds: std::collections::BTreeMap<&'static str, (u32, f32)> = Default::default();
    while t < MAX_SECS {
        app.update();
        t += dt;
        let world = app.world_mut();
        for (i, &e) in ents.iter().enumerate() {
            if outcomes.iter().any(|o| o.owner == launches[i].owner) {
                continue;
            }
            let pos = world.get::<Position>(e).unwrap().0;
            let rot = *world.get::<Rotation>(e).unwrap();
            let ang = *world.get::<AngularVelocity>(e).unwrap();
            max_r[i] = max_r[i].max(Vec2::new(pos.x, pos.z).length());
            if std::env::var("SIM_TRACE").is_ok() && ((t * 120.0) as u32) % 6 == 0 && t < std::env::var("SIM_TMAX").ok().and_then(|v| v.parse().ok()).unwrap_or(2.0) {
                let up = rot.0 * Vec3::Y;
                println!("      [{i}] t={t:.2} pos=({:.2},{:.2},{:.2}) tilt={:.1}° up=({:.2},{:.2}) spin={:.1} ang=({:.1},{:.1},{:.1})", pos.x, pos.y, pos.z, up.angle_between(Vec3::Y).to_degrees(), up.x, up.z, axial_spin(&rot, &ang), ang.0.x, ang.0.y, ang.0.z);
            }
            if t > 1.0 {
                let meter = world.get::<SpinMeter>(e).copied().unwrap_or_default();
                if let Some(f) = check_finish(pos, &meter) {
                    outcomes.push(Outcome { owner: launches[i].owner, finish: f, time: t, max_r: max_r[i] });
                }
            }
        }
        if ents.len() == 2 && std::env::var("SIM_CONTACTS").is_ok() {
            let graph = world.resource::<ContactGraph>();
            let kind_of = |c: Entity| {
                if world.get::<Blade>(c).is_some() {
                    "刃"
                } else if world.get::<RingCollider>(c).is_some() {
                    "本体"
                } else {
                    "轴尖"
                }
            };
            for pair in graph.iter_active() {
                let (Some(a), Some(b)) = (pair.body1, pair.body2) else { continue };
                if a == b || !ents.contains(&a) || !ents.contains(&b) {
                    continue;
                }
                let (k1, k2) = (kind_of(pair.collider1), kind_of(pair.collider2));
                let key: &'static str = match (k1.min(k2), k1.max(k2)) {
                    ("刃", "刃") => "刃-刃",
                    ("刃", "本体") | ("本体", "刃") => "刃-本体",
                    ("本体", "本体") => "本体-本体",
                    _ => "其他",
                };
                let deepest = pair
                    .manifolds
                    .iter()
                    .flat_map(|m| m.points.iter())
                    .map(|c| c.penetration)
                    .fold(f32::MIN, f32::max);
                let e = kinds.entry(key).or_insert((0, f32::MIN));
                if pair.is_touching() {
                    e.0 += 1;
                }
                e.1 = e.1.max(deepest);
            }
        }
        if ents.len() == 2 && std::env::var("SIM_TRACE").is_ok() {
            let graph = world.resource::<ContactGraph>();
            for pair in graph.iter_active() {
                if pair.body1.is_some() && pair.body2.is_some() && pair.body1 != pair.body2
                    && ents.contains(&pair.body1.unwrap()) && ents.contains(&pair.body2.unwrap()) && pair.is_touching()
                {
                    let ring1 = world.get::<RingCollider>(pair.collider1).is_some();
                    let ring2 = world.get::<RingCollider>(pair.collider2).is_some();
                    for m in &pair.manifolds {
                        for c in &m.points {
                            println!("        contact t={t:.3} ring=({ring1},{ring2}) normal={:?} pen={:.3} impulse={:.2} normal_speed={:.2}", m.normal, c.penetration, c.normal_impulse, c.normal_speed);
                        }
                    }
                }
            }
        }
        if ents.len() == 2 {
            let spin_of = |e: Entity| {
                axial_spin(world.get::<Rotation>(e).unwrap(), world.get::<AngularVelocity>(e).unwrap()).abs()
            };
            let spins = [spin_of(ents[0]), spin_of(ents[1])];
            let graph = world.resource::<ContactGraph>();
            let touching_now = graph.iter_active_touching().any(|pair| {
                matches!((pair.body1, pair.body2), (Some(a), Some(b))
                    if a != b && ents.contains(&a) && ents.contains(&b))
            });
            let blade_hit = world.resource::<crate::blade_hits::BladeHitStats>().hits > 0;
            if (touching_now || blade_hit) && first.is_none() {
                first = Some((t, prev_spin));
            }
            if let Some((t0, before)) = first {
                if t - t0 <= 0.15 {
                    for k in 0..2 {
                        let v = world.get::<LinearVelocity>(ents[k]).unwrap().0;
                        hit.kept[k] = spins[k] / before[k].max(1e-3);
                        hit.lift[k] = hit.lift[k].max(v.y);
                        hit.press[k] = hit.press[k].max(-v.y);
                        hit.knock[k] = hit.knock[k].max(Vec2::new(v.x, v.z).length());
                    }
                }
            }
            prev_spin = spins;
            let p0 = world.get::<Position>(ents[0]).unwrap().0;
            let p1 = world.get::<Position>(ents[1]).unwrap().0;
            let touching = p0.distance(p1) < 0.95;
            if touching && !touching_prev {
                contacts += 1;
            }
            touching_prev = touching;
        }
        if outcomes.len() == ents.len() || (stop_on_first && !outcomes.is_empty()) {
            break;
        }
    }
    if std::env::var("SIM_CONTACTS").is_ok() {
        let st = app.world().resource::<crate::blade_hits::BladeHitStats>();
        println!("      解析刃击: {} 次  最大冲量 {:.2}", st.hits, st.max_impulse);
    }
    for (k, (n, pen)) in &kinds {
        println!("      接触 {k}: 接触步数 {n}  最深穿透 {pen:.3}");
    }
    // Sanity: report spin of survivors.
    let world = app.world_mut();
    let mut q = world.query::<(&Top, &Rotation, &AngularVelocity)>();
    for (top, rot, ang) in q.iter(world) {
        if !outcomes.iter().any(|o| o.owner == top.owner) {
            println!("    存活: {:?} 剩余转速 {:.1} rad/s", top.owner, axial_spin(rot, ang).abs());
        }
    }
    (outcomes, contacts, first.map(|_| hit))
}

pub fn run() {
    if let Ok(only) = std::env::var("SIM_TIP") {
        let tip = Tip::ALL[only.parse::<usize>().unwrap()];
        let ring = Ring(std::env::var("SIM_RING").ok().and_then(|v| v.parse().ok()).unwrap_or(0));
        let disk = Disk::ALL[std::env::var("SIM_DISK").ok().and_then(|v| v.parse().ok()).unwrap_or(0)];
        let tilt = std::env::var("SIM_TILT").ok().and_then(|v| v.parse().ok()).unwrap_or(0.05);
        let build = Build { ring, disk, tip, dir: SpinDir::Right };
        let (out, _, _) = simulate(&[Launch { build, owner: Owner::Player, pos: Vec3::new(1.0, 0.6, 0.0), vel: Vec3::new(-0.5, 0.0, 0.8), spin: 150.0, tilt }], false);
        for o in out { println!("  {:.1}s {:?}", o.time, o.finish); }
        return;
    }
    println!("== 单陀螺持久测试 (发射转速 150 rad/s, 中心附近, 轻微倾斜) ==");
    // SIM_DUELS: skip the solo tests.
    let solo_tips: &[Tip] = if std::env::var("SIM_DUELS").is_ok() { &[] } else { &Tip::ALL };
    for &tip in solo_tips {
        for disk in [Disk::Standard, Disk::RimHeavy] {
            let build = Build { ring: Ring::by_name("冲锋刃"), disk, tip, dir: SpinDir::Right };
            let (out, _, _) = simulate(
                &[Launch {
                    build,
                    owner: Owner::Player,
                    pos: Vec3::new(1.0, 0.6, 0.0),
                    vel: Vec3::new(-0.5, 0.0, 0.8),
                    spin: 150.0,
                    tilt: 0.05,
                }],
                false,
            );
            for o in out {
                println!(
                    "  {:<6} + {:<6}: {:>5.1}s 后{:?}  最大游走半径 {:.2}",
                    tip.name(),
                    disk.name(),
                    o.time,
                    o.finish,
                    o.max_r
                );
            }
        }
    }

    println!("\n== 对战测试 ==");
    let matchups = [
        (
            Build { ring: Ring::by_name("冲锋刃"), disk: Disk::Standard, tip: Tip::Rubber, dir: SpinDir::Right },
            Build { ring: Ring::by_name("守护环"), disk: Disk::RimHeavy, tip: Tip::Sharp, dir: SpinDir::Right },
        ),
        (
            Build { ring: Ring::by_name("重锤双翼"), disk: Disk::Standard, tip: Tip::Flat, dir: SpinDir::Right },
            Build { ring: Ring::by_name("风暴六翼"), disk: Disk::Light, tip: Tip::Ball, dir: SpinDir::Left },
        ),
        (
            Build { ring: Ring::by_name("冲锋刃"), disk: Disk::RimHeavy, tip: Tip::Ball, dir: SpinDir::Right },
            Build { ring: Ring::by_name("冲锋刃"), disk: Disk::RimHeavy, tip: Tip::Ball, dir: SpinDir::Right },
        ),
        // Blade-geometry duels: same disk / tip, only the ring differs.
        (duel_build("升龙刃"), duel_build("冲锋刃")),
        (duel_build("重击刃"), duel_build("守护环")),
        (duel_build("镇压刃"), duel_build("升龙刃")),
        (duel_build("重击刃"), duel_build("重击刃")),
        // Blade abilities against the same plain attacker.
        (duel_build("回弹盾"), duel_build("重击刃")),
        (duel_build("缓冲轮"), duel_build("重击刃")),
        (duel_build("噬旋爪"), Build { dir: SpinDir::Left, ..duel_build("重击刃") }),
        (duel_build("噬旋爪"), duel_build("重击刃")),
    ];
    // SIM_A / SIM_B = "ring,disk,tip,dir" (catalogue indices; dir 0 = right):
    // one custom duel instead of the list.
    let custom = |name: &str| {
        let v: Vec<usize> = std::env::var(name).ok()?.split(',').map(|x| x.trim().parse().unwrap()).collect();
        Some(Build {
            ring: Ring(v[0]),
            disk: Disk::ALL[v[1]],
            tip: Tip::ALL[v[2]],
            dir: if v.get(3) == Some(&1) { SpinDir::Left } else { SpinDir::Right },
        })
    };
    let matchups: Vec<(Build, Build)> = match (custom("SIM_A"), custom("SIM_B")) {
        (Some(a), Some(b)) => vec![(a, b)],
        _ => matchups.to_vec(),
    };
    let only: Option<usize> = std::env::var("SIM_DUEL").ok().map(|v| v.parse().unwrap());
    for (mi, (a, b)) in matchups.into_iter().enumerate() {
        if only.is_some_and(|o| o != mi) { continue; }
        println!(
            "  {} {} {} (质量 {:.3} 惯量 {:.4})  vs  {} {} {} (质量 {:.3} 惯量 {:.4})",
            a.ring.name(), a.disk.name(), a.tip.name(), a.stats().mass, a.stats().spin_inertia,
            b.ring.name(), b.disk.name(), b.tip.name(), b.stats().mass, b.stats().spin_inertia
        );
        for seed in 0..4 {
            let side = [0.0, 0.12, -0.12, 0.25][seed];
            let (out, contacts, first_hit) = simulate(
                &[
                    Launch {
                        build: a,
                        owner: Owner::Player,
                        pos: Vec3::new(side, 0.4, 1.3),
                        vel: Vec3::new(0.0, 0.0, -2.5),
                        spin: 170.0,
                        tilt: 0.03,
                    },
                    Launch {
                        build: b,
                        owner: Owner::Ai,
                        pos: Vec3::new(-side, 0.4, -1.3),
                        vel: Vec3::new(0.0, 0.0, 2.5),
                        spin: 160.0,
                        tilt: -0.03,
                    },
                ],
                true,
            );
            if let Some(h) = first_hit {
                println!(
                    "      首撞 0.15s 内: 转速保留 {:.0}% / {:.0}%   上抛 {:.2} / {:.2}   下压 {:.2} / {:.2}   横飞 {:.1} / {:.1}",
                    h.kept[0] * 100.0, h.kept[1] * 100.0,
                    h.lift[0], h.lift[1], h.press[0], h.press[1], h.knock[0], h.knock[1]
                );
            }
            match out.first() {
                Some(o) => println!(
                    "    第{}局: {:?} 在 {:.1}s {:?}（撞击 {} 次）",
                    seed + 1,
                    o.owner,
                    o.time,
                    o.finish,
                    contacts
                ),
                None => println!("    第{}局: {}s 内无人倒下（撞击 {} 次）", seed + 1, MAX_SECS, contacts),
            }        }
    }
}

fn duel_build(ring: &str) -> Build {
    Build { ring: Ring::by_name(ring), disk: Disk::Standard, tip: Tip::Ball, dir: SpinDir::Right }
}
