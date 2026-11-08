//! Skates straight at a level's quarter pipes (faces flagged vert, partway
//! up their curve) from the flat in front, pushing, and reports how far
//! up each run got: launched into vert air off the lip, or turned back
//! somewhere on the way up (and how steep the ground was there).
//!
//! `cargo run --release -p desa_viewer --example vert_check [game data] [level]`
//!
//! `TRACE=n` prints each frame of run `n`.
use std::collections::HashSet;
use std::path::Path;

use desa_viewer::source::GameData;
use glam::Vec3;
use ngc_collision::face_flags as f;
use qb::vm::Program;
use skate::{Input, Physics, Skater, Stats, World};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).cloned().unwrap_or_else(|| "extracted".into());
    let level = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "ToyStory_Bedroom".into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let files = data.load_level(&level).unwrap();
    let mut program = Program::new();
    for file in data.global_scripts().unwrap() {
        program.add(&file).unwrap();
    }
    for file in &files.scripts {
        program.add(file).unwrap();
    }
    let who = std::env::var("CHAR").unwrap_or_else(|_| "woody".into());
    let stats = Stats::of(&program, &who);
    let physics = Physics::new(&program, &stats);
    let collision = ngc_collision::Collision::parse(files.collision.as_ref().unwrap()).unwrap();
    // The ramps' faces, partway up the curve.
    let mut ramps = Vec::new();
    let mut seen = HashSet::new();
    for o in &collision.objects {
        for face in &o.faces {
            if face.flags & f::VERT == 0 || face.flags & f::NON_COLLIDABLE != 0 {
                continue;
            }
            let [a, b, c] = face.indices.map(|i| Vec3::from(o.vertices[i as usize]));
            let n = (b - a).cross(c - a).normalize_or_zero();
            if !(0.3..0.7).contains(&n.y) {
                continue;
            }
            let centre = (a + b + c) / 3.0;
            let cell = (centre / 150.0).floor().as_ivec3();
            if seen.insert(cell) {
                ramps.push((centre, n));
            }
        }
    }
    let world = World::new(collision);
    let trace: Option<usize> = std::env::var("TRACE").ok().and_then(|t| t.parse().ok());
    let dt = 1.0 / 60.0;
    let (mut tried, mut launched) = (0, 0);
    let mut steepest = Vec::new();
    for (centre, n) in ramps {
        let out = Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
        // Flat ground in front, below the face.
        let mut start = None;
        for d in [300.0, 450.0, 600.0] {
            let spot = centre + out * d;
            let Some(g) = world.ray(spot + Vec3::Y * 50.0, spot - Vec3::Y * 600.0) else {
                continue;
            };
            if g.normal.y > 0.97 && g.point.y < centre.y - 10.0 {
                // Nothing in the way between it and the ramp.
                let knee = g.point + Vec3::Y * 20.0;
                let to = centre - out * 5.0;
                let clear = world
                    .ray(knee, Vec3::new(to.x, knee.y, to.z))
                    .is_none_or(|h| h.flags & f::VERT != 0 || h.normal.y > 0.3);
                if clear {
                    start = Some(g.point);
                    break;
                }
            }
        }
        let Some(start) = start else { continue };
        tried += 1;
        let towards = -out;
        let mut skater = Skater::new(start, towards.x.atan2(towards.z));
        skater.velocity = towards * 700.0;
        let traced = trace == Some(tried);
        skater.trace = traced;
        let mut got = false;
        let mut lowest_up: f32 = 1.0;
        let mut highest = start.y;
        for frame in 0..240 {
            // (Pushing on the flat only: up held at the lip breaks out of
            // vert, over onto the deck.)
            let input = Input {
                push: frame < 10,
                ..Input::default()
            };
            skater.update(input, &physics, &world, dt);
            if skater.on_ground {
                lowest_up = lowest_up.min(skater.up.y);
            }
            highest = highest.max(skater.position.y);
            if traced {
                println!(
                    "{frame}: {:?} heading {:.1} ground {} up {:.2} at {:.1} v {:.0} vert {}",
                    skater.action,
                    skater.heading.to_degrees(),
                    skater.on_ground,
                    skater.up,
                    skater.position,
                    skater.velocity,
                    skater.vert.is_some()
                );
            }
            if skater.vert.is_some() {
                got = true;
                break;
            }
        }
        launched += usize::from(got);
        if !got {
            steepest.push((tried, centre, lowest_up, highest - start.y));
        }
    }
    println!("{level}: {launched} of {tried} quarter pipes launched into vert air");
    for (run, centre, up, rise) in steepest.iter().take(40) {
        println!("  run {run}: ramp at {centre:.0}, steepest ridden up.y {up:.2}, rose {rise:.0}");
    }
}
