//! Skates a scripted run without drawing anything and prints every frame:
//! what the skater is doing, where, how fast, its rail and its animation.
//! The same key scripts as the map viewer's `--skate-keys`.
//!
//! `cargo run --release -p desa_viewer --example skate_run -- [game data] LEVEL x,y,z,heading KEYS SECONDS [character]`
//!
//! KEYS is "seconds:+Key" and "seconds:-Key" separated by commas, with keys
//! W A S D Space E Q F R. `TRACE=1` adds the skater's own trace (why it
//! left the ground or a rail).
use std::collections::HashSet;
use std::path::Path;

use desa_viewer::nodes::LevelNodes;
use desa_viewer::source::GameData;
use glam::Vec3;
use qb::vm::Program;
use skate::{Input, Physics, Rails, Segment, Skater, Stats, TrickBook, World};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let level = &args[2];
    let from: Vec<f32> = args[3].split(',').map(|x| x.parse().unwrap()).collect();
    let keys = parse_keys(&args[4]);
    let seconds: f32 = args[5].parse().unwrap();
    let who = args.get(6).map_or("jessie", String::as_str);

    let mut data = GameData::open(Path::new(path)).unwrap();
    let files = data.load_level(level).unwrap();
    let mut program = Program::new();
    for file in data.global_scripts().unwrap() {
        program.add(&file).unwrap();
    }
    for file in &files.scripts {
        program.add(file).unwrap();
    }
    let stats = Stats::of(&program, who);
    let physics = Physics::new(&program, &stats);
    let mut tricks = TrickBook::new(&program, who, &stats);
    // Animation lengths from the character's own files.
    let character = data.load_character(who).unwrap();
    let tables =
        ngc_anim::KeyTables::parse(&character.key_tables.0, &character.key_tables.1).unwrap();
    let lengths: Vec<(String, f32)> = character
        .animations
        .iter()
        .filter_map(|(name, bytes)| {
            let a = ngc_anim::Animation::parse(bytes, &tables).ok()?;
            Some((name.clone(), a.duration))
        })
        .collect();
    tricks.set_durations(|anim| {
        lengths
            .iter()
            .find(|(name, _)| qb::checksum(name) == anim)
            .map(|&(_, d)| d)
    });
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
    let world = World::new(collision).with_rails(rails);

    let mut skater = Skater::new(Vec3::ZERO, 0.0);
    skater.tricks = tricks;
    skater.teleports = desa_viewer::triggers::teleports(&nodes, &program)
        .into_iter()
        .map(|(object, i)| {
            let spawn = &nodes.spawns[i];
            let facing = spawn.facing();
            (object, (spawn.position, facing.x.atan2(facing.z)))
        })
        .collect();
    println!("{} teleporter objects", skater.teleports.len());
    skater.anim_lengths = lengths.into_iter().collect();
    skater.place(
        Vec3::new(from[0], from[1], from[2]),
        from[3].to_radians(),
        &physics,
        &world,
    );
    skater.trace = std::env::var("TRACE").is_ok();
    let mut held = HashSet::new();
    let steps = (seconds * 60.0).round() as usize;
    for step in 0..steps {
        let now = step as f32 / 60.0;
        for &(at, key, down) in &keys {
            if (at - now).abs() < 0.5 / 60.0 {
                if down {
                    held.insert(key);
                } else {
                    held.remove(&key);
                }
            }
        }
        let h = |k: &str| held.contains(k);
        let input = Input {
            push: h("W"),
            brake: h("S"),
            turn: f32::from(u8::from(h("D"))) - f32::from(u8::from(h("A"))),
            crouch: h("Space"),
            grind: h("E"),
            flip: h("Q"),
            grab: h("F"),
            revert: h("R"),
        };
        skater.update(input, &physics, &world, 1.0 / 60.0);
        if std::env::var("TRICK").is_ok() {
            println!(
                "      trick {:?} bail_on {}",
                skater.trick,
                skater.bail_on()
            );
        }
        let p = skater.position;
        println!(
            "{now:5.2} {:?} at {:.0} {:.0} {:.0} v {:.0} heading {:.0} speed {:.0}{} | {} {:.2}{}",
            skater.action,
            p.x,
            p.y,
            p.z,
            skater.velocity,
            skater.heading.to_degrees(),
            skater.speed(),
            skater
                .grind
                .map_or(String::new(), |g| format!(" rail {}", g.segment)),
            skater.anim.first,
            skater.anim_time,
            skater
                .combo_tricks
                .tricks
                .last()
                .map_or(String::new(), |t| format!(
                    " | {} ({})",
                    t.name,
                    skater.combo_tricks.points()
                )),
        );
    }
    println!(
        "score {}, last combo {:?}",
        skater.score,
        skater.last_combo.as_ref().map(|c| (c.total, c.bailed))
    );
}

fn parse_keys(spec: &str) -> Vec<(f32, &'static str, bool)> {
    spec.split(',')
        .filter(|s| !s.is_empty())
        .map(|item| {
            let (at, key) = item.split_once(':').unwrap();
            let down = key.starts_with('+');
            let name = match &key[1..] {
                "W" => "W",
                "A" => "A",
                "S" => "S",
                "D" => "D",
                "Space" => "Space",
                "E" => "E",
                "Q" => "Q",
                "F" => "F",
                "R" => "R",
                other => panic!("unknown key {other}"),
            };
            (at.parse().unwrap(), name, down)
        })
        .collect()
}
