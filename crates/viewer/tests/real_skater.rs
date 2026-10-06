use std::path::Path;

use desa_viewer::nodes::LevelNodes;
use desa_viewer::source::GameData;
use qb::vm::Program;
use skate::{Action, Input, Physics, Rails, Segment, Skater, Stats, World};

/// Skates Jessie from the Hub's start: push for three seconds, then ollie,
/// and check she rolls along the ground, speeds up, leaves it and lands;
/// then push on, bouncing off walls, and check she stays in the level.
/// Run with `cargo test --release -p desa_viewer -- --ignored`.
#[test]
#[ignore = "needs the game data"]
fn real_skater_on_the_hub() {
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let files = data.load_level("HUB").unwrap();
    let mut program = Program::new();
    for file in data.global_scripts().unwrap() {
        program.add(&file).unwrap();
    }
    let physics = Physics::new(&program, &Stats::of(&program, "jessie"));
    println!("{physics:?}");
    assert_eq!(physics.air_gravity, -1350.0);

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
    let start = nodes.start().unwrap();
    // Face the way the spawn's camera looks.
    let facing = start.facing();
    let mut skater = Skater::new(start.position, facing.x.atan2(facing.z));

    // Settle onto the ground, then push.
    let push = Input {
        push: true,
        ..Input::default()
    };
    let mut ground_frames = 0;
    for _ in 0..180 {
        skater.update(push, &physics, &world, 1.0 / 60.0);
        ground_frames += usize::from(skater.on_ground);
    }
    let travelled = skater.position.distance(start.position);
    println!(
        "after 3 s: {:.0} units away, speed {:.0}, on the ground {ground_frames} of 180 frames, {:?}",
        travelled,
        skater.speed(),
        skater.action
    );
    assert!(skater.speed() > 100.0, "pushing should speed it up");

    // Crouch, then let go: an ollie.
    let crouch = Input {
        crouch: true,
        ..Input::default()
    };
    for _ in 0..10 {
        skater.update(crouch, &physics, &world, 1.0 / 60.0);
    }
    let before = skater.position.y;
    let mut highest = before;
    let mut air_frames = 0;
    let mut landed = false;
    for _ in 0..240 {
        skater.update(Input::default(), &physics, &world, 1.0 / 60.0);
        highest = highest.max(skater.position.y);
        if !skater.on_ground {
            air_frames += 1;
        } else if air_frames > 0 {
            landed = true;
            break;
        }
    }
    println!(
        "ollie: {:.0} units high, {air_frames} frames in the air, landed: {landed}",
        highest - before
    );
    assert!(air_frames > 10 && landed);
    assert!(matches!(
        skater.action,
        Action::Landing | Action::Rolling | Action::Standing
    ));

    // Keep pushing: sooner or later a wall turns her away.
    let mut flails = 0;
    let mut turned = 0.0f32;
    for _ in 0..1200 {
        let heading = skater.heading;
        let was = skater.action;
        skater.update(push, &physics, &world, 1.0 / 60.0);
        turned = turned.max((skater.heading - heading).abs());
        let flailing = matches!(skater.action, Action::FlailLeft | Action::FlailRight);
        if flailing && was != skater.action {
            flails += 1;
        }
    }
    println!(
        "20 s more pushing: {flails} flails off walls, sharpest turn {:.0} degrees in a frame, at {:.0} {:.0} {:.0}",
        turned.to_degrees(),
        skater.position.x,
        skater.position.y,
        skater.position.z
    );
    assert!(skater.position.y > -10_000.0, "still in the level");

    // Drop onto the longest level rail, moving along it, holding grind.
    let rail = rails
        .segments
        .iter()
        .filter(|r| (r.end - r.start).normalize().y.abs() < 0.2)
        .max_by(|a, b| a.length().total_cmp(&b.length()))
        .unwrap();
    let along = rail.direction();
    let mut skater = Skater::new(rail.start.lerp(rail.end, 0.25) + glam::Vec3::Y * 15.0, 0.0);
    skater.on_ground = false;
    skater.velocity = along * 400.0;
    let grind = Input {
        grind: true,
        ..Input::default()
    };
    let mut grinding = 0;
    for _ in 0..240 {
        skater.update(grind, &physics, &world, 1.0 / 60.0);
        if skater.grind.is_some() {
            grinding += 1;
        } else if grinding > 0 {
            break;
        }
    }
    println!(
        "grind: on a {:.0}-unit rail for {grinding} frames, off at {:.0} {:.0} {:.0}",
        rail.length(),
        skater.position.x,
        skater.position.y,
        skater.position.z
    );
    assert!(grinding > 10);

    // Every level rail, dropped onto from just above its middle and
    // grinding it whichever way: the skater must end up back on the
    // ground, never below the level (rails dip into the ground at their
    // ends, and some run along walls).
    let mut tried = 0;
    let mut grinded = 0;
    let mut fell = Vec::new();
    for (i, rail) in rails.segments.iter().enumerate() {
        for way in [1.0, -1.0] {
            let middle = rail.start.lerp(rail.end, 0.5);
            let mut skater = Skater::new(middle + glam::Vec3::Y * 10.0, 0.0);
            skater.on_ground = false;
            skater.velocity = rail.direction() * way * 300.0;
            let mut was_grinding = false;
            let mut lowest = f32::MAX;
            for frame in 0..900 {
                // Hold grind just long enough to get on.
                let input = Input {
                    grind: frame < 30,
                    ..Input::default()
                };
                skater.update(input, &physics, &world, 1.0 / 60.0);
                was_grinding |= skater.grind.is_some();
                lowest = lowest.min(skater.position.y);
                if was_grinding && skater.on_ground {
                    break;
                }
            }
            tried += 1;
            grinded += usize::from(was_grinding);
            if lowest < world.floor() - 400.0 {
                fell.push((i, way));
            }
        }
    }
    println!(
        "{grinded} of {tried} rail drops grinded, {} fell out of the level: {fell:?}",
        fell.len()
    );
    // Rail 1567 hangs off a building at the edge of the Hub, over a quarter
    // pipe with no floor in front of it: falling off it leaves the level
    // (the game's out-of-bounds triggers aren't ported). Anything else
    // falling out is a bug.
    assert!(fell.iter().all(|&(i, _)| i == 1567), "{fell:?}");
}
