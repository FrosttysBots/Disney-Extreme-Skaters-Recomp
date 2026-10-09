use std::path::Path;

use desa_viewer::behaviour::Behaviour;
use desa_viewer::nodes::LevelNodes;
use desa_viewer::objects::{self, LevelObjects};
use desa_viewer::source::GameData;
use glam::Vec3;

/// Every level's goals that its own scripts play (not the score, letters
/// and race goals): started as the viewer starts them, then the skater
/// taken to each thing that does something when it's near, in turn. The
/// goals that are only that much get done.
/// Run with `cargo test --release -p desa_viewer -- --ignored`; the game
/// data is a disc image or folder in `DESA_GAME_DATA`, by default `extracted`.
#[test]
#[ignore = "needs the game data"]
fn real_goal_scripts() {
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let sets = data.animation_sets().unwrap();
    let mut done = Vec::new();
    for level in data.levels() {
        let mut files = data.load_level(&level.id).unwrap();
        let nodes = LevelNodes::from_bytes(files.nodes.as_deref().unwrap()).unwrap();
        let mut scripts = data.global_scripts().unwrap();
        scripts.extend(files.scripts.iter().cloned());
        data.add_animations(&mut files, &objects::needed_animations(&nodes, &sets))
            .unwrap();
        let mut objects = LevelObjects::build(&nodes, &files, &sets);
        let first = Behaviour::new(&nodes, &scripts);
        for goal in desa_viewer::goals::level_goals(first.program(), &level.id) {
            if ["HighScore", "ProScore", "Skate", "Race"].contains(&goal.kind.as_str()) {
                continue;
            }
            let g =
                desa_viewer::goals::generic_goal(first.program(), &level.id, &goal.kind).unwrap();
            let mut b = Behaviour::new(&nodes, &scripts);
            let mut run = |b: &mut Behaviour, frames: usize| {
                for step in 0..frames {
                    b.update(&mut objects, step as f32 / 30.0, 1.0 / 30.0, false, None);
                }
            };
            run(&mut b, 30);
            b.active_goal = Some(g.id);
            b.goal_needed = g.needed;
            let mut params: qb::vm::Params = g
                .params
                .iter()
                .map(|(k, v)| (Some(*k), v.clone()))
                .collect();
            params.push((Some(qb::checksum("goal_ID")), qb::Value::Name(g.id)));
            params.push((Some(qb::checksum("talked_to_pro")), qb::Value::Integer(0)));
            b.goal_params = params.clone();
            for script in [g.activate, g.start_script].into_iter().flatten() {
                b.run_level_script(script, params.clone());
            }
            run(&mut b, 90);
            let mut visited: Vec<Vec3> = Vec::new();
            for _ in 0..12 {
                let new: Vec<Vec3> = b
                    .radius_triggers()
                    .into_iter()
                    .filter(|p| !visited.iter().any(|v| v.distance(*p) < 1.0))
                    .collect();
                if new.is_empty() {
                    break;
                }
                for at in new {
                    visited.push(at);
                    b.set_skater(Some(at));
                    run(&mut b, 20);
                    b.set_skater(Some(at + Vec3::Y * 5000.0));
                    run(&mut b, 20);
                }
            }
            // (What the last one set off finishes.)
            run(&mut b, 300);
            println!(
                "{} {} \"{}\": {}/{} after {} visits",
                level.id,
                goal.kind,
                goal.text,
                b.goal_progress(),
                g.needed,
                visited.len()
            );
            if g.needed > 0 && b.goal_progress() >= g.needed {
                done.push(format!("{} {}", level.id, goal.kind));
            }
        }
    }
    println!("done by going near things: {done:?}");
    for goal in [
        "beach Counter",
        "pizza Collect2",
        "canyon Collect",
        "graveyard Counter2",
    ] {
        assert!(done.contains(&goal.to_string()), "{goal}");
    }
}
