//! Skates straight up the Hub's quarter pipes (vert faces) and reports
//! where each run ends: back in front of the ramp, as on vert in the game,
//! or up on the deck behind it.
//!
//! `cargo run --release -p desa_viewer --example vert_ramps [game data] [level]`
use std::path::Path;

use desa_viewer::nodes::LevelNodes;
use desa_viewer::source::GameData;
use glam::Vec3;
use ngc_collision::face_flags;
use qb::vm::Program;
use skate::{Action, Input, Physics, Rails, Segment, Skater, Stats, World};

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "extracted".into());
    let level = std::env::args().nth(2).unwrap_or_else(|| "HUB".into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let files = data.load_level(&level).unwrap();
    let mut program = Program::new();
    for file in data.global_scripts().unwrap() {
        program.add(&file).unwrap();
    }
    let stats = Stats::of(&program, "jessie");
    let physics = Physics::new(&program, &stats);
    let tricks = skate::TrickBook::new(&program, "jessie", &stats);
    let collision = ngc_collision::Collision::parse(files.collision.as_ref().unwrap()).unwrap();
    // Steep vert faces, near the top of quarter pipes, spread out.
    let mut ramps: Vec<(Vec3, Vec3)> = Vec::new();
    for object in &collision.objects {
        for face in &object.faces {
            if face.flags & face_flags::VERT == 0 {
                continue;
            }
            let [a, b, c] = face
                .indices
                .map(|i| Vec3::from(object.vertices[usize::from(i)]));
            let normal = (b - a).cross(c - a).normalize_or_zero();
            if !(0.05..0.35).contains(&normal.y) {
                continue;
            }
            let centre = (a + b + c) / 3.0;
            if ramps.iter().all(|(p, _)| p.distance(centre) > 400.0) {
                ramps.push((centre, normal));
            }
        }
    }
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
    let world = World::new(collision).with_rails(rails);
    let (mut front, mut deck, mut other) = (0, 0, 0);
    for (centre, normal) in ramps.iter().take(30) {
        let out = Vec3::new(normal.x, 0.0, normal.z).normalize();
        // The ground in front of the ramp.
        let probe = *centre + out * 250.0;
        let Some(ground) = world.ray(probe + Vec3::Y * 50.0, probe - Vec3::Y * 400.0) else {
            continue;
        };
        if ground.normal.y < 0.9 {
            continue;
        }
        let into = -out;
        let mut skater = Skater::new(ground.point, into.x.atan2(into.z));
        if std::env::var("STARTS").is_ok() {
            println!(
                "start {:.0},{:.0},{:.0},{:.0}",
                ground.point.x,
                ground.point.y,
                ground.point.z,
                into.x.atan2(into.z).to_degrees()
            );
        }
        skater.velocity = into * 650.0;
        skater.tricks = tricks.clone();
        let push = Input {
            push: true,
            ..Input::default()
        };
        // With LIPS set, press grind at the lip: a lip trick on the coping.
        let lips = std::env::var("LIPS").is_ok();
        let mut lipped = None;
        let mut top = ground.point.y;
        let mut airborne = false;
        let mut landed_at = None;
        for _ in 0..300 {
            // With REVERT set, revert pressed just before landing.
            let reverting = std::env::var("REVERT").is_ok();
            if reverting && airborne {
                // Pressed as it's about to land (within the game's 200 ms).
                let press = skater.landing_soon();
                skater.update(
                    Input {
                        revert: press,
                        ..Input::default()
                    },
                    &physics,
                    &world,
                    1.0 / 60.0,
                );
                if skater.on_ground {
                    let names: Vec<_> = skater
                        .combo_tricks
                        .tricks
                        .iter()
                        .map(|t| t.name.clone())
                        .collect();
                    for _ in 0..60 {
                        skater.update(Input::default(), &physics, &world, 1.0 / 60.0);
                    }
                    println!(
                        "  landed: {:?}, combo {names:?}, a second later banked {} ({:?})",
                        skater.action,
                        skater.score,
                        skater.last_combo.as_ref().map(|c| c.total)
                    );
                    break;
                }
                continue;
            }
            let input = if lips && !skater.on_ground {
                Input {
                    grind: true,
                    ..Input::default()
                }
            } else {
                push
            };
            skater.update(input, &physics, &world, 1.0 / 60.0);
            if let Some(lip) = &skater.lip {
                lipped.get_or_insert(lip.trick.name.clone());
            }
            if lips && !airborne && !skater.on_ground {
                // At takeoff: the nearest rail to the lip.
                let at = skater.position;
                let nearest = world
                    .rails
                    .segments
                    .iter()
                    .map(|r| {
                        let d = r.end - r.start;
                        let t = ((at - r.start).dot(d) / d.length_squared()).clamp(0.0, 1.0);
                        (r.start + d * t).distance(at)
                    })
                    .fold(f32::MAX, f32::min);
                println!(
                    "  took off at {at:.0}, vert {}, nearest rail {nearest:.0} away",
                    skater.vert.is_some()
                );
            }
            top = top.max(skater.position.y);
            if !skater.on_ground {
                airborne = true;
            } else if airborne && landed_at.is_none() {
                landed_at = Some(skater.position);
            }
            if skater.action == Action::Landing && landed_at.is_some() {
                break;
            }
        }
        // In front of the ramp's face, or behind it (the deck)?
        let side = landed_at.map(|p| (p - *centre).dot(out));
        let verdict = match side {
            Some(s) if s > -20.0 => {
                front += 1;
                "back on the ramp"
            }
            Some(_) => {
                deck += 1;
                "on the deck"
            }
            None => {
                other += 1;
                "never left it"
            }
        };
        // Then two seconds rolling, to see it come back down the ramp.
        for _ in 0..120 {
            skater.update(Input::default(), &physics, &world, 1.0 / 60.0);
        }
        let after = skater.position;
        if lips {
            // Then let it ride: off the lip and back down.
            let mut held = 0;
            let mut outcome = None;
            for _ in 0..600 {
                skater.update(Input::default(), &physics, &world, 1.0 / 60.0);
                held += usize::from(skater.lip.is_some());
                if skater.on_ground && skater.lip.is_none() && held > 0 {
                    outcome = Some(skater.action);
                    break;
                }
            }
            println!(
                "ramp at {:.0} {:.0} {:.0}: lip {lipped:?}, held {held} frames, then {outcome:?}, score {} / last combo {:?}",
                centre.x,
                centre.y,
                centre.z,
                skater.score,
                skater.last_combo.as_ref().map(|c| (c.total, c.bailed))
            );
            continue;
        }
        println!(
            "ramp at {:.0} {:.0} {:.0}: rose {:.0} above the ground, {verdict}; 2 s later {:.0} above the ground, {:.0} out, {}",
            centre.x,
            centre.y,
            centre.z,
            top - ground.point.y,
            after.y - ground.point.y,
            (after - *centre).dot(out),
            if skater.on_ground {
                "rolling"
            } else {
                "in the air"
            }
        );
    }
    println!("{front} back on the ramp, {deck} on the deck, {other} never airborne");
}
