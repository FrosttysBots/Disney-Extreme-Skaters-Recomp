//! `desa tex`: list and export `.img.ngc` / `.tex.ngc` files as PNGs.

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ngc_texture::{Image, ImgFile, TexDictionary};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Img,
    Tex,
}

const IMG_SUFFIX: &str = ".img.ngc";
const TEX_SUFFIX: &str = ".tex.ngc";

fn kind_of(path: &Path) -> Option<Kind> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    if name.ends_with(IMG_SUFFIX) {
        Some(Kind::Img)
    } else if name.ends_with(TEX_SUFFIX) {
        Some(Kind::Tex)
    } else {
        None
    }
}

pub fn ls(path: &Path) -> Result<()> {
    let data = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    match kind_of(path) {
        Some(Kind::Img) => {
            let img = ImgFile::parse(&data)?;
            println!(
                "{}x{} (stored {}x{}), {:?}",
                img.width, img.height, img.stored_width, img.stored_height, img.format
            );
        }
        Some(Kind::Tex) => {
            let dict = TexDictionary::parse(&data)?;
            println!(
                "{:>4}  {:<10} {:>9}  {:>6}  {:<13} flag",
                "#", "checksum", "size", "levels", "format"
            );
            for (i, t) in dict.textures.iter().enumerate() {
                println!(
                    "{i:>4}  {:08x}   {:>9}  {:>6}  {:<13} {}",
                    t.checksum,
                    format!("{}x{}", t.width, t.height),
                    t.level_count(),
                    format!("{:?}", t.format),
                    t.flag
                );
            }
            println!("\n{} textures", dict.textures.len());
        }
        None => bail!("expected a {IMG_SUFFIX} or {TEX_SUFFIX} file"),
    }
    Ok(())
}

/// Exports one file, or every texture file under a directory, as PNGs.
/// An image becomes `<name>.png`; a dictionary becomes a folder of
/// `<checksum>.png` files.
pub fn export(input: &Path, out: &Path) -> Result<()> {
    let (root, files) = if input.is_dir() {
        let mut files = Vec::new();
        collect_files(input, &mut files)?;
        files.retain(|p| kind_of(p).is_some());
        files.sort();
        (input, files)
    } else {
        let parent = input.parent().unwrap_or(Path::new(""));
        (parent, vec![input.to_path_buf()])
    };
    if files.is_empty() {
        bail!(
            "no {IMG_SUFFIX} or {TEX_SUFFIX} files in {}",
            input.display()
        );
    }

    let (mut written, mut skipped) = (0usize, Vec::new());
    for path in &files {
        let relative = path.strip_prefix(root).unwrap_or(path);
        match export_file(path, &out.join(relative)) {
            Ok(count) => written += count,
            Err(err) => skipped.push(format!("{}: {err:#}", relative.display())),
        }
    }

    for line in &skipped {
        eprintln!("skipped {line}");
    }
    println!(
        "Wrote {written} PNGs from {} files to {} ({} skipped)",
        files.len() - skipped.len(),
        out.display(),
        skipped.len()
    );
    Ok(())
}

/// `dest` is the output path mirroring the input, still ending in `.img.ngc` / `.tex.ngc`.
fn export_file(path: &Path, dest: &Path) -> Result<usize> {
    let data = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let kind = kind_of(path).context("not a texture file")?;
    let name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .context("bad file name")?;
    let stem = &name[..name.len() - IMG_SUFFIX.len()]; // both suffixes are 8 bytes
    let parent = dest.parent().unwrap_or(Path::new(""));

    match kind {
        Kind::Img => {
            let image = ImgFile::parse(&data)?.decode()?;
            fs::create_dir_all(parent)?;
            write_png(&parent.join(format!("{stem}.png")), &image)?;
            Ok(1)
        }
        Kind::Tex => {
            let dict = TexDictionary::parse(&data)?;
            let folder = parent.join(stem);
            fs::create_dir_all(&folder)?;
            for texture in &dict.textures {
                let image = texture.decode_level(0)?;
                write_png(
                    &folder.join(format!("{:08x}.png", texture.checksum)),
                    &image,
                )?;
            }
            Ok(dict.textures.len())
        }
    }
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("could not list {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

pub fn write_png(path: &Path, image: &Image) -> Result<()> {
    let file =
        File::create(path).with_context(|| format!("could not create {}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&image.rgba)?;
    Ok(())
}
