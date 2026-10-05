//! The side panel. Drawing returns a list of actions for the app to apply,
//! so the panel never touches the GPU or the game data directly.

use desa_viewer::collision::CollisionView;
use desa_viewer::nodes::Spawn;
use desa_viewer::source::LevelInfo;

pub enum Action {
    OpenDisc,
    OpenFolder,
    LoadLevel(usize),
    GoToSpawn(usize),
    ResetCamera,
    /// Show a character (an index into `characters`), or none.
    LoadCharacter(Option<usize>),
    LookAtCharacter,
}

/// The character shown on the current spawn point and its animation.
pub struct CharacterModel {
    pub characters: Vec<LevelInfo>,
    pub current: Option<usize>,
    pub animations: Vec<String>,
    pub animation: usize,
    pub playing: bool,
    pub speed: f32,
    /// Seconds into the animation.
    pub time: f32,
    pub duration: f32,
    /// Whether the character has blink frames, and whether to use them.
    pub can_blink: bool,
    pub blink: bool,
}

/// Everything the panel shows or edits.
pub struct Model {
    pub data_path: Option<String>,
    pub levels: Vec<LevelInfo>,
    pub current: Option<usize>,
    pub loading: Option<String>,
    pub stats: Option<String>,
    pub spawns: Vec<(String, String)>,
    pub has_collision: bool,
    pub show_sky: bool,
    pub show_rails: bool,
    pub show_spawns: bool,
    pub brighten: f32,
    pub collision: CollisionView,
    pub speed: f32,
    pub camera_text: String,
    pub message: Option<String>,
    pub panel_open: bool,
    pub character: CharacterModel,
}

impl Model {
    pub fn set_spawns(&mut self, spawns: &[Spawn]) {
        self.spawns = spawns
            .iter()
            .map(|s| (s.label.clone(), s.kind.clone()))
            .collect();
    }
}

pub const CONTROLS: &str = "\
Hold right mouse: look around
W A S D: move
E / Space: up    Q / Ctrl: down
Shift: move 5x faster
Mouse wheel: change speed
K: cycle collision view
Tab: next spawn point
R: back to the start
P: play or pause the character
[ and ]: previous or next animation
F1: hide or show this panel";

pub fn draw(ctx: &egui::Context, model: &mut Model) -> Vec<Action> {
    let mut actions = Vec::new();
    if let Some(title) = &model.loading {
        egui::Area::new(egui::Id::new("loading"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.heading(format!("Loading {title}..."));
                });
            });
    }
    if !model.panel_open {
        return actions;
    }

    egui::SidePanel::left("panel").resizable(true).default_width(270.0).show(ctx, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("DESA Map Viewer");
            ui.label("Disney's Extreme Skate Adventure");
            ui.separator();

            ui.label(egui::RichText::new("Game data").strong());
            match &model.data_path {
                Some(path) => {
                    ui.label(egui::RichText::new(path).small());
                }
                None => {
                    ui.label("Open your game disc image (.iso) to start.");
                }
            }
            ui.horizontal(|ui| {
                if ui.button("Open disc image...").clicked() {
                    actions.push(Action::OpenDisc);
                }
                if ui.button("Open folder...").on_hover_text("A folder with the extracted .prg archives").clicked() {
                    actions.push(Action::OpenFolder);
                }
            });
            if let Some(message) = &model.message {
                ui.colored_label(egui::Color32::from_rgb(255, 120, 100), message);
            }

            if !model.levels.is_empty() {
                ui.separator();
                ui.label(egui::RichText::new("Levels").strong());
                for (i, level) in model.levels.iter().enumerate() {
                    if ui.selectable_label(model.current == Some(i), &level.title).clicked() && model.loading.is_none() {
                        actions.push(Action::LoadLevel(i));
                    }
                }
            }

            if model.current.is_some() {
                ui.separator();
                ui.label(egui::RichText::new("Show").strong());
                ui.checkbox(&mut model.show_sky, "Sky");
                ui.checkbox(&mut model.show_rails, "Rails");
                ui.checkbox(&mut model.show_spawns, "Spawn points");
                ui.add(egui::Slider::new(&mut model.brighten, 0.0..=1.0).text("brighten dark areas"))
                    .on_hover_text("Raises the darkest lighting so you can see into shadows. 0 = as in the game.");
                ui.add_enabled_ui(model.has_collision, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Collision:");
                        ui.radio_value(&mut model.collision, CollisionView::Hidden, "Off");
                        ui.radio_value(&mut model.collision, CollisionView::Overlay, "Overlay");
                        ui.radio_value(&mut model.collision, CollisionView::Only, "Only");
                    });
                });
                if model.collision != CollisionView::Hidden {
                    ui.label(
                        egui::RichText::new("red = quarter pipes, blue = wallride, yellow = triggers").small(),
                    );
                }

                if !model.character.characters.is_empty() {
                    ui.separator();
                    character_section(ui, &mut model.character, &mut actions);
                }

                ui.separator();
                ui.label(egui::RichText::new("Camera").strong());
                ui.add(egui::Slider::new(&mut model.speed, 50.0..=20_000.0).logarithmic(true).text("speed"));
                ui.label(egui::RichText::new(&model.camera_text).small().monospace());
                if ui.button("Back to the start").clicked() {
                    actions.push(Action::ResetCamera);
                }

                if !model.spawns.is_empty() {
                    ui.separator();
                    egui::CollapsingHeader::new(format!("Spawn points ({})", model.spawns.len()))
                        .default_open(true)
                        .show(ui, |ui| {
                            for (i, (label, kind)) in model.spawns.iter().enumerate() {
                                let text = if kind.is_empty() { label.clone() } else { format!("{label}  ({kind})") };
                                if ui.button(text).clicked() {
                                    actions.push(Action::GoToSpawn(i));
                                }
                            }
                        });
                }
                if let Some(stats) = &model.stats {
                    ui.separator();
                    ui.label(egui::RichText::new(stats).small());
                }
            }

            ui.separator();
            egui::CollapsingHeader::new("Controls").default_open(true).show(ui, |ui| {
                ui.label(egui::RichText::new(CONTROLS).small());
            });
        });
    });
    actions
}

fn character_section(ui: &mut egui::Ui, model: &mut CharacterModel, actions: &mut Vec<Action>) {
    ui.label(egui::RichText::new("Character").strong());
    let name = |i: Option<usize>| match i {
        Some(i) => model.characters[i].title.clone(),
        None => "None".to_string(),
    };
    let mut picked = model.current;
    egui::ComboBox::from_id_salt("character")
        .selected_text(name(model.current))
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut picked, None, "None");
            for i in 0..model.characters.len() {
                ui.selectable_value(&mut picked, Some(i), name(Some(i)));
            }
        });
    if picked != model.current {
        actions.push(Action::LoadCharacter(picked));
    }
    if model.current.is_none() || model.animations.is_empty() {
        return;
    }

    let current = model.animations[model.animation].clone();
    egui::ComboBox::from_id_salt("animation")
        .selected_text(current)
        .height(400.0)
        .show_ui(ui, |ui| {
            for (i, animation) in model.animations.iter().enumerate() {
                ui.selectable_value(&mut model.animation, i, animation);
            }
        });
    ui.horizontal(|ui| {
        if ui
            .button(if model.playing { "Pause" } else { "Play" })
            .clicked()
        {
            model.playing = !model.playing;
        }
        ui.add(
            egui::Slider::new(&mut model.speed, 0.1..=2.0)
                .logarithmic(true)
                .text("speed"),
        );
    });
    ui.add(
        egui::Slider::new(&mut model.time, 0.0..=model.duration.max(0.01))
            .text("seconds")
            .fixed_decimals(2),
    );
    ui.add_enabled_ui(model.can_blink, |ui| {
        ui.checkbox(&mut model.blink, "Blink").on_hover_text(
            "Uses the blink frames every character ships with. The GameCube \
             game's code never uses them, so the original may not blink; the \
             timing here is our own.",
        );
    });
    if ui
        .button("Look at the character")
        .on_hover_text("The character stands on the last spawn point you went to (Tab).")
        .clicked()
    {
        actions.push(Action::LookAtCharacter);
    }
}
