//! Remembered choices (`%APPDATA%\desa-map-viewer\settings.txt`) and
//! finding the game data on first run.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct Settings {
    /// The disc image or folder last opened.
    pub data_path: Option<PathBuf>,
    /// The archive id of the last level viewed, and of the last but the
    /// Skate Shop (the main menu's Free Skate goes there).
    pub last_level: Option<String>,
    pub last_skated: Option<String>,
    /// The id of the character last shown.
    pub last_character: Option<String>,
    pub speed: Option<f32>,
    /// The best two-minute run scores, by `level.character`.
    pub best: BTreeMap<String, u32>,
    /// Text kept by name (a saved created skater's name: `cak.0`).
    pub names: BTreeMap<String, String>,
    /// The gaps landed on each level, by name.
    pub gaps: BTreeMap<String, BTreeSet<String>>,
    /// The Video Options: the window and how often it's drawn.
    pub video: Video,
    /// Never saved (screenshots).
    pub read_only: bool,
}

/// How the window shows: in a window, over the whole screen without a
/// border, or taking the screen over (its size the screen's mode).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowMode {
    #[default]
    Windowed,
    Borderless,
    Fullscreen,
}

impl WindowMode {
    pub const ALL: [WindowMode; 3] = [Self::Windowed, Self::Borderless, Self::Fullscreen];

    pub fn name(self) -> &'static str {
        match self {
            Self::Windowed => "Windowed",
            Self::Borderless => "Borderless",
            Self::Fullscreen => "Fullscreen",
        }
    }
}

/// The frame limits offered (0: none).
pub const FRAME_LIMITS: [u32; 6] = [0, 30, 60, 120, 144, 240];

/// The picture's shapes offered (width by height; `None`: the window's
/// own, filling it). Another than the window's, the picture's as large as
/// fits, black bars round it.
pub const ASPECTS: [Option<(u32, u32)>; 6] = [
    None,
    Some((4, 3)),
    Some((16, 10)),
    Some((16, 9)),
    Some((21, 9)),
    Some((32, 9)),
];

/// What's round a picture not the window's shape (see `Video::aspect`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Bars {
    #[default]
    Black,
    /// One of the game's loading screens, by name (`loadscrn_pizza_buzz`).
    Art(String),
    /// The player's own: `bars.png` (or `.jpg`) beside the program (see
    /// `custom_bars`).
    Custom,
}

impl Bars {
    fn save(&self) -> String {
        match self {
            Bars::Black => "black".into(),
            Bars::Custom => "custom".into(),
            Bars::Art(name) => format!("art:{name}"),
        }
    }

    fn load(text: &str) -> Bars {
        match text {
            "custom" => Bars::Custom,
            _ => match text.strip_prefix("art:") {
                Some(name) if !name.is_empty() => Bars::Art(name.to_string()),
                _ => Bars::Black,
            },
        }
    }
}

/// The player's own picture for the bars: `bars.png`, `bars.jpg` or
/// `bars.jpeg` beside the program, or in the settings' folder.
pub fn custom_bars() -> Option<PathBuf> {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.to_path_buf()));
    let settings = settings_file().and_then(|f| Some(f.parent()?.to_path_buf()));
    [beside, settings]
        .into_iter()
        .flatten()
        .flat_map(|dir| ["bars.png", "bars.jpg", "bars.jpeg"].map(|name| dir.join(name)))
        .find(|p| p.is_file())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Video {
    pub mode: WindowMode,
    /// The window's size (or, full screen, the screen mode's), in pixels;
    /// `None`, as it opens.
    pub size: Option<(u32, u32)>,
    pub vsync: bool,
    /// Frames a second at most (0: no limit).
    pub frame_limit: u32,
    /// The picture's shape (one of `ASPECTS`; `None`, the window's).
    pub aspect: Option<(u32, u32)>,
    /// What's round it, at a shape of its own.
    pub bars: Bars,
}

impl Default for Video {
    fn default() -> Self {
        Video {
            mode: WindowMode::Windowed,
            size: None,
            vsync: true,
            frame_limit: 0,
            aspect: None,
            bars: Bars::Black,
        }
    }
}

fn settings_file() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("desa-map-viewer").join("settings.txt"))
}

impl Settings {
    pub fn load() -> Self {
        let mut settings = Settings::default();
        let Some(text) = settings_file().and_then(|f| fs::read_to_string(f).ok()) else {
            return settings;
        };
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "data_path" if !value.is_empty() => settings.data_path = Some(PathBuf::from(value)),
                "last_level" if !value.is_empty() => settings.last_level = Some(value.to_string()),
                "last_skated" if !value.is_empty() => {
                    settings.last_skated = Some(value.to_string())
                }
                "last_character" if !value.is_empty() => {
                    settings.last_character = Some(value.to_string())
                }
                "speed" => settings.speed = value.parse().ok(),
                "video.mode" => {
                    if let Some(mode) = WindowMode::ALL
                        .into_iter()
                        .find(|m| m.name().eq_ignore_ascii_case(value))
                    {
                        settings.video.mode = mode;
                    }
                }
                "video.size" => {
                    settings.video.size = value
                        .split_once('x')
                        .and_then(|(w, h)| Some((w.trim().parse().ok()?, h.trim().parse().ok()?)))
                        .filter(|&(w, h): &(u32, u32)| w >= 320 && h >= 240);
                }
                "video.vsync" => settings.video.vsync = value != "0",
                "video.frame_limit" => {
                    settings.video.frame_limit = value.parse().unwrap_or(0);
                }
                "video.bars" => settings.video.bars = Bars::load(value),
                "video.aspect" => {
                    settings.video.aspect = value
                        .split_once(':')
                        .and_then(|(w, h)| Some((w.trim().parse().ok()?, h.trim().parse().ok()?)))
                        .filter(|a| ASPECTS.contains(&Some(*a)));
                }
                key if key.starts_with("gaps.") => {
                    settings.gaps.insert(
                        key["gaps.".len()..].to_string(),
                        value
                            .split('|')
                            .filter(|g| !g.is_empty())
                            .map(str::to_string)
                            .collect(),
                    );
                }
                key if key.starts_with("name.") => {
                    settings
                        .names
                        .insert(key["name.".len()..].to_string(), value.to_string());
                }
                key if key.starts_with("best.") => {
                    if let Ok(score) = value.parse() {
                        settings
                            .best
                            .insert(key["best.".len()..].to_string(), score);
                    }
                }
                _ => {}
            }
        }
        settings
    }

    /// Saves the settings; failures are ignored (they only cost convenience).
    pub fn save(&self) {
        if self.read_only {
            return;
        }
        let Some(file) = settings_file() else { return };
        if let Some(dir) = file.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let mut text = String::new();
        if let Some(path) = &self.data_path {
            text += &format!("data_path={}\n", path.display());
        }
        if let Some(level) = &self.last_level {
            text += &format!("last_level={level}\n");
        }
        if let Some(level) = &self.last_skated {
            text += &format!(
                "last_skated={level}
"
            );
        }
        if let Some(character) = &self.last_character {
            text += &format!("last_character={character}\n");
        }
        if let Some(speed) = self.speed {
            text += &format!("speed={speed}\n");
        }
        text += &format!(
            "video.mode={}
",
            self.video.mode.name()
        );
        if let Some((w, h)) = self.video.size {
            text += &format!(
                "video.size={w}x{h}
"
            );
        }
        text += &format!(
            "video.vsync={}
",
            u8::from(self.video.vsync)
        );
        text += &format!(
            "video.frame_limit={}
",
            self.video.frame_limit
        );
        text += &format!("video.bars={}\n", self.video.bars.save());
        if let Some((w, h)) = self.video.aspect {
            text += &format!(
                "video.aspect={w}:{h}
"
            );
        }
        for (level, gaps) in &self.gaps {
            let names: Vec<&str> = gaps.iter().map(String::as_str).collect();
            text += &format!("gaps.{level}={}\n", names.join("|"));
        }
        for (key, score) in &self.best {
            text += &format!("best.{key}={score}\n");
        }
        for (key, name) in &self.names {
            text += &format!("name.{key}={}\n", name.replace('\n', " "));
        }
        let _ = fs::write(file, text);
    }
}

/// Looks for the game near the program and the current folder: a GameCube
/// disc image, or an `extracted` folder with the level archives.
pub fn find_game_data() -> Option<PathBuf> {
    let mut places = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        // The program's folder and up to three above it (it may sit in
        // target/release inside the project).
        places.extend(exe.ancestors().skip(1).take(4).map(Path::to_path_buf));
    }
    if let Ok(cwd) = std::env::current_dir() {
        places.push(cwd);
    }
    for dir in &places {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut entries: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        if let Some(iso) = entries.iter().find(|p| is_disc_image(p)) {
            return Some(iso.clone());
        }
        let extracted = dir.join("extracted");
        if desa_viewer::source::GameData::open(&extracted).is_ok() {
            return Some(extracted);
        }
    }
    None
}

fn is_disc_image(path: &Path) -> bool {
    let is_image = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "iso" | "gcm"));
    is_image && starts_with_game_code(path)
}

/// Whether the disc's game code is this game's (`GEXE` in the US; the
/// fourth letter is the region). Reads only the first 4 bytes.
fn starts_with_game_code(path: &Path) -> bool {
    use std::io::Read;
    let mut header = [0u8; 4];
    fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut header))
        .is_ok_and(|_| header.starts_with(b"GEX"))
}
