use std::path::Path;

use desa_viewer::nodes::LevelNodes;
use desa_viewer::source::GameData;
use glam::Vec3;
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
    // Every character's trick table loads: air tricks, five grinds and
    // manuals, lips, and specials.
    for id in [
        "buzz", "woody", "jessie", "zurg", "tarzan", "tantor", "jane", "terk", "simba", "timon",
        "rafiki", "nala", "kid",
    ] {
        let stats = Stats::of(&program, id);
        let book = skate::TrickBook::new(&program, id, &stats);
        let grinds: std::collections::BTreeSet<_> =
            book.grinds.iter().map(|(_, t)| t.name.clone()).collect();
        let manuals: std::collections::BTreeSet<_> =
            book.manuals.iter().map(|(_, t)| t.name.clone()).collect();
        println!(
            "{id}: {} tricks, {} grinds, {} manuals, {} lips, specials {:?} / {:?} / {:?}",
            book.tricks.len(),
            grinds.len(),
            manuals.len(),
            book.lips.len(),
            book.special_air
                .map(|(_, _, i)| book.tricks[i].name.clone()),
            book.special_manual.as_ref().map(|(_, _, (n, _))| n.clone()),
            book.special_lip.as_ref().map(|(_, _, l)| l.name.clone()),
        );
        assert!(book.tricks.len() >= 5, "{id}");
        assert_eq!(grinds.len(), 5, "{id}");
        assert_eq!(manuals.len(), 5, "{id}");
        assert!(!book.lips.is_empty(), "{id}");
        assert!(
            book.special_air.is_some() && book.special_manual.is_some(),
            "{id}"
        );
    }
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
    // Bailing forwards turns her to face the way she's sliding
    // (`TurnToFaceVelocity`), so she gets up facing it, not back the way
    // she came.
    let sliding = Vec3::new(trickster.velocity.x, 0.0, trickster.velocity.z);
    if sliding.length() > 10.0 {
        let facing = trickster.forward().dot(sliding.normalize());
        assert!(facing > 0.99, "bailed facing {facing} along the slide");
    }
    assert!(matches!(
        trickster.bail_anims,
        ("Bail1", "BailGetUp1") | ("Bail2", "BailGetUp2")
    ));

    // A flip while spinning: the spin counts in 180s and multiplies it.
    let mut spinner = Skater::new(start.position, facing.x.atan2(facing.z));
    spinner.tricks = trickster.tricks.clone();
    for _ in 0..90 {
        spinner.update(push, &physics, &world, 1.0 / 60.0);
    }
    let spin_flip = Input {
        flip: true,
        brake: true,
        turn: 1.0,
        ..Input::default()
    };
    // A clean 180: turn for long enough (about 0.43 s at the air rotation
    // stat), then straighten out and land fakie. Held all the way round
    // it would land sideways: the game's yaw bail.
    let crouch = Input {
        crouch: true,
        ..Input::default()
    };
    for _ in 0..12 {
        spinner.update(crouch, &physics, &world, 1.0 / 60.0);
    }
    spinner.update(Input::default(), &physics, &world, 1.0 / 60.0);
    let spin_frames = (std::f32::consts::PI / physics.air_rotation * 60.0) as usize;
    for frame in 0..240 {
        let input = if frame < spin_frames {
            spin_flip
        } else {
            Input::default()
        };
        spinner.update(input, &physics, &world, 1.0 / 60.0);
        if frame > 2 && spinner.on_ground {
            break;
        }
    }
    let landed = spinner.last_combo.clone().expect("a combo");
    println!(
        "spinning flip: {} x {} spins, total {}, bailed {}, landed backwards {}",
        landed.combo.tricks[0].name,
        landed.combo.tricks[0].spins,
        landed.total,
        landed.bailed,
        spinner.landing.backwards
    );
    // Down-right is the "L" flip slot: Quickest Boots in the West, 250
    // points, x1.5 for one 180.
    assert!(!landed.bailed);
    assert!(spinner.landing.backwards, "a 180 lands fakie");
    // `FlipAndRotate`: turned round to face the way she's going, riding
    // switch.
    let going = Vec3::new(spinner.velocity.x, 0.0, spinner.velocity.z).normalize();
    assert!(
        spinner.forward().dot(going) > 0.99,
        "facing the way she rolls"
    );
    assert!(spinner.flipped, "riding switch");
    assert_eq!(landed.combo.tricks[0].name, "Quickest Boots in the West");
    assert_eq!(landed.combo.tricks[0].spins, 1);
    assert_eq!(landed.total, 375);

    // Spinning all the way down lands sideways: at speed, the yaw bail.
    let mut sideways = Skater::new(start.position, facing.x.atan2(facing.z));
    sideways.tricks = trickster.tricks.clone();
    for _ in 0..90 {
        sideways.update(crouch, &physics, &world, 1.0 / 60.0);
    }
    sideways.update(Input::default(), &physics, &world, 1.0 / 60.0);
    let spin = Input {
        turn: 1.0,
        ..Input::default()
    };
    for frame in 0..240 {
        sideways.update(spin, &physics, &world, 1.0 / 60.0);
        if frame > 2 && sideways.on_ground {
            break;
        }
    }
    println!(
        "spun all the way: {:?}, speed {:.0}",
        sideways.action,
        sideways.speed()
    );
    assert_eq!(sideways.action, Action::Bail);

    // The special grab: down, right, then flip, with the special meter
    // full; without it, the same presses do an ordinary trick.
    let special_attempt = |full: bool| {
        let mut skater = Skater::new(start.position, facing.x.atan2(facing.z));
        skater.tricks = trickster.tricks.clone();
        for _ in 0..90 {
            skater.update(push, &physics, &world, 1.0 / 60.0);
        }
        if full {
            skater.special_meter = 3000.0;
            skater.special = true;
        }
        let crouch = Input {
            crouch: true,
            ..Input::default()
        };
        for _ in 0..12 {
            skater.update(crouch, &physics, &world, 1.0 / 60.0);
        }
        let down = Input {
            brake: true,
            ..Input::default()
        };
        let right = Input {
            turn: 1.0,
            ..Input::default()
        };
        let flip = Input {
            flip: true,
            ..Input::default()
        };
        for input in [
            Input::default(),
            down,
            Input::default(),
            right,
            Input::default(),
            flip,
        ] {
            skater.update(input, &physics, &world, 1.0 / 60.0);
        }
        let trick = skater
            .trick
            .map(|t| skater.tricks.tricks[t.trick].name.clone());
        (trick, skater.special)
    };
    let (with_meter, still_special) = special_attempt(true);
    let (without, _) = special_attempt(false);
    println!(
        "special: {with_meter:?} (meter still full: {still_special}), without the meter: {without:?}"
    );
    assert_eq!(with_meter.as_deref(), Some("Sit a Spell"));
    assert_ne!(without.as_deref(), Some("Sit a Spell"));

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

    // From a manual, Circle with a direction branches to another manual
    // (`GroundManualTrickBranches`): with up, manual 2.
    let mut brancher = Skater::new(start.position, facing.x.atan2(facing.z));
    brancher.tricks = book.clone();
    for _ in 0..90 {
        brancher.update(push, &physics, &world, 1.0 / 60.0);
    }
    let tap = |push: bool, brake: bool, grab: bool| Input {
        push,
        brake,
        grab,
        ..Input::default()
    };
    for input in [
        tap(false, false, false),
        tap(true, false, false),
        tap(false, false, false),
        tap(false, true, false),
        tap(false, false, false),
        tap(true, false, false),
        tap(true, false, true),
        tap(false, false, false),
    ] {
        brancher.update(input, &physics, &world, 1.0 / 60.0);
    }
    let names: Vec<_> = brancher
        .combo_tricks
        .tricks
        .iter()
        .map(|t| t.name.clone())
        .collect();
    println!("manual branch: {names:?}");
    assert!(brancher.manual);
    assert_eq!(names, ["Twistin' in the Wind", "Hunker Down, Li'l Lady"]);

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

    // Stopped facing into a corner (by the Hub's fountain steps), with a
    // slope rolling her into it: she stays put, not turned off one wall
    // then the other every frame (which shook the camera).
    let mut cornered = Skater::new(Vec3::ZERO, 0.0);
    cornered.auto_kick = false;
    cornered.place(Vec3::new(5835.5, 30.0, 5917.2), -0.01, &physics, &world);
    let mut spun = 0.0f32;
    for _ in 0..120 {
        let heading = cornered.heading;
        cornered.update(Input::default(), &physics, &world, 1.0 / 60.0);
        spun = spun.max((cornered.heading - heading).abs());
    }
    println!("in the corner: turned at most {spun:.3} in a frame");
    assert!(spun < 0.1, "turning on the spot in a corner");

    // AutoKick pushes play their whole kick (`DoAPush` waits for the
    // animation), not one frame on, one off at the kick speed.
    let mut kicker = Skater::new(start.position, facing.x.atan2(facing.z));
    kicker.anim_lengths.insert("PushCycle1".into(), 1.2);
    let (mut run, mut shortest) = (0, usize::MAX);
    for _ in 0..1200 {
        kicker.update(Input::default(), &physics, &world, 1.0 / 60.0);
        if kicker.pushing {
            run += 1;
        } else if run > 0 {
            shortest = shortest.min(run);
            run = 0;
        }
    }
    println!("AutoKick: shortest push {shortest} frames");
    assert!(shortest != usize::MAX, "pushed and coasted");
    assert!(shortest >= 70, "a push cut short: {shortest} frames");

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

    // Grinding with a direction pressed: that direction's grind
    // (`GrindTricks`: right is grind 3).
    let rail = rails
        .segments
        .iter()
        .filter(|r| (r.end - r.start).normalize().y.abs() < 0.2)
        .max_by(|a, b| a.length().total_cmp(&b.length()))
        .unwrap();
    let mut grinder = Skater::new(rail.start.lerp(rail.end, 0.25) + glam::Vec3::Y * 15.0, 0.0);
    grinder.tricks = book.clone();
    grinder.on_ground = false;
    grinder.velocity = rail.direction() * 400.0;
    let right = Input {
        turn: 1.0,
        ..Input::default()
    };
    grinder.update(right, &physics, &world, 1.0 / 60.0);
    let grind = Input {
        grind: true,
        ..Input::default()
    };
    for _ in 0..20 {
        grinder.update(grind, &physics, &world, 1.0 / 60.0);
        if grinder.grind.is_some() {
            break;
        }
    }
    let grinding = grinder.balance_trick.as_ref().map(|t| t.name.clone());
    println!("grind with right: {grinding:?}");
    assert_eq!(grinding.as_deref(), Some("Yeeeeee-HAW!"));

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

/// A spine transfer on Camp: Jessie rides up one side of a spine holding
/// the spine button (crouched, for the speed), goes over and comes down
/// the far side riding forwards, with "Spine Transfer" in the combo.
#[test]
#[ignore = "needs the game data"]
fn spine_transfer_on_camp() {
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let files = data.load_level("camp").unwrap();
    let mut program = Program::new();
    for file in data.global_scripts().unwrap() {
        program.add(&file).unwrap();
    }
    let physics = Physics::new(&program, &Stats::of(&program, "jessie"));
    let collision = ngc_collision::Collision::parse(files.collision.as_ref().unwrap()).unwrap();
    let world = World::new(collision);
    let mut skater = Skater::new(Vec3::ZERO, 0.0);
    skater.place(
        Vec3::new(-8816.0, -1.0, 2440.0),
        90f32.to_radians(),
        &physics,
        &world,
    );
    let spine = Input {
        push: true,
        crouch: true,
        revert: true,
        ..Input::default()
    };
    let mut transferred = false;
    let mut landed = None;
    for frame in 0..180 {
        skater.update(spine, &physics, &world, 1.0 / 60.0);
        transferred |= skater
            .combo_tricks
            .tricks
            .iter()
            .any(|t| t.name == "Spine Transfer");
        if frame > 30 && skater.on_ground && transferred {
            landed = Some(skater.position);
            break;
        }
    }
    println!("spine transfer: {transferred}, landed at {landed:?}");
    assert!(transferred, "a spine transfer");
    let landed = landed.expect("down the other side");
    // The spine's apex is at x -8560: over it, riding on along +X.
    assert!(landed.x > -8560.0, "{landed}");
    assert!(skater.velocity.x > 0.0 && !skater.landing.backwards);
}
