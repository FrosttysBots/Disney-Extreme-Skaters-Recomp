//! Looks for broken character animations: plays each one at 60 frames a
//! second and reports bones that turn implausibly far in one frame (a
//! decoding problem shows as limbs snapping about).
//!
//! `cargo run --release -p desa_viewer --example anim_check [game data] [character|all] [name]`
//!
//! With a name, prints that animation's worst bones and keys.
use std::path::Path;

use desa_viewer::source::GameData;
use ngc_anim::{Animation, KeyTables, Skeleton};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).cloned().unwrap_or_else(|| "extracted".into());
    let who = args.get(2).cloned().unwrap_or_else(|| "all".into());
    let only = args.get(3).cloned();
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let ids: Vec<String> = if who == "all" {
        data.characters().into_iter().map(|c| c.id).collect()
    } else {
        vec![who]
    };
    for id in ids {
        let files = match data.load_character(&id) {
            Ok(f) => f,
            Err(e) => {
                println!("{id}: {e:#}");
                continue;
            }
        };
        let tables = KeyTables::parse(&files.key_tables.0, &files.key_tables.1).unwrap();
        let skeleton = Skeleton::parse(&files.skeleton).unwrap();
        if let Ok(names) = std::env::var("DUMP") {
            for name in names.split(',') {
                let (_, bytes) = files
                    .animations
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case(name))
                    .unwrap();
                let words: Vec<String> = bytes[..0x24]
                    .chunks(4)
                    .map(|w| format!("{:08x}", u32::from_be_bytes(w.try_into().unwrap())))
                    .collect();
                println!("{name}: {} bytes, header {}", bytes.len(), words.join(" "));
                let bones = u32::from_be_bytes(bytes[12..16].try_into().unwrap()) as usize;
                let sizes: Vec<u16> = (0..bones * 2)
                    .map(|i| u16::from_be_bytes([bytes[0x24 + i * 2], bytes[0x25 + i * 2]]))
                    .collect();
                println!("  rotation sizes {:?}", &sizes[..bones]);
                println!("  translation sizes {:?}", &sizes[bones..]);
                let mut at = 0x24 + bones * 4;
                for (i, &len) in sizes.iter().enumerate() {
                    let len = usize::from(len);
                    if i < 6 || (bones..bones + 6).contains(&i) {
                        let hex: Vec<String> = bytes[at..at + len]
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect();
                        println!("  stream {i}: {}", hex.join(" "));
                    }
                    at += len;
                }
            }
            continue;
        }
        if std::env::var("LENGTHS").is_ok() {
            lengths(&files, &tables);
            continue;
        }
        if let Ok(spec) = std::env::var("POSES") {
            compare(&files, &tables, &spec);
            continue;
        }
        let mut bad = Vec::new();
        for (name, bytes) in &files.animations {
            if only.as_ref().is_some_and(|o| !o.eq_ignore_ascii_case(name)) {
                continue;
            }
            let anim = match Animation::parse(bytes, &tables) {
                Ok(a) => a,
                Err(e) => {
                    println!("{id} {name}: {e}");
                    continue;
                }
            };
            // The worst per-frame turn of any bone, in degrees.
            let frames = (anim.duration * 60.0).ceil() as usize;
            let mut worst = (0.0f32, 0usize, 0usize);
            let mut last = anim.sample(0.0);
            for f in 1..=frames {
                let now = anim.sample(f as f32 / 60.0);
                for (b, (a, n)) in last.iter().zip(&now).enumerate() {
                    let angle = a.0.angle_between(n.0).to_degrees();
                    if angle > worst.0 {
                        worst = (angle, b, f);
                    }
                }
                last = now;
            }
            if only.is_some() {
                println!(
                    "{id} {name}: {:.2}s, {} bones (skeleton {}), worst turn {:.0} deg (bone {} at frame {})",
                    anim.duration,
                    anim.tracks.len(),
                    skeleton.bones.len(),
                    worst.0,
                    worst.1,
                    worst.2
                );
                let track = &anim.tracks[worst.1];
                for k in &track.rotations {
                    let (axis, angle) = k.rotation.to_axis_angle();
                    println!(
                        "  frame {:4}: {:6.1} deg about {axis:.2}",
                        k.frame,
                        angle.to_degrees()
                    );
                }
            }
            if worst.0 > 30.0 {
                bad.push((name.clone(), worst.0, worst.1, worst.2));
            }
        }
        bad.sort_by(|a, b| b.1.total_cmp(&a.1));
        println!(
            "{id}: {} of {} animations jump",
            bad.len(),
            files.animations.len()
        );
        for (name, deg, bone, frame) in bad.iter().take(40) {
            println!("  {name}: {deg:.0} deg, bone {bone}, frame {frame}");
        }
    }
}

/// `POSES=A@t,B@t`: each bone's turn between two animations' poses.
fn compare(files: &desa_viewer::source::CharacterFiles, tables: &KeyTables, spec: &str) {
    let pose = |s: &str| {
        let (name, t) = s.split_once('@').unwrap();
        let bytes = &files
            .animations
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .unwrap()
            .1;
        let a = Animation::parse(bytes, tables).unwrap();
        let t: f32 = if t == "end" {
            a.duration
        } else {
            t.parse().unwrap()
        };
        a.sample(t)
    };
    let (a, b) = spec.split_once(',').unwrap();
    let (a, b) = (pose(a), pose(b));
    for (i, (x, y)) in a.iter().zip(&b).enumerate() {
        println!(
            "  bone {i:2}: {:6.1} deg, translation {:.2} vs {:.2}",
            x.0.angle_between(y.0).to_degrees(),
            x.1,
            y.1
        );
    }
}

/// `LENGTHS=1`: each animation's bone lengths against the `default`
/// animation's (the model's pose): a wrong bone order stretches them.
fn lengths(files: &desa_viewer::source::CharacterFiles, tables: &KeyTables) {
    let parse = |name: &str| {
        files
            .animations
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .and_then(|(_, b)| Animation::parse(b, tables).ok())
    };
    let Some(rest) = parse("default").or_else(|| parse("StandIdle")) else {
        println!("no default animation");
        return;
    };
    let rest = rest.sample(0.0);
    let mut rows = Vec::new();
    for (name, bytes) in &files.animations {
        let Ok(anim) = Animation::parse(bytes, tables) else {
            continue;
        };
        let pose = anim.sample(anim.duration / 2.0);
        let off: f32 = pose
            .iter()
            .zip(&rest)
            .skip(1)
            .map(|(a, b)| (a.1.length() - b.1.length()).abs())
            .sum::<f32>()
            / (pose.len().max(2) - 1) as f32;
        rows.push((name.clone(), off));
    }
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (name, off) in rows.iter().take(25) {
        println!("  {name}: bone lengths off by {off:.2} on average");
    }
}
