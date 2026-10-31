use std::path::Path;

use desa_viewer::nodes::LevelNodes;
use desa_viewer::source::GameData;
use glam::Vec3;
use qb::vm::Program;
use skate::{Input, Physics, Rails, Segment, Skater, Stats, World};

/// Every level's every rail, dropped onto from just above its middle,
/// both ways, balanced perfectly: where the grind ends, it should be the
/// end of the rail (or a corner too sharp to follow), not partway along.
/// `LEVEL=HUB RAIL=30` traces one.
#[test]
#[ignore = "needs the game data"]
fn real_grinds_run_to_the_end() {
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let mut program = Program::new();
    for file in data.global_scripts().unwrap() {
        program.add(&file).unwrap();
    }
    let physics = Physics::new(&program, &Stats::of(&program, "jessie"));
    let only = std::env::var("LEVEL").ok();
    let mut early_total = 0;
    for level in data.levels() {
        if only
            .as_ref()
            .is_some_and(|o| !o.eq_ignore_ascii_case(&level.id))
        {
            continue;
        }
        let Ok(files) = data.load_level(&level.id) else {
            continue;
        };
        let (Some(c), Some(n)) = (files.collision.as_ref(), files.nodes.as_ref()) else {
            continue;
        };
        let collision = ngc_collision::Collision::parse(c).unwrap();
        let nodes = LevelNodes::from_bytes(n).unwrap();
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
        let (mut tried, mut on, mut ends, mut corners) = (0, 0, 0, 0);
        let mut early = Vec::new();
        for (i, rail) in rails.segments.iter().enumerate() {
            if rail.length() < 40.0 {
                continue;
            }
            let traced = std::env::var("RAIL")
                .ok()
                .and_then(|r| r.parse::<usize>().ok());
            if traced.is_some_and(|r| r != i) {
                continue;
            }
            for way in [1.0f32, -1.0] {
                tried += 1;
                let middle = rail.start.lerp(rail.end, 0.5);
                let mut skater = Skater::new(middle + Vec3::Y * 10.0, 0.0);
                skater.on_ground = false;
                skater.perfect_rail = true;
                skater.trace = traced.is_some();
                skater.velocity = rail.direction() * way * 400.0;
                let mut last = None;
                let mut frames = 0;
                for frame in 0..1200 {
                    let input = Input {
                        grind: frame < 20,
                        ..Input::default()
                    };
                    skater.update(input, &physics, &world, 1.0 / 60.0);
                    match skater.grind {
                        Some(g) => {
                            last = Some((g, skater.position));
                            frames += 1;
                        }
                        None if last.is_some() => break,
                        None => {}
                    }
                }
                let Some((g, at)) = last else { continue };
                on += 1;
                if skater.grind.is_some() {
                    continue;
                }
                let s = rails.segments[g.segment];
                let end = if g.forwards { s.end } else { s.start };
                let step = g.speed / 60.0 + 2.0;
                if at.distance(end) <= step {
                    match rails.next(g.segment, g.forwards) {
                        None => ends += 1,
                        Some(_) => corners += 1,
                    }
                } else {
                    early.push((i, way, g.segment, at, at.distance(end), frames));
                }
            }
        }
        println!(
            "{}: {} segments, {tried} drops, {on} grinded, {ends} off the end, {corners} at corners, {} early",
            level.id,
            rails.segments.len(),
            early.len()
        );
        for (i, way, seg, at, left, frames) in early.iter().filter(|e| e.4 > 25.0).take(8) {
            println!(
                "  rail {i} ({way}): off segment {seg} at {:.0} {:.0} {:.0}, {left:.0} short of its end, after {frames} frames",
                at.x, at.y, at.z
            );
        }
        early_total += early.len();
    }
    println!("early: {early_total}");
    // (What's left are walls across the rails' lines: a gate, a toilet, a
    // building's corner, a few short of a post at a rail's end.)
    if only.is_none() {
        assert!(early_total < 700, "{early_total} grinds ended early");
    }
}
