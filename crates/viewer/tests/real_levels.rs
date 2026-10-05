use std::path::Path;

use desa_viewer::source::GameData;
use ngc_anim::CameraPath;

/// Loads every level's files from a real disc and parses its camera paths.
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

    let mut total = 0;
    for level in &levels {
        let files = data.load_level(&level.id).unwrap();
        for (name, bytes) in &files.cameras {
            let path =
                CameraPath::parse(bytes).unwrap_or_else(|e| panic!("{} {name}: {e}", level.id));
            assert!(path.last_key_time() <= path.duration, "{} {name}", level.id);
        }
        println!("{}: {} camera paths", level.title, files.cameras.len());
        total += files.cameras.len();
    }
    assert_eq!(total, 292);
}
