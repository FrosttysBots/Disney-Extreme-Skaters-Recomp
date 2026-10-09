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
            // The pro's line for each letter, where the goal names them.
            let streams = data.goal_streams().unwrap();
            let named = goal.streams.iter().flatten().count();
            let mut said = 0;
            for line in goal.streams.iter().flatten() {
                let range = streams.get(line).expect("a letter line on the disc");
                let sound = data.stream(*range).unwrap().expect("the line's sound");
                assert!(ngc_sound::Sound::parse(&sound).is_ok());
                said += 1;
            }
            println!("{}: {said} of {named} letter lines", level.id);
        }
        // Each character's collectibles on the levels of its world.
        let mut collectibles = 0;
        for character in [
            "woody", "buzz", "jessie", "zurg", "tarzan", "jane", "terk", "tantor", "simba", "nala",
            "rafiki", "timon", "kid",
        ] {
            let list = desa_viewer::goals::collectibles(behaviour.program(), character)
                .unwrap_or_else(|| panic!("{character}'s collectibles"));
            assert_eq!(list.objects.len(), 25, "{character}");
            let here = list
                .objects
                .iter()
                .filter(|o| behaviour.object(**o).is_some())
                .count();
            assert!(here == 0 || here == 25, "{} {character}: {here}", level.id);
            // And the world's special item with them.
            let special = list.special.as_ref().expect("a special item").0;
            assert_eq!(
                behaviour.object(special).is_some(),
                here == 25,
                "{} {character}'s special",
                level.id
            );
            collectibles += here;
        }
        println!("{}: {collectibles} collectibles", level.id);
        let goals = desa_viewer::goals::level_goals(behaviour.program(), &level.id);
        println!(
            "{}: goals {:?}",
            level.id,
            goals
                .iter()
                .map(|g| format!("{}: {}", g.kind, g.text))
                .collect::<Vec<_>>()
        );
        if level.id != "SkateShop" {
            assert!(goals.len() >= 5, "{}: {} goals", level.id, goals.len());
        }
        let mut particles = desa_viewer::particles::Particles::new(behaviour.program(), &nodes);
        for _ in 0..120 {
            particles.update(behaviour.program(), 1.0 / 60.0, glam::Vec3::ZERO);
        }
        println!(
            "{}: {} of {} particle emitters make systems",
            level.id,
            particles.len(),
            nodes.emitters.len()
        );
        if let Some(race) = desa_viewer::goals::race(behaviour.program(), &level.id) {
            println!(
                "{}: race {:?}, {} waypoints, {:.0} s in all",
                level.id,
                race.name,
                race.waypoints.len(),
                race.waypoints.iter().map(|w| w.2).sum::<f32>()
            );
            for (name, _, _) in &race.waypoints {
                assert!(
                    nodes
                        .nodes
                        .iter()
                        .any(|n| n.name == *name && n.position.is_some()),
                    "{} race waypoint",
                    level.id
                );
            }
        }
        let effects = desa_viewer::triggers::teleport_effects(&nodes, behaviour.program());
        let mut said: Vec<String> = effects
            .values()
            .map(|e| format!("{:?} {:08x}", e.message, e.sound.unwrap_or(0)))
            .collect();
        said.sort();
        said.dedup();
        println!("{}: {} teleporters: {said:?}", level.id, effects.len());
        if level.id == "HUB" {
            // The harbour water splashes.
            assert!(
                effects
                    .values()
                    .any(|e| e.sound == Some(qb::checksum("bigsplash")))
            );
        }
        let breaks = desa_viewer::triggers::breakables(&nodes, behaviour.program());
        let killed: std::collections::HashSet<u32> = breaks
            .values()
            .flat_map(|(_, k, _)| k.iter().copied())
            .collect();
        for k in killed.iter().take(3) {
            if let Some(p) = nodes
                .nodes
                .iter()
                .find(|n| n.name == *k)
                .and_then(|n| n.position)
            {
                println!("{} breakable at {:.0} {:.0} {:.0}", level.id, p.x, p.y, p.z);
            }
        }
        let sectors = killed
            .iter()
            .filter(|k| !nodes.hidden_sectors.contains(k) && behaviour.object(**k).is_none())
            .count();
        println!(
            "{}: {} breaking triggers, {} things broken ({sectors} sectors there at the start)",
            level.id,
            breaks.len(),
            killed.len()
        );
        let pros: Vec<String> = ["HighScore", "ProScore", "SKATE", "Race"]
            .iter()
            .filter_map(|kind| {
                let pro = desa_viewer::goals::goal_pro(behaviour.program(), &level.id, kind)?;
                let object = behaviour.object(pro);
                Some(format!(
                    "{kind}: {}",
                    object.map_or("not an object".to_string(), |o| format!(
                        "{} at {:.0} {:.0} {:.0}",
                        nodes.objects[o].label,
                        nodes.objects[o].position.x,
                        nodes.objects[o].position.y,
                        nodes.objects[o].position.z
                    ))
                ))
            })
            .collect();
        println!("{}: pros {pros:?}", level.id);
        let streams = data.goal_streams().unwrap();
        let intros: Vec<&str> = ["HighScore", "ProScore", "SKATE", "Race"]
            .into_iter()
            .filter(|kind| {
                desa_viewer::goals::goal_intro_line(behaviour.program(), &level.id, kind)
                    .is_some_and(|l| streams.contains_key(&l))
            })
            .collect();
        println!("{}: intro lines for {intros:?}", level.id);
        if level.id != "SkateShop" {
            assert!(intros.len() >= 3, "{}: goal intro lines", level.id);
        }
        let warps = desa_viewer::warps::warps(behaviour.program(), &nodes);
        // Each warp's camera path is the level's.
        for warp in &warps {
            if let Some(camera) = warp.camera {
                assert!(
                    files.cameras.iter().any(|(n, _)| qb::checksum(n) == camera),
                    "{}: {}'s warp camera",
                    level.id,
                    warp.title
                );
            }
        }
        println!(
            "{}: warps {:?}",
            level.id,
            warps
                .iter()
                .map(|w| format!(
                    "{} at {:.0} {:.0} {:.0}",
                    w.title, w.position.x, w.position.y, w.position.z
                ))
                .collect::<Vec<_>>()
        );
        if level.id == "HUB" {
            assert_eq!(warps.len(), 9, "the Hub's portals");
        } else if level.id != "SkateShop" {
            assert_eq!(warps.len(), 1, "{}: the way back", level.id);
        }
        let mut objects = built;
        for step in 0..600 {
            behaviour.update(&mut objects, step as f32 / 30.0, 1.0 / 30.0, false, None);
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
