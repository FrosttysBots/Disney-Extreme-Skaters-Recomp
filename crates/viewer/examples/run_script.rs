//! Runs one of the game's scripts with a host that logs every command (and
//! answers yes to every question), for seeing what a script does:
//! `cargo run --example run_script -- create_menu_bg num_items=4`.
use std::path::Path;

use qb::vm::{Host, Outcome, Program, Thread};
use qb::{Value, checksum};

struct Log;

impl Host for Log {
    fn command(&mut self, target: Option<u32>, name: u32, args: &Value) -> Outcome {
        println!("{target:?} {name:#010x} {args:?}");
        Outcome::Done(true)
    }
}

fn main() {
    let mut argv = std::env::args().skip(1);
    let script = argv.next().unwrap_or_else(|| "create_menu_bg".into());
    let params: Vec<(Option<u32>, Value)> = argv
        .filter_map(|a| {
            let (k, v) = a.split_once('=')?;
            let v = v
                .parse::<i32>()
                .map(Value::Integer)
                .or_else(|_| v.parse::<f32>().map(Value::Float))
                .unwrap_or_else(|_| Value::Name(checksum(v)));
            Some((Some(checksum(k)), v))
        })
        .collect();
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = desa_viewer::source::GameData::open(Path::new(&path)).unwrap();
    let mut program = Program::new();
    for file in data.global_scripts().unwrap() {
        program.add(&file).unwrap();
    }
    let mut thread = Thread::new(checksum(&script), params);
    thread.run(&program, &mut Log, 0.0);
}
