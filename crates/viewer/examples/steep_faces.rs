//! How a level's steep faces (steeper than `Wall_Non_Skatable_Angle` but
//! facing up) are flagged: vert, skatable, or neither (walls to the
//! skater), and how the ones of a ramp's curve go.
//!
//! `cargo run --release -p desa_viewer --example steep_faces [game data] [level]`
use std::path::Path;

use desa_viewer::source::GameData;
use glam::Vec3;
use ngc_collision::face_flags as f;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).cloned().unwrap_or_else(|| "extracted".into());
    let level = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "ToyStory_Bedroom".into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let files = data.load_level(&level).unwrap();
    let collision = ngc_collision::Collision::parse(files.collision.as_ref().unwrap()).unwrap();
    // By steepness (normal's y in tenths), the faces' count by flags.
    let mut table = [[0usize; 4]; 11];
    let mut flags_seen = std::collections::BTreeMap::new();
    for o in &collision.objects {
        for face in &o.faces {
            let [a, b, c] = face.indices.map(|i| Vec3::from(o.vertices[i as usize]));
            let n = (b - a).cross(c - a).normalize_or_zero();
            if n == Vec3::ZERO || face.flags & f::NON_COLLIDABLE != 0 {
                continue;
            }
            let y = n.y.abs();
            let row = (y * 10.0).floor().min(10.0) as usize;
            let col = if face.flags & f::VERT != 0 {
                0
            } else if face.flags & f::SKATABLE != 0 {
                1
            } else if face.flags & f::NOT_SKATABLE != 0 {
                2
            } else {
                3
            };
            table[row][col] += 1;
            if (0.05..0.42).contains(&y) {
                *flags_seen.entry(face.flags).or_insert(0usize) += 1;
            }
        }
    }
    println!("|normal.y|  vert  skatable  not_skatable  none");
    for (row, counts) in table.iter().enumerate() {
        println!(
            "{:.1}-{:.1}  {:6} {:9} {:13} {:6}",
            row as f32 / 10.0,
            (row + 1) as f32 / 10.0,
            counts[0],
            counts[1],
            counts[2],
            counts[3]
        );
    }
    println!("steep (0.05..0.42) faces by flags: {flags_seen:x?}");
}
