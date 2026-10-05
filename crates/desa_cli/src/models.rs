//! `desa model`: inspect and export `.mdl.ngc` / `.scn.ngc` files.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ngc_model::{Scene, pass_flags};
use ngc_texture::TexDictionary;

use crate::textures::write_png;

const SUFFIXES: [&str; 2] = [".mdl.ngc", ".scn.ngc"];

/// The file name without its two-part extension, e.g. `beach` for `beach.scn.ngc`.
fn stem(path: &Path) -> Result<&str> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("bad file name")?;
    let lower = name.to_ascii_lowercase();
    SUFFIXES
        .iter()
        .find(|s| lower.ends_with(*s))
        .map(|s| &name[..name.len() - s.len()])
        .with_context(|| format!("expected a {} or {} file", SUFFIXES[0], SUFFIXES[1]))
}

fn load(path: &Path) -> Result<Scene> {
    let data = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    Scene::parse(&data).with_context(|| format!("could not parse {}", path.display()))
}

pub fn info(path: &Path) -> Result<()> {
    let scene = load(path)?;
    let meshes: usize = scene.sectors.iter().map(|s| s.meshes.len()).sum();
    let vertices: usize = scene.sectors.iter().map(|s| s.positions.len()).sum();
    let triangles: usize = scene
        .sectors
        .iter()
        .flat_map(|s| &s.meshes)
        .map(|m| m.triangles().count())
        .sum();
    println!(
        "{} materials, {} sectors, {meshes} meshes, {vertices} vertices, {triangles} triangles",
        scene.materials.len(),
        scene.sectors.len()
    );

    let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in scene.sectors.iter().flat_map(|s| &s.positions) {
        for i in 0..3 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    if vertices > 0 {
        println!("bounds: {min:?} to {max:?}");
    }
    Ok(())
}

/// Writes `<out>/<name>.obj`, `<name>.mtl` and the PNGs it uses in
/// `<out>/textures/`. Textures come from `textures`, or by default from
/// `<name>.tex.ngc` next to the input.
pub fn export(path: &Path, out: &Path, textures: Option<&Path>) -> Result<()> {
    let scene = load(path)?;
    let name = stem(path)?;

    let tex_path = match textures {
        Some(p) => Some(p.to_path_buf()),
        None => find_sibling_textures(path, name),
    };
    let tex_data = match &tex_path {
        Some(p) => Some(fs::read(p).with_context(|| format!("could not read {}", p.display()))?),
        None => None,
    };
    let dictionary = match &tex_data {
        Some(data) => Some(TexDictionary::parse(data).context("could not parse the texture file")?),
        None => None,
    };

    fs::create_dir_all(out.join("textures"))?;

    // Materials: write the PNGs each one needs, then the .mtl.
    let used: BTreeSet<u32> = scene
        .sectors
        .iter()
        .flat_map(|s| &s.meshes)
        .map(|m| m.material)
        .collect();
    let mut mtl = BufWriter::new(File::create(out.join(format!("{name}.mtl")))?);
    let (mut written, mut missing) = (0, BTreeSet::new());
    for &checksum in &used {
        writeln!(mtl, "newmtl m_{checksum:08x}\nKd 1 1 1")?;
        let Some(pass) = scene.material(checksum).and_then(|m| m.passes.first()) else {
            continue;
        };
        if pass.texture == 0 {
            continue;
        }
        let Some(texture) = dictionary
            .as_ref()
            .and_then(|d| d.textures.iter().find(|t| t.checksum == pass.texture))
        else {
            missing.insert(pass.texture);
            continue;
        };
        let file = format!("textures/{:08x}.png", pass.texture);
        let png = out.join(&file);
        if !png.exists() {
            write_png(&png, &texture.decode_level(0)?)?;
            written += 1;
        }
        writeln!(mtl, "map_Kd {file}")?;
        let material = scene.material(checksum).unwrap();
        if pass.flags & pass_flags::TRANSPARENT != 0 || material.alpha_cutoff > 0 {
            writeln!(mtl, "map_d {file}")?;
        }
    }
    mtl.flush()?;

    let (vertices, triangles) = write_obj(&scene, name, &out.join(format!("{name}.obj")))?;

    println!(
        "Wrote {name}.obj ({vertices} vertices, {triangles} triangles, {} materials) and {written} textures to {}",
        used.len(),
        out.display()
    );
    match &tex_path {
        Some(p) => {
            if !missing.is_empty() {
                println!("{} textures weren't in {}", missing.len(), p.display());
            }
        }
        None => println!("No {name}.tex.ngc found next to the model; exported without textures"),
    }
    Ok(())
}

fn find_sibling_textures(path: &Path, name: &str) -> Option<PathBuf> {
    let wanted = format!("{name}.tex.ngc").to_ascii_lowercase();
    fs::read_dir(path.parent()?)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.to_ascii_lowercase() == wanted)
        })
}

/// Returns (vertex count, triangle count).
fn write_obj(scene: &Scene, name: &str, path: &Path) -> Result<(usize, usize)> {
    let mut obj = BufWriter::new(File::create(path)?);
    writeln!(obj, "# Exported by desa from {name}\nmtllib {name}.mtl")?;

    // OBJ indices are 1-based and global per attribute.
    let (mut v_base, mut vt_base, mut vn_base, mut triangles) = (1usize, 1usize, 1usize, 0usize);
    for sector in &scene.sectors {
        writeln!(obj, "o {:08x}", sector.checksum)?;
        for (i, p) in sector.positions.iter().enumerate() {
            match sector.colors.get(i) {
                // 0x80 is full brightness; brighter values are clamped.
                Some(c) => {
                    let ch = |v: u8| (f32::from(v) / 128.0).min(1.0);
                    writeln!(
                        obj,
                        "v {} {} {} {:.4} {:.4} {:.4}",
                        p[0],
                        p[1],
                        p[2],
                        ch(c[0]),
                        ch(c[1]),
                        ch(c[2])
                    )?
                }
                None => writeln!(obj, "v {} {} {}", p[0], p[1], p[2])?,
            }
        }
        let has_uv = sector.uv_sets > 0;
        if has_uv {
            for i in 0..sector.positions.len() {
                let [u, v] = sector.uv(i, 0).unwrap();
                // Textures are stored bottom row first, which already
                // matches OBJ's bottom-left UV origin.
                writeln!(obj, "vt {u} {v}")?;
            }
        }
        for n in &sector.normals {
            writeln!(obj, "vn {} {} {}", n[0], n[1], n[2])?;
        }
        let has_normals = !sector.normals.is_empty();

        for mesh in &sector.meshes {
            writeln!(obj, "usemtl m_{:08x}", mesh.material)?;
            for tri in mesh.triangles() {
                write!(obj, "f")?;
                for &i in &tri {
                    let i = usize::from(i);
                    match (has_uv, has_normals) {
                        (true, true) => {
                            write!(obj, " {}/{}/{}", v_base + i, vt_base + i, vn_base + i)?
                        }
                        (true, false) => write!(obj, " {}/{}", v_base + i, vt_base + i)?,
                        (false, true) => write!(obj, " {}//{}", v_base + i, vn_base + i)?,
                        (false, false) => write!(obj, " {}", v_base + i)?,
                    }
                }
                writeln!(obj)?;
                triangles += 1;
            }
        }

        let count = sector.positions.len();
        v_base += count;
        if has_uv {
            vt_base += count;
        }
        if has_normals {
            vn_base += count;
        }
    }
    obj.flush()?;
    if v_base == 1 {
        bail!("{name} has no geometry");
    }
    Ok((v_base - 1, triangles))
}
