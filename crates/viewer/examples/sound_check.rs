//! Checks the skater's sounds: on each level, which terrains' sounds the
//! level's archive has (and which fall back to concrete), that they decode,
//! and that the default sound device plays one.
//!
//! `cargo run --release -p desa_viewer --example sound_check [game data] [--play]`
use std::path::Path;

use desa_viewer::sounds::{Moment, TerrainSounds};
use desa_viewer::source::GameData;
use qb::vm::Program;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).cloned().unwrap_or_else(|| "extracted".into());
    let play = args.iter().any(|a| a == "--play");
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let mut program = Program::new();
    for file in data.global_scripts().unwrap() {
        program.add(&file).unwrap();
    }
    let terrain = TerrainSounds::new(&program);
    println!("{} terrain sounds in the scripts", terrain.len());
    let skater = data.sounds("skater_sounds.prg").unwrap();
    for name in [
        "bail_knee1",
        "bodysmacka",
        "hud_jumpgap",
        "hud_specialtrickaa",
        "copinghit3_11",
        "goaldone",
        "gapsound",
    ] {
        println!("skater sound {name}: {}", skater.contains_key(name));
    }
    // Sounds the levels' object scripts play (birds taking off).
    for (level, name) in [
        ("hub", "wingflaps_away_01"),
        ("beach", "shortgull"),
        ("graveyard", "crow01"),
    ] {
        let files = data.sounds(&format!("{level}.prg")).unwrap();
        println!("{level} sound {name}: {}", files.contains_key(name));
    }
    for level in data.levels() {
        let files = data.sounds(&format!("{}.prg", level.id)).unwrap();
        let (mut found, mut fallback, mut missing, mut bad) = (0, 0, 0, 0);
        for t in 0..50u16 {
            for moment in [
                Moment::Roll,
                Moment::Jump,
                Moment::Land,
                Moment::Grind,
                Moment::Bonk,
            ] {
                match terrain.get(t, moment) {
                    Some(s) if files.contains_key(&s.file) => {
                        found += 1;
                        if ngc_sound::Sound::parse(&files[&s.file]).is_err() {
                            bad += 1;
                        }
                    }
                    Some(_)
                        if terrain
                            .get(1, moment)
                            .is_some_and(|c| files.contains_key(&c.file)) =>
                    {
                        fallback += 1
                    }
                    _ => missing += 1,
                }
            }
        }
        println!(
            "{}: {} sound files; terrain sounds {found} found, {fallback} on concrete, {missing} none, {bad} undecodable",
            level.id,
            files.len()
        );
    }
    // The songs and the levels' ambiences.
    let name = |v: &qb::Value, key: &str| match v.get(qb::checksum(key)) {
        Some(qb::Value::String(s)) => s.rsplit(['\\', '/']).next().map(str::to_string),
        _ => None,
    };
    let songs: Vec<String> = program
        .value(qb::checksum("playlist_tracks"))
        .and_then(|v| v.as_array())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| name(v, "on_disk"))
        .collect();
    let ambiences: Vec<String> = program
        .values()
        .filter_map(|(_, v)| name(v, "ambient_track"))
        .collect();
    for track in songs.iter().chain(&ambiences) {
        match data.music(track).unwrap() {
            Some(bytes) => {
                let dtk = ngc_sound::Dtk::new(bytes);
                println!("music {track}: {:.0} s", dtk.seconds());
            }
            None => println!("music {track}: MISSING"),
        }
    }
    // Each character's voice lines.
    for who in ["jessie", "woody", "simba", "zurg", "kid"] {
        let lines = data.voices(who).unwrap();
        let decoded = lines
            .iter()
            .filter(|(_, d)| ngc_sound::Sound::parse(d).is_ok())
            .count();
        let tricks = lines.iter().filter(|(k, _)| k == "trick").count();
        println!(
            "voices {who}: {} lines ({tricks} trick), {decoded} decode",
            lines.len()
        );
    }
    if play {
        let files = data.sounds("hub.prg").unwrap();
        let sound = ngc_sound::Sound::parse(&files["ollieconc"]).unwrap();
        match rodio::DeviceSinkBuilder::open_default_sink() {
            Ok(sink) => {
                let player = rodio::Player::connect_new(sink.mixer());
                player.append(rodio::buffer::SamplesBuffer::new(
                    std::num::NonZero::new(1).unwrap(),
                    std::num::NonZero::new(sound.sample_rate).unwrap(),
                    sound.floats(),
                ));
                player.sleep_until_end();
                println!("played ollieconc ({:.2} s)", sound.seconds());
            }
            Err(e) => println!("no sound device: {e}"),
        }
    }
}
