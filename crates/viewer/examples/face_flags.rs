//! Counts each level's collision faces by flag.
//!
//! `cargo run --release -p desa_viewer --example face_flags [game data]`
use std::path::Path;

use desa_viewer::source::GameData;
use ngc_collision::face_flags as f;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "extracted".into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    for level in data.levels() {
        let files = data.load_level(&level.id).unwrap();
        let Some(collision) = files.collision.as_ref() else {
            continue;
        };
        let collision = ngc_collision::Collision::parse(collision).unwrap();
        let mut counts = [0usize; 5];
        for face in collision.objects.iter().flat_map(|o| &o.faces) {
            for (i, flag) in [
                f::SKATABLE,
                f::NOT_SKATABLE,
                f::WALL_RIDABLE,
                f::VERT,
                f::TRIGGER,
            ]
            .into_iter()
            .enumerate()
            {
                counts[i] += usize::from(face.flags & flag != 0);
            }
        }
        println!(
            "{}: skatable {}, not skatable {}, wall-ridable {}, vert {}, trigger {}",
            level.id, counts[0], counts[1], counts[2], counts[3], counts[4]
        );
    }
}
