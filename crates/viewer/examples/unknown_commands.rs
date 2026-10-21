//! Runs every level's scripts a while and lists the commands they used
//! that aren't understood yet, most used first:
//! `cargo run --release --example unknown_commands`.
use std::collections::HashMap;
use std::path::Path;

use desa_viewer::behaviour::Behaviour;
use desa_viewer::nodes::LevelNodes;
use desa_viewer::objects::{self, LevelObjects};
use desa_viewer::source::GameData;

fn main() {
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let sets = data.animation_sets().unwrap();
    let mut symbols = qb::Symbols::new();
    for file in data.global_scripts().unwrap() {
        if let Ok(t) = qb::tokenize(&file) {
            symbols.add_tokens(&t);
        }
    }
    let mut all: HashMap<String, (usize, Vec<String>)> = HashMap::new();
    for level in data.levels() {
        let mut files = data.load_level(&level.id).unwrap();
        let Some(n) = files.nodes.as_deref() else {
            continue;
        };
        let nodes = LevelNodes::from_bytes(n).unwrap();
        let mut scripts = data.global_scripts().unwrap();
        scripts.extend(files.scripts.iter().cloned());
        for file in &files.scripts {
            if let Ok(t) = qb::tokenize(file) {
                symbols.add_tokens(&t);
            }
        }
        data.add_animations(&mut files, &objects::needed_animations(&nodes, &sets))
            .unwrap();
        let mut objects = LevelObjects::build(&nodes, &files, &sets);
        let mut b = Behaviour::new(&nodes, &scripts);
        b.goal_prefix = desa_viewer::goals::goal_prefix(&level.id);
        for step in 0..600 {
            b.update(&mut objects, step as f32 / 30.0, 1.0 / 30.0, false, None);
        }
        for (name, count) in &b.unknown {
            let e = all.entry(symbols.name(*name)).or_default();
            e.0 += count;
            e.1.push(level.id.clone());
        }
    }
    let mut list: Vec<_> = all.into_iter().collect();
    list.sort_by_key(|(_, (c, _))| std::cmp::Reverse(*c));
    for (name, (count, levels)) in list.iter().take(60) {
        println!("{count:6} {name} ({})", levels.join(" "));
    }
}
