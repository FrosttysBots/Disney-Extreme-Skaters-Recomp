//! Lists a character's animation names matching a word:
//! `cargo run --example anim_names -- jessie wall`.
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let id = args.get(1).map_or("jessie", String::as_str);
    let word = args.get(2).map_or("", String::as_str).to_ascii_lowercase();
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = desa_viewer::source::GameData::open(Path::new(&path)).unwrap();
    let files = data.load_character(id).unwrap();
    for (name, bytes) in &files.animations {
        if name.to_ascii_lowercase().contains(&word) {
            println!("{name} ({} bytes)", bytes.len());
        }
    }
}
