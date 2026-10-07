use std::path::Path;

use desa_viewer::behaviour::Behaviour;
use desa_viewer::nodes::LevelNodes;
use desa_viewer::objects::{self, LevelObjects};
use desa_viewer::source::GameData;
use ngc_anim::CameraPath;

/// Loads every level's files from a real disc, parses its camera paths and
/// builds its objects and pedestrians.
/// Run with `cargo test --release -p desa_viewer -- --ignored`; the game
/// data is a disc image or folder in `DESA_GAME_DATA`, by default `extracted`.
#[test]
#[ignore = "needs the game data"]
fn real_level_camera_paths() {
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let levels = data.levels();
    assert_eq!(levels.len(), 11);

    let sets = data.animation_sets().unwrap();
    let (mut total, mut placed, mut pedestrians, mut animated) = (0, 0, 0, 0);
    let mut moved = 0;
    let mut unknown = std::collections::HashMap::new();
    for level in &levels {
        let mut files = data.load_level(&level.id).unwrap();
        let nodes = LevelNodes::from_bytes(files.nodes.as_deref().unwrap()).unwrap();
        data.add_animations(&mut files, &objects::needed_animations(&nodes, &sets))
            .unwrap();
        let built = LevelObjects::build(&nodes, &files, &sets);
        assert!(
            built.missing.is_empty(),
            "{}: {:?}",
            level.id,
            built.missing
        );
        let crowd = built.crowd.len() + built.goal_crowd.len();
        // Every member's posed mesh lines up with the uploaded one.
        assert_eq!(built.crowd.pose(1.0).len(), built.crowd.mesh.vertices.len());
        assert_eq!(
            built.goal_crowd.pose(1.0).len(),
            built.goal_crowd.mesh.vertices.len()
        );
        // Animated vertex colors, split between the start and goal layers.
        let hidden = &nodes.hidden_sectors;
        for goal in [false, true] {
            let level = desa_viewer::level::Level::from_bytes_filtered(
                &files.scene,
                files.textures.as_deref(),
                |s| hidden.contains(&s) == goal,
            )
            .unwrap();
            let anim = &level.color_animation;
            assert_eq!(
                anim.first_vertex + anim.vertices.len(),
                level.vertices.len()
            );
            animated += anim.vertices.len();
        }
        // Run the objects' scripts for 20 seconds.
        let mut scripts = data.global_scripts().unwrap();
        scripts.extend(files.scripts.iter().cloned());
        let mut behaviour = Behaviour::new(&nodes, &scripts);
        // The S-K-A-T-E letters goal, where the level has the letters.
        let has_letters = nodes
            .objects
            .iter()
            .any(|o| o.name == qb::checksum("TRG_Goal_Letter_S"));
        let letters = desa_viewer::goals::skate_letters(behaviour.program(), &level.id);
        println!("{}: letters {has_letters}, goal {letters:?}", level.id);
        for pro in [false, true] {
            let goal = desa_viewer::goals::score_goal(behaviour.program(), &level.id, pro);
            println!("{}: score goal {goal:?}", level.id);
            if level.id != "SkateShop" {
                assert!(goal.is_some_and(|g| g.score >= 10_000), "{}", level.id);
            }
        }
        if has_letters {
            let goal = letters.expect("a letters goal");
            assert!(goal.letters.iter().all(|l| behaviour.object(*l).is_some()));
        }
        let mut objects = built;
        for step in 0..600 {
            behaviour.update(&mut objects, step as f32 / 30.0, 1.0 / 30.0, false);
        }
        let movers = (0..nodes.objects.len())
            .filter(|&i| behaviour.position(i).distance(nodes.objects[i].position) > 10.0)
            .count();
        moved += movers;
        for (name, count) in behaviour.unknown {
            *unknown.entry(name).or_insert(0usize) += count;
        }
        placed += nodes.objects.len();
        pedestrians += crowd;
        for (name, bytes) in &files.cameras {
            let path =
                CameraPath::parse(bytes).unwrap_or_else(|e| panic!("{} {name}: {e}", level.id));
            assert!(path.last_key_time() <= path.duration, "{} {name}", level.id);
        }
        println!(
            "{}: {} camera paths, {} object nodes ({crowd} pedestrians), {} sectors hidden at the start",
            level.title,
            files.cameras.len(),
            nodes.objects.len(),
            nodes.hidden_sectors.len()
        );
        total += files.cameras.len();
    }
    assert_eq!(total, 292);
    println!("{placed} object nodes, {pedestrians} pedestrians, {animated} animated vertex colors");
    assert_eq!(animated, 803);
    let mut unknown: Vec<_> = unknown.into_iter().collect();
    unknown.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    let mut symbols = qb::Symbols::new();
    for file in data.global_scripts().unwrap() {
        if let Ok(tokens) = qb::tokenize(&file) {
            symbols.add_tokens(&tokens);
        }
    }
    println!(
        "{moved} objects moved in 20 seconds; commands not understood: {:?}",
        unknown
            .iter()
            .take(15)
            .map(|(n, c)| (symbols.name(*n), *c))
            .collect::<Vec<_>>()
    );
}
