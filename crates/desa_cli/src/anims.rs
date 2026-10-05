//! `desa anim`: inspect animations and export posed characters.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ngc_anim::{Animation, KeyTables, Skeleton, pose};
use ngc_model::Scene;

/// Finds `<ancestor>/skeletons/<rest...>` above `start`, ignoring case.
fn find_above(start: &Path, rest: &[&str]) -> Option<PathBuf> {
    start.ancestors().find_map(|dir| {
        let mut path = dir.to_path_buf();
        for part in std::iter::once(&"skeletons").chain(rest) {
            path = find_ignoring_case(&path, part)?;
        }
        Some(path)
    })
}

fn find_ignoring_case(dir: &Path, name: &str) -> Option<PathBuf> {
    let wanted = name.to_ascii_lowercase();
    fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.to_ascii_lowercase() == wanted)
        })
}

fn read(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).with_context(|| format!("could not read {}", path.display()))
}

/// The key tables from `<dir>`, or from `skeletons/anims/` above `near`.
fn load_tables(dir: Option<&Path>, near: &Path) -> Result<KeyTables> {
    let (q, t) = match dir {
        Some(d) => (d.join("standardkeyq.bin"), d.join("standardkeyt.bin")),
        None => (
            find_above(near, &["anims", "standardkeyq.bin"])
                .context("couldn't find skeletons/anims/standardkeyq.bin; pass --tables")?,
            find_above(near, &["anims", "standardkeyt.bin"])
                .context("couldn't find skeletons/anims/standardkeyt.bin; pass --tables")?,
        ),
    };
    Ok(KeyTables::parse(&read(&q)?, &read(&t)?)?)
}

fn load_animation(path: &Path, tables: &KeyTables) -> Result<Animation> {
    Animation::parse(&read(path)?, tables)
        .with_context(|| format!("could not parse {}", path.display()))
}

pub fn info(path: &Path, tables: Option<&Path>) -> Result<()> {
    let tables = load_tables(tables, path)?;
    let anim = load_animation(path, &tables)?;
    let rotation_keys: usize = anim.tracks.iter().map(|t| t.rotations.len()).sum();
    let translation_keys: usize = anim.tracks.iter().map(|t| t.translations.len()).sum();
    println!(
        "{:.2} seconds ({} frames), {} bones, {rotation_keys} rotation keys, {translation_keys} translation keys",
        anim.duration,
        (anim.duration * 60.0).round(),
        anim.tracks.len()
    );
    Ok(())
}

/// Exports `skin` posed by `anim` at `time` seconds. The skeleton defaults
/// to `skeletons/skeletons/<name>.ske` and the rest pose to
/// `default.ska.ngc` beside the animation.
pub struct PoseOptions<'a> {
    pub skin: &'a Path,
    pub anim: &'a Path,
    pub time: f32,
    pub out: &'a Path,
    pub skeleton: Option<&'a Path>,
    pub rest: Option<&'a Path>,
    pub tables: Option<&'a Path>,
}

pub fn pose(options: &PoseOptions) -> Result<()> {
    let tables = load_tables(options.tables, options.anim)?;
    let name = options
        .skin
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.split('.').next())
        .context("bad skin file name")?;

    let skeleton_path =
        match options.skeleton {
            Some(p) => p.to_path_buf(),
            None => find_above(options.skin, &["skeletons", &format!("{name}.ske")]).with_context(
                || format!("couldn't find skeletons/skeletons/{name}.ske; pass --skeleton"),
            )?,
        };
    let skeleton = Skeleton::parse(&read(&skeleton_path)?)
        .with_context(|| format!("could not parse {}", skeleton_path.display()))?;

    let rest_path = match options.rest {
        Some(p) => p.to_path_buf(),
        None => options
            .anim
            .parent()
            .and_then(|dir| find_ignoring_case(dir, "default.ska.ngc"))
            .context("couldn't find default.ska.ngc beside the animation; pass --rest")?,
    };
    let rest_anim = load_animation(&rest_path, &tables)?;
    let anim = load_animation(options.anim, &tables)?;
    for (what, a) in [("animation", &anim), ("rest animation", &rest_anim)] {
        if a.tracks.len() != skeleton.bones.len() {
            bail!(
                "the {what} has {} bones but the skeleton has {}",
                a.tracks.len(),
                skeleton.bones.len()
            );
        }
    }

    let rest = pose::model_space(&skeleton, &rest_anim.sample(0.0));
    let posed = pose::model_space(&skeleton, &anim.sample(options.time));
    let matrices = pose::skinning(&rest, &posed);

    let mut scene = Scene::parse(&read(options.skin)?)
        .with_context(|| format!("could not parse {}", options.skin.display()))?;
    let mut skinned = 0;
    for sector in &mut scene.sectors {
        let Some(skin) = &sector.skin else { continue };
        for (i, position) in sector.positions.iter_mut().enumerate() {
            let p = pose::skin_point(
                &matrices,
                skin.bones[i],
                skin.weights[i],
                (*position).into(),
            );
            *position = p.into();
        }
        skinned += sector.positions.len();
    }
    if skinned == 0 {
        bail!("{} has no skinned vertices", options.skin.display());
    }
    println!(
        "Posed {skinned} vertices with {} bones at {:.2}s of {:.2}s",
        skeleton.bones.len(),
        options.time.min(anim.duration),
        anim.duration
    );
    crate::models::export_scene(&scene, options.skin, options.out, None)
}
