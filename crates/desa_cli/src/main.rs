use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use gc_disc::{Disc, NodeKind};

mod textures;

#[derive(Parser)]
#[command(
    name = "desa",
    version,
    about = "Tools for Disney's Extreme Skate Adventure (GameCube)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show the disc header, executable and file system summary
    Info { image: PathBuf },
    /// List every file and directory on the disc
    Ls {
        image: PathBuf,
        /// Only list paths starting with this prefix (case-insensitive)
        #[arg(long)]
        prefix: Option<String>,
    },
    /// List the sections of main.dol with their load addresses (for Ghidra)
    Dol { image: PathBuf },
    /// Extract the disc into <OUT>/sys and <OUT>/files, like Dolphin's "Extract Entire Disc"
    Extract {
        image: PathBuf,
        out: PathBuf,
        /// Only extract files whose path starts with this prefix (skips sys/)
        #[arg(long)]
        prefix: Option<String>,
    },
    /// Work with .prg archives, which hold almost all of the game's data
    Prg {
        #[command(subcommand)]
        command: PrgCommand,
    },
    /// Work with .img.ngc images and .tex.ngc texture dictionaries
    Tex {
        #[command(subcommand)]
        command: TexCommand,
    },
}

#[derive(Subcommand)]
enum PrgCommand {
    /// List the files inside a .prg archive
    Ls { archive: PathBuf },
    /// Unpack a .prg archive, or every .prg in a directory, into <OUT>/<archive name>/
    Unpack { input: PathBuf, out: PathBuf },
}

#[derive(Subcommand)]
enum TexCommand {
    /// Describe an image, or list the textures in a dictionary
    Ls { file: PathBuf },
    /// Export a texture file, or every one under a directory, as PNGs
    Export { input: PathBuf, out: PathBuf },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Info { image } => info(&image),
        Command::Ls { image, prefix } => ls(&image, prefix.as_deref()),
        Command::Dol { image } => dol(&image),
        Command::Extract { image, out, prefix } => extract(&image, &out, prefix.as_deref()),
        Command::Prg { command } => match command {
            PrgCommand::Ls { archive } => prg_ls(&archive),
            PrgCommand::Unpack { input, out } => prg_unpack(&input, &out),
        },
        Command::Tex { command } => match command {
            TexCommand::Ls { file } => textures::ls(&file),
            TexCommand::Export { input, out } => textures::export(&input, &out),
        },
    }
}

fn open(image: &Path) -> Result<Disc<std::io::BufReader<File>>> {
    Disc::open(image).with_context(|| format!("could not open {}", image.display()))
}

fn info(image: &Path) -> Result<()> {
    let disc = open(image)?;
    let header = disc.header();
    let dol = disc.dol();
    let fst = disc.fst();

    println!("Game ID:          {}", header.game_id());
    println!("Title:            {}", header.title);
    println!("Region:           {}", header.region());
    println!(
        "Disc / revision:  {} / {}",
        header.disc_number + 1,
        header.version
    );
    println!(
        "Audio streaming:  {}",
        if header.audio_streaming { "yes" } else { "no" }
    );
    println!();
    println!(
        "main.dol:         offset {:#x}, {} ({} sections), entry point {:#010x}",
        header.dol_offset,
        human_size(dol.file_size()),
        dol.sections.len(),
        dol.entry_point
    );
    println!(
        "FST:              offset {:#x}, {:#x} bytes",
        header.fst_offset, header.fst_size
    );
    println!(
        "Files:            {} files in {} directories, {} total",
        fst.files().count(),
        fst.directories().count(),
        human_size(fst.total_file_size())
    );
    Ok(())
}

fn ls(image: &Path, prefix: Option<&str>) -> Result<()> {
    let disc = open(image)?;
    for node in disc
        .fst()
        .entries()
        .filter(|n| matches_prefix(&n.path, prefix))
    {
        match node.kind {
            NodeKind::Directory { .. } => println!("{:>12}  {}/", "", node.path),
            NodeKind::File { size, .. } => println!("{size:>12}  {}", node.path),
        }
    }
    Ok(())
}

fn dol(image: &Path) -> Result<()> {
    let disc = open(image)?;
    let dol = disc.dol();
    println!(
        "{:<8} {:>10} {:>12} {:>12} {:>10}",
        "section", "file off", "address", "end", "size"
    );
    for s in &dol.sections {
        println!(
            "{:<8} {:>#10x} {:>#12x} {:>#12x} {:>#10x}",
            s.name(),
            s.file_offset,
            s.address,
            u64::from(s.address) + u64::from(s.size),
            s.size
        );
    }
    println!(
        "{:<8} {:>10} {:>#12x} {:>#12x} {:>#10x}",
        ".bss",
        "-",
        dol.bss_address,
        u64::from(dol.bss_address) + u64::from(dol.bss_size),
        dol.bss_size
    );
    println!("\nEntry point: {:#010x}", dol.entry_point);
    Ok(())
}

fn extract(image: &Path, out: &Path, prefix: Option<&str>) -> Result<()> {
    let mut disc = open(image)?;

    if prefix.is_none() {
        let sys_dir = out.join("sys");
        fs::create_dir_all(&sys_dir)?;
        for file in disc.system_files() {
            write_range(&mut disc, file.offset, file.size, &sys_dir.join(file.name))?;
        }
    }

    let entries: Vec<(String, NodeKind)> = disc
        .fst()
        .entries()
        .filter(|n| matches_prefix(&n.path, prefix))
        .map(|n| (n.path.clone(), n.kind))
        .collect();

    let files_dir = out.join("files");
    let (mut count, mut bytes) = (0u64, 0u64);
    for (path, kind) in entries {
        let dest = files_dir.join(safe_relative_path(&path)?);
        match kind {
            NodeKind::Directory { .. } => fs::create_dir_all(&dest)?,
            NodeKind::File { offset, size } => {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                write_range(&mut disc, offset.into(), size.into(), &dest)?;
                count += 1;
                bytes += u64::from(size);
            }
        }
    }

    println!(
        "Extracted {count} files ({}) to {}",
        human_size(bytes),
        out.display()
    );
    Ok(())
}

fn read_archive(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).with_context(|| format!("could not read {}", path.display()))
}

fn prg_ls(path: &Path) -> Result<()> {
    let data = read_archive(path)?;
    let archive = prg::Archive::parse(&data)
        .with_context(|| format!("could not parse {}", path.display()))?;
    println!("{:>10} {:>10}  name", "size", "packed");
    for entry in archive.entries() {
        let packed = entry
            .compressed_size
            .map_or_else(|| "stored".to_string(), |c| c.to_string());
        println!("{:>10} {packed:>10}  {}", entry.size, entry.path());
    }
    println!("\n{} files", archive.entries().len());
    Ok(())
}

fn prg_unpack(input: &Path, out: &Path) -> Result<()> {
    let archives = if input.is_dir() {
        let mut found: Vec<PathBuf> = fs::read_dir(input)
            .with_context(|| format!("could not list {}", input.display()))?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("prg"))
            })
            .collect();
        found.sort();
        found
    } else {
        vec![input.to_path_buf()]
    };
    if archives.is_empty() {
        bail!("no .prg files in {}", input.display());
    }

    let (mut files, mut bytes) = (0u64, 0u64);
    for path in &archives {
        let data = read_archive(path)?;
        let archive = prg::Archive::parse(&data)
            .with_context(|| format!("could not parse {}", path.display()))?;
        let stem = path
            .file_stem()
            .with_context(|| format!("no file name in {}", path.display()))?;
        let root = out.join(stem);

        for entry in archive.entries() {
            let dest = root.join(safe_relative_path(&entry.path())?);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            let contents = entry
                .contents()
                .with_context(|| format!("in {}", path.display()))?;
            fs::write(&dest, &contents)
                .with_context(|| format!("could not write {}", dest.display()))?;
            files += 1;
            bytes += u64::from(entry.size);
        }
        println!(
            "{:>6} files  {}",
            archive.entries().len(),
            path.file_name().unwrap_or_default().to_string_lossy()
        );
    }

    println!(
        "\nUnpacked {files} files ({}) from {} archives to {}",
        human_size(bytes),
        archives.len(),
        out.display()
    );
    Ok(())
}

fn write_range(
    disc: &mut Disc<impl std::io::Read + std::io::Seek>,
    offset: u64,
    size: u64,
    dest: &Path,
) -> Result<()> {
    let file =
        File::create(dest).with_context(|| format!("could not create {}", dest.display()))?;
    let mut writer = BufWriter::new(file);
    disc.copy_range(offset, size, &mut writer)
        .with_context(|| format!("could not extract {}", dest.display()))?;
    writer.flush()?;
    Ok(())
}

fn matches_prefix(path: &str, prefix: Option<&str>) -> bool {
    prefix.is_none_or(|p| {
        let p = p.replace('\\', "/");
        path.to_ascii_lowercase()
            .starts_with(&p.trim_start_matches('/').to_ascii_lowercase())
    })
}

/// Turns a disc path into a relative filesystem path, refusing anything
/// (like `..` or a drive letter) that could escape the output directory.
fn safe_relative_path(path: &str) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for part in path.split('/') {
        let mut components = Path::new(part).components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(name)), None) => out.push(name),
            // Some archive names start with `./`, which is harmless.
            (Some(Component::CurDir), None) => {}
            _ => bail!("refusing to extract suspicious path `{path}`"),
        }
    }
    if out.as_os_str().is_empty() {
        bail!("refusing to extract empty path `{path}`");
    }
    Ok(out)
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_paths_are_accepted() {
        assert_eq!(
            safe_relative_path("levels/park.pre").unwrap(),
            Path::new("levels").join("park.pre")
        );
        assert_eq!(
            safe_relative_path("./anims/Buzz/default.ska.ngc").unwrap(),
            Path::new("anims").join("Buzz").join("default.ska.ngc")
        );
    }

    #[test]
    fn escaping_paths_are_rejected() {
        for bad in [
            "../evil",
            "levels/../../evil",
            "C:",
            "a\\..\\b",
            "",
            ".",
            "./..",
            "levels//park.pre",
        ] {
            assert!(
                safe_relative_path(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn prefix_matching_ignores_case_and_slashes() {
        assert!(matches_prefix("Levels/Park.pre", Some("/levels")));
        assert!(matches_prefix("anything", None));
        assert!(!matches_prefix("audio/x.dsp", Some("levels")));
    }
}
