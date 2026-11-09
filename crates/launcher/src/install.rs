//! What's installed where: the launcher's settings, the versions in the
//! install folder, downloading one, shortcuts, the launcher updating
//! itself, starting the game, and uninstalling.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::net::{self, GAME_ASSET, LAUNCHER_ASSET, Release, VIEWER_ASSET};

/// What the play button starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Program {
    Game,
    Viewer,
}

impl Program {
    pub fn label(self) -> &'static str {
        match self {
            Program::Game => "Disney's Extreme Skate Adventure",
            Program::Viewer => "DESA Map Viewer",
        }
    }

    pub fn asset(self) -> &'static str {
        match self {
            Program::Game => GAME_ASSET,
            Program::Viewer => VIEWER_ASSET,
        }
    }
}

/// The launcher's settings, `%APPDATA%\desa-launcher\launcher.txt`.
#[derive(Clone, Debug)]
pub struct Config {
    pub setup_done: bool,
    pub install_dir: PathBuf,
    pub disc: Option<PathBuf>,
    pub program: Program,
    /// A tag, or `None` for the latest.
    pub version: Option<String>,
    pub close_on_play: bool,
    /// The release the launcher itself came from (empty: unknown).
    pub launcher_tag: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            setup_done: false,
            install_dir: default_install_dir(),
            disc: None,
            program: Program::Game,
            version: None,
            close_on_play: false,
            launcher_tag: String::new(),
        }
    }
}

fn config_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("desa-launcher").join("launcher.txt")
}

pub fn default_install_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\"))
        .join("Programs")
        .join("DESA Recomp")
}

impl Config {
    pub fn load() -> Config {
        let mut c = Config::default();
        let Ok(text) = fs::read_to_string(config_path()) else {
            return c;
        };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let v = v.trim();
            match k.trim() {
                "setup_done" => c.setup_done = v == "1",
                "install_dir" if !v.is_empty() => c.install_dir = PathBuf::from(v),
                "disc" if !v.is_empty() => c.disc = Some(PathBuf::from(v)),
                "program" => {
                    c.program = if v == "viewer" {
                        Program::Viewer
                    } else {
                        Program::Game
                    }
                }
                "version" => c.version = (!v.is_empty() && v != "latest").then(|| v.to_string()),
                "close_on_play" => c.close_on_play = v == "1",
                "launcher_tag" => c.launcher_tag = v.to_string(),
                _ => {}
            }
        }
        c
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        fs::create_dir_all(path.parent().unwrap())?;
        let flag = |b: bool| if b { "1" } else { "0" };
        let text = format!(
            "setup_done={}\ninstall_dir={}\ndisc={}\nprogram={}\nversion={}\nclose_on_play={}\nlauncher_tag={}\n",
            flag(self.setup_done),
            self.install_dir.display(),
            self.disc
                .as_ref()
                .map(|d| d.display().to_string())
                .unwrap_or_default(),
            if self.program == Program::Viewer {
                "viewer"
            } else {
                "game"
            },
            self.version.as_deref().unwrap_or("latest"),
            flag(self.close_on_play),
            self.launcher_tag,
        );
        fs::write(&path, text).with_context(|| format!("couldn't save {}", path.display()))
    }

    pub fn versions_dir(&self) -> PathBuf {
        self.install_dir.join("versions")
    }

    pub fn version_dir(&self, tag: &str) -> PathBuf {
        self.versions_dir().join(tag)
    }

    /// The installed versions, newest first.
    pub fn installed(&self) -> Vec<String> {
        let mut tags: Vec<String> = fs::read_dir(self.versions_dir())
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| {
                        e.path().join(GAME_ASSET).is_file() && e.path().join(VIEWER_ASSET).is_file()
                    })
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default();
        tags.sort_by(|a, b| net::compare_tags(b, a));
        tags
    }

    pub fn launcher_path(&self) -> PathBuf {
        self.install_dir.join(LAUNCHER_ASSET)
    }
}

/// How far a download's got, shared with the window.
#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub what: String,
    pub done: u64,
    pub total: u64,
}

/// Downloads `release`'s two programs into its version folder (and the
/// launcher's new self, if it has one and this is the installed
/// launcher). Returns whether the launcher was replaced.
pub fn install_version(
    config: &Config,
    release: &Release,
    report: &mut dyn FnMut(Progress),
) -> Result<bool> {
    let dir = config.version_dir(&release.tag);
    fs::create_dir_all(&dir).with_context(|| format!("couldn't make {}", dir.display()))?;
    let mut wanted: Vec<&net::Asset> = [GAME_ASSET, VIEWER_ASSET]
        .iter()
        .filter_map(|n| release.asset(n))
        .collect();
    if wanted.len() < 2 {
        bail!("{} doesn't have both programs to download", release.tag);
    }
    let new_launcher = release
        .asset(LAUNCHER_ASSET)
        .filter(|_| release.tag != config.launcher_tag && running_installed(config));
    if let Some(a) = new_launcher {
        wanted.push(a);
    }
    let total: u64 = wanted.iter().map(|a| a.size).sum();
    let mut before = 0u64;
    for asset in &wanted {
        let what = format!("Downloading {} {}", release.tag, pretty_asset(&asset.name));
        let bytes = net::get(&asset.url, &mut |done, _| {
            report(Progress {
                what: what.clone(),
                done: before + done,
                total,
            });
            true
        })?;
        before += asset.size;
        let target = if asset.name == LAUNCHER_ASSET {
            config.install_dir.join("DESA-Launcher.new.exe")
        } else {
            dir.join(&asset.name)
        };
        let part = target.with_extension("part");
        fs::write(&part, &bytes).with_context(|| format!("couldn't write {}", part.display()))?;
        fs::rename(&part, &target)?;
    }
    if new_launcher.is_some() {
        replace_launcher(config)?;
        return Ok(true);
    }
    Ok(false)
}

fn pretty_asset(name: &str) -> &'static str {
    match name {
        GAME_ASSET => "(game)",
        VIEWER_ASSET => "(Map Viewer)",
        _ => "(launcher)",
    }
}

/// Whether this is the launcher in the install folder (not a copy run
/// from Downloads, say).
pub fn running_installed(config: &Config) -> bool {
    let here = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok());
    let installed = config.launcher_path().canonicalize().ok();
    here.is_some() && here == installed
}

/// Swaps the running launcher for the downloaded one: Windows lets a
/// running program be renamed, not overwritten.
fn replace_launcher(config: &Config) -> Result<()> {
    let current = config.launcher_path();
    let old = config.install_dir.join("DESA-Launcher.old.exe");
    let _ = fs::remove_file(&old);
    fs::rename(&current, &old).context("couldn't move the old launcher aside")?;
    fs::rename(config.install_dir.join("DESA-Launcher.new.exe"), &current)
        .context("couldn't put the new launcher in place")?;
    Ok(())
}

/// Clears what an update left behind.
pub fn tidy(config: &Config) {
    let _ = fs::remove_file(config.install_dir.join("DESA-Launcher.old.exe"));
    let _ = fs::remove_file(config.install_dir.join("DESA-Launcher.new.exe"));
}

/// Copies this launcher into the install folder (setup).
pub fn copy_self(config: &Config) -> Result<()> {
    fs::create_dir_all(&config.install_dir)
        .with_context(|| format!("couldn't make {}", config.install_dir.display()))?;
    if running_installed(config) {
        return Ok(());
    }
    let here = std::env::current_exe()?;
    fs::copy(&here, config.launcher_path()).context("couldn't copy the launcher")?;
    Ok(())
}

const SHORTCUT: &str = "Disney's Extreme Skate Adventure.lnk";

fn shortcut_dirs() -> Vec<(bool, PathBuf)> {
    let mut out = Vec::new();
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        out.push((true, PathBuf::from(profile).join("Desktop")));
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        out.push((
            false,
            PathBuf::from(appdata).join("Microsoft\\Windows\\Start Menu\\Programs"),
        ));
    }
    out
}

/// The game's logo as an icon file, for the shortcuts: made from your
/// disc on this computer, never downloaded.
pub fn logo_icon_path(config: &Config) -> PathBuf {
    config.install_dir.join("logo.ico")
}

/// Writes the logo (RGBA) as `logo.ico` in the install folder, if it's
/// installed there and the icon isn't yet; then gives the shortcuts
/// already made that icon. True if it was written.
pub fn write_logo_icon(config: &Config, rgba: &[u8], width: u32, height: u32) -> Result<bool> {
    let path = logo_icon_path(config);
    if !config.launcher_path().is_file() || path.is_file() {
        return Ok(false);
    }
    let ico = ico_bytes(rgba, width, height)?;
    fs::write(&path, ico).with_context(|| format!("couldn't write {}", path.display()))?;
    let (mut desktop, mut start_menu) = (false, false);
    for (is_desktop, dir) in shortcut_dirs() {
        if dir.join(SHORTCUT).is_file() {
            if is_desktop {
                desktop = true;
            } else {
                start_menu = true;
            }
        }
    }
    if desktop || start_menu {
        make_shortcuts(config, desktop, start_menu)?;
    }
    Ok(true)
}

/// An `.ico` of one PNG picture of `rgba`.
fn ico_bytes(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(rgba)?;
    }
    // (0 for a side means 256.)
    let side = |n: u32| if n >= 256 { 0 } else { n as u8 };
    let mut ico = vec![0, 0, 1, 0, 1, 0];
    ico.extend_from_slice(&[side(width), side(height), 0, 0, 1, 0, 32, 0]);
    ico.extend_from_slice(&(png_bytes.len() as u32).to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());
    ico.extend_from_slice(&png_bytes);
    Ok(ico)
}

/// Desktop and Start menu shortcuts to the installed launcher, with the
/// logo from the disc if it's been made, else the launcher's own icon.
pub fn make_shortcuts(config: &Config, desktop: bool, start_menu: bool) -> Result<()> {
    let target = config.launcher_path();
    let logo = logo_icon_path(config);
    let icon = if logo.is_file() { logo } else { target.clone() };
    for (is_desktop, dir) in shortcut_dirs() {
        if (is_desktop && !desktop) || (!is_desktop && !start_menu) {
            continue;
        }
        let link = dir.join(SHORTCUT);
        let quote = |p: &Path| p.display().to_string().replace('\'', "''");
        let script = format!(
            "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.WorkingDirectory='{}';$s.IconLocation='{},0';$s.Description='Disney''s Extreme Skate Adventure launcher';$s.Save()",
            quote(&link),
            quote(&target),
            quote(&config.install_dir),
            quote(&icon),
        );
        let status = hidden(Command::new("powershell").args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ]))
        .status()
        .context("couldn't run PowerShell for the shortcut")?;
        if !status.success() {
            bail!("couldn't make the shortcut {}", link.display());
        }
    }
    Ok(())
}

/// No console window flashing up for a helper.
fn hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

/// Starts `program` from version `tag`, on the disc image.
pub fn launch(config: &Config, tag: &str, program: Program) -> Result<()> {
    let dir = config.version_dir(tag);
    let exe = dir.join(program.asset());
    if !exe.is_file() {
        bail!("{tag} isn't installed");
    }
    let mut cmd = Command::new(&exe);
    cmd.current_dir(&dir);
    if let Some(disc) = &config.disc {
        cmd.arg(disc);
    }
    cmd.spawn()
        .with_context(|| format!("couldn't start {}", exe.display()))?;
    Ok(())
}

/// Removes one installed version.
pub fn remove_version(config: &Config, tag: &str) -> Result<()> {
    let dir = config.version_dir(tag);
    fs::remove_dir_all(&dir).with_context(|| format!("couldn't remove {}", dir.display()))
}

/// Removes what setup made: the versions, the shortcuts, the launcher's
/// settings, and (after this launcher closes) the launcher and the install
/// folder if that leaves it empty. The game's own settings and saves
/// (`%APPDATA%\desa-map-viewer`) are kept.
pub fn uninstall(config: &Config) -> Result<()> {
    let _ = fs::remove_dir_all(config.versions_dir());
    let _ = fs::remove_file(logo_icon_path(config));
    for (_, dir) in shortcut_dirs() {
        let _ = fs::remove_file(dir.join(SHORTCUT));
    }
    let _ = fs::remove_file(config_path());
    let _ = config_path().parent().map(fs::remove_dir);
    let launcher = config.launcher_path();
    let script = format!(
        "ping -n 3 127.0.0.1 >nul & del /f /q \"{}\" & rmdir \"{}\"",
        launcher.display(),
        config.install_dir.display()
    );
    hidden(Command::new("cmd").args(["/c", &script]))
        .spawn()
        .context("couldn't schedule removing the launcher")?;
    Ok(())
}

/// Opens a folder in Explorer.
pub fn open_folder(path: &Path) {
    let _ = fs::create_dir_all(path);
    let _ = Command::new("explorer").arg(path).spawn();
}

#[cfg(test)]
mod tests {
    #[test]
    fn logo_icon_is_an_ico() {
        let rgba = vec![200u8; 4 * 4 * 4];
        let ico = super::ico_bytes(&rgba, 4, 4).unwrap();
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 1, 0]);
        assert_eq!(&ico[22..26], &[0x89, b'P', b'N', b'G']);
        if let Ok(out) = std::env::var("ICO_TEST_OUT") {
            std::fs::write(out, &ico).unwrap();
        }
    }
}
