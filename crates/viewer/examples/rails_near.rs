//! Lists a level's rail segments near a point:
//! `cargo run --example rails_near -- HUB 5883 1193 -9243 300`.
use std::path::Path;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let level = a.get(1).cloned().unwrap_or_else(|| "HUB".into());
    let p = glam::Vec3::new(
        a[2].parse().unwrap(),
        a[3].parse().unwrap(),
        a[4].parse().unwrap(),
    );
    let r: f32 = a.get(5).and_then(|s| s.parse().ok()).unwrap_or(300.0);
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = desa_viewer::source::GameData::open(Path::new(&path)).unwrap();
    let files = data.load_level(&level).unwrap();
    let nodes =
        desa_viewer::nodes::LevelNodes::from_bytes(files.nodes.as_deref().unwrap()).unwrap();
    for s in &nodes.rails {
        if s.start.distance(p) < r || s.end.distance(p) < r {
            println!(
                "{:?}: {:.0} {:.0} {:.0} -> {:.0} {:.0} {:.0}",
                nodes.labels.get(&s.node),
                s.start.x,
                s.start.y,
                s.start.z,
                s.end.x,
                s.end.y,
                s.end.z
            );
        }
    }
}
