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
    /// The gaps landed on each level, by name.
    pub gaps: BTreeMap<String, BTreeSet<String>>,
    /// Never saved (screenshots).
    pub read_only: bool,
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
        for (level, gaps) in &self.gaps {
            let names: Vec<&str> = gaps.iter().map(String::as_str).collect();
            text += &format!("gaps.{level}={}\n", names.join("|"));
        }
        for (key, score) in &self.best {
            text += &format!("best.{key}={score}\n");
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
