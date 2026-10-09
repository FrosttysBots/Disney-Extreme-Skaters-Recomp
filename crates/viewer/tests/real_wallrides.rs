use std::path::Path;

use desa_viewer::nodes::LevelNodes;
use desa_viewer::source::GameData;
use glam::Vec3;
use qb::vm::Program;
use skate::{Input, Physics, Skater, Stats, World};

/// Every level: jumps alongside a sample of its big walls holding grind.
/// Most should be ridden; none should leave the skater under the level.
/// `SHOW=pizza` lists spots a push, ollie and grind from the ground ride.
#[test]
#[ignore = "needs the game data"]
fn real_wallrides() {
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let mut program = Program::new();
    for file in data.global_scripts().unwrap() {
        program.add(&file).unwrap();
    }
    let physics = Physics::new(&program, &Stats::of(&program, "jessie"));
    let (mut tried_all, mut rode_all) = (0, 0);
    for level in data.levels() {
        let Ok(files) = data.load_level(&level.id) else {
            continue;
        };
        let (Some(c), Some(_)) = (files.collision.as_ref(), files.nodes.as_ref()) else {
            continue;
        };
        let _ = LevelNodes::from_bytes(files.nodes.as_ref().unwrap());
        let collision = ngc_collision::Collision::parse(c).unwrap();
        // Big upright faces, every so often.
        let mut walls = Vec::new();
        for object in &collision.objects {
            for face in &object.faces {
                let [a, b, c] = face
                    .indices
                    .map(|i| Vec3::from(object.vertices[usize::from(i)]));
                let cross = (b - a).cross(c - a);
                if cross.length() < 2.0 * 150.0 * 150.0 {
                    continue;
                }
                let n = cross.normalize();
                if n.y.abs() < 0.1 {
                    walls.push(((a + b + c) / 3.0, n));
                }
            }
        }
        let step = (walls.len() / 60).max(1);
        let world = World::new(collision);
        let (mut tried, mut rode, mut lost) = (0, 0, Vec::new());
        for &(centre, n) in walls.iter().step_by(step) {
            for side in [1.0f32, -1.0] {
                // Out of the wall either side (whichever's open), along it.
                let n = n * side;
                let along = Vec3::new(-n.z, 0.0, n.x).normalize();
                let start = centre + n * 60.0 - along * 200.0;
                if world.ray(centre + n * 2.0, start).is_some() {
                    continue;
                }
                tried += 1;
                let mut skater = Skater::new(start, 0.0);
                skater.on_ground = false;
                skater.velocity = along * 600.0 - n * 150.0 + Vec3::Y * 200.0;
                let mut on_wall = 0;
                let mut lowest = f32::MAX;
                for _ in 0..240 {
                    let input = Input {
                        grind: true,
                        ..Input::default()
                    };
                    skater.update(input, &physics, &world, 1.0 / 60.0);
                    on_wall += usize::from(skater.wall.is_some());
                    lowest = lowest.min(skater.position.y);
                    if skater.on_ground {
                        break;
                    }
                }
                if on_wall > 0 {
                    rode += 1;
                }
                if std::env::var("SHOW").is_ok_and(|l| l == level.id) && on_wall > 5 {
                    // From the ground: push, ollie, grind, as a player would.
                    let Some(ground) = world.ray(start, start - Vec3::Y * 300.0) else {
                        continue;
                    };
                    for angle in [10f32, 20.0, 30.0] {
                        let dir = (along * angle.to_radians().cos() - n * angle.to_radians().sin())
                            .normalize();
                        let from = ground.point - along * 250.0;
                        if world
                            .ray(from + Vec3::Y * 20.0, ground.point + Vec3::Y * 20.0)
                            .is_some()
                            || world
                                .ray(from + Vec3::Y * 20.0, from - Vec3::Y * 40.0)
                                .is_none()
                        {
                            continue;
                        }
                        let heading = dir.x.atan2(dir.z);
                        let mut s = Skater::new(from, heading);
                        s.place(from, heading, &physics, &world);
                        let mut frames = 0;
                        for f in 0..180 {
                            let t = f as f32 / 60.0;
                            let input = Input {
                                push: true,
                                crouch: (0.9..1.2).contains(&t),
                                grind: t >= 1.25,
                                ..Input::default()
                            };
                            s.update(input, &physics, &world, 1.0 / 60.0);
                            frames += usize::from(s.wall.is_some());
                        }
                        if frames > 10 {
                            println!(
                                "  from {:.0},{:.0},{:.0} heading {:.0}: rode {frames} frames",
                                from.x,
                                from.y,
                                from.z,
                                heading.to_degrees()
                            );
                        }
                    }
                }
                if on_wall > 0 && lowest < world.floor() - 400.0 {
                    lost.push(centre);
                }
            }
        }
        println!(
            "{}: {rode} of {tried} walls ridden, {} lost under the level",
            level.id,
            lost.len()
        );
        for at in lost.iter().take(5) {
            println!("  lost at a wall at {:.0} {:.0} {:.0}", at.x, at.y, at.z);
        }
        assert!(lost.len() * 20 <= tried.max(1), "{}: {lost:?}", level.id);
        tried_all += tried;
        rode_all += rode;
    }
    println!("{rode_all} of {tried_all}");
    assert!(rode_all * 3 > tried_all);
}
