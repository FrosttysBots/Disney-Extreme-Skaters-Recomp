use std::path::Path;

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
}
