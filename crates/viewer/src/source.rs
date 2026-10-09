//! Reading levels straight from the game's `.prg` archives, either inside
//! the disc image or in a folder of extracted archives.
//!
//! Each level `X` is spread over three archives: `XScn.prg` (the scene,
//! its sky and their textures), `Xcol.prg` (collision) and `X.prg` (among
//! other things, the level script with the node array).
//!
//! Models placed by the node array (`Model = "gameobjects\\milk\\milk.mdl"`)
//! are `Models/<that path>.ngc` in `X.prg`, or in `XPed.prg` for
//! pedestrians. A pedestrian's `AnimName` names a script in
//! `scripts/allanims.qb` (in `qb.prg`) that loads its animations, each
//! with a role such as `Ped_Guide_Idle1`; most are `anims/Ped_<name>/` in
//! `X.prg`, but some borrow another's (birds use Zazu's) or a playable
//! character's (`anims/Buzz/` in `anims_buzz.prg`).
//!
//! Each playable character `X` has `anims_X.prg`: its model, board and
//! animations. Skeletons and the animation key tables are shared, in
//! `skeletons.prg`.

use std::collections::HashMap;
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
    /// Object and pedestrian models and their textures, by their path in
    /// the node array: lowercase, with `/`, without `.ngc`
    /// (`gameobjects/milk/milk.mdl`, `gameobjects/milk/milk.tex`).
    pub models: HashMap<String, Vec<u8>>,
    /// Bone animations by path: lowercase, with `/`, without `.ngc`
    /// (`anims/ped_zazu/idle.ska`). See [`GameData::add_animations`].
    pub animations: HashMap<String, Vec<u8>>,
    /// Every skeleton by lowercase name, and the animation key tables
    /// (`standardkeyq.bin`, `standardkeyt.bin`), from `skeletons.prg`.
    pub skeletons: HashMap<String, Vec<u8>>,
    pub key_tables: Option<(Vec<u8>, Vec<u8>)>,
    /// The level's own scripts (every `.qb` in `X.prg`).
    pub scripts: Vec<Vec<u8>>,
    /// Particle textures (`images/particles/NAME.img.ngc`) by name's
    /// checksum, as `CreateParticleSystem ... texture = NAME` names them.
    pub particle_images: HashMap<u32, Vec<u8>>,
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

/// Adds an archive's models and textures (under `Models/`) to `models`.
fn collect_models(archive: &Archive, models: &mut HashMap<String, Vec<u8>>) -> Result<()> {
    for e in archive.entries() {
        let path = e.path().replace('\\', "/").to_ascii_lowercase();
        let path = path.trim_start_matches("./");
        let Some(key) = path
            .strip_prefix("models/")
            .and_then(|p| p.strip_suffix(".ngc"))
        else {
            continue;
        };
        if [".mdl", ".skin", ".tex"].iter().any(|s| key.ends_with(s)) {
            models.insert(key.to_string(), e.contents()?.into_owned());
        }
    }
    Ok(())
}

/// An animation's path as [`LevelFiles::animations`] keys it: lowercase,
/// with `/`, without a leading `./` or the `.ngc`.
fn animation_key(path: &str) -> String {
    let path = path.replace('\\', "/").to_ascii_lowercase();
    let path = path.trim_start_matches("./");
    path.strip_suffix(".ngc").unwrap_or(path).to_string()
}

/// Reads `LoadAnim Name = "..." descChecksum = role` lines from each script.
fn animation_sets(script: &[u8]) -> Result<HashMap<u32, Vec<(String, String)>>> {
    use qb::{Definition, Symbols, Token, checksum, parse_definitions, tokenize};
    let tokens = tokenize(script).context("could not read allanims.qb")?;
    let mut symbols = Symbols::new();
    symbols.add_tokens(&tokens);
    let (name_key, role_key) = (checksum("Name"), checksum("descChecksum"));
    let mut sets = HashMap::new();
    for def in parse_definitions(&tokens).context("could not parse allanims.qb")? {
        let Definition::Script {
            name,
            tokens: range,
        } = def
        else {
            continue;
        };
        let body: Vec<&Token> = tokens[range].iter().map(|(_, t)| t).collect();
        let mut loads = Vec::new();
        let mut path = None;
        for w in body.windows(3) {
            match (w[0], w[1], w[2]) {
                (Token::Name(k), Token::Equals, Token::String(p)) if *k == name_key => {
                    path = Some(animation_key(p));
                }
                (Token::Name(k), Token::Equals, Token::Name(role)) if *k == role_key => {
                    if let Some(p) = path.take() {
                        loads.push((p, symbols.name(*role)));
                    }
                }
                _ => {}
            }
        }
        if !loads.is_empty() {
            sets.insert(name, loads);
        }
    }
    Ok(sets)
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
        let mut models = HashMap::new();
        let mut animations: HashMap<String, Vec<u8>> = HashMap::new();
        let mut scripts = Vec::new();
        let mut particle_images = HashMap::new();
        let (nodes, cameras) = match self.read_archive(&format!("{id}.prg"))? {
            Some(data) => {
                let archive =
                    Archive::parse(&data).with_context(|| format!("could not read {id}.prg"))?;
                let nodes = entry(&archive, &|p| p.ends_with(&format!("/{name}.qb")))?;
                collect_models(&archive, &mut models)?;
                for e in archive.entries() {
                    let path = e.path().replace('\\', "/").to_ascii_lowercase();
                    if path.ends_with(".qb") {
                        scripts.push(e.contents()?.into_owned());
                    }
                    if let Some(name) = path
                        .split("images/particles/")
                        .nth(1)
                        .and_then(|n| n.strip_suffix(".img.ngc"))
                    {
                        particle_images.insert(qb::checksum(name), e.contents()?.into_owned());
                    }
                }
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
                    let contents = e.contents()?.into_owned();
                    if ngc_anim::is_camera_path(&contents) {
                        // Some names end in a space.
                        cameras.push((file_name[..stem].trim().to_string(), contents));
                    } else {
                        animations.insert(animation_key(&path), contents);
                    }
                }
                cameras.sort_by_key(|c| c.0.to_ascii_lowercase());
                (nodes, cameras)
            }
            None => (None, Vec::new()),
        };
        if let Some(data) = self.read_archive(&format!("{id}Ped.prg"))? {
            let archive =
                Archive::parse(&data).with_context(|| format!("could not read {id}Ped.prg"))?;
            collect_models(&archive, &mut models)?;
        }
        let (skeletons, key_tables) = self.skeletons()?;

        Ok(LevelFiles {
            scene,
            textures,
            sky,
            collision,
            nodes,
            cameras,
            models,
            animations,
            skeletons,
            key_tables,
            scripts,
            particle_images,
        })
    }

    /// Every script in `qb.prg` (the game's shared scripts).
    pub fn global_scripts(&mut self) -> Result<Vec<Vec<u8>>> {
        let data = self.read_archive("qb.prg")?.context("qb.prg is missing")?;
        let archive = Archive::parse(&data).context("could not read qb.prg")?;
        archive
            .entries()
            .iter()
            .filter(|e| e.path().to_ascii_lowercase().ends_with(".qb"))
            .map(|e| Ok(e.contents()?.into_owned()))
            .collect()
    }

    /// The pedestrian animation sets in `scripts/allanims.qb`: for each
    /// `animload_...` script (by checksum), the animations it loads as
    /// (path as in [`LevelFiles::animations`], role name).
    pub fn animation_sets(&mut self) -> Result<HashMap<u32, Vec<(String, String)>>> {
        let data = self.read_archive("qb.prg")?.context("qb.prg is missing")?;
        let archive = Archive::parse(&data).context("could not read qb.prg")?;
        let script = entry(&archive, &|p| p.ends_with("allanims.qb"))?
            .context("qb.prg has no allanims.qb")?;
        animation_sets(&script)
    }

    /// Adds animations that aren't in the level's archive, from the
    /// playable characters' archives (`anims/buzz/...` is in
    /// `anims_buzz.prg`). Paths that can't be found are left out.
    pub fn add_animations(&mut self, files: &mut LevelFiles, paths: &[String]) -> Result<()> {
        let mut wanted: Vec<&String> = paths
            .iter()
            .filter(|p| !files.animations.contains_key(*p))
            .collect();
        wanted.sort();
        wanted.dedup();
        let mut folders: Vec<&str> = wanted.iter().filter_map(|p| p.split('/').nth(1)).collect();
        folders.dedup();
        for folder in folders {
            let Some(data) = self.read_archive(&format!("anims_{folder}.prg"))? else {
                continue;
            };
            let archive = Archive::parse(&data)
                .with_context(|| format!("could not read anims_{folder}.prg"))?;
            for e in archive.entries() {
                let key = animation_key(&e.path());
                if wanted.contains(&&key) {
                    files.animations.insert(key, e.contents()?.into_owned());
                }
            }
        }
        Ok(())
    }

    /// Every skeleton in `skeletons.prg` by lowercase name, and the key
    /// tables. Empty if the archive is missing.
    #[allow(clippy::type_complexity)]
    fn skeletons(&mut self) -> Result<(HashMap<String, Vec<u8>>, Option<(Vec<u8>, Vec<u8>)>)> {
        let Some(data) = self.read_archive("skeletons.prg")? else {
            return Ok((HashMap::new(), None));
        };
        let archive = Archive::parse(&data).context("could not read skeletons.prg")?;
        let mut skeletons = HashMap::new();
        for e in archive.entries() {
            let path = e.path().replace('\\', "/").to_ascii_lowercase();
            if let Some(stem) = path.rsplit('/').next().and_then(|n| n.strip_suffix(".ske")) {
                skeletons.insert(stem.to_string(), e.contents()?.into_owned());
            }
        }
        let tables = match (
            entry(&archive, &|p| p.ends_with("standardkeyq.bin"))?,
            entry(&archive, &|p| p.ends_with("standardkeyt.bin"))?,
        ) {
            (Some(q), Some(t)) => Some((q, t)),
            _ => None,
        };
        Ok((skeletons, tables))
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

    /// A character's voice lines (`streams/streams.wad`, indexed by
    /// `streams.hed`: `\Streams\pros\jessie\jessie_bail01`...): each line's
    /// kind (`bail`, `trick`) and its `.dsp` sound.
    pub fn voices(&mut self, character: &str) -> Result<Vec<(String, Vec<u8>)>> {
        let prefix = format!("\\streams\\pros\\{}\\", character.to_ascii_lowercase());
        let Some(index) = self.stream_file("streams.hed", None)? else {
            return Ok(Vec::new());
        };
        // Entries: offset, size (little-endian), and a name padded to 4.
        let mut lines = Vec::new();
        let mut at = 0;
        while at + 8 <= index.len() {
            let offset = u32::from_le_bytes(index[at..at + 4].try_into().unwrap()) as u64;
            let size = u32::from_le_bytes(index[at + 4..at + 8].try_into().unwrap()) as u64;
            at += 8;
            let Some(end) = index[at..].iter().position(|&b| b == 0) else {
                break;
            };
            let name = String::from_utf8_lossy(&index[at..at + end]).to_ascii_lowercase();
            at = (at + end + 4) & !3;
            if name.is_empty() {
                break;
            }
            if let Some(rest) = name.strip_prefix(&prefix) {
                let kind: String = rest
                    .rsplit('_')
                    .next()
                    .unwrap_or("")
                    .chars()
                    .filter(|c| !c.is_ascii_digit())
                    .collect();
                lines.push((kind, offset, size));
            }
        }
        // From a disc image the whole file is read once; from a folder, just
        // each line.
        let whole = if matches!(self, GameData::Disc { .. }) && !lines.is_empty() {
            self.stream_file("streams.wad", None)?
        } else {
            None
        };
        let mut out = Vec::new();
        for (kind, offset, size) in lines {
            let data = match &whole {
                Some(whole) => whole
                    .get(offset as usize..(offset + size) as usize)
                    .map(<[u8]>::to_vec),
                None => self.stream_file("streams.wad", Some((offset, size)))?,
            };
            if let Some(data) = data {
                out.push((kind, data));
            }
        }
        Ok(out)
    }

    /// The goal pedestrians' voice lines (`streams/goalpeds/NAME`): where
    /// each is in `streams.wad`, by its name's checksum (as goal scripts
    /// name them: `S_Stream = hench_beach_Letter_S`).
    pub fn goal_streams(&mut self) -> Result<HashMap<u32, (u64, u64)>> {
        let prefix = r"\streams\goalpeds\";
        let Some(index) = self.stream_file("streams.hed", None)? else {
            return Ok(HashMap::new());
        };
        let mut out = HashMap::new();
        let mut at = 0;
        while at + 8 <= index.len() {
            let offset = u32::from_le_bytes(index[at..at + 4].try_into().unwrap()) as u64;
            let size = u32::from_le_bytes(index[at + 4..at + 8].try_into().unwrap()) as u64;
            at += 8;
            let Some(end) = index[at..].iter().position(|&b| b == 0) else {
                break;
            };
            let name = String::from_utf8_lossy(&index[at..at + end]).to_ascii_lowercase();
            at = (at + end + 4) & !3;
            if name.is_empty() {
                break;
            }
            if let Some(stem) = name.strip_prefix(prefix) {
                out.insert(qb::checksum(stem), (offset, size));
            }
        }
        Ok(out)
    }

    /// One voice line out of `streams.wad`.
    pub fn stream(&mut self, (offset, size): (u64, u64)) -> Result<Option<Vec<u8>>> {
        self.stream_file("streams.wad", Some((offset, size)))
    }

    /// A file of the `streams` folder, or a range of it.
    fn stream_file(&mut self, name: &str, range: Option<(u64, u64)>) -> Result<Option<Vec<u8>>> {
        use std::io::{Read, Seek, SeekFrom};
        let data = match self {
            GameData::Disc { disc, .. } => {
                let file = disc
                    .fst()
                    .files()
                    .find(|n| n.path.to_ascii_lowercase() == format!("streams/{name}"))
                    .and_then(|n| Some((n.path.clone(), n.file_range()?)));
                let Some((path, (start, length))) = file else {
                    return Ok(None);
                };
                match range {
                    // Just the part wanted, straight off the disc.
                    Some((offset, size)) if offset + size <= length => {
                        Some(disc.read_range(start + offset, size)?)
                    }
                    Some(_) => None,
                    None => Some(disc.read_file(&path)?),
                }
            }
            GameData::Folder(dir) => {
                let Some(path) = dir.parent().map(|d| d.join("streams").join(name)) else {
                    return Ok(None);
                };
                let Ok(mut file) = File::open(&path) else {
                    return Ok(None);
                };
                match range {
                    Some((offset, size)) => {
                        file.seek(SeekFrom::Start(offset))?;
                        let mut buf = vec![0; size as usize];
                        file.read_exact(&mut buf)?;
                        Some(buf)
                    }
                    None => Some(fs::read(&path)?),
                }
            }
        };
        Ok(data)
    }

    /// A streamed music track (`music/dtk/NAME.dtk`, ignoring case), or
    /// none if it isn't there.
    pub fn music(&mut self, name: &str) -> Result<Option<Vec<u8>>> {
        let file = format!("{}.dtk", name.to_ascii_lowercase());
        match self {
            GameData::Disc { disc, .. } => {
                let found = disc
                    .fst()
                    .files()
                    .find(|n| {
                        let p = n.path.to_ascii_lowercase();
                        p.starts_with("music/") && p.ends_with(&format!("/{file}"))
                    })
                    .map(|n| n.path.clone());
                match found {
                    Some(path) => Ok(Some(disc.read_file(&path)?)),
                    None => Ok(None),
                }
            }
            GameData::Folder(dir) => {
                // The archives' folder is `.../files/pre`; music is beside it.
                let Some(music) = dir.parent().map(|d| d.join("music").join("dtk")) else {
                    return Ok(None);
                };
                let found = fs::read_dir(&music).ok().and_then(|entries| {
                    entries
                        .filter_map(|e| e.ok())
                        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(&file))
                        .map(|e| e.path())
                });
                Ok(found.map(fs::read).transpose()?)
            }
        }
    }

    /// Every sound (`sounds/dsp/.../NAME.dsp`) in archive `name` (e.g.
    /// `hub.prg`, `skater_sounds.prg`), by its lowercase file name without
    /// `.dsp`. None if there's no such archive.
    pub fn sounds(&mut self, name: &str) -> Result<HashMap<String, Vec<u8>>> {
        let Some(data) = self.read_archive(name)? else {
            return Ok(HashMap::new());
        };
        let archive = Archive::parse(&data).with_context(|| format!("could not read {name}"))?;
        let mut out = HashMap::new();
        for e in archive.entries() {
            let path = e.path().to_ascii_lowercase();
            let file = path.rsplit(['/', '\\']).next().unwrap_or(&path);
            if let Some(stem) = file.strip_suffix(".dsp") {
                out.insert(stem.to_string(), e.contents()?.into_owned());
            }
        }
        Ok(out)
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
