//! Lists a level's rail nodes with trigger scripts:
//! `cargo run --example rail_scripts -- Tarzan_Treehouse`; also counts the
//! segments from `LipOverride` nodes.
use std::path::Path;

fn main() {
    let level = std::env::args().nth(1).unwrap_or_else(|| "HUB".into());
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = desa_viewer::source::GameData::open(Path::new(&path)).unwrap();
    let files = data.load_level(&level).unwrap();
    let nodes =
        desa_viewer::nodes::LevelNodes::from_bytes(files.nodes.as_deref().unwrap()).unwrap();
    let mut program = qb::vm::Program::new();
    for file in data.global_scripts().unwrap().iter().chain(&files.scripts) {
        program.add(file).unwrap();
    }
    let gaps = desa_viewer::triggers::gaps(&nodes, &program);
    for (name, script) in &nodes.rail_scripts {
        println!(
            "{:?}: {script:#010x} gap {:?}",
            nodes.labels.get(name),
            gaps.get(name)
        );
    }
    let named = nodes.rails.iter().filter(|r| r.node != 0).count();
    println!(
        "{} rail scripts, {} of {} segments named",
        nodes.rail_scripts.len(),
        named,
        nodes.rails.len()
    );
    let wide: Vec<_> = nodes.rails.iter().filter(|r| r.lip_override).collect();
    println!(
        "{} LipOverride segments, {} unnamed",
        wide.len(),
        wide.iter().filter(|r| r.node == 0).count()
    );
}
