//! `desa col`: inspect and export `.col.ngc` collision files.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result};
use ngc_collision::{Collision, face_flags};

fn load(path: &Path) -> Result<Collision> {
    let data = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    Collision::parse(&data).with_context(|| format!("could not parse {}", path.display()))
}

pub fn info(path: &Path) -> Result<()> {
    let col = load(path)?;
    let skipped: usize = col.objects.iter().map(|o| o.skipped_faces).sum();
    println!(
        "{} objects, {} vertices, {} faces, {} BSP bytes",
        col.objects.len(),
        col.vertex_count(),
        col.face_count(),
        col.bsp.len()
    );
    if col.repaired_fields > 0 || skipped > 0 {
        println!(
            "{} corrupted counts repaired, {skipped} placeholder faces skipped",
            col.repaired_fields
        );
    }

    let faces = || col.objects.iter().flat_map(|o| &o.faces);
    let total = col.face_count().max(1) as f32;
    let names: [(u16, &str); 9] = [
        (face_flags::SKATABLE, "skatable"),
        (face_flags::NOT_SKATABLE, "not skatable"),
        (face_flags::WALL_RIDABLE, "wall-ridable"),
        (face_flags::VERT, "vert"),
        (face_flags::NON_COLLIDABLE, "non-collidable"),
        (face_flags::TRIGGER, "trigger"),
        (face_flags::CAMERA_COLLIDABLE, "camera-collidable?"),
        (face_flags::NO_SKATER_SHADOW, "no skater shadow?"),
        (face_flags::INVISIBLE, "invisible?"),
    ];
    println!("\nface flags (share of faces):");
    for bit in 0..16 {
        let mask = 1u16 << bit;
        let count = faces().filter(|f| f.flags & mask != 0).count();
        if count > 0 {
            let name = names
                .iter()
                .find(|(m, _)| *m == mask)
                .map_or("", |(_, n)| n);
            println!(
                "  {mask:#06x} {:>6.1}%  {name}",
                count as f32 / total * 100.0
            );
        }
    }

    let mut terrains: BTreeMap<u16, usize> = BTreeMap::new();
    for face in faces() {
        *terrains.entry(face.terrain).or_default() += 1;
    }
    println!("\nterrain types (faces): {terrains:?}");
    Ok(())
}

/// Writes an OBJ with one object per collision object, and faces grouped
/// into materials by their most notable flag (see `category`).
pub fn export(path: &Path, out: &Path) -> Result<()> {
    let col = load(path)?;
    let name = out
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("collision");
    let mtl_name = format!("{name}.mtl");
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut mtl = BufWriter::new(File::create(out.with_file_name(&mtl_name))?);
    for (category, color) in CATEGORIES {
        writeln!(
            mtl,
            "newmtl {category}\nKd {} {} {}",
            color[0], color[1], color[2]
        )?;
    }
    mtl.flush()?;

    let mut obj = BufWriter::new(File::create(out)?);
    writeln!(obj, "# Collision exported by desa\nmtllib {mtl_name}")?;
    let mut base = 1;
    for object in &col.objects {
        if object.faces.is_empty() {
            base += object.vertices.len();
            continue;
        }
        writeln!(obj, "o {:08x}", object.checksum)?;
        for v in &object.vertices {
            writeln!(obj, "v {} {} {}", v[0], v[1], v[2])?;
        }
        let mut current = "";
        for face in &object.faces {
            let category = category(face.flags);
            if category != current {
                writeln!(obj, "usemtl {category}")?;
                current = category;
            }
            let [a, b, c] = face.indices.map(|i| base + usize::from(i));
            writeln!(obj, "f {a} {b} {c}")?;
        }
        base += object.vertices.len();
    }
    obj.flush()?;
    println!(
        "Wrote {} ({} faces in {} objects)",
        out.display(),
        col.face_count(),
        col.objects.iter().filter(|o| !o.faces.is_empty()).count()
    );
    Ok(())
}

/// Material names and colors, matching the viewer's overlay.
const CATEGORIES: [(&str, [f32; 3]); 5] = [
    ("trigger", [1.0, 0.86, 0.16]),
    ("vert", [0.92, 0.24, 0.2]),
    ("wallride", [0.24, 0.47, 1.0]),
    ("not_skatable", [0.67, 0.31, 0.86]),
    ("solid", [0.78, 0.78, 0.78]),
];

fn category(flags: u16) -> &'static str {
    if flags & (face_flags::TRIGGER | face_flags::NON_COLLIDABLE) != 0 {
        "trigger"
    } else if flags & face_flags::VERT != 0 {
        "vert"
    } else if flags & face_flags::WALL_RIDABLE != 0 {
        "wallride"
    } else if flags & face_flags::NOT_SKATABLE != 0 {
        "not_skatable"
    } else {
        "solid"
    }
}
