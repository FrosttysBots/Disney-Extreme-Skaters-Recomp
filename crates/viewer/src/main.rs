//! `desa-viewer`: fly around a level from the game's files.

mod app;
mod camera;
mod level;
mod renderer;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Parser;
use glam::Vec3;

use camera::FlyCamera;
use level::Level;

#[derive(Parser)]
#[command(
    name = "desa-viewer",
    about = "Fly around a Disney's Extreme Skate Adventure level",
    after_help = app::CONTROLS
)]
struct Args {
    /// The level scene, e.g. extracted/unpacked/beachScn/Levels/beach/beach.scn.ngc
    scene: PathBuf,
    /// Texture dictionary (default: the .tex.ngc with the same name next to the scene)
    #[arg(long)]
    textures: Option<PathBuf>,
    /// Sky scene (default: <name>_sky/<name>_sky.scn.ngc beside the level's folder)
    #[arg(long)]
    sky: Option<PathBuf>,
    /// Don't draw a sky
    #[arg(long, conflicts_with = "sky")]
    no_sky: bool,
    /// Start position as x,y,z,yaw,pitch (degrees); press C in the viewer to print one
    #[arg(long, value_parser = FlyCamera::parse, allow_hyphen_values = true)]
    camera: Option<FlyCamera>,
    /// Render one frame to this PNG instead of opening a window
    #[arg(long)]
    screenshot: Option<PathBuf>,
    /// Screenshot size as WIDTHxHEIGHT
    #[arg(long, default_value = "1280x720", requires = "screenshot")]
    size: String,
    /// Animation time in seconds for --screenshot (scrolling textures)
    #[arg(long, default_value_t = 0.0, requires = "screenshot")]
    time: f32,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let name = args
        .scene
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.split('.').next().unwrap_or(n).to_string())
        .unwrap_or_default();

    let world = Level::load(&args.scene, args.textures.as_deref())?;
    let sky_path = if args.no_sky {
        None
    } else {
        args.sky.clone().or_else(|| level::find_sky(&args.scene))
    };
    let sky = match &sky_path {
        Some(path) => Some(Level::load(path, None)?),
        None => None,
    };
    println!(
        "{name}: {} vertices, {} triangles, {} materials, {} textures{}",
        world.vertices.len(),
        world.indices.len() / 3,
        world.batches.len(),
        world.textures.len() - 1,
        if sky.is_some() { ", with sky" } else { "" }
    );

    let (center, radius) = world.focus;
    let start = args.camera.unwrap_or_else(|| {
        FlyCamera::looking_at(center + Vec3::new(0.0, radius * 0.5, radius * 1.2), center)
    });

    match &args.screenshot {
        Some(out) => {
            let (width, height) = parse_size(&args.size)?;
            renderer::screenshot(&world, sky.as_ref(), &start, width, height, args.time, out)?;
            println!(
                "Wrote {} ({width}x{height}, camera {})",
                out.display(),
                start.describe()
            );
            Ok(())
        }
        None => {
            println!("{}", app::CONTROLS);
            app::run(name, world, sky, start)
        }
    }
}

fn parse_size(text: &str) -> Result<(u32, u32)> {
    let (w, h) = text
        .split_once('x')
        .context("size must look like 1280x720")?;
    let (w, h): (u32, u32) = (w.parse()?, h.parse()?);
    if !(16..=8192).contains(&w) || !(16..=8192).contains(&h) {
        bail!("size must be between 16 and 8192 pixels on each side");
    }
    Ok((w, h))
}
