use std::path::Path;

use desa_viewer::nodes::LevelNodes;
use desa_viewer::source::GameData;
use qb::vm::Program;
use skate::{Action, Input, Physics, Skater, Stats, World};

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
    let world = World::new(collision);
    let nodes = LevelNodes::from_bytes(files.nodes.as_ref().unwrap()).unwrap();
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
}
