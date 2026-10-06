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
    let physics = Physics::new(&program, &Stats::of(&program, "jessie"));
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
        skater.velocity = into * 650.0;
        let push = Input {
            push: true,
            ..Input::default()
        };
        let mut top = ground.point.y;
        let mut airborne = false;
        let mut landed_at = None;
        for _ in 0..300 {
            skater.update(push, &physics, &world, 1.0 / 60.0);
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
