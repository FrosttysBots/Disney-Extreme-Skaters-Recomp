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

    // Jessie's tricks, from her trick table.
    let book = skate::TrickBook::new(&program, "jessie", &Stats::of(&program, "jessie"));
    for trick in &book.tricks {
        println!(
            "trick: {} ({:?}, {} points, speed {:.2})",
            trick.name, trick.kind, trick.score, trick.speed
        );
    }
    println!("grind {:?}, manual {:?}", book.grind, book.manual);
    let flip_down = book
        .air_trick(skate::Button::Flip, Some(skate::Dir::Down))
        .unwrap();
    assert_eq!(book.tricks[flip_down].name, "Roll'Em Roll'Em Roll'Em");
    let grab_left = book
        .air_trick(skate::Button::Grab, Some(skate::Dir::Left))
        .unwrap();
    assert_eq!(book.tricks[grab_left].name, "Well Howdy There");

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

    // Tricks: an ollie with a quick flip lands clean and banks its points;
    // holding a grab all the way down is a bail. (Animations aren't loaded
    // here: each trick animation is taken as 0.4 seconds.)
    let mut tricks = book.clone();
    tricks.set_durations(|_| Some(0.4));
    let ollie_with = |skater: &mut Skater, trick: Input, hold: bool| {
        let crouch = Input {
            crouch: true,
            ..Input::default()
        };
        for _ in 0..12 {
            skater.update(crouch, &physics, &world, 1.0 / 60.0);
        }
        skater.update(Input::default(), &physics, &world, 1.0 / 60.0);
        skater.update(trick, &physics, &world, 1.0 / 60.0);
        for _ in 0..240 {
            let input = if hold { trick } else { Input::default() };
            skater.update(input, &physics, &world, 1.0 / 60.0);
            if skater.on_ground {
                break;
            }
        }
    };
    let flip = Input {
        flip: true,
        brake: true,
        ..Input::default()
    };
    let grab = Input {
        grab: true,
        ..Input::default()
    };
    let mut trickster = Skater::new(start.position, facing.x.atan2(facing.z));
    trickster.tricks = tricks;
    for _ in 0..90 {
        trickster.update(push, &physics, &world, 1.0 / 60.0);
    }
    ollie_with(&mut trickster, flip, false);
    println!(
        "flip: {:?}, score {}",
        trickster.last_combo, trickster.score
    );
    assert_eq!(trickster.score, 500, "Roll'Em Roll'Em Roll'Em, landed");
    // Again from the start, holding a grab.
    let mut trickster = {
        let mut fresh = Skater::new(start.position, facing.x.atan2(facing.z));
        fresh.tricks = trickster.tricks.clone();
        fresh.score = trickster.score;
        fresh
    };
    for _ in 0..90 {
        trickster.update(push, &physics, &world, 1.0 / 60.0);
    }
    ollie_with(&mut trickster, grab, true);
    println!(
        "held grab: {:?}, {:?}",
        trickster.last_combo, trickster.action
    );
    assert!(trickster.last_combo.as_ref().is_some_and(|c| c.bailed));
    assert_eq!(trickster.action, Action::Bail);
    assert_eq!(trickster.score, 500, "the bail scores nothing");

    // A manual: tap up, then down, and let it ride. Left alone it falls
    // off the meter within seconds; balanced, it lasts.
    let manual_frames = |skater: &mut Skater, careful: bool| {
        let tap = |push: bool, brake: bool| Input {
            push,
            brake,
            ..Input::default()
        };
        let release = tap(false, false);
        for input in [
            release,
            tap(true, false),
            release,
            tap(false, true),
            release,
        ] {
            skater.update(input, &physics, &world, 1.0 / 60.0);
        }
        assert!(skater.manual, "up then down starts a manual");
        let mut frames = 0;
        while skater.manual && frames < 600 {
            let ahead = skater.balance.angle + skater.balance.speed * 20.0;
            let input = if careful {
                tap(ahead > 100.0, ahead < -100.0)
            } else {
                Input::default()
            };
            skater.update(input, &physics, &world, 1.0 / 60.0);
            frames += 1;
        }
        frames
    };
    let mut lazy = Skater::new(start.position, facing.x.atan2(facing.z));
    let mut careful = Skater::new(start.position, facing.x.atan2(facing.z));
    for _ in 0..90 {
        lazy.update(push, &physics, &world, 1.0 / 60.0);
        careful.update(push, &physics, &world, 1.0 / 60.0);
    }
    assert!(lazy.on_ground && careful.on_ground);
    let lazy_frames = manual_frames(&mut lazy, false);
    let careful_frames = manual_frames(&mut careful, true);
    println!(
        "manual: {lazy_frames} frames left alone (then {:?}), {careful_frames} balanced",
        lazy.action
    );
    assert!(lazy_frames > 30 && lazy_frames < 600);
    assert!(matches!(
        lazy.action,
        Action::BailManual | Action::Rolling | Action::Air | Action::Standing
    ));
    assert!(careful_frames > lazy_frames);

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
    let mut fell_through = Vec::new();
    for (i, rail) in rails.segments.iter().enumerate() {
        for way in [1.0, -1.0] {
            let middle = rail.start.lerp(rail.end, 0.5);
            let mut skater = Skater::new(middle + glam::Vec3::Y * 10.0, 0.0);
            skater.on_ground = false;
            skater.velocity = rail.direction() * way * 300.0;
            let mut was_grinding = false;
            let mut lowest = f32::MAX;
            let mut through_ground = false;
            for frame in 0..900 {
                // Hold grind just long enough to get on.
                let input = Input {
                    grind: frame < 30,
                    ..Input::default()
                };
                let before = skater.position;
                skater.update(input, &physics, &world, 1.0 / 60.0);
                was_grinding |= skater.grind.is_some();
                lowest = lowest.min(skater.position.y);
                // Falling through ground (crossing a face that faces up
                // without landing on it) is a bug; through a real hole in
                // the level, it isn't. Respawns (big jumps) aside.
                let after = skater.position;
                if !skater.on_ground && skater.grind.is_none() && before.distance(after) < 200.0 {
                    let lift = glam::Vec3::Y * 2.0;
                    through_ground |= world
                        .ray(before + lift, after + lift)
                        .is_some_and(|hit| hit.normal.y > 0.5 && after.y < hit.point.y - 5.0);
                }
                if was_grinding && skater.on_ground {
                    break;
                }
            }
            tried += 1;
            grinded += usize::from(was_grinding);
            if lowest < world.floor() - 400.0 {
                fell.push((i, way));
                if through_ground {
                    fell_through.push((i, way));
                }
            }
        }
    }
    println!(
        "{grinded} of {tried} rail drops grinded, {} fell out of the level: {fell:?}",
        fell.len()
    );
    // Falling off a rail into a real hole in the level (like the ring of
    // rails round the open hole at 4326, 10168, or rail 1567 off the edge of
    // a building) leaves the level, as the game's out-of-bounds triggers
    // would catch; the skater's safety net puts it back. Falling out
    // through ground is a bug.
    assert!(
        fell_through.is_empty(),
        "through the ground: {fell_through:?}"
    );
}
