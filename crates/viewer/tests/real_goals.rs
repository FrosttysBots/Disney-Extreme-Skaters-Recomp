use std::path::Path;

use desa_viewer::behaviour::Behaviour;
use desa_viewer::nodes::LevelNodes;
use desa_viewer::objects::{self, LevelObjects};
use desa_viewer::source::GameData;
use glam::Vec3;

/// Every level's goals that its own scripts play (not the score, letters
/// and race goals): started as the viewer starts them, then the skater
/// taken to each thing that does something when it's near, in turn
/// (following what moves off), every bouncy thing knocked and breakable
/// broken first; then every gap landed and trick spot done.
/// `GOAL=HUB:Collect` runs one; `DESA_QB_TRACE` and `DESA_PRINTF` trace
/// its scripts.
/// The goals that are only that much get done.
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
    let mut gapped = Vec::new();
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
            // `GOAL=HUB:Collect` runs just that one.
            if let Ok(only) = std::env::var("GOAL") {
                if only != format!("{}:{}", level.id, goal.kind) {
                    continue;
                }
            }
            let g =
                desa_viewer::goals::generic_goal(first.program(), &level.id, &goal.kind).unwrap();
            let mut b = Behaviour::new(&nodes, &scripts);
            b.goal_prefix = desa_viewer::goals::goal_prefix(&level.id);
            let mut run = |b: &mut Behaviour, frames: usize| {
                for step in 0..frames {
                    b.update(&mut objects, step as f32 / 30.0, 1.0 / 30.0, false, None);
                }
            };
            run(&mut b, 30);
            b.active_goal = Some(g.id);
            b.goal_needed = g.needed;
            b.goal_collects = desa_viewer::goals::counts_collected(&goal.kind);
            let mut params: qb::vm::Params = g
                .params
                .iter()
                .map(|(k, v)| (Some(*k), v.clone()))
                .collect();
            params.push((Some(qb::checksum("goal_ID")), qb::Value::Name(g.id)));
            params.push((Some(qb::checksum("talked_to_pro")), qb::Value::Integer(0)));
            b.goal_params = params.clone();
            if let Some(pro) = g.pro {
                b.create_object(pro);
            }
            for script in [g.activate, g.start_script].into_iter().flatten() {
                b.run_level_script(script, params.clone());
            }
            run(&mut b, 90);
            // Every bouncy with a script knocked (each that's there), first:
            // leaving Hub's store, its groceries are put away.
            for bouncy in &nodes.bouncies {
                let Some(script) = bouncy.collide_script else {
                    continue;
                };
                if !b.is_alive(bouncy.name) {
                    continue;
                }
                b.set_skater(Some(bouncy.position));
                b.run_object_script(bouncy.name, script);
                run(&mut b, 3);

                // (Off it, but not far: Hub's store puts its groceries
                // away when the skater leaves.)
                b.set_skater(Some(bouncy.position + Vec3::Y * 200.0));
                run(&mut b, 1);
            }
            // And every breakable that's there touched (its script run),
            // there from the start or created since.
            let mut shown: std::collections::HashMap<u32, bool> = Default::default();
            shown.extend(b.other_creates.drain(..));
            let breakables = desa_viewer::triggers::breakables(&nodes, first.program());
            let mut breakables: Vec<_> = breakables.into_iter().collect();
            breakables.sort_by_key(|(object, _)| *object);
            for (object, (script, ..)) in &breakables {
                let there = shown
                    .get(object)
                    .copied()
                    .unwrap_or(!nodes.hidden_sectors.contains(object));
                if there {
                    b.run_level_script(*script, Vec::new());
                    run(&mut b, 2);
                }
            }
            // And the other trigger faces that are there touched (as the
            // viewer runs them: Pizza's stools).
            shown.extend(b.other_creates.drain(..));
            let teleports = desa_viewer::triggers::teleports(&nodes, first.program());
            let gaps = desa_viewer::triggers::gaps(&nodes, first.program());
            for &(object, script) in nodes.geometry_scripts.iter().chain(&nodes.rail_scripts) {
                let other = !teleports.contains_key(&object)
                    && !gaps.contains_key(&object)
                    && !breakables.iter().any(|(o, _)| *o == object);
                let there = shown
                    .get(&object)
                    .copied()
                    .unwrap_or(!nodes.hidden_sectors.contains(&object));
                if other && there {
                    b.run_level_script(script, Vec::new());
                    run(&mut b, 20);
                }
            }
            // (Something that moves off, herded, is followed till it
            // stops: a skater keeping up with it.)
            // (Each place once for each thing it does: a pro taking back
            // the fruit just found is a visit again.)
            let mut visited: Vec<(Vec3, Option<u32>)> = Vec::new();
            for _ in 0..12 {
                let new: Vec<(usize, Vec3)> = b
                    .radius_trigger_objects()
                    .into_iter()
                    .map(|o| (o, b.position(o)))
                    .filter(|(o, p)| {
                        let script = b.radius_script(*o);
                        !visited
                            .iter()
                            .any(|(v, s)| v.distance(*p) < 1.0 && *s == script)
                    })
                    .collect();
                if new.is_empty() {
                    break;
                }
                for (object, at) in new {
                    visited.push((at, b.radius_script(object)));
                    let mut was = at;
                    let mut still = 0;
                    for step in 0..900 {
                        let now = b.position(object);
                        b.set_skater(Some(now));
                        run(&mut b, 1);
                        still = if b.position(object).distance(was) < 0.01 {
                            still + 1
                        } else {
                            0
                        };
                        was = b.position(object);
                        if step >= 20 && still > 45 {
                            break;
                        }
                    }
                    b.set_skater(Some(at + Vec3::Y * 5000.0));
                    run(&mut b, 20);
                }
            }
            // (What the last one set off finishes.)
            run(&mut b, 300);
            let near = b.goal_progress();
            // Then every gap landed and trick spot done: their scripts.
            let gap_scripts: Vec<u32> = desa_viewer::triggers::gaps(&nodes, first.program())
                .values()
                .flatten()
                .filter_map(|g| match g {
                    skate::GapTrigger::End { script, .. } => *script,
                    skate::GapTrigger::Start { trick, .. } => trick.as_ref().map(|t| t.script),
                })
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            // (Landed: on the ground, not bailing.)
            b.skater_state.on_ground = true;
            // (Twice: those asked in order, Zurg's spine's three tricks,
            // get done in order the second time round.)
            for script in gap_scripts.iter().chain(&gap_scripts) {
                b.run_level_script(*script, Vec::new());
                run(&mut b, 120);
            }
            run(&mut b, 300);
            if g.needed > 0 && b.goal_progress() >= g.needed && near < g.needed {
                gapped.push(format!("{} {}", level.id, goal.kind));
            }
            println!(
                "{} {} \"{}\": {}/{} after {} visits",
                level.id,
                goal.kind,
                goal.text,
                b.goal_progress(),
                g.needed,
                visited.len()
            );
            if g.needed > 0 && near >= g.needed {
                done.push(format!("{} {}", level.id, goal.kind));
            }
        }
    }
    println!("done by going near things: {done:?}");
    println!("{} done in all", done.len() + gapped.len());
    if std::env::var("GOAL").is_ok() {
        return;
    }
    assert!(done.len() + gapped.len() >= 42);
    println!("then by landing every gap: {gapped:?}");
    for goal in [
        "camp Gaps3",
        "camp Gaps2",
        "canyon Gaps",
        "graveyard Trickspot",
        "HUB Gaps2",
        "PrideRock Gaps",
        "camp Collect",
        "canyon Gaps3",
        "ToyStory_Bedroom Trickspot",
    ] {
        assert!(gapped.contains(&goal.to_string()), "{goal}");
    }
    for goal in [
        "beach Counter",
        "pizza Collect2",
        "canyon Collect",
        "graveyard Counter2",
        "HUB Collect3",
        "canyon Collect2",
        "pizza Collect3",
        "PrideRock Collect2",
        "HUB Collect",
        "beach Counter3",
        "camp Counter",
        "beach Counter2",
        "HUB Collect2",
        "HUB Counter",
        "PrideRock Collect",
    ] {
        assert!(done.contains(&goal.to_string()), "{goal}");
    }
}

/// Pizza's Skatetris: its list of trick slots and timings, and a trick in
/// each slot for the character playing it.
#[test]
#[ignore = "needs the game data"]
fn real_skatetris() {
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let files = data.load_level("pizza").unwrap();
    let nodes = LevelNodes::from_bytes(files.nodes.as_deref().unwrap()).unwrap();
    let mut scripts = data.global_scripts().unwrap();
    scripts.extend(files.scripts.iter().cloned());
    let b = Behaviour::new(&nodes, &scripts);
    let goal = desa_viewer::goals::generic_goal(b.program(), "pizza", "Tetris").unwrap();
    let tetris = desa_viewer::goals::Tetris::of(&goal).unwrap();
    println!("{tetris:?}");
    assert_eq!(tetris.combos.len(), 8);
    assert_eq!(tetris.trick_time, 3.0);
    assert_eq!(tetris.stop_adding, 13.0);
    let stats = skate::Stats::of(b.program(), "jessie");
    let book = skate::TrickBook::new(b.program(), "jessie", &stats);
    for slot in &tetris.combos {
        let trick = book.slot_trick(*slot).expect("a trick in the slot");
        println!("{}", book.tricks[trick].name);
    }
}
