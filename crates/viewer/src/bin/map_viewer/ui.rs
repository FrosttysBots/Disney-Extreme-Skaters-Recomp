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
    ToggleSkate,
    /// A two-minute run from the level's start, or (`Some(pro)`) the
    /// level's High Score or Pro Score goal (again).
    StartRun(Option<bool>),
    /// The level's S-K-A-T-E letters goal (again).
    StartLetters,
    /// Through the warp the skater's at, or not.
    Warp,
    StayHere,
    /// Watch the run just skated again, or stop watching.
    Replay,
    StopReplay,
    PlayCameraPath(usize),
    StopCameraPath,
}

/// The level's camera paths and the one playing.
pub struct CameraPathModel {
    /// Names and lengths in seconds.
    pub paths: Vec<(String, f32)>,
    pub playing: Option<usize>,
    pub time: f32,
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
    /// Whether the level has collision to skate on, and whether skating.
    pub can_skate: bool,
    pub skating: bool,
    /// The balance meter (-1 to 1) while in a manual or a grind.
    pub balance: Option<f32>,
    /// Points banked, and the combo on screen.
    pub score: u32,
    /// The special meter (0 to 1) and whether it's full.
    pub special: (f32, bool),
    /// The AutoKick option.
    pub auto_kick: bool,
    /// The skater's sounds (and the level's ambience) on, and the songs.
    pub sound: bool,
    pub music: bool,
    /// The gamepad rumbles where the game's does.
    pub rumble: bool,
    /// In a two-minute run: the seconds left, and once it's over the
    /// score, the best before it, and whether it beat it.
    pub run_clock: Option<f32>,
    pub run_result: Option<(u32, u32, bool)>,
    /// The run's goal (`Some(pro)`) and its name and score, whether it was
    /// won, and the level's score goals' names (high, pro) if it has them.
    pub run_goal: Option<(bool, String, u32)>,
    pub run_goal_won: Option<bool>,
    pub score_goals: [Option<String>; 2],
    /// The character's collectibles shown while skating, and how many of
    /// this level's are got.
    pub collect: bool,
    pub collected: Option<(u32, u32)>,
    /// Watching the run again.
    pub replaying: bool,
    /// The level has S-K-A-T-E letters; while collecting them, which are
    /// got; once it's over, whether they all were, in how long, and the
    /// best time before.
    pub can_letters: bool,
    pub letters: Option<[bool; 5]>,
    pub letters_result: Option<(bool, f32, Option<f32>)>,
    /// Where the skater is and what it's doing, for bug reports.
    pub skate_status: String,
    /// The character's tricks and how to do them (while skating).
    pub trick_list: Vec<String>,
    /// The level's gaps (name, points) and whether each has been landed.
    pub gap_list: Vec<(String, u32, bool)>,
    /// The level's records for the character (shown in the panel), and a
    /// new one being announced.
    pub records: Vec<String>,
    pub record_message: Option<String>,
    /// At a warp: the level it goes to.
    pub warp_prompt: Option<String>,
    pub combo: Option<String>,
    /// A message flashed up while skating ("Sketchy", a gap's name).
    pub message: Option<String>,
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
    /// Objects and pedestrians there at the start.
    pub show_objects: bool,
    /// Geometry, objects and pedestrians that goals create later.
    pub show_goal_objects: bool,
    pub brighten: f32,
    pub collision: CollisionView,
    pub speed: f32,
    pub camera_text: String,
    pub message: Option<String>,
    pub panel_open: bool,
    pub character: CharacterModel,
    pub camera_paths: CameraPathModel,
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
W A S D: move (also stops a camera path)
E / Space: up    Q / Ctrl: down
Shift: move 5x faster
Mouse wheel: change speed
K: cycle collision view
Tab: next spawn point
R: back to the start
P: play or pause the character
[ and ]: previous or next animation
F1: hide or show this panel
Skating: W push, S brake, A/D steer,
  Space crouch (let go: ollie),
  E to grind (ollie at a rail, then E),
  tap W then S to manual
  (balance: W/S in a manual, A/D on a rail),
  in the air Q flip, F grab (+ W/S/A/D),
  R revert (tap on the ground: 180 slide;
  hold going up a quarter pipe: spine transfer),
  S held: step off the board, Tab next spawn; or a gamepad
  (stick, A ollie, X flip, B grab, Y grind),
  Esc stop";

/// The score (top right) and the combo (bottom centre, above the balance
/// meter).
fn trick_text(ctx: &egui::Context, score: u32, combo: Option<&str>) {
    let shadowed = |ui: &mut egui::Ui, text: &str, size: f32| {
        ui.label(
            egui::RichText::new(text)
                .size(size)
                .strong()
                .color(egui::Color32::WHITE)
                .background_color(egui::Color32::from_black_alpha(140)),
        );
    };
    egui::Area::new(egui::Id::new("score"))
        .anchor(egui::Align2::RIGHT_TOP, [-16.0, 16.0])
        .interactable(false)
        .show(ctx, |ui| shadowed(ui, &format!("{score}"), 22.0));
    if let Some(combo) = combo {
        egui::Area::new(egui::Id::new("combo"))
            .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -70.0])
            .interactable(false)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    for line in combo.lines() {
                        shadowed(ui, line, 18.0);
                    }
                })
            });
    }
}

/// The special meter, under the score: it fills with points and, full
/// (glowing), allows the character's special tricks until it drains.
fn special_meter(ctx: &egui::Context, (fill, full): (f32, bool)) {
    egui::Area::new(egui::Id::new("special"))
        .anchor(egui::Align2::RIGHT_TOP, [-16.0, 52.0])
        .interactable(false)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(160.0, 12.0), egui::Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(rect, 3.0, egui::Color32::from_black_alpha(160));
            let mut bar = rect.shrink(2.0);
            bar.set_width(bar.width() * fill.clamp(0.0, 1.0));
            let colour = if full {
                egui::Color32::from_rgb(255, 210, 40)
            } else {
                egui::Color32::from_rgb(70, 140, 255)
            };
            painter.rect_filled(bar, 2.0, colour);
            if full {
                painter.text(
                    rect.left_center() - egui::vec2(8.0, 0.0),
                    egui::Align2::RIGHT_CENTER,
                    "SPECIAL",
                    egui::FontId::proportional(13.0),
                    colour,
                );
            }
        });
}

/// A two-minute run's clock, top centre (`the_time`), red for the last
/// ten seconds.
fn run_clock(ctx: &egui::Context, left: f32) {
    let seconds = left.ceil() as u32;
    let colour = if left <= 10.0 {
        egui::Color32::from_rgb(255, 80, 60)
    } else {
        egui::Color32::WHITE
    };
    egui::Area::new(egui::Id::new("run_clock"))
        .anchor(egui::Align2::CENTER_TOP, [0.0, 16.0])
        .interactable(false)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(format!("{}:{:02}", seconds / 60, seconds % 60))
                    .size(26.0)
                    .strong()
                    .color(colour)
                    .background_color(egui::Color32::from_black_alpha(140)),
            );
        });
}

/// A score goal's target, under the clock: green once reached.
fn goal_target(ctx: &egui::Context, score: u32, won: bool) {
    egui::Area::new(egui::Id::new("goal_target"))
        .anchor(egui::Align2::CENTER_TOP, [0.0, 56.0])
        .interactable(false)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(format!("Goal {score}"))
                    .size(18.0)
                    .strong()
                    .color(if won {
                        egui::Color32::from_rgb(90, 230, 90)
                    } else {
                        egui::Color32::WHITE
                    })
                    .background_color(egui::Color32::from_black_alpha(120)),
            );
        });
}

/// The end of a two-minute run: the score against the best, and for a
/// score goal whether it was reached.
fn run_result(
    ctx: &egui::Context,
    (score, best, record): (u32, u32, bool),
    goal: Option<(bool, &str, Option<bool>)>,
    actions: &mut Vec<Action>,
) {
    egui::Area::new(egui::Id::new("run_result"))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, -40.0])
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    match goal {
                        Some((_, name, Some(true))) => {
                            ui.heading(name);
                            ui.label(
                                egui::RichText::new("Goal complete!")
                                    .size(18.0)
                                    .color(egui::Color32::from_rgb(90, 230, 90)),
                            );
                        }
                        Some((_, name, _)) => {
                            ui.heading(name);
                            ui.label(
                                egui::RichText::new("Goal failed")
                                    .size(18.0)
                                    .color(egui::Color32::from_rgb(255, 90, 70)),
                            );
                        }
                        None => {
                            ui.heading("Run over");
                        }
                    }
                    ui.label(egui::RichText::new(format!("{score}")).size(30.0).strong());
                    if record {
                        ui.label(
                            egui::RichText::new("New best!")
                                .size(18.0)
                                .color(egui::Color32::from_rgb(255, 220, 60)),
                        );
                        if best > 0 {
                            ui.label(format!("Last best {best}"));
                        }
                    } else {
                        ui.label(format!("Best {best}"));
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Replay").clicked() {
                            actions.push(Action::Replay);
                        }
                        if ui.button("Again").clicked() {
                            actions.push(Action::StartRun(goal.map(|(pro, ..)| pro)));
                        }
                        if ui.button("Done").clicked() {
                            actions.push(Action::ToggleSkate);
                        }
                    });
                });
            });
        });
}

/// The S-K-A-T-E letters, under the clock: bright when got.
fn letters_hud(ctx: &egui::Context, got: [bool; 5]) {
    egui::Area::new(egui::Id::new("letters"))
        .anchor(egui::Align2::CENTER_TOP, [0.0, 56.0])
        .interactable(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (letter, got) in "SKATE".chars().zip(got) {
                    let colour = if got {
                        egui::Color32::from_rgb(255, 205, 50)
                    } else {
                        egui::Color32::from_white_alpha(60)
                    };
                    ui.label(
                        egui::RichText::new(letter.to_string())
                            .size(24.0)
                            .strong()
                            .color(colour)
                            .background_color(egui::Color32::from_black_alpha(120)),
                    );
                }
            });
        });
}

/// `m:ss`.
fn clock_text(seconds: f32) -> String {
    let s = seconds.max(0.0).round() as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The end of the letters goal: done in how long against the best, or out
/// of time.
fn letters_result(
    ctx: &egui::Context,
    (won, time, best): (bool, f32, Option<f32>),
    actions: &mut Vec<Action>,
) {
    egui::Area::new(egui::Id::new("letters_result"))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, -40.0])
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    if won {
                        ui.heading("S-K-A-T-E!");
                        ui.label(egui::RichText::new(clock_text(time)).size(30.0).strong());
                        match best {
                            Some(best) if best <= time => {
                                ui.label(format!("Best {}", clock_text(best)));
                            }
                            Some(best) => {
                                ui.label(
                                    egui::RichText::new("New best!")
                                        .size(18.0)
                                        .color(egui::Color32::from_rgb(255, 220, 60)),
                                );
                                ui.label(format!("Last best {}", clock_text(best)));
                            }
                            None => {
                                ui.label(
                                    egui::RichText::new("New best!")
                                        .size(18.0)
                                        .color(egui::Color32::from_rgb(255, 220, 60)),
                                );
                            }
                        }
                    } else {
                        ui.heading("Out of time");
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Again").clicked() {
                            actions.push(Action::StartLetters);
                        }
                        if ui.button("Done").clicked() {
                            actions.push(Action::ToggleSkate);
                        }
                    });
                });
            });
        });
}

/// While a replay plays: a label saying so, and a button to stop it.
fn replay_banner(ctx: &egui::Context, actions: &mut Vec<Action>) {
    egui::Area::new(egui::Id::new("replay"))
        .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -16.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("REPLAY")
                        .size(22.0)
                        .strong()
                        .color(egui::Color32::from_rgb(255, 80, 60))
                        .background_color(egui::Color32::from_black_alpha(140)),
                );
                if ui.button("Stop").clicked() {
                    actions.push(Action::StopReplay);
                }
            });
        });
}

/// The balance meter, bottom centre: a bar with a marker that slides to
/// either end as the skater leans.
fn balance_meter(ctx: &egui::Context, meter: f32) {
    egui::Area::new(egui::Id::new("balance"))
        .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -40.0])
        .interactable(false)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(260.0, 18.0), egui::Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(rect, 4.0, egui::Color32::from_black_alpha(160));
            let middle = rect.center();
            painter.line_segment(
                [middle - egui::vec2(0.0, 7.0), middle + egui::vec2(0.0, 7.0)],
                egui::Stroke::new(1.0_f32, egui::Color32::GRAY),
            );
            // Green near the middle, red near the ends.
            let danger = meter.abs();
            let colour =
                egui::Color32::from_rgb((255.0 * danger) as u8, (255.0 * (1.0 - danger)) as u8, 40);
            let x = middle.x + meter * (rect.width() / 2.0 - 6.0);
            painter.rect_filled(
                egui::Rect::from_center_size(egui::pos2(x, middle.y), egui::vec2(8.0, 14.0)),
                2.0,
                colour,
            );
        });
}

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
    if let Some(meter) = model.character.balance {
        balance_meter(ctx, meter);
    }
    if model.character.skating {
        trick_text(ctx, model.character.score, model.character.combo.as_deref());
        if let Some(message) = &model.character.message {
            egui::Area::new(egui::Id::new("message"))
                .anchor(egui::Align2::CENTER_TOP, [0.0, 90.0])
                .interactable(false)
                .show(ctx, |ui| {
                    ui.label(
                        egui::RichText::new(message)
                            .size(24.0)
                            .strong()
                            .color(egui::Color32::from_rgb(255, 220, 60))
                            .background_color(egui::Color32::from_black_alpha(140)),
                    );
                });
        }
        special_meter(ctx, model.character.special);
        if let Some(text) = &model.character.record_message {
            egui::Area::new(egui::Id::new("record"))
                .anchor(egui::Align2::CENTER_TOP, [0.0, 130.0])
                .interactable(false)
                .show(ctx, |ui| {
                    ui.label(
                        egui::RichText::new(text)
                            .size(22.0)
                            .strong()
                            .color(egui::Color32::from_rgb(110, 230, 110))
                            .background_color(egui::Color32::from_black_alpha(140)),
                    );
                });
        }
        if let Some((got, of)) = model.character.collected {
            egui::Area::new(egui::Id::new("collected"))
                .anchor(egui::Align2::RIGHT_TOP, [-16.0, 74.0])
                .interactable(false)
                .show(ctx, |ui| {
                    ui.label(
                        egui::RichText::new(format!("{got}/{of}"))
                            .size(16.0)
                            .strong()
                            .color(egui::Color32::from_rgb(170, 190, 255))
                            .background_color(egui::Color32::from_black_alpha(120)),
                    );
                });
        }
        if let Some(left) = model.character.run_clock {
            run_clock(ctx, left);
        }
        if let Some((_, _, score)) = &model.character.run_goal {
            goal_target(ctx, *score, model.character.run_goal_won == Some(true));
        }
        if let Some(got) = model.character.letters {
            letters_hud(ctx, got);
        }
        if let Some(result) = model.character.letters_result {
            letters_result(ctx, result, &mut actions);
        }
        if let Some(title) = &model.character.warp_prompt {
            egui::Area::new(egui::Id::new("warp"))
                .anchor(egui::Align2::CENTER_CENTER, [0.0, -40.0])
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.heading(format!("Warp to {title}?"));
                            ui.horizontal(|ui| {
                                if ui.button("Warp").clicked() {
                                    actions.push(Action::Warp);
                                }
                                if ui.button("Stay").clicked() {
                                    actions.push(Action::StayHere);
                                }
                            });
                            ui.label(egui::RichText::new("Enter to warp, Esc to stay").small());
                        });
                    });
                });
        }
        if model.character.replaying {
            replay_banner(ctx, &mut actions);
        } else if let Some(result) = model.character.run_result {
            let goal = model
                .character
                .run_goal
                .as_ref()
                .map(|(pro, name, _)| (*pro, name.as_str(), model.character.run_goal_won));
            run_result(ctx, result, goal, &mut actions);
        }
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
                ui.checkbox(&mut model.show_objects, "Objects and pedestrians");
                ui.checkbox(&mut model.show_goal_objects, "Goal objects").on_hover_text(
                    "Pickups, goal pedestrians, warp portals and other things that \
                     goals and scripts add later. Not there when the level starts.",
                );
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

                if !model.camera_paths.paths.is_empty() {
                    ui.separator();
                    camera_path_section(ui, &model.camera_paths, &mut actions);
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
    ui.add_enabled_ui(model.can_skate, |ui| {
        let label = if model.skating {
            "Stop skating"
        } else {
            "Skate"
        };
        if ui
            .button(label)
            .on_hover_text(
                "Skate the character around the level: W push, S brake, A/D steer, \
                 hold Space to crouch and let go to ollie, ollie at a rail and press E to grind, \
                 tap W then S to manual (balance with W/S, or A/D on a rail), \
                 Q to flip and F to grab in the air (with W/S/A/D), Esc to stop. The game's own physics, as far as it's been read.",
            )
            .clicked()
        {
            actions.push(Action::ToggleSkate);
        }
        if ui
            .button("2 minute run")
            .on_hover_text(
                "The game's single session (Trick Attack): two minutes from the level's start \
                 to score all you can. When the clock runs out, the combo you're in still \
                 counts once it lands. The best score for each level and character is kept.",
            )
            .clicked()
        {
            actions.push(Action::StartRun(None));
        }
    });
    ui.horizontal(|ui| {
        for (pro, name) in [false, true].into_iter().zip(&model.score_goals) {
            let label = if pro {
                "Pro Score goal"
            } else {
                "High Score goal"
            };
            ui.add_enabled_ui(model.can_skate && name.is_some(), |ui| {
                if ui
                    .button(label)
                    .on_hover_text(format!(
                        "The level's goal \"{}\": reach its score before the clock runs out, \
                         from the goal's start.",
                        name.as_deref().unwrap_or("")
                    ))
                    .clicked()
                {
                    actions.push(Action::StartRun(Some(pro)));
                }
            });
        }
    });
    ui.add_enabled_ui(model.can_skate && model.can_letters, |ui| {
        if ui
            .button("S-K-A-T-E letters")
            .on_hover_text(
                "The level's letters goal: collect the five spinning letters S, K, A, T and E \
                 before the clock (the level's own goal time) runs out. The best time for each \
                 level and character is kept.",
            )
            .clicked()
        {
            actions.push(Action::StartLetters);
        }
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut model.sound, "Sound").on_hover_text(
            "The skater's sounds (rolling, ollies, landings, grinds, bails) and the level's ambience.",
        );
        ui.checkbox(&mut model.music, "Music")
            .on_hover_text("The game's soundtrack, one song after another.");
        ui.checkbox(&mut model.collect, "Collectibles").on_hover_text(
            "The character's 25 collectibles on each level of its world (Jessie's \
             Cowgirl Boots, Woody's Badges...), there to pick up while skating. \
             What's collected is kept.",
        );
        ui.checkbox(&mut model.rumble, "Rumble")
            .on_hover_text("The gamepad rumbles for ollies, landings, grinds, reverts and bails, as in the game.");
    });
    ui.checkbox(&mut model.auto_kick, "AutoKick").on_hover_text(
        "The game's controller option: the skater pushes by itself while under its kick speed. Off, hold W to push.",
    );
    if model.skating {
        if !model.trick_list.is_empty() {
            egui::CollapsingHeader::new("Tricks")
                .id_salt("trick_list")
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(
                            "Flip = Q / X, Grab = F / B, Grind = E / Y; directions W A S D",
                        )
                        .small(),
                    );
                    for line in &model.trick_list {
                        ui.label(egui::RichText::new(line).small().monospace());
                    }
                });
        }
        if !model.records.is_empty() {
            egui::CollapsingHeader::new("Records")
                .id_salt("records")
                .show(ui, |ui| {
                    for line in &model.records {
                        ui.label(egui::RichText::new(line).small());
                    }
                });
        }
        if !model.gap_list.is_empty() {
            let found = model.gap_list.iter().filter(|g| g.2).count();
            egui::CollapsingHeader::new(format!("Gaps ({found}/{})", model.gap_list.len()))
                .id_salt("gap_list")
                .show(ui, |ui| {
                    for (name, score, got) in &model.gap_list {
                        let text = format!("{} {name} ({score})", if *got { "✔" } else { "  " });
                        let text = egui::RichText::new(text).small();
                        ui.label(if *got {
                            text.color(egui::Color32::from_rgb(120, 220, 120))
                        } else {
                            text.weak()
                        });
                    }
                });
        }
        ui.label(egui::RichText::new(&model.skate_status).small().monospace())
            .on_hover_text("Where the skater is and what it's doing: handy for bug reports.");
        return;
    }
    if ui
        .button("Look at the character")
        .on_hover_text("The character stands on the last spawn point you went to (Tab).")
        .clicked()
    {
        actions.push(Action::LookAtCharacter);
    }
}

fn camera_path_section(ui: &mut egui::Ui, model: &CameraPathModel, actions: &mut Vec<Action>) {
    if let Some(i) = model.playing {
        let (name, duration) = &model.paths[i];
        ui.horizontal(|ui| {
            ui.label(format!(
                "Playing {name}  {:.1} / {duration:.1} s",
                model.time
            ));
            if ui.button("Stop").clicked() {
                actions.push(Action::StopCameraPath);
            }
        });
    }
    egui::CollapsingHeader::new(format!("Camera paths ({})", model.paths.len()))
        .default_open(false)
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new("The level's cutscene, goal and fly-through cameras.").small(),
            );
            for (i, (name, duration)) in model.paths.iter().enumerate() {
                let text = format!("{name}  ({duration:.1} s)");
                if ui
                    .selectable_label(model.playing == Some(i), text)
                    .clicked()
                {
                    actions.push(Action::PlayCameraPath(i));
                }
            }
        });
}
