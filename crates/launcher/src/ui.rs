//! The launcher's window: a sidebar (the title, Play, Patch Notes,
//! Installations, Settings), the Play page over the disc's own loading
//! screens with the big play button, and the first run's setup.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use egui::{
    Align2, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, RichText, Sense, Stroke,
    TextureHandle, pos2, vec2,
};

use crate::install::{self, Config, Program, Progress};
use crate::net::{self, Release};

pub const NAVY: Color32 = Color32::from_rgb(13, 17, 32);
const SIDEBAR: Color32 = Color32::from_rgb(18, 25, 46);
const CARD: Color32 = Color32::from_rgba_premultiplied(10, 14, 28, 215);
const ORANGE: Color32 = Color32::from_rgb(255, 140, 26);
const YELLOW: Color32 = Color32::from_rgb(255, 210, 63);
const TEAL: Color32 = Color32::from_rgb(39, 196, 180);
const GREEN: Color32 = Color32::from_rgb(64, 191, 64);
const GREEN_DARK: Color32 = Color32::from_rgb(38, 128, 38);
const TEXT: Color32 = Color32::from_rgb(232, 236, 245);
const MUTED: Color32 = Color32::from_rgb(141, 151, 176);
const RED: Color32 = Color32::from_rgb(230, 80, 80);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Play,
    Notes,
    Installs,
    Settings,
}

impl Page {
    pub fn parse(s: &str) -> Option<Page> {
        Some(match s {
            "play" => Page::Play,
            "notes" => Page::Notes,
            "installs" => Page::Installs,
            "settings" => Page::Settings,
            _ => return None,
        })
    }
}

enum Msg {
    Releases(Result<Vec<Release>, String>),
    Progress(Progress),
    Installed(Result<(String, bool), String>),
    SetupDone(Result<(), String>),
    Background(ngc_texture::Image),
}

/// The first run's steps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Welcome,
    Folder,
    Disc,
    Shortcuts,
    Installing,
    Done,
}

pub struct Setup {
    pub step: Step,
    desktop: bool,
    start_menu: bool,
    disc_note: Option<(bool, String)>,
}

pub struct Launcher {
    pub config: Config,
    pub page: Page,
    releases: Option<Result<Vec<Release>, String>>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    busy: Option<Progress>,
    message: Option<(bool, String)>,
    backgrounds: Vec<TextureHandle>,
    pending: Vec<ngc_texture::Image>,
    started: Instant,
    pub setup: Option<Setup>,
    restart: bool,
    pub quit: bool,
    loading_art: bool,
    /// Your own `logo.png`, if you've put one by the launcher.
    logo: Option<Option<TextureHandle>>,
}

impl Launcher {
    pub fn new(config: Config) -> Launcher {
        let (tx, rx) = channel();
        install::tidy(&config);
        let setup = (!config.setup_done).then_some(Setup {
            step: Step::Welcome,
            desktop: true,
            start_menu: true,
            disc_note: None,
        });
        let mut config = config;
        if config.disc.is_none() {
            config.disc = game_disc();
        }
        let mut l = Launcher {
            config,
            page: Page::Play,
            releases: None,
            tx,
            rx,
            busy: None,
            message: None,
            backgrounds: Vec::new(),
            pending: Vec::new(),
            started: Instant::now(),
            setup,
            restart: false,
            quit: false,
            loading_art: false,
            logo: None,
        };
        l.refresh();
        l.load_art();
        l
    }

    /// For screenshots: everything fetched before the first frame.
    pub fn wait_for_data(&mut self) {
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
        while (self.releases.is_none() || self.loading_art) && Instant::now() < deadline {
            if let Ok(msg) = self.rx.recv_timeout(std::time::Duration::from_millis(100)) {
                self.handle(msg);
            }
            if self.pending.len() >= 2 {
                self.loading_art = false;
            }
        }
    }

    pub fn refresh(&mut self) {
        self.releases = None;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Releases(net::releases().map_err(|e| format!("{e:#}"))));
        });
    }

    /// The disc's loading screens, decoded in the background.
    fn load_art(&mut self) {
        self.backgrounds.clear();
        let Some(disc) = self.config.disc.clone() else {
            return;
        };
        self.loading_art = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let Ok(mut data) = desa_viewer::source::GameData::open(&disc) else {
                return;
            };
            let mut names: Vec<String> = data
                .loading_screens()
                .into_iter()
                .filter(|n| n.to_ascii_lowercase().starts_with("loadscrn"))
                .collect();
            // A different order each run.
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let n = names.len().max(1);
            names.rotate_left((seed as usize * 7) % n);
            for name in names.iter().step_by(2).take(14) {
                if let Ok(Some(image)) = data.loading_screen(name)
                    && tx.send(Msg::Background(image)).is_err()
                {
                    return;
                }
            }
        });
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Releases(r) => self.releases = Some(r),
            Msg::Progress(p) => self.busy = Some(p),
            Msg::Installed(r) => {
                self.busy = None;
                match r {
                    Ok((tag, replaced)) => {
                        if replaced {
                            self.config.launcher_tag = tag.clone();
                            self.restart = true;
                        }
                        let _ = self.config.save();
                        self.message = Some((false, format!("{tag} is installed.")));
                    }
                    Err(e) => self.message = Some((true, e)),
                }
            }
            Msg::SetupDone(r) => {
                self.busy = None;
                if let Some(s) = &mut self.setup {
                    match r {
                        Ok(()) => s.step = Step::Done,
                        Err(e) => {
                            s.step = Step::Shortcuts;
                            self.message = Some((true, e));
                        }
                    }
                }
            }
            Msg::Background(image) => self.pending.push(image),
        }
    }

    fn latest(&self) -> Option<&Release> {
        self.releases
            .as_ref()?
            .as_ref()
            .ok()?
            .iter()
            .find(|r| r.playable())
    }

    fn release(&self, tag: &str) -> Option<&Release> {
        self.releases
            .as_ref()?
            .as_ref()
            .ok()?
            .iter()
            .find(|r| r.tag == tag)
    }

    /// The version the play button means: the chosen one, or the latest
    /// (online), or the newest installed (offline).
    fn target(&self) -> Option<String> {
        if let Some(v) = &self.config.version {
            return Some(v.clone());
        }
        self.latest()
            .map(|r| r.tag.clone())
            .or_else(|| self.config.installed().into_iter().next())
    }

    fn install(&mut self, tag: String) {
        let Some(release) = self.release(&tag).cloned() else {
            self.message = Some((true, format!("{tag} isn't on GitHub")));
            return;
        };
        self.busy = Some(Progress {
            what: format!("Starting {tag}"),
            ..Default::default()
        });
        self.message = None;
        let config = self.config.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let progress_tx = tx.clone();
            let r = install::install_version(&config, &release, &mut |p| {
                let _ = progress_tx.send(Msg::Progress(p));
            });
            let _ = tx.send(Msg::Installed(
                r.map(|replaced| (release.tag.clone(), replaced))
                    .map_err(|e| format!("{e:#}")),
            ));
        });
    }

    fn play(&mut self) {
        let Some(tag) = self.target() else {
            self.message = Some((true, "No version to play: check your connection.".into()));
            return;
        };
        if !self.config.installed().contains(&tag) {
            self.install(tag);
            return;
        }
        if self.config.disc.is_none() {
            self.pick_disc();
            if self.config.disc.is_none() {
                return;
            }
        }
        match install::launch(&self.config, &tag, self.config.program) {
            Ok(()) => {
                self.message = Some((
                    false,
                    format!("Started {} {tag}.", self.config.program.label()),
                ));
                if self.config.close_on_play {
                    self.quit = true;
                }
            }
            Err(e) => self.message = Some((true, format!("{e:#}"))),
        }
    }

    fn pick_disc(&mut self) -> Option<(bool, String)> {
        let picked = rfd::FileDialog::new()
            .set_title("Your Disney's Extreme Skate Adventure disc image")
            .add_filter("GameCube disc image", &["iso", "gcm"])
            .pick_file()?;
        let note = check_disc(&picked);
        if note.0 {
            self.config.disc = Some(picked);
            let _ = self.config.save();
            self.load_art();
        }
        Some(note)
    }

    // ---- drawing ----

    pub fn ui(&mut self, ctx: &egui::Context) {
        if self.logo.is_none() {
            self.logo = Some(
                user_logo()
                    .map(|image| ctx.load_texture("logo", image, egui::TextureOptions::LINEAR)),
            );
        }
        while let Ok(msg) = self.rx.try_recv() {
            self.handle(msg);
        }
        for image in self.pending.drain(..) {
            let color = egui::ColorImage::from_rgba_unmultiplied(
                [image.width as usize, image.height as usize],
                &image.rgba,
            );
            let n = self.backgrounds.len();
            self.backgrounds.push(ctx.load_texture(
                format!("art{n}"),
                color,
                egui::TextureOptions::LINEAR,
            ));
        }

        egui::SidePanel::left("nav")
            .exact_width(236.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(SIDEBAR))
            .show(ctx, |ui| self.sidebar(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(NAVY))
            .show(ctx, |ui| match self.page {
                Page::Play => self.play_page(ui),
                Page::Notes => self.notes_page(ui),
                Page::Installs => self.installs_page(ui),
                Page::Settings => self.settings_page(ui),
            });
        if self.setup.is_some() {
            self.setup_window(ctx);
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let logo = self.logo.clone().flatten();
        let head = if logo.is_some() { 214.0 } else { 196.0 };
        let area =
            Rect::from_min_size(rect.left_top() + vec2(18.0, 14.0), vec2(200.0, head - 24.0));
        title(ui.painter(), area, logo.as_ref());
        ui.add_space(head);
        for (page, label, icon) in [
            (Page::Play, "PLAY", "▶"),
            (Page::Notes, "PATCH NOTES", "📰"),
            (Page::Installs, "INSTALLATIONS", "📦"),
            (Page::Settings, "SETTINGS", "⚙"),
        ] {
            let selected = self.page == page;
            let (r, response) = ui.allocate_exact_size(vec2(rect.width(), 46.0), Sense::click());
            let p = ui.painter();
            if selected {
                p.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(255, 140, 26, 34));
                p.rect_filled(
                    Rect::from_min_size(r.min, vec2(5.0, r.height())),
                    0.0,
                    ORANGE,
                );
            } else if response.hovered() {
                p.rect_filled(r, 0.0, Color32::from_white_alpha(10));
            }
            p.text(
                r.left_center() + vec2(26.0, 0.0),
                Align2::LEFT_CENTER,
                icon,
                FontId::proportional(17.0),
                if selected { ORANGE } else { MUTED },
            );
            p.text(
                r.left_center() + vec2(56.0, 1.0),
                Align2::LEFT_CENTER,
                label,
                impact(19.0),
                if selected { TEXT } else { MUTED },
            );
            if response.clicked() {
                self.page = page;
            }
        }
        // The foot: where updates come from.
        let foot = rect.left_bottom() + vec2(20.0, -22.0);
        let p = ui.painter();
        for (i, line) in ["A fan-made Rust recomp,", "not affiliated with Disney"]
            .iter()
            .enumerate()
        {
            p.text(
                foot - vec2(0.0, 40.0 - i as f32 * 15.0),
                Align2::LEFT_BOTTOM,
                *line,
                FontId::proportional(11.0),
                MUTED,
            );
        }
        let gh = p.text(
            foot,
            Align2::LEFT_BOTTOM,
            "github.com/FrosttysBots",
            FontId::proportional(12.5),
            TEAL,
        );
        let response = ui.interact(gh, ui.id().with("gh"), Sense::click());
        if response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            open_url(&format!("https://github.com/{}", net::REPO));
        }
    }

    fn play_page(&mut self, ui: &mut egui::Ui) {
        let full = ui.max_rect();
        let bar_h = 104.0;
        let art = Rect::from_min_max(full.min, pos2(full.max.x, full.max.y - bar_h));
        let t = self.started.elapsed().as_secs_f32();
        self.draw_art(ui, art, t);

        // What's new, over the art.
        if let Some(latest) = self.latest().cloned() {
            let card = Rect::from_min_size(art.min + vec2(28.0, 26.0), vec2(420.0, 0.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
                card.min,
                vec2(420.0, art.height() - 52.0),
            )));
            egui::Frame::new()
                .fill(CARD)
                .corner_radius(CornerRadius::same(10))
                .inner_margin(16.0)
                .stroke(Stroke::new(1.0_f32, Color32::from_white_alpha(18)))
                .show(&mut child, |ui| {
                    ui.set_width(388.0);
                    ui.horizontal(|ui| {
                        chip(ui, "WHAT'S NEW", ORANGE);
                        ui.label(RichText::new(&latest.tag).color(MUTED).size(13.0));
                        ui.label(RichText::new(&latest.date).color(MUTED).size(13.0));
                    });
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(release_title(&latest))
                            .font(impact(26.0))
                            .color(TEXT),
                    );
                    ui.add_space(4.0);
                    for line in highlights(&latest.body, 5) {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new("•").color(YELLOW).strong());
                            ui.label(RichText::new(line).color(TEXT).size(14.0));
                        });
                    }
                    ui.add_space(6.0);
                    if ui
                        .link(
                            RichText::new("Read the patch notes →")
                                .color(TEAL)
                                .size(14.0),
                        )
                        .clicked()
                    {
                        self.page = Page::Notes;
                    }
                });
        }

        // The bar along the bottom.
        let bar = Rect::from_min_max(pos2(full.min.x, full.max.y - bar_h), full.max);
        let p = ui.painter();
        p.rect_filled(bar, 0.0, Color32::from_rgb(16, 21, 40));
        p.line_segment(
            [bar.left_top(), bar.right_top()],
            Stroke::new(2.0_f32, ORANGE.gamma_multiply(0.7)),
        );

        // Left: what and which version.
        let mut left = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
            bar.min + vec2(24.0, 18.0),
            vec2(260.0, 72.0),
        )));
        left.label(RichText::new("PROGRAM").size(11.0).color(MUTED));
        let mut program = self.config.program;
        egui::ComboBox::from_id_salt("program")
            .width(230.0)
            .selected_text(program.label())
            .show_ui(&mut left, |ui| {
                ui.selectable_value(&mut program, Program::Game, Program::Game.label());
                ui.selectable_value(&mut program, Program::Viewer, Program::Viewer.label());
            });
        left.add_space(2.0);
        let installed = self.config.installed();
        let mut version = self.config.version.clone();
        let latest_tag = self.latest().map(|r| r.tag.clone());
        let shown = match (&version, &latest_tag) {
            (Some(v), _) => v.clone(),
            (None, Some(t)) => format!("Latest ({t})"),
            (None, None) => "Latest".into(),
        };
        let mut tags: Vec<String> = self
            .releases
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .map(|r| {
                r.iter()
                    .filter(|r| r.playable())
                    .map(|r| r.tag.clone())
                    .collect()
            })
            .unwrap_or_default();
        for t in &installed {
            if !tags.contains(t) {
                tags.push(t.clone());
            }
        }
        egui::ComboBox::from_id_salt("version")
            .width(230.0)
            .selected_text(shown)
            .show_ui(&mut left, |ui| {
                ui.selectable_value(&mut version, None, "Latest");
                for t in &tags {
                    let mark = if installed.contains(t) { "  ✔" } else { "" };
                    ui.selectable_value(&mut version, Some(t.clone()), format!("{t}{mark}"));
                }
            });
        if program != self.config.program || version != self.config.version {
            self.config.program = program;
            self.config.version = version;
            let _ = self.config.save();
        }

        // Middle: the big button, or the download's progress.
        let button = Rect::from_center_size(bar.center() + vec2(-10.0, 0.0), vec2(280.0, 66.0));
        let target = self.target();
        let ready = target.as_ref().is_some_and(|t| installed.contains(t));
        let newer = match (&latest_tag, installed.first()) {
            (Some(l), Some(i)) => self.config.version.is_none() && net::compare_tags(l, i).is_gt(),
            _ => false,
        };
        if let Some(progress) = &self.busy {
            let fraction = if progress.total > 0 {
                progress.done as f32 / progress.total as f32
            } else {
                0.0
            };
            let p = ui.painter();
            p.rect_filled(button, 8.0, Color32::from_rgb(28, 36, 64));
            let mut fill = button;
            fill.max.x = fill.min.x + button.width() * fraction.clamp(0.0, 1.0);
            p.rect_filled(fill, 8.0, GREEN_DARK);
            p.text(
                button.center() - vec2(0.0, 9.0),
                Align2::CENTER_CENTER,
                format!("{:.0}%", fraction * 100.0),
                impact(24.0),
                TEXT,
            );
            p.text(
                button.center() + vec2(0.0, 16.0),
                Align2::CENTER_CENTER,
                format!(
                    "{}  ·  {:.1} / {:.1} MB",
                    progress.what,
                    progress.done as f64 / 1e6,
                    progress.total as f64 / 1e6
                ),
                FontId::proportional(11.5),
                TEXT,
            );
        } else {
            let label = if target.is_none() {
                "OFFLINE"
            } else if !ready {
                if installed.is_empty() {
                    "INSTALL"
                } else {
                    "UPDATE"
                }
            } else {
                "PLAY"
            };
            if big_button(ui, button, label, target.is_some()) {
                self.play();
            }
        }

        // Right: how things stand.
        let right = pos2(bar.max.x - 24.0, bar.min.y + 26.0);
        let p = ui.painter();
        let status = match (&self.releases, &target) {
            (None, _) => ("Checking GitHub for updates…".to_string(), MUTED),
            (Some(Err(_)), Some(t)) => (format!("Offline · {t} installed"), MUTED),
            (Some(Err(_)), None) => ("Offline · nothing installed".into(), RED),
            (Some(Ok(_)), Some(t)) if ready && !newer => (format!("{t} · up to date"), TEAL),
            (Some(Ok(_)), Some(t)) if ready => (format!("{t} installed"), TEXT),
            (Some(Ok(_)), Some(t)) => (format!("{t} ready to download"), YELLOW),
            (Some(Ok(_)), None) => ("No releases found".into(), RED),
        };
        p.text(
            right,
            Align2::RIGHT_CENTER,
            status.0,
            FontId::proportional(14.0),
            status.1,
        );
        let disc = match &self.config.disc {
            Some(d) => format!(
                "Disc: {}",
                d.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            ),
            None => "No disc image chosen".into(),
        };
        p.text(
            right + vec2(0.0, 22.0),
            Align2::RIGHT_CENTER,
            ellipsize(&disc, 34),
            FontId::proportional(12.5),
            if self.config.disc.is_some() {
                MUTED
            } else {
                YELLOW
            },
        );
        if let Some((error, text)) = &self.message {
            p.text(
                right + vec2(0.0, 44.0),
                Align2::RIGHT_CENTER,
                ellipsize(text, 40),
                FontId::proportional(12.5),
                if *error { RED } else { GREEN },
            );
        }
        if self.restart {
            let r = Rect::from_min_size(art.right_top() + vec2(-330.0, 20.0), vec2(306.0, 44.0));
            let p = ui.painter();
            p.rect_filled(r, 8.0, TEAL.gamma_multiply(0.9));
            p.text(
                r.center(),
                Align2::CENTER_CENTER,
                "Launcher updated: reopen it to finish",
                FontId::proportional(14.0),
                NAVY,
            );
        }
    }

    /// The disc's loading screens, one fading into the next, or our own
    /// sunset over a ramp without one.
    fn draw_art(&self, ui: &egui::Ui, rect: Rect, t: f32) {
        let p = ui.painter_at(rect);
        if self.backgrounds.is_empty() {
            sunset(&p, rect, t);
        } else {
            let hold = 9.0;
            let n = self.backgrounds.len();
            let i = (t / hold) as usize % n;
            let phase = (t % hold) / hold;
            cover(&p, rect, &self.backgrounds[i], 1.0, phase);
            let fade = ((phase - 0.85) / 0.15).clamp(0.0, 1.0);
            if fade > 0.0 && n > 1 {
                cover(&p, rect, &self.backgrounds[(i + 1) % n], fade, 0.0);
            }
        }
        // Darker toward the bottom and left, for the text.
        gradient(
            &p,
            rect,
            Color32::TRANSPARENT,
            Color32::from_black_alpha(170),
            true,
        );
        let left = Rect::from_min_size(rect.min, vec2(rect.width() * 0.55, rect.height()));
        gradient(
            &p,
            left,
            Color32::from_black_alpha(120),
            Color32::TRANSPARENT,
            false,
        );
    }

    fn notes_page(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "PATCH NOTES", "Every release, straight from GitHub.");
        let releases = match &self.releases {
            None => {
                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    ui.add_space(32.0);
                    ui.spinner();
                    ui.label(RichText::new("Fetching releases…").color(MUTED));
                });
                return;
            }
            Some(Err(e)) => {
                offline_note(ui, e);
                return;
            }
            Some(Ok(r)) => r.clone(),
        };
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                for r in releases.iter().take(25) {
                    ui.horizontal(|ui| {
                        ui.add_space(32.0);
                        egui::Frame::new()
                            .fill(Color32::from_rgb(20, 27, 50))
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(18.0)
                            .show(ui, |ui| {
                                ui.set_width((ui.available_width() - 40.0).min(820.0));
                                ui.vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(release_title(r))
                                                .font(impact(24.0))
                                                .color(TEXT),
                                        );
                                        ui.add_space(8.0);
                                        chip(ui, &r.tag, TEAL);
                                        if r.prerelease {
                                            chip(ui, "PRE-RELEASE", ORANGE);
                                        }
                                        ui.label(RichText::new(&r.date).color(MUTED));
                                    });
                                    ui.add_space(6.0);
                                    markdown(ui, notes_only(&r.body));
                                });
                            });
                    });
                    ui.add_space(14.0);
                }
            });
    }

    fn installs_page(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "INSTALLATIONS",
            "Versions on this computer, and the ones on GitHub.",
        );
        let installed = self.config.installed();
        let online: Vec<Release> = self
            .releases
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .map(|r| r.iter().filter(|r| r.playable()).cloned().collect())
            .unwrap_or_default();
        let mut tags: Vec<(String, String)> = online
            .iter()
            .map(|r| (r.tag.clone(), r.date.clone()))
            .collect();
        for t in &installed {
            if !tags.iter().any(|(x, _)| x == t) {
                tags.push((t.clone(), String::new()));
            }
        }
        tags.sort_by(|a, b| net::compare_tags(&b.0, &a.0));
        let busy = self.busy.is_some();
        let mut action: Option<(u8, String)> = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                for (i, (tag, date)) in tags.iter().enumerate() {
                    let is_in = installed.contains(tag);
                    ui.horizontal(|ui| {
                        ui.add_space(32.0);
                        egui::Frame::new()
                            .fill(Color32::from_rgb(20, 27, 50))
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(egui::Margin::symmetric(16, 12))
                            .show(ui, |ui| {
                                ui.set_width((ui.available_width() - 40.0).min(820.0));
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(tag).font(impact(22.0)).color(TEXT));
                                    if i == 0 {
                                        chip(ui, "LATEST", ORANGE);
                                    }
                                    if is_in {
                                        chip(ui, "INSTALLED", GREEN);
                                    }
                                    ui.label(RichText::new(date).color(MUTED));
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if is_in {
                                                if ui
                                                    .add_enabled(!busy, egui::Button::new("Delete"))
                                                    .clicked()
                                                {
                                                    action = Some((2, tag.clone()));
                                                }
                                                if ui.button("Folder").clicked() {
                                                    install::open_folder(
                                                        &self.config.version_dir(tag),
                                                    );
                                                }
                                                if ui
                                                    .add(
                                                        egui::Button::new(
                                                            RichText::new("  Play  ").strong(),
                                                        )
                                                        .fill(GREEN_DARK),
                                                    )
                                                    .clicked()
                                                {
                                                    action = Some((0, tag.clone()));
                                                }
                                            } else if ui
                                                .add_enabled(!busy, egui::Button::new("Download"))
                                                .clicked()
                                            {
                                                action = Some((1, tag.clone()));
                                            }
                                        },
                                    );
                                });
                            });
                    });
                    ui.add_space(8.0);
                }
                if tags.is_empty() {
                    offline_note(ui, "Nothing installed yet, and GitHub can't be reached.");
                }
            });
        match action {
            Some((0, tag)) => {
                self.config.version = Some(tag);
                let _ = self.config.save();
                self.play();
            }
            Some((1, tag)) => self.install(tag),
            Some((_, tag)) => {
                let sure = rfd::MessageDialog::new()
                    .set_title("Delete version")
                    .set_description(format!("Delete {tag} from this computer?"))
                    .set_buttons(rfd::MessageButtons::YesNo)
                    .show();
                if sure == rfd::MessageDialogResult::Yes {
                    if let Err(e) = install::remove_version(&self.config, &tag) {
                        self.message = Some((true, format!("{e:#}")));
                    }
                    if self.config.version.as_deref() == Some(tag.as_str()) {
                        self.config.version = None;
                        let _ = self.config.save();
                    }
                }
            }
            None => {}
        }
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "SETTINGS",
            "Your disc, where the game lives, and the launcher.",
        );
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                let width = (ui.available_width() - 72.0).min(820.0);
                section(
                    ui,
                    width,
                    "DISC IMAGE",
                    |ui, me: &mut Launcher| {
                        ui.label(
                            RichText::new(
                                "Your own copy of the US GameCube game (a plain .iso or .gcm). \
                         Nothing from the game is downloaded.",
                            )
                            .color(MUTED),
                        );
                        ui.horizontal(|ui| {
                            let text = me
                                .config
                                .disc
                                .as_ref()
                                .map(|d| d.display().to_string())
                                .unwrap_or_else(|| "Not chosen".into());
                            ui.label(RichText::new(ellipsize(&text, 70)).color(TEXT).monospace());
                            if ui.button("Browse…").clicked()
                                && let Some((false, e)) = me.pick_disc()
                            {
                                me.message = Some((true, e));
                            }
                        });
                    },
                    self,
                );
                section(
                    ui,
                    width,
                    "INSTALL FOLDER",
                    |ui, me: &mut Launcher| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(ellipsize(
                                    &me.config.install_dir.display().to_string(),
                                    70,
                                ))
                                .color(TEXT)
                                .monospace(),
                            );
                            if ui.button("Open").clicked() {
                                install::open_folder(&me.config.install_dir);
                            }
                        });
                    },
                    self,
                );
                section(
                    ui,
                    width,
                    "LAUNCHER",
                    |ui, me: &mut Launcher| {
                        let mut close = me.config.close_on_play;
                        if ui
                            .checkbox(&mut close, "Close the launcher when the game starts")
                            .changed()
                        {
                            me.config.close_on_play = close;
                            let _ = me.config.save();
                        }
                        ui.horizontal(|ui| {
                            if ui.button("Check for updates").clicked() {
                                me.refresh();
                            }
                            if ui.button("Make desktop & Start menu shortcuts").clicked() {
                                me.message =
                                    Some(match install::make_shortcuts(&me.config, true, true) {
                                        Ok(()) => (false, "Shortcuts made.".into()),
                                        Err(e) => (true, format!("{e:#}")),
                                    });
                            }
                        });
                        if !me.config.launcher_tag.is_empty() {
                            ui.label(
                                RichText::new(format!("Launcher from {}", me.config.launcher_tag))
                                    .color(MUTED),
                            );
                        }
                    },
                    self,
                );
                section(
                    ui,
                    width,
                    "UNINSTALL",
                    |ui, me: &mut Launcher| {
                        ui.label(
                            RichText::new(
                                "Removes the installed versions, the shortcuts and this launcher. \
                         Your game settings and records are kept.",
                            )
                            .color(MUTED),
                        );
                        if ui
                            .add(
                                egui::Button::new(RichText::new("Uninstall…").color(TEXT))
                                    .fill(Color32::from_rgb(120, 30, 30)),
                            )
                            .clicked()
                        {
                            let sure = rfd::MessageDialog::new()
                                .set_title("Uninstall")
                                .set_description(format!(
                                    "Remove Disney's Extreme Skate Adventure (the recomp) from {}?",
                                    me.config.install_dir.display()
                                ))
                                .set_buttons(rfd::MessageButtons::YesNo)
                                .show();
                            if sure == rfd::MessageDialogResult::Yes {
                                match install::uninstall(&me.config) {
                                    Ok(()) => me.quit = true,
                                    Err(e) => me.message = Some((true, format!("{e:#}"))),
                                }
                            }
                        }
                    },
                    self,
                );
                if let Some((error, text)) = &self.message {
                    ui.horizontal(|ui| {
                        ui.add_space(32.0);
                        ui.label(RichText::new(text).color(if *error { RED } else { GREEN }));
                    });
                }
            });
    }

    fn setup_window(&mut self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        egui::Area::new("setup_dim".into())
            .fixed_pos(screen.min)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let p = ui.painter();
                if self.backgrounds.is_empty() {
                    sunset(p, screen, self.started.elapsed().as_secs_f32());
                } else {
                    cover(p, screen, &self.backgrounds[0], 1.0, 0.0);
                }
                p.rect_filled(screen, 0.0, Color32::from_black_alpha(150));
                ui.allocate_rect(screen, Sense::click());
            });
        egui::Area::new("setup".into())
            .order(egui::Order::Tooltip)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(18, 25, 46))
                    .corner_radius(CornerRadius::same(14))
                    .stroke(Stroke::new(2.0_f32, ORANGE))
                    .inner_margin(28.0)
                    .show(ui, |ui| {
                        ui.set_width(560.0);
                        let (r, _) = ui.allocate_exact_size(vec2(560.0, 150.0), Sense::hover());
                        let area = Rect::from_center_size(r.center(), vec2(200.0, 150.0));
                        title(ui.painter(), area, self.logo.clone().flatten().as_ref());
                        self.setup_step(ui);
                    });
            });
    }

    fn setup_step(&mut self, ui: &mut egui::Ui) {
        let Some(step) = self.setup.as_ref().map(|s| s.step) else {
            return;
        };
        let heading = |ui: &mut egui::Ui, text: &str| {
            ui.label(RichText::new(text).font(impact(26.0)).color(YELLOW));
            ui.add_space(4.0);
        };
        let body = |ui: &mut egui::Ui, text: &str| {
            ui.label(RichText::new(text).color(TEXT).size(14.5));
        };
        let mut next = None;
        match step {
            Step::Welcome => {
                heading(ui, "WELCOME, SKATER!");
                body(
                    ui,
                    "This sets up the Rust recomp of Disney's Extreme Skate Adventure: it downloads \
                     the game from GitHub, keeps it up to date, and plays it from your own disc image.",
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "No game data is downloaded: you need your own copy of the GameCube game.",
                    )
                    .color(MUTED),
                );
                ui.add_space(14.0);
                if wide_button(ui, "GET STARTED") {
                    next = Some(Step::Folder);
                }
            }
            Step::Folder => {
                heading(ui, "WHERE TO INSTALL");
                body(ui, "The game's versions and the launcher go here.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(ellipsize(
                            &self.config.install_dir.display().to_string(),
                            56,
                        ))
                        .monospace()
                        .color(TEXT),
                    );
                    if ui.button("Change…").clicked()
                        && let Some(dir) = rfd::FileDialog::new().pick_folder()
                    {
                        self.config.install_dir = dir.join("DESA Recomp");
                    }
                });
                ui.add_space(14.0);
                if wide_button(ui, "NEXT") {
                    next = Some(Step::Disc);
                }
            }
            Step::Disc => {
                heading(ui, "YOUR DISC IMAGE");
                body(
                    ui,
                    "Pick your copy of the US GameCube game, a plain .iso or .gcm. (Convert .rvz files \
                     to ISO in Dolphin first: right-click the game, Convert File.)",
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let text = self
                        .config
                        .disc
                        .as_ref()
                        .map(|d| d.display().to_string())
                        .unwrap_or_else(|| "Not chosen".into());
                    ui.label(RichText::new(ellipsize(&text, 52)).monospace().color(TEXT));
                    if ui.button("Browse…").clicked()
                        && let Some(note) = self.pick_disc()
                        && let Some(s) = &mut self.setup
                    {
                        s.disc_note = Some(note);
                    }
                });
                let note = self
                    .setup
                    .as_ref()
                    .and_then(|s| s.disc_note.clone())
                    .or_else(|| self.config.disc.as_ref().map(|d| check_disc(d)));
                if let Some((ok, text)) = note {
                    ui.label(RichText::new(text).color(if ok { GREEN } else { RED }));
                }
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if wide_button(ui, "NEXT") {
                        next = Some(Step::Shortcuts);
                    }
                    if self.config.disc.is_none()
                        && ui
                            .link(RichText::new("Skip for now").color(MUTED))
                            .clicked()
                    {
                        next = Some(Step::Shortcuts);
                    }
                });
            }
            Step::Shortcuts => {
                heading(ui, "READY TO DROP IN");
                if let Some(s) = &mut self.setup {
                    ui.checkbox(&mut s.desktop, "Desktop shortcut");
                    ui.checkbox(&mut s.start_menu, "Start menu shortcut");
                }
                ui.add_space(6.0);
                let latest = self.latest().map(|r| r.tag.clone());
                body(
                    ui,
                    &match &latest {
                        Some(t) => format!("Installing {t}, the latest release."),
                        None if self.releases.is_none() => {
                            "Checking GitHub for the latest release…".into()
                        }
                        None => "GitHub can't be reached: check your connection.".into(),
                    },
                );
                if let Some((true, e)) = &self.message {
                    ui.label(RichText::new(e).color(RED));
                }
                ui.add_space(14.0);
                if latest.is_some() && wide_button(ui, "INSTALL") {
                    self.run_setup();
                    next = Some(Step::Installing);
                } else if latest.is_none()
                    && self.releases.as_ref().is_some_and(|r| r.is_err())
                    && ui.button("Try again").clicked()
                {
                    self.refresh();
                }
            }
            Step::Installing => {
                heading(ui, "INSTALLING…");
                let (what, fraction) = match &self.busy {
                    Some(p) if p.total > 0 => (p.what.clone(), p.done as f32 / p.total as f32),
                    Some(p) => (p.what.clone(), 0.0),
                    None => ("Starting".into(), 0.0),
                };
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .show_percentage()
                        .fill(GREEN_DARK),
                );
                ui.label(RichText::new(what).color(MUTED));
            }
            Step::Done => {
                heading(ui, "ALL SET!");
                body(
                    ui,
                    "The game is installed. Open it any time from the launcher; it checks GitHub for \
                     updates each time it starts.",
                );
                ui.add_space(14.0);
                if wide_button(ui, "LET'S SKATE") {
                    self.finish_setup();
                }
            }
        }
        if let Some(step) = next
            && let Some(s) = &mut self.setup
        {
            s.step = step;
        }
    }

    fn run_setup(&mut self) {
        let Some(release) = self.latest().cloned() else {
            return;
        };
        let (desktop, start_menu) = self
            .setup
            .as_ref()
            .map(|s| (s.desktop, s.start_menu))
            .unwrap_or((true, true));
        self.message = None;
        let config = self.config.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let r = (|| -> anyhow::Result<()> {
                install::copy_self(&config)?;
                install::make_shortcuts(&config, desktop, start_menu)?;
                let progress_tx = tx.clone();
                install::install_version(&config, &release, &mut |p| {
                    let _ = progress_tx.send(Msg::Progress(p));
                })?;
                Ok(())
            })();
            let _ = tx.send(Msg::SetupDone(r.map_err(|e| format!("{e:#}"))));
        });
        if let Some(r) = self.latest() {
            self.config.launcher_tag = r.tag.clone();
        }
    }

    fn finish_setup(&mut self) {
        self.config.setup_done = true;
        let _ = self.config.save();
        self.setup = None;
        // Run from Downloads: hand over to the installed launcher.
        if !install::running_installed(&self.config) {
            let path = self.config.launcher_path();
            if std::process::Command::new(&path).spawn().is_ok() {
                self.quit = true;
            }
        }
    }
}

// ---- pieces ----

/// The game's disc image the game itself last used, if any.
fn game_disc() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("APPDATA")?)
        .join("desa-map-viewer")
        .join("settings.txt");
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("data_path="))
        .map(|p| PathBuf::from(p.trim()))
        .filter(|p| p.is_file())
}

fn check_disc(path: &std::path::Path) -> (bool, String) {
    match desa_viewer::source::GameData::open(path) {
        Ok(data) => {
            let levels = data.levels().len();
            if levels == 0 {
                (
                    false,
                    "That disc doesn't look like Disney's Extreme Skate Adventure.".into(),
                )
            } else {
                (true, format!("✔ Found the game: {levels} levels."))
            }
        }
        Err(e) => (false, format!("Can't read that disc: {e:#}")),
    }
}

pub fn impact(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("impact".into()))
}

/// Your own `logo.png`, from beside the launcher or in its settings
/// folder: the launcher carries no logo of the game's.
fn user_logo() -> Option<egui::ColorImage> {
    let mut places = Vec::new();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.to_path_buf()))
    {
        places.push(dir.join("logo.png"));
    }
    if let Some(app) = std::env::var_os("APPDATA") {
        places.push(PathBuf::from(app).join("desa-launcher").join("logo.png"));
    }
    places.into_iter().find_map(|path| {
        let file = std::fs::File::open(path).ok()?;
        let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder.read_info().ok()?;
        let mut buf = vec![0; reader.output_buffer_size()?];
        let info = reader.next_frame(&mut buf).ok()?;
        let (w, h) = (info.width as usize, info.height as usize);
        let rgba: Vec<u8> = match info.color_type {
            png::ColorType::Rgba => buf[..w * h * 4].to_vec(),
            png::ColorType::Rgb => buf[..w * h * 3]
                .chunks(3)
                .flat_map(|c| [c[0], c[1], c[2], 255])
                .collect(),
            png::ColorType::GrayscaleAlpha => buf[..w * h * 2]
                .chunks(2)
                .flat_map(|c| [c[0], c[0], c[0], c[1]])
                .collect(),
            png::ColorType::Grayscale => {
                buf[..w * h].iter().flat_map(|&g| [g, g, g, 255]).collect()
            }
            png::ColorType::Indexed => return None,
        };
        Some(egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba))
    })
}

/// The title in `area`: your logo fitted in, or our own type.
fn title(p: &egui::Painter, area: Rect, logo: Option<&TextureHandle>) {
    match logo {
        Some(tex) => {
            let [w, h] = tex.size();
            let scale = (area.width() / w as f32).min(area.height() / h as f32);
            let rect = Rect::from_center_size(area.center(), vec2(w as f32, h as f32) * scale);
            p.image(
                tex.id(),
                rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        None => title_block(p, area.left_top() + vec2(0.0, 8.0), 1.0),
    }
}

/// The title: our own type, not the box art's logo.
pub fn title_block(p: &egui::Painter, at: Pos2, scale: f32) {
    let s = scale;
    let outlined = |text: &str, pos: Pos2, size: f32, color: Color32, angle: f32| {
        let galley = p.layout_no_wrap(text.to_string(), impact(size * s), color);
        for (dx, dy) in [
            (-2.0, 0.0),
            (2.0, 0.0),
            (0.0, -2.0),
            (0.0, 2.0),
            (2.0, 3.0),
            (3.0, 4.0),
        ] {
            p.add(
                egui::epaint::TextShape::new(
                    pos + vec2(dx, dy) * s,
                    galley.clone(),
                    Color32::BLACK,
                )
                .with_override_text_color(Color32::from_rgb(10, 12, 24))
                .with_angle(angle),
            );
        }
        p.add(egui::epaint::TextShape::new(pos, galley, color).with_angle(angle));
    };
    let tilt = -0.07;
    p.text(
        at + vec2(4.0, 0.0) * s,
        Align2::LEFT_TOP,
        "D I S N E Y ' S",
        FontId::proportional(13.0 * s),
        TEXT,
    );
    outlined("EXTREME", at + vec2(0.0, 22.0) * s, 46.0, YELLOW, tilt);
    outlined("SKATE", at + vec2(10.0, 70.0) * s, 52.0, ORANGE, tilt);
    let band = Rect::from_min_size(at + vec2(2.0, 130.0) * s, vec2(178.0, 24.0) * s);
    p.rect_filled(band, 4.0, TEAL);
    p.text(
        band.center(),
        Align2::CENTER_CENTER,
        "ADVENTURE",
        impact(19.0 * s),
        NAVY,
    );
}

fn chip(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.22))
        .stroke(Stroke::new(1.0_f32, color))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(color).size(11.5).strong());
        });
}

/// The big green button; true when clicked.
fn big_button(ui: &mut egui::Ui, rect: Rect, label: &str, enabled: bool) -> bool {
    let response = ui.interact(rect, ui.id().with("play"), Sense::click());
    let p = ui.painter();
    let hovered = response.hovered() && enabled;
    let down = response.is_pointer_button_down_on() && enabled;
    let (top, edge) = if !enabled {
        (Color32::from_rgb(60, 66, 84), Color32::from_rgb(40, 44, 58))
    } else if hovered {
        (Color32::from_rgb(84, 214, 84), GREEN_DARK)
    } else {
        (GREEN, GREEN_DARK)
    };
    let shift = if down { 3.0 } else { 0.0 };
    p.rect_filled(rect.translate(vec2(0.0, 5.0)), 10.0, edge);
    let face = rect.translate(vec2(0.0, shift));
    p.rect_filled(face, 10.0, top);
    p.rect_filled(
        Rect::from_min_size(face.min + vec2(6.0, 4.0), vec2(face.width() - 12.0, 6.0)),
        3.0,
        Color32::from_white_alpha(40),
    );
    p.text(
        face.center() + vec2(2.0, 2.0),
        Align2::CENTER_CENTER,
        label,
        impact(36.0),
        Color32::from_black_alpha(90),
    );
    p.text(
        face.center(),
        Align2::CENTER_CENTER,
        label,
        impact(36.0),
        Color32::WHITE,
    );
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    enabled && response.clicked()
}

fn wide_button(ui: &mut egui::Ui, label: &str) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(220.0, 50.0), Sense::hover());
    big_button_small(ui, rect, label)
}

fn big_button_small(ui: &mut egui::Ui, rect: Rect, label: &str) -> bool {
    let response = ui.interact(rect, ui.id().with(label), Sense::click());
    let p = ui.painter();
    let top = if response.hovered() {
        Color32::from_rgb(84, 214, 84)
    } else {
        GREEN
    };
    p.rect_filled(rect.translate(vec2(0.0, 4.0)), 8.0, GREEN_DARK);
    p.rect_filled(rect, 8.0, top);
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        impact(26.0),
        Color32::WHITE,
    );
    response.clicked()
}

fn page_header(ui: &mut egui::Ui, title: &str, sub: &str) {
    ui.add_space(26.0);
    ui.horizontal(|ui| {
        ui.add_space(32.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(title).font(impact(36.0)).color(TEXT));
            ui.label(RichText::new(sub).color(MUTED).size(14.0));
        });
    });
    let y = ui.cursor().min.y + 10.0;
    let x = ui.max_rect().min.x + 32.0;
    ui.painter().line_segment(
        [pos2(x, y), pos2(x + 120.0, y)],
        Stroke::new(4.0_f32, ORANGE),
    );
    ui.add_space(26.0);
}

fn section(
    ui: &mut egui::Ui,
    width: f32,
    title: &str,
    add: impl FnOnce(&mut egui::Ui, &mut Launcher),
    me: &mut Launcher,
) {
    ui.horizontal(|ui| {
        ui.add_space(32.0);
        egui::Frame::new()
            .fill(Color32::from_rgb(20, 27, 50))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(18.0)
            .show(ui, |ui| {
                ui.set_width(width);
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).font(impact(20.0)).color(YELLOW));
                    ui.add_space(4.0);
                    add(ui, me);
                });
            });
    });
    ui.add_space(12.0);
}

fn offline_note(ui: &mut egui::Ui, e: &str) {
    ui.horizontal(|ui| {
        ui.add_space(32.0);
        ui.label(RichText::new(format!("Can't reach GitHub right now. ({e})")).color(MUTED));
    });
}

/// A picture filling `rect` (cropped to keep its shape), drifting slowly.
fn cover(p: &egui::Painter, rect: Rect, tex: &TextureHandle, alpha: f32, drift: f32) {
    let [w, h] = tex.size();
    let (iw, ih) = (w as f32, h as f32);
    let scale = (rect.width() / iw).max(rect.height() / ih) * (1.04 + 0.03 * drift);
    let size = vec2(iw * scale, ih * scale);
    let pan = vec2(
        -(size.x - rect.width()) * (0.3 + 0.4 * drift),
        -(size.y - rect.height()) * 0.5,
    );
    let img = Rect::from_min_size(rect.min + pan, size);
    p.image(
        tex.id(),
        img,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::from_white_alpha((alpha * 255.0) as u8),
    );
}

/// A gradient across `rect`, from `a` to `b` (down if `vertical`, else
/// across).
fn gradient(p: &egui::Painter, rect: Rect, a: Color32, b: Color32, vertical: bool) {
    let mut mesh = egui::Mesh::default();
    let (tl, tr, bl, br) = if vertical { (a, a, b, b) } else { (a, b, a, b) };
    mesh.colored_vertex(rect.left_top(), tl);
    mesh.colored_vertex(rect.right_top(), tr);
    mesh.colored_vertex(rect.left_bottom(), bl);
    mesh.colored_vertex(rect.right_bottom(), br);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    p.add(mesh);
}

/// Our own picture without a disc: a sunset sky, the sun, and a
/// quarter pipe against it.
fn sunset(p: &egui::Painter, rect: Rect, t: f32) {
    let mid = rect.min.y + rect.height() * 0.62;
    gradient(
        p,
        Rect::from_min_max(rect.min, pos2(rect.max.x, mid)),
        Color32::from_rgb(52, 32, 92),
        Color32::from_rgb(250, 128, 60),
        true,
    );
    gradient(
        p,
        Rect::from_min_max(pos2(rect.min.x, mid), rect.max),
        Color32::from_rgb(250, 150, 70),
        Color32::from_rgb(40, 20, 50),
        true,
    );
    let sun = pos2(
        rect.min.x + rect.width() * 0.68,
        mid - 30.0 + (t * 0.2).sin() * 3.0,
    );
    p.circle_filled(sun, rect.height() * 0.16, Color32::from_rgb(255, 214, 102));
    for i in 0..5 {
        let y = sun.y + 18.0 + i as f32 * 14.0;
        p.rect_filled(
            Rect::from_min_max(
                pos2(sun.x - 200.0, y),
                pos2(sun.x + 200.0, y + 3.0 + i as f32),
            ),
            0.0,
            Color32::from_rgb(250, 140, 66),
        );
    }
    // The quarter pipe: a curve from the ground up to its lip.
    let ground = rect.max.y - rect.height() * 0.12;
    let left = rect.min.x + rect.width() * 0.04;
    let r = rect.height() * 0.42;
    let dark = Color32::from_rgb(22, 14, 34);
    let mut mesh = egui::Mesh::default();
    let steps = 24;
    for i in 0..=steps {
        let a = i as f32 / steps as f32 * std::f32::consts::FRAC_PI_2;
        let x = left + r - r * a.sin();
        let y = ground - r + r * a.cos();
        mesh.colored_vertex(pos2(x + r * 0.0, y.min(ground)), dark);
        mesh.colored_vertex(pos2(left - 40.0, y.min(ground)), dark);
    }
    for i in 0..steps as u32 {
        let k = i * 2;
        mesh.add_triangle(k, k + 1, k + 2);
        mesh.add_triangle(k + 1, k + 3, k + 2);
    }
    p.add(mesh);
    p.rect_filled(
        Rect::from_min_max(pos2(rect.min.x, ground), rect.max),
        0.0,
        dark,
    );
    // The coping.
    p.line_segment(
        [pos2(left - 40.0, ground - r), pos2(left + 6.0, ground - r)],
        Stroke::new(5.0_f32, Color32::from_rgb(140, 140, 160)),
    );
}

fn release_title(r: &Release) -> String {
    let name = r.name.trim();
    let name = name
        .strip_prefix(&r.tag)
        .map(|s| s.trim_start_matches([':', ' ', '-']))
        .unwrap_or(name);
    if name.is_empty() {
        r.tag.to_uppercase()
    } else {
        let mut c = name.chars();
        c.next()
            .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
            .unwrap_or_default()
    }
}

/// The notes' own part (not the standing Downloads / What's in it).
fn notes_only(body: &str) -> &str {
    body.find("\n## Downloads")
        .map(|i| &body[..i])
        .unwrap_or(body)
}

/// The first `n` top-level bullet points of the notes, plain.
fn highlights(body: &str, n: usize) -> Vec<String> {
    notes_only(body)
        .lines()
        .filter_map(|l| l.strip_prefix("- "))
        .map(|l| {
            let plain = plain(l);
            // Just the lead-in: "Water, ported from ...: Calm..." → "Water".
            match plain.split_once(". ") {
                Some((a, _)) if a.len() > 24 => ellipsize(a, 150),
                _ => ellipsize(&plain, 150),
            }
        })
        .take(n)
        .collect()
}

/// Markdown's marks taken out: `**`, backticks, links to their text.
fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('[') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after
            .find("](")
            .and_then(|j| after[j..].find(')').map(|k| (j, j + k)))
        {
            Some((j, end)) => {
                out.push_str(&after[..j]);
                rest = &after[end + 1..];
            }
            None => {
                out.push('[');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.replace("**", "").replace('`', "")
}

fn ellipsize(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n - 1).collect::<String>() + "…"
    }
}

/// Enough markdown for release notes: headings, bullets (nested),
/// paragraphs.
fn markdown(ui: &mut egui::Ui, text: &str) {
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            ui.add_space(4.0);
        } else if let Some(h) = trimmed.strip_prefix("## ") {
            ui.add_space(4.0);
            ui.label(RichText::new(plain(h)).font(impact(19.0)).color(YELLOW));
        } else if let Some(h) = trimmed.strip_prefix("### ") {
            ui.label(RichText::new(plain(h)).strong().color(TEXT));
        } else {
            let indent = trimmed.len() - trimmed.trim_start().len();
            let t = trimmed.trim_start();
            if let Some(item) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(8.0 + indent as f32 * 7.0);
                    ui.label(RichText::new(if indent > 0 { "◦" } else { "•" }).color(ORANGE));
                    ui.label(RichText::new(plain(item)).color(TEXT).size(14.0));
                });
            } else {
                ui.label(RichText::new(plain(t)).color(TEXT).size(14.0));
            }
        }
    }
}

pub fn open_url(url: &str) {
    let _ = std::process::Command::new("explorer").arg(url).spawn();
}

/// Windows' Impact for the headings, Segoe UI for the rest, if there.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let dir = std::path::Path::new("C:\\Windows\\Fonts");
    let mut add = |name: &str, file: &str| -> bool {
        match std::fs::read(dir.join(file)) {
            Ok(bytes) => {
                fonts
                    .font_data
                    .insert(name.into(), Arc::new(egui::FontData::from_owned(bytes)));
                true
            }
            Err(_) => false,
        }
    };
    let has_impact = add("impact", "impact.ttf");
    let has_segoe = add("segoe", "segoeui.ttf");
    let mut impact_family = vec![];
    if has_impact {
        impact_family.push("impact".to_string());
    }
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    if has_segoe {
        proportional.insert(0, "segoe".into());
    }
    impact_family.extend(proportional.iter().cloned());
    fonts
        .families
        .insert(FontFamily::Name("impact".into()), impact_family);
    ctx.set_fonts(fonts);
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(32, 42, 74);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(44, 58, 100);
    style.visuals.selection.bg_fill = ORANGE.gamma_multiply(0.6);
    style.visuals.hyperlink_color = TEAL;
    style.spacing.button_padding = vec2(10.0, 5.0);
    ctx.set_style(style);
}
