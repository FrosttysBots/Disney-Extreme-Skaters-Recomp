//! Drops a skater onto every rail of every level, both ways, holding
//! grind, and reports any that fall below the level (before the skater's
//! safety net puts them back).
//!
//! `cargo run --release -p desa_viewer --example rail_drops [game data]`
use std::path::Path;

use desa_viewer::nodes::LevelNodes;
use desa_viewer::source::GameData;
use qb::vm::Program;
use skate::{Input, Physics, Rails, Segment, Skater, Stats, World};

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "extracted".into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let mut program = Program::new();
    for file in data.global_scripts().unwrap() {
        program.add(&file).unwrap();
    }
    let physics = Physics::new(&program, &Stats::of(&program, "jessie"));
    for level in data.levels() {
        let files = data.load_level(&level.id).unwrap();
        let (Some(collision), Some(nodes)) = (files.collision.as_ref(), files.nodes.as_ref())
        else {
            continue;
        };
        let collision = ngc_collision::Collision::parse(collision).unwrap();
        let nodes = LevelNodes::from_bytes(nodes).unwrap();
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
        let floor = world.floor();
        let (mut tried, mut grinded, mut fell) = (0, 0, Vec::new());
        for (i, rail) in rails.segments.iter().enumerate() {
            for way in [1.0f32, -1.0] {
                let middle = rail.start.lerp(rail.end, 0.5);
                let mut skater = Skater::new(middle + glam::Vec3::Y * 10.0, 0.0);
                skater.on_ground = false;
                skater.velocity = rail.direction() * way * 300.0;
                let mut was = false;
                let mut lowest = f32::MAX;
                for frame in 0..900 {
                    let input = Input {
                        grind: frame < 30,
                        ..Input::default()
                    };
                    skater.update(input, &physics, &world, 1.0 / 60.0);
                    was |= skater.grind.is_some();
                    lowest = lowest.min(skater.position.y);
                    if was && skater.on_ground {
                        break;
                    }
                }
                tried += 1;
                grinded += usize::from(was);
                if lowest < floor - 400.0 {
                    fell.push((i, way));
                }
            }
        }
        println!(
            "{}: {grinded}/{tried} grinded, {} fell: {:?}",
            level.id,
            fell.len(),
            &fell[..fell.len().min(8)]
        );
    }
}
