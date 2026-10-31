//! A level's collision objects by name: their bounds and trigger faces.
//! `cargo run --example object_bounds -- HUB Lighthouse_CounterKiller`.
use std::path::Path;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let level = a.get(1).cloned().unwrap_or_else(|| "HUB".into());
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = desa_viewer::source::GameData::open(Path::new(&path)).unwrap();
    let files = data.load_level(&level).unwrap();
    let collision = ngc_collision::Collision::parse(files.collision.as_deref().unwrap()).unwrap();
    for name in &a[2..] {
        let sum = qb::checksum(name);
        for object in collision.objects.iter().filter(|o| o.checksum == sum) {
            let triggers = object
                .faces
                .iter()
                .filter(|f| f.flags & ngc_collision::face_flags::TRIGGER != 0)
                .count();
            println!(
                "{name}: bbox {:?}, {} faces ({triggers} trigger)",
                object.bbox.map(|v| v.round()),
                object.faces.len()
            );
            for f in &object.faces {
                let v = f
                    .indices
                    .map(|i| object.vertices[usize::from(i)].map(|x| x.round()));
                println!("  {:#06x} {:?}", f.flags, v);
            }
        }
    }
}
