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

/// The screen's sprites and fonts, by lowercase name (see
/// [`GameData::ui_files`]).
#[derive(Clone, Debug, Default)]
pub struct UiFiles {
    pub images: HashMap<String, Vec<u8>>,
    pub fonts: HashMap<String, Vec<u8>>,
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
/// A choice for one of the created kid's parts, from the Skate Shop's
/// lists (`skater_m_hair`, `hat`...): its name as the shop shows it, its
/// mesh, and the textures it swaps (`replace = "hair_boy_1_a1.png" with =
/// "textures/boy/hair_boy_1_a2"`).
#[derive(Clone, Debug, PartialEq)]
pub struct KidPart {
    pub name: String,
    /// Its `desc_id` in its list (`Buzzcut`, `#"Skin 4"`), as the game's
    /// menus name it.
    pub desc_id: u32,
    /// Another `desc_id` it goes by (a skin's body: `BoyBody2` for `#"Skin
    /// 3"`), or 0.
    pub alias: u32,
    pub mesh: Option<String>,
    pub swaps: Vec<(String, String)>,
    /// Whether the shop lets its colour be changed (not `no_color`).
    pub colorable: bool,
    /// The colour picked for it, as the shop's sliders give it: hue in
    /// degrees, saturation and value out of 100 (`h`, `S`, `v`); `None`
    /// keeps its own (`use_default_hsv`).
    pub color: Option<[f32; 3]>,
}

/// A part's colour (`SetPlayerAppearanceColor h S v`) as the game sets it
/// on the part's materials (0x8006D4BC): the hue, saturation and value
/// to RGB, out of 255, where 128 leaves the part as it is.
pub fn kid_part_tint([h, s, v]: [f32; 3]) -> [u8; 3] {
    let (s, v) = (s / 100.0, v / 100.0);
    let h = h.rem_euclid(360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r, g, b].map(|c| ((c + m) * 255.0 + 0.5).clamp(0.0, 255.0) as u8)
}

/// One of the created kid's parts: its choices and the one picked (the
/// game's own to start, `appearance_reset_boy` or `_girl`).
#[derive(Clone, Debug, PartialEq)]
pub struct KidSlot {
    pub label: &'static str,
    /// The list's name (`skater_m_hair`, `hat`), as the game's menus
    /// name the part.
    pub list: u32,
    pub choices: Vec<KidPart>,
    pub picked: usize,
    /// Its colour picked (see [`KidPart::color`]).
    pub color: Option<[f32; 3]>,
}

impl KidSlot {
    /// The choice picked, in the colour picked.
    pub fn worn(&self) -> KidPart {
        KidPart {
            color: self.color.filter(|_| self.choices[self.picked].colorable),
            ..self.choices[self.picked].clone()
        }
    }
}

/// Bones scaled, by their names (a body shape: `head = (0.9, 0.9, 0.9)`).
pub type BodyShape = Vec<(u32, [f32; 3])>;

pub struct CharacterFiles {
    pub skin: Vec<u8>,
    pub textures: Option<Vec<u8>>,
    /// The skateboard, skinned to the character's skeleton.
    pub board: Option<(Vec<u8>, Option<Vec<u8>>)>,
    /// More of the model, skinned to the same skeleton: the created
    /// kid's clothes and hair.
    pub parts: Vec<(Vec<u8>, Option<Vec<u8>>)>,
    /// The colour each of those is tinted (see [`kid_part_tint`]).
    pub part_tints: Vec<Option<[u8; 3]>>,
    /// Textures (`.img.ngc` files) for checksums the model's own texture
    /// file hasn't: the created girl's head.
    pub extra_textures: Vec<(u32, Vec<u8>)>,
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
        if [".mdl", ".skin", ".tex", ".col"].iter().any(|s| key.ends_with(s)) {
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
                // (The created skater, the kid, has animations here but its
                // model's made of parts in skaterparts.prg: see
                // `load_character`.)
                Some(name[6..6 + id.len()].to_string())
            })
            .map(|id| LevelInfo {
                title: title(&id),
                id,
            })
            .collect();
        // The created kid as a girl too (the skater select's first kid),
        // on the kid's animations.
        if characters.iter().any(|c| c.id.eq_ignore_ascii_case("kid")) {
            characters.push(LevelInfo {
                title: "Kid (Girl)".into(),
                id: "kid_girl".into(),
            });
        }
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

    /// Where a movie is (`movies/NAME.bik`, ignoring case): the disc's
    /// range of it, or the file in an extracted folder (`movies` beside
    /// `pre`).
    pub fn movie(&self, name: &str) -> Option<crate::movie::MovieSource> {
        let file = format!("{}.bik", name.to_ascii_lowercase());
        match self {
            GameData::Disc { path, disc } => {
                let (offset, size) = disc
                    .fst()
                    .files()
                    .find(|n| n.path.to_ascii_lowercase() == format!("movies/{file}"))?
                    .file_range()?;
                Some(crate::movie::MovieSource {
                    path: path.clone(),
                    offset,
                    size,
                })
            }
            GameData::Folder(dir) => {
                let movies = dir.parent()?.join("movies");
                let path = fs::read_dir(&movies).ok()?.find_map(|e| {
                    let e = e.ok()?;
                    e.file_name()
                        .to_str()?
                        .eq_ignore_ascii_case(&file)
                        .then(|| e.path())
                })?;
                let size = fs::metadata(&path).ok()?.len();
                Some(crate::movie::MovieSource {
                    path,
                    offset: 0,
                    size,
                })
            }
        }
    }

    /// The screen's pieces: the panel sprites (`images/PanelSprites/...`,
    /// by lowercase file name without `.img.ngc`) and fonts (by lowercase
    /// name without `.fnt.ngc`) of the archives named, later ones over
    /// earlier (`panelsprites.prg` for every level, then a theme's:
    /// `hubpanel.prg`). Languages' own copies (`French/`...) are left out.
    pub fn ui_files(&mut self, archives: &[&str]) -> Result<UiFiles> {
        let mut out = UiFiles::default();
        for name in archives {
            let Some(data) = self.read_archive(name)? else {
                continue;
            };
            let archive =
                Archive::parse(&data).with_context(|| format!("could not read {name}"))?;
            for e in archive.entries() {
                let path = e.path().to_ascii_lowercase().replace('\\', "/");
                if ["/french/", "/german/", "/italian/", "/spanish/"]
                    .iter()
                    .any(|l| path.contains(l))
                {
                    continue;
                }
                let file = path.rsplit('/').next().unwrap_or(&path).to_string();
                if let Some(image) = file.strip_suffix(".img.ngc") {
                    out.images
                        .insert(image.to_string(), e.contents()?.into_owned());
                } else if let Some(font) = file.strip_suffix(".fnt.ngc") {
                    out.fonts
                        .insert(font.to_string(), e.contents()?.into_owned());
                }
            }
        }
        Ok(out)
    }

    /// The movies on the disc (`movies/*.bik`), by name, sorted.
    pub fn movies(&self) -> Vec<String> {
        let mut names: Vec<String> = match self {
            GameData::Disc { disc, .. } => disc
                .fst()
                .files()
                .filter(|n| {
                    let p = n.path.to_ascii_lowercase();
                    p.starts_with("movies/") && p.ends_with(".bik")
                })
                .map(|n| n.name.trim_end_matches(".bik").to_string())
                .collect(),
            GameData::Folder(dir) => dir
                .parent()
                .and_then(|d| fs::read_dir(d.join("movies")).ok())
                .map(|entries| {
                    entries
                        .filter_map(|e| {
                            let name = e.ok()?.file_name().into_string().ok()?;
                            name.to_ascii_lowercase()
                                .ends_with(".bik")
                                .then(|| name[..name.len() - 4].to_string())
                        })
                        .collect()
                })
                .unwrap_or_default(),
        };
        names.sort_by_key(|n| n.to_ascii_lowercase());
        names
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

    /// The created kid's parts (as a boy or a girl) and their choices, from
    /// the Skate Shop's lists. The skin's choices are the bodies they put on
    /// (`SetPart part = body desc_id = BoyBody1`...).
    pub fn kid_wardrobe(&mut self, girl: bool) -> Result<Vec<KidSlot>> {
        Ok(self.kid_wardrobe_and_shape(girl)?.0)
    }

    /// [`GameData::kid_wardrobe`], and the kid's body shape as the Skate
    /// Shop makes it (`body_shape = cak_scale_info`: its bones scaled by
    /// name, `head = (0.9, 0.9, 0.9)`; the overall `scale` left out).
    pub fn kid_wardrobe_and_shape(&mut self, girl: bool) -> Result<(Vec<KidSlot>, BodyShape)> {
        use qb::{Value, checksum};
        let mut program = qb::vm::Program::new();
        for file in self.global_scripts()? {
            let _ = program.add(&file);
        }
        for file in self.load_level("SkateShop")?.scripts {
            let _ = program.add(&file);
        }
        let text = |v: &Value| match v {
            Value::String(s) | Value::LocalString(s) => Some(s.clone()),
            _ => None,
        };
        let id_of = |v: &Value| v.get(checksum("desc_id")).and_then(Value::as_name);
        // An entry with the structs it names filled in (`common_deck_
        // graphic_params`: the texture a deck replaces).
        let expand = |entry: &Value| -> Value {
            let Value::Struct(items) = entry else {
                return entry.clone();
            };
            let mut out = Vec::new();
            for (k, v) in items {
                match (k, v) {
                    (None, Value::Name(n)) => match program.value(*n) {
                        Some(Value::Struct(base)) => out.extend(base.iter().cloned()),
                        _ => out.push((None, v.clone())),
                    },
                    _ => out.push((*k, v.clone())),
                }
            }
            Value::Struct(out)
        };
        let part = |entry: &Value| -> KidPart {
            let entry = &expand(entry);
            let field = |name: &str| entry.get(checksum(name)).and_then(text);
            let name = entry
                .get(checksum("frontend_desc"))
                .and_then(|v| text(v).or_else(|| program.value(v.as_name()?).and_then(text)))
                .unwrap_or_default();
            let mut swaps = Vec::new();
            for n in ["", "1", "2", "3", "4", "5"] {
                let from = field(&format!("replace{n}"));
                if let (Some(from), Some(to)) = (from, field(&format!("with{n}"))) {
                    swaps.push((from, to));
                }
            }
            KidPart {
                name,
                desc_id: id_of(entry).unwrap_or(0),
                alias: 0,
                colorable: field("mesh").is_some() && !entry.has_flag(checksum("no_color")),
                mesh: field("mesh"),
                swaps,
                color: None,
            }
        };
        let list = |name: &str| -> Vec<Value> {
            program
                .value(checksum(name))
                .and_then(Value::as_array)
                .map(<[Value]>::to_vec)
                .unwrap_or_default()
        };
        let reset = program
            .value(checksum(if girl {
                "appearance_reset_girl"
            } else {
                "appearance_reset_boy"
            }))
            .cloned();
        let picked_in = |array: &str| id_of(reset.as_ref()?.get(checksum(array))?);
        let sex = if girl { "f" } else { "m" };
        let bodies = list("body");
        let mut slots = Vec::new();
        // The skin: the bodies (`BoyBody`, `BoyBody1`...), by the order the
        // skins come in.
        let head = format!("skater_{sex}_head");
        let skins = list(&head);
        let body_name = if girl { "GirlBody" } else { "BoyBody" };
        let mut skin_choices = Vec::new();
        for (i, skin) in skins.iter().enumerate() {
            let id = if i == 0 {
                body_name.to_string()
            } else {
                format!("{body_name}{i}")
            };
            let Some(body) = bodies.iter().find(|b| id_of(b) == Some(checksum(&id))) else {
                continue;
            };
            skin_choices.push(KidPart {
                name: part(skin).name,
                desc_id: id_of(skin).unwrap_or(0),
                alias: id_of(body).unwrap_or(0),
                ..part(body)
            });
        }
        let skin_picked = picked_in(&head)
            .and_then(|id| skins.iter().position(|s| id_of(s) == Some(id)))
            .unwrap_or(0);
        slots.push(KidSlot {
            label: "Skin",
            list: checksum(&head),
            choices: skin_choices
                .into_iter()
                .map(|c| KidPart {
                    colorable: false,
                    ..c
                })
                .collect(),
            picked: skin_picked,
            color: None,
        });
        for (label, array) in [
            ("Hair", format!("skater_{sex}_hair")),
            ("Shirt", format!("skater_{sex}_torso")),
            ("Legs", format!("skater_{sex}_legs")),
            ("Shoes", "shoes".into()),
            ("Socks", format!("skater_{sex}_socks")),
            ("Hat", "hat".into()),
            ("Glasses", "glasses".into()),
            ("Face paint", "face_paints".into()),
            ("Accessory", "accessories".into()),
            ("Elbow pads", format!("skater_{sex}_elbowpads")),
            ("Knee pads", format!("skater_{sex}_kneepads")),
            ("Backpack", format!("skater_{sex}_backpack")),
            ("Hat logo", "hat_logo".into()),
            ("Front logo", "front_logo".into()),
            ("Back logo", "back_logo".into()),
            ("Deck", "deck_graphic".into()),
            ("Griptape", "griptape".into()),
        ] {
            // (The decks the kid can have: its own and the blank, not the
            // other skaters', `skater = buzz`.)
            let entries: Vec<Value> = list(&array)
                .into_iter()
                .filter(|e| {
                    e.get(checksum("skater"))
                        .and_then(Value::as_name)
                        .is_none_or(|s| s == checksum("kid") || s == checksum("none"))
                })
                .collect();
            if entries.is_empty() {
                continue;
            }
            let choices: Vec<KidPart> = entries.iter().map(part).collect();
            // Not in the game's set-up: the first that's nothing at all.
            let picked = picked_in(&array)
                .and_then(|id| entries.iter().position(|e| id_of(e) == Some(id)))
                .or_else(|| {
                    choices
                        .iter()
                        .position(|c| c.mesh.is_none() && c.swaps.is_empty())
                })
                .unwrap_or(0);
            // (Its colour in the game's set-up, if not its own.)
            let color = reset
                .as_ref()
                .and_then(|r| r.get(checksum(&array)))
                .filter(|p| p.get(checksum("use_default_hsv")).and_then(Value::as_int) == Some(0))
                .and_then(|p| {
                    let n = |k: &str| p.get(checksum(k)).and_then(Value::as_f32);
                    Some([n("h")?, n("S")?, n("v")?])
                });
            slots.push(KidSlot {
                label,
                list: checksum(&array),
                choices,
                picked,
                color,
            });
        }
        let shape = match reset
            .as_ref()
            .and_then(|r| r.get(checksum("body_shape")))
            .and_then(Value::as_name)
            .and_then(|n| program.value(n))
        {
            Some(Value::Struct(items)) => items
                .iter()
                .filter_map(|(k, v)| Some((k.filter(|k| *k != checksum("scale"))?, v.as_vector()?)))
                .collect(),
            _ => Vec::new(),
        };
        Ok((slots, shape))
    }

    /// The created kid dressed in `outfit` (a choice for each slot of
    /// [`GameData::kid_wardrobe`], the skin first).
    pub fn load_kid(&mut self, girl: bool, outfit: &[KidPart]) -> Result<CharacterFiles> {
        self.load_dressed(if girl { "kid_girl" } else { "kid" }, Some(outfit))
    }

    pub fn load_character(&mut self, id: &str) -> Result<CharacterFiles> {
        self.load_dressed(id, None)
    }

    fn load_dressed(&mut self, id: &str, outfit: Option<&[KidPart]>) -> Result<CharacterFiles> {
        let girl = id.eq_ignore_ascii_case("kid_girl");
        let id = if girl { "kid" } else { id };
        let name = id.to_ascii_lowercase();
        let data = self
            .read_archive(&format!("anims_{id}.prg"))?
            .with_context(|| format!("anims_{id}.prg is missing"))?;
        let archive =
            Archive::parse(&data).with_context(|| format!("could not read anims_{id}.prg"))?;
        let file = |suffix: &str| entry(&archive, &|p| p.ends_with(&format!("/{name}{suffix}")));
        let mut parts = Vec::new();
        let mut part_tints = Vec::new();
        let mut extra_textures = Vec::new();
        let (skin, textures, board) = if name == "kid" {
            // The kid as the career starts it (`appearance_kid`: the boy's
            // body, `KidBody`, on the kid's board), from the created
            // skater's parts.
            let kit = self
                .read_archive("skaterparts.prg")?
                .context("skaterparts.prg is missing")?;
            let kit = Archive::parse(&kit).context("could not read skaterparts.prg")?;
            let part = |path: &str| entry(&kit, &|p| p.eq_ignore_ascii_case(path));
            let body = if girl { "girl/girl" } else { "boy/boy" };
            let skin = part(&format!("models/{body}.skin.ngc"))?
                .with_context(|| format!("skaterparts.prg has no {body}.skin"))?;
            let deck = if girl {
                "girl/girlboard"
            } else {
                "boy/boyboard"
            };
            let board = match part(&format!("models/{deck}.skin.ngc"))? {
                Some(board) => Some((board, part(&format!("models/{deck}.tex.ngc"))?)),
                None => None,
            };
            // Dressed as the game's created skater starts (its default
            // boy and girl, CASMENU.q), the meshes the Skate Shop's part
            // lists give them. The boy: Buzzcut, Shortsleeve 1, Cargo
            // Pants 1, Tennis Shoes 1, Skate Helmet 1, Elbow Pads 1. The
            // girl: Pigtails 1, Shortsleeve 1, Shorts 3, Flipflops 1,
            // Skate Helmet 2, Elbow and Knee Pads 1.
            let pieces: &[&str] = if girl {
                &[
                    "girl/hair_girl_2",
                    "girl/shirt_girl_1",
                    "girl/pants_girl_1",
                    "boy/shoe_4",
                    "boy/hat_12",
                    "boy/pads_elbows",
                    "boy/pads_knees",
                ]
            } else {
                &[
                    "boy/hair_boy_1",
                    "boy/shirt_boy_1",
                    "boy/pants_boy_2",
                    "boy/shoe_8",
                    "boy/hat_12",
                    "boy/pads_elbows",
                ]
            };
            // The girl's head isn't in her girl.tex (its `girl_a1.png`,
            // 6563d500, is always swapped for the skin chosen): Skin 4's,
            // the default girl's (GirlBody3: `textures/girl/girl_a4`).
            if girl {
                if let Some(head) = part("textures/girl/girl_a4.img.ngc")? {
                    extra_textures.push((0x6563_d500, head));
                }
            }
            if let Some(outfit) = outfit {
                // Dressed as picked: the skin's body, the other parts'
                // meshes, and every texture they swap.
                let (skin_part, rest) = outfit.split_first().context("no skin picked")?;
                let mesh_of = |mesh: &str| mesh.strip_suffix(".skin").unwrap_or(mesh).to_string();
                let body = skin_part
                    .mesh
                    .as_deref()
                    .map_or(format!("models/{body}"), mesh_of);
                let skin = part(&format!("{body}.skin.ngc"))?
                    .with_context(|| format!("skaterparts.prg has no {body}.skin"))?;
                for piece in rest {
                    let Some(mesh) = piece.mesh.as_deref().map(mesh_of) else {
                        continue;
                    };
                    if let Some(skin) = part(&format!("{mesh}.skin.ngc"))? {
                        parts.push((skin, part(&format!("{mesh}.tex.ngc"))?));
                        part_tints.push(piece.color.map(kid_part_tint));
                    }
                }
                // (The girl's missing head texture as the skin picked has it.)
                extra_textures.clear();
                if girl {
                    let head = skin_part
                        .swaps
                        .iter()
                        .find(|(from, _)| from.eq_ignore_ascii_case("girl_a1.png"))
                        .map_or("textures/girl/girl_a1", |(_, to)| to.as_str());
                    if let Some(head) = part(&format!("{head}.img.ngc"))? {
                        extra_textures.push((0x6563_d500, head));
                    }
                }
                for (from, to) in outfit.iter().flat_map(|p| &p.swaps) {
                    if let Some(image) = part(&format!("{to}.img.ngc"))? {
                        for sum in crate::level::texture_checksums(from) {
                            extra_textures.push((sum, image.clone()));
                        }
                    }
                }
                (skin, part(&format!("{body}.tex.ngc"))?, board)
            } else {
                for piece in pieces {
                    if let Some(mesh) = part(&format!("models/{piece}.skin.ngc"))? {
                        parts.push((mesh, part(&format!("models/{piece}.tex.ngc"))?));
                    }
                }
                (skin, part(&format!("models/{body}.tex.ngc"))?, board)
            }
        } else {
            let skin =
                file(".skin.ngc")?.with_context(|| format!("anims_{id}.prg has no model"))?;
            let board = match file("board.skin.ngc")? {
                Some(board) => Some((board, file("board.tex.ngc")?)),
                None => None,
            };
            (skin, file(".tex.ngc")?, board)
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
            parts,
            part_tints,
            extra_textures,
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

#[cfg(test)]
mod kid_tests {
    use super::kid_part_tint;

    #[test]
    fn tints() {
        // Grey at half value leaves a part as it is (128).
        assert_eq!(kid_part_tint([0.0, 0.0, 50.0]), [128, 128, 128]);
        assert_eq!(kid_part_tint([0.0, 100.0, 50.0]), [128, 0, 0]);
        assert_eq!(kid_part_tint([120.0, 100.0, 50.0]), [0, 128, 0]);
        assert_eq!(kid_part_tint([240.0, 50.0, 50.0]), [64, 64, 128]);
    }
}
