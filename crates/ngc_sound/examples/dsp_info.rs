//! Prints what's in `.dsp` sounds, and writes one as a `.wav` to listen to.
//!
//! `cargo run -p ngc_sound --example dsp_info -- FILE.dsp... [--wav OUT.wav]`
use std::path::Path;

use ngc_sound::Sound;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let wav = args
        .iter()
        .position(|a| a == "--wav")
        .and_then(|i| args.get(i + 1).cloned());
    for path in args.iter().take_while(|a| *a != "--wav") {
        let data = std::fs::read(path).unwrap();
        match Sound::parse(&data) {
            Ok(sound) => {
                let peak = sound
                    .samples
                    .iter()
                    .map(|s| s.unsigned_abs())
                    .max()
                    .unwrap_or(0);
                println!(
                    "{}: {} Hz, {:.2} s, loop {:?}, peak {peak}",
                    Path::new(path).file_name().unwrap().to_string_lossy(),
                    sound.sample_rate,
                    sound.seconds(),
                    sound.loop_start
                );
                if let Some(out) = &wav {
                    write_wav(out, &sound);
                }
            }
            Err(e) => println!("{path}: {e}"),
        }
    }
}

fn write_wav(path: &str, sound: &Sound) {
    let data_len = (sound.samples.len() * 2) as u32;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&sound.sample_rate.to_le_bytes());
    out.extend_from_slice(&(sound.sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in &sound.samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, out).unwrap();
}
