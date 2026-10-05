//! Reading levels straight from the game's `.prg` archives, either inside
//! the disc image or in a folder of extracted archives.
//!
//! Each level `X` is spread over three archives: `XScn.prg` (the scene,
//! its sky and their textures), `Xcol.prg` (collision) and `X.prg` (among
//! other things, the level script with the node array).
//!
//! Each playable character `X` has `anims_X.prg`: its model, board and
//! animations. Skeletons and the animation key tables are shared, in
//! `skeletons.prg`.

use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use gc_disc::Disc;
use prg::Archive;

pub enum GameData {
    Disc {
        path: PathBuf,
        disc: Box<Disc<BufReader<File>>>,
    },
    /// A folder holding the `.prg` files (the disc's `pre` folder).
    Folder(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LevelInfo {
    /// The archive name, e.g. `ToyStory_Bedroom`.
    pub id: String,
    /// A readable name, e.g. `Toy Story Bedroom`.
    pub title: String,
}

/// The raw files for one level. Only the scene is required.
pub struct LevelFiles {
    pub scene: Vec<u8>,
    pub textures: Option<Vec<u8>>,
    pub sky: Option<(Vec<u8>, Option<Vec<u8>>)>,
    pub collision: Option<Vec<u8>>,
    pub nodes: Option<Vec<u8>>,
    /// Camera paths (cutscenes, goal intros, fly-throughs) by name, from
    /// the level's own archive.
    pub cameras: Vec<(String, Vec<u8>)>,
}

/// The raw files for one playable character.
pub struct CharacterFiles {
    pub skin: Vec<u8>,
    pub textures: Option<Vec<u8>>,
    /// The skateboard, skinned to the character's skeleton.
    pub board: Option<(Vec<u8>, Option<Vec<u8>>)>,
    pub skeleton: Vec<u8>,
    /// `standardkeyq.bin` and `standardkeyt.bin`.
    pub key_tables: (Vec<u8>, Vec<u8>),
    /// Animation names (file names without `.ska.ngc`) and contents.
    pub animations: Vec<(String, Vec<u8>)>,
}

/// Reads one file from an archive by a test on its lowercased path.
fn entry(archive: &Archive, pred: &dyn Fn(&str) -> bool) -> Result<Option<Vec<u8>>> {
    match archive
        .entries()
        .iter()
        .find(|e| pred(&e.path().to_ascii_lowercase()))
    {
        Some(e) => Ok(Some(e.contents()?.into_owned())),
        None => Ok(None),
    }
}

impl GameData {
    /// Opens a disc image (`.iso` / `.gcm`), or a folder that contains the
    /// `.prg` archives somewhere below it (`pre`, `files/pre`, ...).
    pub fn open(path: &Path) -> Result<Self> {
        if path.is_file() {
            let disc =
                Disc::open(path).with_context(|| format!("could not open {}", path.display()))?;
            return Ok(GameData::Disc {
                path: path.to_path_buf(),
                disc: Box::new(disc),
            });
        }
        match find_prg_folder(path, 3) {
            Some(folder) => Ok(GameData::Folder(folder)),
            None => bail!("no level archives (*Scn.prg) found in {}", path.display()),
        }
    }

    /// The disc image or folder this data comes from.
    pub fn path(&self) -> &Path {
        match self {
            GameData::Disc { path, .. } => path,
            GameData::Folder(path) => path,
        }
    }

    fn archive_names(&self) -> Vec<String> {
        match self {
            GameData::Disc { disc, .. } => disc
                .fst()
                .files()
                .filter(|n| n.path.to_ascii_lowercase().starts_with("pre/"))
                .map(|n| n.name.clone())
                .collect(),
            GameData::Folder(dir) => fs::read_dir(dir)
                .map(|entries| {
                    entries
                        .filter_map(|e| e.ok()?.file_name().into_string().ok())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Reads an archive by file name, ignoring case. `None` if it isn't there.
    fn read_archive(&mut self, name: &str) -> Result<Option<Vec<u8>>> {
        let Some(actual) = self
            .archive_names()
            .into_iter()
            .find(|n| n.eq_ignore_ascii_case(name))
        else {
            return Ok(None);
        };
        Ok(Some(match self {
            GameData::Disc { disc, .. } => disc.read_file(&format!("pre/{actual}"))?,
            GameData::Folder(dir) => fs::read(dir.join(&actual))?,
        }))
    }

    /// Every level on the disc, sorted by title.
    pub fn levels(&self) -> Vec<LevelInfo> {
        let mut levels: Vec<LevelInfo> = self
            .archive_names()
            .iter()
            .filter_map(|name| {
                let lower = name.to_ascii_lowercase();
                lower
                    .strip_suffix("scn.prg")
                    .map(|stem| name[..stem.len()].to_string())
            })
            .map(|id| LevelInfo {
                title: title(&id),
                id,
            })
            .collect();
        levels.sort_by(|a, b| a.title.cmp(&b.title));
        levels
    }

    pub fn load_level(&mut self, id: &str) -> Result<LevelFiles> {
        let scn_data = self
            .read_archive(&format!("{id}Scn.prg"))?
            .with_context(|| format!("{id}Scn.prg is missing"))?;
        let scn =
            Archive::parse(&scn_data).with_context(|| format!("could not read {id}Scn.prg"))?;

        let is_scene = |p: &str| p.ends_with(".scn.ngc") && !p.ends_with("_sky.scn.ngc");
        let scene_entry = scn
            .entries()
            .iter()
            .find(|e| is_scene(&e.path().to_ascii_lowercase()))
            .with_context(|| format!("{id}Scn.prg has no level scene"))?;
        let scene_path = scene_entry.path().to_ascii_lowercase();
        let scene = scene_entry.contents()?.into_owned();
        // e.g. levels/beach/beach
        let base = scene_path
            .strip_suffix(".scn.ngc")
            .unwrap_or(&scene_path)
            .to_string();
        let name = base.rsplit('/').next().unwrap_or(&base).to_string();

        let textures = entry(&scn, &|p| p == format!("{base}.tex.ngc"))?;
        let sky_scene = entry(&scn, &|p| p.ends_with("_sky.scn.ngc"))?;
        let sky = match sky_scene {
            Some(scene) => Some((scene, entry(&scn, &|p| p.ends_with("_sky.tex.ngc"))?)),
            None => None,
        };

        let collision = match self.read_archive(&format!("{id}col.prg"))? {
            Some(data) => {
                let archive =
                    Archive::parse(&data).with_context(|| format!("could not read {id}col.prg"))?;
                entry(&archive, &|p| p.ends_with(&format!("/{name}.col.ngc")))?
            }
            None => None,
        };
        let (nodes, cameras) = match self.read_archive(&format!("{id}.prg"))? {
            Some(data) => {
                let archive =
                    Archive::parse(&data).with_context(|| format!("could not read {id}.prg"))?;
                let nodes = entry(&archive, &|p| p.ends_with(&format!("/{name}.qb")))?;
                let mut cameras = Vec::new();
                for e in archive.entries() {
                    let path = e.path();
                    let file_name = path.rsplit(['/', '\\']).next().unwrap_or(&path);
                    let Some(stem) = file_name
                        .to_ascii_lowercase()
                        .strip_suffix(".ska.ngc")
                        .map(str::len)
                    else {
                        continue;
                    };
                    let contents = e.contents()?;
                    if ngc_anim::is_camera_path(&contents) {
                        // Some names end in a space.
                        cameras.push((file_name[..stem].trim().to_string(), contents.into_owned()));
                    }
                }
                cameras.sort_by_key(|c| c.0.to_ascii_lowercase());
                (nodes, cameras)
            }
            None => (None, Vec::new()),
        };

        Ok(LevelFiles {
            scene,
            textures,
            sky,
            collision,
            nodes,
            cameras,
        })
    }

    /// Every playable character, sorted by title. The id is the archive
    /// name without `anims_`, e.g. `jessie`.
    pub fn characters(&self) -> Vec<LevelInfo> {
        let mut characters: Vec<LevelInfo> = self
            .archive_names()
            .iter()
            .filter_map(|name| {
                let lower = name.to_ascii_lowercase();
                let id = lower.strip_prefix("anims_")?.strip_suffix(".prg")?;
                // The created skater is assembled from parts in
                // skaterparts.prg, so its archive has animations but no model.
                (id != "kid").then(|| name[6..6 + id.len()].to_string())
            })
            .map(|id| LevelInfo {
                title: title(&id),
                id,
            })
            .collect();
        characters.sort_by(|a, b| a.title.cmp(&b.title));
        characters
    }

    pub fn load_character(&mut self, id: &str) -> Result<CharacterFiles> {
        let name = id.to_ascii_lowercase();
        let data = self
            .read_archive(&format!("anims_{id}.prg"))?
            .with_context(|| format!("anims_{id}.prg is missing"))?;
        let archive =
            Archive::parse(&data).with_context(|| format!("could not read anims_{id}.prg"))?;
        let file = |suffix: &str| entry(&archive, &|p| p.ends_with(&format!("/{name}{suffix}")));
        let skin = file(".skin.ngc")?.with_context(|| format!("anims_{id}.prg has no model"))?;
        let textures = file(".tex.ngc")?;
        let board = match file("board.skin.ngc")? {
            Some(board) => Some((board, file("board.tex.ngc")?)),
            None => None,
        };
        let mut animations = Vec::new();
        for e in archive.entries() {
            let path = e.path();
            let file_name = path.rsplit(['/', '\\']).next().unwrap_or(&path);
            let lower = file_name.to_ascii_lowercase();
            if let Some(stem) = lower.strip_suffix(".ska.ngc") {
                animations.push((
                    file_name[..stem.len()].to_string(),
                    e.contents()?.into_owned(),
                ));
            }
        }
        animations.sort_by_key(|a| a.0.to_ascii_lowercase());

        let data = self
            .read_archive("skeletons.prg")?
            .context("skeletons.prg is missing")?;
        let skeletons = Archive::parse(&data).context("could not read skeletons.prg")?;
        let skeleton = entry(&skeletons, &|p| p.ends_with(&format!("/{name}.ske")))?
            .with_context(|| format!("skeletons.prg has no skeleton for {id}"))?;
        let table = |file: &str| {
            entry(&skeletons, &|p| p.ends_with(file))?
                .with_context(|| format!("skeletons.prg has no {file}"))
        };
        let key_tables = (table("standardkeyq.bin")?, table("standardkeyt.bin")?);

        Ok(CharacterFiles {
            skin,
            textures,
            board,
            skeleton,
            key_tables,
            animations,
        })
    }
}

/// Finds a folder (at most `depth` levels below `dir`) with `*Scn.prg` files.
fn find_prg_folder(dir: &Path, depth: u32) -> Option<PathBuf> {
    let entries: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    let has_levels = entries.iter().any(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.to_ascii_lowercase().ends_with("scn.prg"))
    });
    if has_levels {
        return Some(dir.to_path_buf());
    }
    if depth == 0 {
        return None;
    }
    entries
        .iter()
        .filter(|p| p.is_dir())
        .find_map(|p| find_prg_folder(p, depth - 1))
}

/// A readable level name from its archive name.
pub fn title(id: &str) -> String {
    match id.to_ascii_lowercase().as_str() {
        "hub" => return "Hub".into(),
        "zurghome" => return "Zurg's Home".into(),
        _ => {}
    }
    let mut out = String::new();
    let mut previous_lower = false;
    for c in id.chars() {
        if c == '_' {
            out.push(' ');
            previous_lower = false;
            continue;
        }
        if c.is_ascii_uppercase() && previous_lower {
            out.push(' ');
        }
        previous_lower = c.is_ascii_lowercase();
        out.push(if out.is_empty() {
            c.to_ascii_uppercase()
        } else {
            c
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_readable() {
        assert_eq!(title("ToyStory_Bedroom"), "Toy Story Bedroom");
        assert_eq!(title("PrideRock"), "Pride Rock");
        assert_eq!(title("beach"), "Beach");
        assert_eq!(title("HUB"), "Hub");
    }
}
