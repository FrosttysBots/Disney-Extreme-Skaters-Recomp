//! Skates a level the way a player does and reports problems:
//! - rail approaches: roll beside each rail, ollie towards it holding the
//!   grind button, and count how often it grinds;
//! - random skating (pushing, steering, ollies, grinds, manuals, tricks):
//!   frames where the skater crossed ground without landing on it.
//!
//! `cargo run --release -p desa_viewer --example skate_check [game data] [level] [runs]`
//!
//! `MISSES=1` lists the rail approaches that didn't grind; `TRACE=run:frame`
//! prints 25 frames of a random run from there (and why the ground was
//! lost); `PROBE=x,y,z` lists the faces around a spot.
use std::path::Path;

use desa_viewer::nodes::LevelNodes;
use desa_viewer::source::GameData;
use glam::Vec3;
use qb::vm::Program;
use skate::{ChaseCamera, Input, Physics, Rails, Segment, Skater, Stats, TrickBook, World};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).cloned().unwrap_or_else(|| "extracted".into());
    let level = args.get(2).cloned().unwrap_or_else(|| "HUB".into());
    let runs: u32 = args.get(3).map_or(30, |r| r.parse().unwrap());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let files = data.load_level(&level).unwrap();
    let mut program = Program::new();
    for file in data.global_scripts().unwrap() {
        program.add(&file).unwrap();
    }
    let stats = Stats::of(&program, "jessie");
    let physics = Physics::new(&program, &stats);
    let mut tricks = TrickBook::new(&program, "jessie", &stats);
    tricks.set_durations(|_| Some(0.6));
    let collision = ngc_collision::Collision::parse(files.collision.as_ref().unwrap()).unwrap();
    let nodes = LevelNodes::from_bytes(files.nodes.as_ref().unwrap()).unwrap();
    let rails = Rails::new(
        nodes
            .rails
            .iter()
            .map(|r| Segment {
                start: r.start,
                end: r.end,
            })
            .collect(),
    );
    let world = World::new(collision).with_rails(rails.clone());
    let dt = 1.0 / 60.0;

    // PROBE=x,y,z: every face on the vertical line through a spot, and
    // what's around it at knee height.
    if let Ok(probe) = std::env::var("PROBE") {
        let v: Vec<f32> = probe.split(',').map(|x| x.parse().unwrap()).collect();
        let at = Vec3::new(v[0], v[1], v[2]);
        let to = at - Vec3::Y * 60.0;
        let mut from = at + Vec3::Y * 30.0;
        while let Some(hit) = world.ray(from, to) {
            println!(
                "down: {:.2} normal {:.2} flags {:#x}",
                hit.point, hit.normal, hit.flags
            );
            from = hit.point - Vec3::Y * 0.01;
        }
        for (dx, dz) in [(15.0, 0.0), (-15.0, 0.0), (0.0, 15.0), (0.0, -15.0)] {
            let knee = at + Vec3::Y * 8.1;
            if let Some(hit) = world.ray(knee, knee + Vec3::new(dx, 0.0, dz)) {
                println!(
                    "side {dx} {dz}: {:.2} normal {:.2} flags {:#x}",
                    hit.point, hit.normal, hit.flags
                );
            }
        }
        return;
    }

    // Rail approaches.
    let (mut tried, mut grinded) = (0, 0);
    for rail in rails
        .segments
        .iter()
        .filter(|r| r.length() > 150.0)
        .step_by(7)
    {
        let along = rail.direction();
        let flat = Vec3::new(along.x, 0.0, along.z).normalize_or_zero();
        if flat == Vec3::ZERO {
            continue;
        }
        let side = Vec3::new(-flat.z, 0.0, flat.x);
        for s in [1.0, -1.0] {
            // Ground 30 beside the rail's first third.
            let spot = rail.start.lerp(rail.end, 0.3) + side * s * 30.0;
            let Some(ground) = world.ray(spot + Vec3::Y * 60.0, spot - Vec3::Y * 200.0) else {
                continue;
            };
            if ground.normal.y < 0.9 || ground.point.y > rail.start.y.max(rail.end.y) + 5.0 {
                continue;
            }
            // Heading along the rail, turned 20 degrees towards it.
            let towards = (flat - side * s * 0.36).normalize();
            let mut skater = Skater::new(ground.point, towards.x.atan2(towards.z));
            skater.velocity = towards * 400.0;
            tried += 1;
            let mut got = false;
            for frame in 0..120 {
                let input = Input {
                    crouch: frame < 10,
                    grind: frame > 10,
                    ..Input::default()
                };
                skater.update(input, &physics, &world, dt);
                if skater.grind.is_some() {
                    got = true;
                    break;
                }
            }
            grinded += usize::from(got);
            if !got && std::env::var("MISSES").is_ok() {
                println!(
                    "missed: rail {:.0} -> {:.0}, {:.0} above the ground",
                    rail.start,
                    rail.end,
                    rail.start.lerp(rail.end, 0.3).y - ground.point.y
                );
            }
        }
    }
    println!("rail approaches: {grinded} of {tried} grinded");

    // Random skating.
    let start = nodes.start().unwrap();
    let facing = start.facing();
    let mut seed: u32 = 7;
    let mut random = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / (1u32 << 24) as f32
    };
    let (mut throughs, mut resets, mut hidden) = (0, 0, 0);
    for run in 0..runs {
        let mut skater = Skater::new(start.position, facing.x.atan2(facing.z));
        skater.tricks = tricks.clone();
        skater.spawns = nodes
            .spawns
            .iter()
            .map(|s| (s.position, s.facing().x.atan2(s.facing().z)))
            .collect();
        let mut input = Input::default();
        let mut chase = ChaseCamera::behind(&skater, &physics);
        let mut takeoff = start.position;
        let mut checked = false;
        for frame in 0..3600 {
            if frame % 20 == 0 {
                input = Input {
                    push: random() < 0.6,
                    brake: random() < 0.1,
                    turn: (random() * 3.0).floor() - 1.0,
                    crouch: random() < 0.3,
                    grind: random() < 0.4,
                    flip: random() < 0.15,
                    grab: random() < 0.15,
                };
            }
            let before = skater.position;
            skater.update(input, &physics, &world, dt);
            let after = skater.position;
            // The camera must always see the skater.
            chase.update(&skater, &physics, &world, dt);
            if world.ray(chase.target, chase.eye).is_some() {
                hidden += 1;
            }
            let trace = std::env::var("TRACE").ok().and_then(|t| {
                let (r, f) = t.split_once(':')?;
                Some((r.parse::<u32>().ok()?, f.parse::<u32>().ok()?))
            });
            skater.trace = trace.is_some_and(|(r, f)| r == run && (f..f + 25).contains(&frame));
            if trace.is_some_and(|(r, f)| r == run && (f..f + 25).contains(&frame)) {
                println!(
                    "{frame}: {:?} ground {} grind {:?} v {:.0} at {after:.1} (input push {} crouch {} grind {})",
                    skater.action,
                    skater.on_ground,
                    skater.grind.map(|g| g.segment),
                    skater.velocity,
                    input.push,
                    input.crouch,
                    input.grind
                );
            }
            if skater.on_ground {
                takeoff = after;
                checked = false;
            } else if !checked && after.y < takeoff.y - 200.0 {
                // Falling well below where it took off: was there ground
                // in the way?
                checked = true;
                let top = Vec3::new(after.x, takeoff.y + 50.0, after.z);
                if let Some(hit) = world.ray(top, after) {
                    println!(
                        "run {run} frame {frame}: falling past ground at {:.0} (normal {:.2}), took off at {takeoff:.0}",
                        hit.point, hit.normal.y
                    );
                }
            }
            if before.distance(after) > 200.0 {
                resets += 1;
                let below = world.ray(after + Vec3::Y * 5.0, after - Vec3::Y * 20.0);
                println!(
                    "run {run} frame {frame}: reset from {before:.0} to {after:.0}, standing on ground: {}",
                    below.is_some()
                );
                continue;
            }
            if skater.on_ground || skater.grind.is_some() {
                continue;
            }
            let lift = Vec3::Y * 2.0;
            if let Some(hit) = world.ray(before + lift, after + lift) {
                // Under ground now: the ground is straight above the feet
                // (stepping off an edge only clips its corner).
                let under = world
                    .ray(Vec3::new(after.x, hit.point.y + 1.0, after.z), after)
                    .is_some_and(|h| h.normal.y > 0.5);
                if hit.normal.y > 0.5 && after.y < hit.point.y - 5.0 && under {
                    throughs += 1;
                    if throughs <= 12 {
                        println!(
                            "run {run} frame {frame}: through ground at {:.0} (flags {:#x}), {before:.0} -> {after:.0}, {:?}, vert {}",
                            hit.point,
                            hit.flags,
                            skater.action,
                            skater.vert.is_some()
                        );
                    }
                }
            }
        }
    }
    println!(
        "{runs} runs of 60 s: {throughs} frames through ground, {resets} resets, camera behind a wall {hidden} frames"
    );
}
