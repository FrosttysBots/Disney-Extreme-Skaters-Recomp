//! The game's screen elements: the menus and panels its scripts build
//! (`CreateScreenElement`, `SetScreenElementProps`, `DoMorph`...), run
//! here so the scripts' own menus work: the pause menu, the main menu.
//!
//! Elements form a tree under `root_window`, on the game's 640x480
//! screen. Each has a place in its parent (`pos`, from the parent's top
//! left, with `just` saying which point of it sits there: `[center
//! top]`), a `scale` (passed on to its children), a colour (`rgba`, where
//! 128 is full), and per type:
//!
//! - `SpriteElement`: a panel sprite (`texture = paused`).
//! - `TextElement` / `TextBlockElement`: text in one of the fonts (`font =
//!   small`, passed down from parents), a block wrapped to its `Dims`.
//! - `ContainerElement`: holds others; `focusable_child` passes focus on.
//! - `VMenu` / `HMenu`: stacks its children (lined up by
//!   `internal_just`, a vertical one's spaced by `padding_scale`, both
//!   with `spacing_between` between), keeps one focused, and
//!   moves the focus with the pad (`pad_up`/`pad_down`, or left/right).
//!
//! Events (`focus`, `unfocus`, `pad_choose`, `pad_back`, `pad_start`...)
//! go to an element's `event_handlers` (`{ pad_choose script params =
//! {...} }`), each running its script on the element. The pad's go to the
//! deepest focused element and up through its parents till one handles
//! them. Scripts run on an element (its handlers, `RunScriptOnScreenElement`)
//! act on it when they name none: `DoMorph` (animated over `time`
//! seconds, waiting), `SetProps`, `GetTags`, `Die`.

use std::collections::HashMap;

use glam::Vec2;
use qb::vm::{Host, Outcome, Params, Program, Thread};
use qb::{Value, checksum};

use crate::font::Font;

/// The screen's size, as the scripts place things on it.
pub const WIDTH: f32 = 640.0;
pub const HEIGHT: f32 = 480.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Container,
    Sprite,
    Text,
    TextBlock,
    VMenu,
    HMenu,
}

/// What a morph changes over time.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Look {
    pos: Vec2,
    scale: Vec2,
    /// 0 to 1 (the scripts' 0 to 128).
    rgba: [f32; 4],
    alpha: f32,
}

#[derive(Clone, Copy, Debug)]
struct Morph {
    from: Look,
    to: Look,
    t: f32,
    time: f32,
}

#[derive(Clone, Debug)]
struct Element {
    kind: Kind,
    id: u32,
    parent: Option<usize>,
    children: Vec<usize>,
    look: Look,
    morph: Option<Morph>,
    /// Which point of it is at `pos`: 0 left/top, 0.5 centre, 1 right/bottom.
    just: Vec2,
    internal_just: Vec2,
    texture: Option<u32>,
    text: String,
    font: Option<u32>,
    dims: Option<Vec2>,
    z: f32,
    hidden: bool,
    behind_parent: bool,
    not_focusable: bool,
    focused: bool,
    /// A menu's focused child (its place among the children).
    selected: Option<usize>,
    focusable_child: Option<u32>,
    /// A scrolling menu (`vscrollingmenu`): it holds its menu where that
    /// says, not lined up as items are.
    scrolling: bool,
    /// How far a scrolling menu's scrolled (down or across), in its
    /// units: following its menu's selected item.
    scroll: std::cell::Cell<f32>,
    wrap: bool,
    padding: f32,
    /// A menu's gap between its children.
    spacing: f32,
    /// A menu's items each this far on from the last, whatever their size
    /// (`regular_space_amount`: the keyboard's rows).
    regular: Option<f32>,
    /// A scrolling menu's rows shown at once (`num_items_to_show`).
    show: Option<usize>,
    /// Text's own size within the element (`internal_scale`).
    internal_scale: f32,
    /// A sprite's turn, degrees (`rot_angle`).
    angle: f32,
    /// (event, script, params).
    handlers: Vec<(u32, u32, Params)>,
    tags: Params,
}

impl Element {
    fn new(kind: Kind, id: u32, parent: Option<usize>) -> Element {
        Element {
            kind,
            id,
            parent,
            children: Vec::new(),
            look: Look {
                pos: Vec2::ZERO,
                scale: Vec2::ONE,
                rgba: [1.0; 4],
                alpha: 1.0,
            },
            morph: None,
            just: Vec2::splat(0.5),
            internal_just: Vec2::new(0.5, 0.0),
            texture: None,
            text: String::new(),
            font: None,
            dims: None,
            z: 0.0,
            hidden: false,
            behind_parent: false,
            not_focusable: false,
            focused: false,
            selected: None,
            focusable_child: None,
            scrolling: false,
            scroll: std::cell::Cell::new(0.0),
            wrap: true,
            padding: 1.0,
            spacing: 0.0,
            regular: None,
            show: None,
            internal_scale: 1.0,
            angle: 0.0,
            handlers: Vec::new(),
            tags: vec![(Some(checksum("id")), Value::Name(id))],
        }
    }
}

/// Something to draw, in screen units (640x480), tinted by `rgba` (0 to
/// 1, alpha included).
#[derive(Clone, Debug, PartialEq)]
pub enum Draw {
    /// A panel sprite, by name checksum, over `rect` (left, top, right,
    /// bottom), turned `angle` degrees (clockwise) about its middle.
    Sprite {
        texture: u32,
        rect: [f32; 4],
        rgba: [f32; 4],
        angle: f32,
    },
    /// A glyph of a font (by name checksum): its rectangle in the font's
    /// atlas (x, y, width, height), drawn over `rect`.
    Glyph {
        font: u32,
        source: [u16; 4],
        rect: [f32; 4],
        rgba: [f32; 4],
    },
}

/// The pad's events, as the menus take them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pad {
    Up,
    Down,
    Left,
    Right,
    Choose,
    Back,
    Start,
}

impl Pad {
    fn event(self) -> u32 {
        checksum(match self {
            Pad::Up => "pad_up",
            Pad::Down => "pad_down",
            Pad::Left => "pad_left",
            Pad::Right => "pad_right",
            Pad::Choose => "pad_choose",
            Pad::Back => "pad_back",
            Pad::Start => "pad_start",
        })
    }
}

/// A script running on an element, and what to run when it's done
/// (`RunScriptOnScreenElement ... callback = script`).
struct Running {
    element: usize,
    thread: Thread,
    then: Option<(u32, Params)>,
}

/// What the Skate Shop's Edit Skater menus do to the created kid, in
/// order: a part put on (`EditPlayerAppearance target = SetPart`, by its
/// list and `desc_id`) or taken off (`ClearPart`, `None`), a part's
/// colour (`SetPlayerAppearanceColor`, `None` its own), the appearance
/// kept as it is (`RememberTemporaryProfile`: a part accepted) or put
/// back to what was kept (`RestoreTemporaryAppearance`: one tried on).
#[derive(Clone, Debug, PartialEq)]
pub enum AppearanceEdit {
    Part(u32, Option<u32>),
    Color(u32, Option<[f32; 3]>),
    /// (By the temporary profile's name: `stage_profile0`, `cak_profile2`,
    /// a created skater's slot.)
    Keep(Option<u32>),
    Restore(Option<u32>),
    /// Made over as the game's new boy or girl (`RememberTemporaryAppearance
    /// appearance_structure = appearance_reset_girl`: Create New's gender):
    /// whether a girl.
    Reset(bool),
    /// A whole appearance put on (`SetPlayerAppearance appearance_structure
    /// = (info_esc [ i ].appearance)`: the Extreme Skate Crew's kids).
    Apply(Appearance),
}

/// An appearance as the game writes one: a boy's or a girl's, each part
/// by its list (`skater_m_hair = { desc_id = #"Medium 4" h = 0 S = 90 v =
/// 60 use_default_hsv = 0 }`, `body` for the skin), the body shape's bones
/// and its overall scale.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Appearance {
    pub girl: bool,
    pub parts: Vec<(u32, u32, Option<[f32; 3]>)>,
    pub shape: Vec<(u32, [f32; 3])>,
    pub scale: Option<f32>,
}

impl Appearance {
    /// Read from the struct the scripts give (or the global it names).
    pub fn of(value: &Value, program: &Program) -> Option<Appearance> {
        let c = checksum;
        let value = match value {
            Value::Name(n) => program.value(*n)?,
            v => v,
        };
        let Value::Struct(items) = value else {
            return None;
        };
        let mut out = Appearance::default();
        for (k, v) in items {
            let Some(k) = *k else { continue };
            if k == c("body_shape") {
                let shape = match v {
                    Value::Name(n) => program.value(*n),
                    v => Some(v),
                };
                if let Some(Value::Struct(bones)) = shape {
                    for (bone, s) in bones {
                        match (bone, s) {
                            (Some(b), s) if *b == c("scale") => out.scale = s.as_f32(),
                            (Some(b), s) => {
                                if let Some(s) = s.as_vector() {
                                    out.shape.push((*b, s));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                continue;
            }
            let Some(desc) = v.get(c("desc_id")).and_then(Value::as_name) else {
                continue;
            };
            let n = |key: &str| v.get(c(key)).and_then(Value::as_f32);
            let own = v.get(c("use_default_hsv")).and_then(Value::as_int) != Some(0);
            let color = match (n("h"), n("S"), n("v")) {
                (Some(h), Some(s), Some(val)) if !own => Some([h, s, val]),
                _ => None,
            };
            let girlish = [
                "skater_f_head",
                "skater_f_hair",
                "skater_f_torso",
                "skater_f_legs",
            ]
            .iter()
            .any(|f| k == c(f));
            if girlish || (k == c("body") && program_name_starts(desc, "GirlBody")) {
                out.girl = true;
            }
            out.parts.push((k, desc, color));
        }
        Some(out)
    }
}

/// Whether a `desc_id` is one of the girls' bodies (`GirlBody`,
/// `GirlBody1`...).
fn program_name_starts(desc: u32, prefix: &str) -> bool {
    std::iter::once(prefix.to_string())
        .chain((1..10).map(|i| format!("{prefix}{i}")))
        .any(|n| checksum(&n) == desc)
}

pub struct Screen {
    elements: Vec<Option<Element>>,
    by_id: HashMap<u32, usize>,
    aliases: HashMap<u32, usize>,
    fonts: HashMap<u32, Font>,
    /// The sprites' sizes, by name checksum.
    images: HashMap<u32, Vec2>,
    running: Vec<Running>,
    starting: Vec<Running>,
    next_id: u32,
    /// Sounds the scripts played (by name), for the viewer to take.
    pub sounds: Vec<u32>,
    /// Calls of the commands the viewer listens for (`unpausegame`...),
    /// with their arguments, for it to take.
    pub requests: Vec<(u32, Value)>,
    listening: Vec<u32>,
    /// Commands scripts used that aren't understood, with counts.
    pub unknown: HashMap<u32, usize>,
    /// The level on, as `LevelIs` asks (`load_skateshop`).
    pub level: Option<u32>,
    /// The goal on (its id) and its settings, as
    /// `GoalManager_GetGoalParams` gives them to the UI's scripts.
    pub goal: Option<(u32, Params)>,
    /// Whether there's a goal to retry (`GoalManager_CanRetryGoal`: one
    /// played since the level loaded).
    pub can_retry: bool,
    /// The character skating (`jessie`), for `GetSkaterProfileInfo`, and
    /// the one picked in the skater select (`SelectCurrentSkater`).
    pub character: Option<String>,
    pub selected_skater: Option<u32>,
    /// The level's gaps for its checklist (`AddGapsToMenu`): name, points
    /// and times landed.
    pub gaps: Vec<(String, u32, u32)>,
    /// The level's goals for View Goals (`GoalManager_AddViewGoalsList`):
    /// id, what the list calls it, and whether it's won.
    pub goal_list: Vec<(u32, String, bool)>,
    /// The music and effects volumes as the sound options show them, 0
    /// to 10 (`GetValueFromVolume cdvol`, `sfxvol`), and whether the
    /// menu's changed them (`SetMusicVolume`, `SetSfxVolume`).
    /// Every level's goals won and gaps landed so far (by its
    /// `load_<level>` checksum), for the level select and Game Progress.
    pub progress: HashMap<u32, (i32, i32)>,
    /// Every level's records for the character (by its `load_<level>`
    /// checksum): best session score, best combo, longest grind, manual
    /// and lip (hundredths of a second), most tricks in a combo.
    pub records: HashMap<u32, [i32; 6]>,
    pub music_level: i32,
    pub sound_level: i32,
    pub volumes_changed: bool,
    /// The controller's rumble, as the controls options show and set it
    /// (`VibrationIsOn`, `VibrationOn`, `VibrationOff`).
    pub rumble: bool,
    pub rumble_changed: bool,
    /// The full controls or the simplified ones, the same way
    /// (`HawkControlsIsOn`, `HawkControlsOn`, `HawkControlsOff`).
    pub hawk_controls: bool,
    pub hawk_controls_changed: bool,
    /// Rumbles the menus asked for (`VibrateController`: the Controls
    /// menu's test): the strong motor or not, and how hard.
    pub vibrations: Vec<(bool, f32)>,
    /// The song the menus asked for, as its file's name (`AddMusicTrack
    /// "music\Jessie"`: the skater select's character theme), or none
    /// (`StopMusic`), when they've asked since the viewer last looked.
    pub music: Option<Option<String>>,
    /// The music and streams paused or going on again (`PauseMusic 1`,
    /// `PauseStream 0`: the pause menu's), for whoever plays them.
    pub music_paused: Option<bool>,
    /// The created kid's parts as worn (each list's name and the choice's
    /// `desc_id`), for the Edit Skater menus to ask after; and the parts
    /// they change (`EditPlayerAppearance target = SetPart`, `ClearPart`:
    /// `None`), for whoever dresses the kid.
    pub kid_appearance: Vec<(u32, u32, Option<[f32; 3]>)>,
    /// Where the menus have put the skater (`skater:Obj_MoveToNode Name =
    /// TRG_Start_P1`: the Edit Skater set), till they put it back on the
    /// stage (`None`).
    pub skater_node: Option<u32>,
    pub appearance_edits: Vec<AppearanceEdit>,
    /// The game's temporary profiles as remembered (`RememberTemporaryProfile
    /// Name = cak_profile0`), the created kid's profile as the menus have
    /// changed it (`SetSkaterProfileInfo params = { is_male = 0 }`), and the
    /// created skaters' slots saved (each its index and whether a girl).
    temp_profiles: HashMap<u32, Params>,
    profile_overrides: Params,
    pub saved_kids: Vec<(usize, bool)>,
    /// A movie asked for on the level's screen (`playmovie "movies\x"
    /// mesh = moviescreen loop`: the skater select's character clip), by
    /// its file's name and whether it loops; or none (`StopMovie`).
    pub screen_movie: Option<Option<(String, bool)>>,
    /// A camera path the menus play (`playskaterCamAnim Name =
    /// Camera_stage play_hold`: the skater select's view of the stage).
    pub camera: Option<u32>,
    /// Movies the menus play over the whole screen (`playmovie
    /// "movies\intro"`: the Options' Movies), by file name.
    pub movies: Vec<String>,
    /// Scripts to stop once the frame's have run: an element's (of one
    /// name, if named).
    stopping: Vec<(usize, Option<u32>)>,
}

const ROOT: usize = 0;

impl Screen {
    /// An empty screen, drawing with these fonts and sprites (sizes), by
    /// name checksum.
    pub fn new(fonts: HashMap<u32, Font>, images: HashMap<u32, (u32, u32)>) -> Screen {
        let root_id = checksum("root_window");
        let mut root = Element::new(Kind::Container, root_id, None);
        root.just = Vec2::ZERO;
        root.dims = Some(Vec2::new(WIDTH, HEIGHT));
        root.focused = true;
        root.tags
            .push((Some(checksum("menu_state")), Value::Name(checksum("off"))));
        Screen {
            elements: vec![Some(root)],
            by_id: HashMap::from([(root_id, ROOT)]),
            aliases: HashMap::new(),
            fonts,
            images: images
                .into_iter()
                .map(|(k, (w, h))| (k, Vec2::new(w as f32, h as f32)))
                .collect(),
            running: Vec::new(),
            starting: Vec::new(),
            next_id: 0,
            sounds: Vec::new(),
            requests: Vec::new(),
            listening: Vec::new(),
            unknown: HashMap::new(),
            level: None,
            goal: None,
            can_retry: false,
            character: None,
            selected_skater: None,
            gaps: Vec::new(),
            goal_list: Vec::new(),
            progress: HashMap::new(),
            records: HashMap::new(),
            music_level: 10,
            sound_level: 10,
            volumes_changed: false,
            rumble: true,
            vibrations: Vec::new(),
            music: None,
            music_paused: None,
            kid_appearance: Vec::new(),
            skater_node: None,
            appearance_edits: Vec::new(),
            temp_profiles: HashMap::new(),
            profile_overrides: Vec::new(),
            saved_kids: Vec::new(),
            screen_movie: None,
            camera: None,
            movies: Vec::new(),
            rumble_changed: false,
            hawk_controls: true,
            hawk_controls_changed: false,
            stopping: Vec::new(),
        }
    }

    /// The fonts drawn with.
    pub fn font(&self, name: u32) -> Option<&Font> {
        self.fonts.get(&name)
    }

    /// Commands and scripts (by name) whose calls go to
    /// [`Screen::requests`] for the viewer (`unpausegame`, or a screen it
    /// does itself: `launch_select_skater_menu`); listened-for scripts
    /// aren't run.
    pub fn listen(&mut self, commands: &[&str]) {
        self.listening.extend(commands.iter().map(|c| checksum(c)));
    }

    /// Runs a script (`create_pause_menu`) on the root.
    pub fn run(&mut self, script: u32, params: Params) {
        self.starting.push(Running {
            element: ROOT,
            thread: Thread::new(script, params),
            then: None,
        });
    }

    /// Takes everything off the screen (the scripts running stop).
    pub fn clear(&mut self) {
        self.destroy(ROOT);
        self.running.clear();
        self.starting.clear();
        self.aliases.clear();
        if let Some(root) = self.el_mut(ROOT) {
            root.handlers.clear();
        }
    }

    /// A screen command from scripts run elsewhere (the level's: a goal's
    /// `CreateScreenElement`, `RunScriptOnScreenElement`...), done here
    /// as if from the root.
    pub fn command(&mut self, name: u32, args: &Value, program: &Program) {
        let mut host = Ui {
            screen: self,
            element: ROOT,
            program,
        };
        host.command(None, name, args);
    }

    /// Sets an element's properties (as `SetScreenElementProps`), by id
    /// or alias; false if it isn't there.
    pub fn set(&mut self, id: &str, props: &Value, program: &Program) -> bool {
        match self.find(checksum(id)) {
            Some(i) => {
                self.apply(i, props, program);
                true
            }
            None => false,
        }
    }

    /// Sets the properties of an element named as scripts name them
    /// (`{ the_balance_meter child = 0 }`).
    pub fn set_resolved(&mut self, id: &Value, props: &Value, program: &Program) -> bool {
        match self.resolve(id) {
            Some(i) => {
                self.apply(i, props, program);
                true
            }
            None => false,
        }
    }

    /// An element's size as it's drawn (its own scale in), by id or alias.
    pub fn size_of(&self, id: &str) -> Option<Vec2> {
        self.find(checksum(id)).map(|i| self.local_rect(i).1)
    }

    /// Sets the properties of every sprite showing a texture (by name).
    pub fn set_sprites(&mut self, texture: &str, props: &Value, program: &Program) {
        let t = checksum(texture);
        let found: Vec<usize> = (0..self.elements.len())
            .filter(|i| {
                self.el(*i)
                    .is_some_and(|e| e.kind == Kind::Sprite && e.texture == Some(t))
            })
            .collect();
        for i in found {
            self.apply(i, props, program);
        }
    }

    /// Takes an element (and what's in it) away, by id or alias.
    pub fn destroy_id(&mut self, id: &str) {
        if let Some(i) = self.find(checksum(id)) {
            self.destroy(i);
        }
    }

    /// A sprite's size, by name.
    pub fn image_size(&self, name: &str) -> Option<Vec2> {
        self.images.get(&checksum(name)).copied()
    }

    /// Whether something on the screen has the focus (a menu, a speech
    /// box waiting for a press): the pad's for it, not the skater.
    pub fn takes_pad(&self) -> bool {
        self.el(ROOT).is_some_and(|r| {
            r.children
                .iter()
                .any(|c| self.el(*c).is_some_and(|c| c.focused && !c.hidden))
        })
    }

    /// Whether anything's on the screen.
    pub fn is_empty(&self) -> bool {
        self.el(ROOT).is_none_or(|r| r.children.is_empty())
    }

    /// Whether an element by this id (or alias) is there.
    pub fn exists(&self, id: &str) -> bool {
        self.find(checksum(id)).is_some()
    }

    /// The scripts run on for `dt` seconds, and the morphs moved on.
    pub fn update(&mut self, program: &Program, dt: f32) {
        for e in self.elements.iter_mut().flatten() {
            if let Some(m) = &mut e.morph {
                m.t += dt;
                let k = if m.time <= 0.0 {
                    1.0
                } else {
                    (m.t / m.time).min(1.0)
                };
                e.look = lerp(m.from, m.to, k);
                if k >= 1.0 {
                    e.morph = None;
                }
            }
        }
        let mut running = std::mem::take(&mut self.running);
        running.append(&mut self.starting);
        for r in &mut running {
            // (A script goes on when its element's gone: an item's choice
            // takes its menu away, then makes the next.)
            if r.thread.is_finished() {
                continue;
            }
            let mut host = Ui {
                screen: self,
                element: r.element,
                program,
            };
            r.thread.run(program, &mut host, dt);
        }
        // Done: their callbacks run (on the root).
        for r in &mut running {
            if r.thread.is_finished() {
                if let Some((script, params)) = r.then.take() {
                    self.starting.push(Running {
                        element: ROOT,
                        thread: Thread::new(script, params),
                        then: None,
                    });
                }
            }
        }
        running.retain(|r| !r.thread.is_finished());
        for (i, only) in std::mem::take(&mut self.stopping) {
            running
                .retain(|r| r.element != i || only.is_some_and(|n| r.thread.script() != Some(n)));
        }
        running.append(&mut self.running);
        self.running = running;
    }

    /// A pad event: to the deepest focused element, then up through its
    /// parents till one takes it.
    pub fn pad(&mut self, pad: Pad) {
        // The deepest focused element showing (focus can be given to a
        // menu deep inside one that hasn't it: the gap checklist's).
        let mut at = ROOT;
        let mut deepest = 0;
        let mut stack = vec![(ROOT, 0usize)];
        while let Some((i, depth)) = stack.pop() {
            let Some(e) = self.el(i).filter(|e| !e.hidden) else {
                continue;
            };
            if e.focused && depth >= deepest {
                at = i;
                deepest = depth;
            }
            for &c in e.children.iter().rev() {
                stack.push((c, depth + 1));
            }
        }
        let event = pad.event();
        let mut el = Some(at);
        while let Some(i) = el {
            let mut taken = self.navigate(i, pad);
            taken |= self.handle(i, event, Vec::new());
            if taken {
                break;
            }
            el = self.el(i).and_then(|e| e.parent);
        }
    }

    /// The element tree, one per line, indented (for looking into what
    /// a script made): kind, id, where, size, colour, and any text or
    /// sprite.
    pub fn describe(&self) -> String {
        let mut out = String::new();
        self.describe_element(ROOT, 0, &mut out);
        out
    }

    fn describe_element(&self, i: usize, depth: usize, out: &mut String) {
        let Some(e) = self.el(i) else { return };
        let (top_left, size) = self.local_rect(i);
        out.push_str(&format!(
            "{:indent$}{:?} {:08x} at ({:.0}, {:.0}) size ({:.0}, {:.0}) scale ({:.2}, {:.2}) rgba {:?} alpha {:.2}{}{}{}{}
",
            "",
            e.kind,
            e.id,
            top_left.x,
            top_left.y,
            size.x,
            size.y,
            e.look.scale.x,
            e.look.scale.y,
            e.look.rgba.map(|c| (c * 128.0).round() as i32),
            e.look.alpha,
            if e.text.is_empty() { String::new() } else { format!(" {:?}", e.text) },
            e.texture.map_or(String::new(), |t| format!(" texture {t:08x}")),
            if e.hidden { " hidden" } else { "" },
            if e.focused { " focused" } else { "" },
            indent = depth * 2
        ));
        for &c in &e.children {
            self.describe_element(c, depth + 1, out);
        }
    }

    /// What to draw, back to front.
    pub fn draw(&self) -> Vec<Draw> {
        // As the game draws its screen: one list, by z_priority and then
        // the order the elements were made in (a key's label, made after
        // the key's middle pieces, goes over them), not parent by parent.
        let mut out = Vec::new();
        self.draw_element(ROOT, Vec2::ZERO, Vec2::ONE, 1.0, (0.0, None), &mut out);
        out.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        out.into_iter().map(|(_, _, d)| d).collect()
    }

    fn el(&self, i: usize) -> Option<&Element> {
        self.elements.get(i).and_then(Option::as_ref)
    }

    fn el_mut(&mut self, i: usize) -> Option<&mut Element> {
        self.elements.get_mut(i).and_then(Option::as_mut)
    }

    fn find(&self, id: u32) -> Option<usize> {
        self.aliases
            .get(&id)
            .or_else(|| self.by_id.get(&id))
            .copied()
            .filter(|i| self.el(*i).is_some())
    }

    /// An element named in a script: by id or alias, or `{ <id> child = n }`
    /// (the child can be one of its own: `child = { n child = 0 }`, the
    /// keyboard's key's letter).
    fn resolve(&self, value: &Value) -> Option<usize> {
        match value {
            Value::Name(n) => self.find(*n),
            Value::Struct(items) => {
                let base = items.iter().find_map(|(k, v)| match (k, v) {
                    (None, v) => self.resolve(v),
                    _ => None,
                })?;
                match items.iter().find(|(k, _)| *k == Some(checksum("child"))) {
                    Some((_, child)) => self.child_of(base, child),
                    None => Some(base),
                }
            }
            _ => None,
        }
    }

    /// `child = n` of `base`, or `child = { n child = m }`: the n-th's m-th.
    fn child_of(&self, base: usize, child: &Value) -> Option<usize> {
        match child {
            Value::Integer(n) => self.el(base)?.children.get((*n).max(0) as usize).copied(),
            Value::Struct(items) => {
                let n = items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Integer(n)) => Some(*n),
                    _ => None,
                })?;
                let at = self.el(base)?.children.get(n.max(0) as usize).copied()?;
                match items.iter().find(|(k, _)| *k == Some(checksum("child"))) {
                    Some((_, deeper)) => self.child_of(at, deeper),
                    None => Some(at),
                }
            }
            _ => None,
        }
    }

    // ---- Layout ----

    fn font_of(&self, i: usize) -> Option<&Font> {
        let mut at = Some(i);
        while let Some(e) = at.and_then(|i| self.el(i)) {
            if let Some(f) = e.font.and_then(|f| self.fonts.get(&f)) {
                return Some(f);
            }
            at = e.parent;
        }
        self.fonts.get(&checksum("small"))
    }

    /// The text's lines: a block's wrapped to its width.
    fn lines(&self, i: usize) -> Vec<String> {
        let Some(e) = self.el(i) else {
            return Vec::new();
        };
        let mut lines = Vec::new();
        for paragraph in e.text.split('\n') {
            match (e.kind, e.dims, self.font_of(i)) {
                (Kind::TextBlock, Some(dims), Some(font)) if dims.x > 0.0 => {
                    let mut line = String::new();
                    for word in paragraph.split(' ') {
                        let tried = if line.is_empty() {
                            word.to_string()
                        } else {
                            format!("{line} {word}")
                        };
                        if self.text_width(font, &tried) * e.internal_scale > dims.x
                            && !line.is_empty()
                        {
                            lines.push(std::mem::take(&mut line));
                            line = word.to_string();
                        } else {
                            line = tried;
                        }
                    }
                    lines.push(line);
                }
                _ => lines.push(paragraph.to_string()),
            }
        }
        lines
    }

    /// Its size before its own scale.
    fn size(&self, i: usize) -> Vec2 {
        let Some(e) = self.el(i) else {
            return Vec2::ZERO;
        };
        match e.kind {
            Kind::Sprite => self
                .images
                .get(&e.texture.unwrap_or(0))
                .copied()
                .unwrap_or(Vec2::ZERO),
            Kind::Text | Kind::TextBlock => {
                let Some(font) = self.font_of(i) else {
                    return Vec2::ZERO;
                };
                let lines = self.lines(i);
                let wide = lines
                    .iter()
                    .map(|l| self.text_width(font, l))
                    .fold(0.0, f32::max);
                let k = e.internal_scale;
                let (wide, tall) = (
                    wide * k,
                    font.line_height as f32 * lines.len().max(1) as f32 * k,
                );
                match (e.kind, e.dims) {
                    (Kind::TextBlock, Some(d)) => Vec2::new(d.x, d.y.max(tall)),
                    _ => Vec2::new(wide, tall),
                }
            }
            Kind::VMenu | Kind::HMenu => {
                let across = e.kind == Kind::HMenu;
                let (mut long, mut wide) = (0.0f32, 0.0f32);
                for &c in &e.children {
                    let Some(child) = self.el(c).filter(|c| !c.hidden) else {
                        continue;
                    };
                    let s = self.size(c) * child.look.scale;
                    if across {
                        long += e.regular.unwrap_or(s.x + e.spacing);
                        wide = wide.max(s.y);
                    } else {
                        long += e.regular.unwrap_or(s.y * e.padding + e.spacing);
                        wide = wide.max(s.x);
                    }
                }
                let stacked = if across {
                    Vec2::new(long, wide)
                } else {
                    Vec2::new(wide, long)
                };
                e.dims.map_or(stacked, |d| d.max(stacked))
            }
            Kind::Container => e.dims.unwrap_or(Vec2::ZERO),
        }
    }

    /// Where a child sits in its parent's units: a menu's are stacked.
    fn local_pos(&self, i: usize) -> Vec2 {
        let Some(e) = self.el(i) else {
            return Vec2::ZERO;
        };
        let Some(parent) = e.parent.and_then(|p| self.el(p)) else {
            return e.look.pos;
        };
        if !matches!(parent.kind, Kind::VMenu | Kind::HMenu) || parent.scrolling {
            return e.look.pos;
        }
        let across = parent.kind == Kind::HMenu;
        let menu = self.size(e.parent.unwrap());
        let mut cursor = 0.0;
        for &c in &parent.children {
            let Some(child) = self.el(c).filter(|c| !c.hidden) else {
                continue;
            };
            let s = self.size(c) * child.look.scale;
            if c == i {
                // Lined up by the menu's internal_just, its own just
                // putting that point there.
                return if across {
                    Vec2::new(
                        cursor + s.x * e.just.x,
                        parent.internal_just.y * menu.y - s.y * parent.internal_just.y
                            + s.y * e.just.y,
                    )
                } else {
                    Vec2::new(
                        parent.internal_just.x * menu.x - s.x * parent.internal_just.x
                            + s.x * e.just.x,
                        cursor + s.y * e.just.y,
                    )
                };
            }
            cursor += parent.regular.unwrap_or(if across {
                s.x + parent.spacing
            } else {
                s.y * parent.padding + parent.spacing
            });
        }
        e.look.pos
    }

    /// Its top left in its parent's units, and its size (its own scale
    /// in).
    fn local_rect(&self, i: usize) -> (Vec2, Vec2) {
        let Some(e) = self.el(i) else {
            return (Vec2::ZERO, Vec2::ZERO);
        };
        let size = self.size(i) * e.look.scale;
        (self.local_pos(i) - e.just * size, size)
    }

    /// Element `i` and what's in it, each draw with where it goes in the
    /// game's order: its z (its own, else its parent's) and the order it
    /// was made in (or, behind its parent, just before the parent's).
    fn draw_element(
        &self,
        i: usize,
        origin: Vec2,
        scale: Vec2,
        alpha: f32,
        layer: (f32, Option<f64>),
        out: &mut Vec<(f32, f64, Draw)>,
    ) {
        self.draw_element_except(i, origin, scale, alpha, layer, out, &[]);
    }

    /// As `draw_element`, its children in `except` left out (a scrolling
    /// menu's rows out of its box).
    #[allow(clippy::too_many_arguments)]
    fn draw_element_except(
        &self,
        i: usize,
        origin: Vec2,
        scale: Vec2,
        alpha: f32,
        (parent_z, behind): (f32, Option<f64>),
        out: &mut Vec<(f32, f64, Draw)>,
        except: &[usize],
    ) {
        let Some(e) = self.el(i) else { return };
        if e.hidden {
            return;
        }
        let z = if e.z != 0.0 { e.z } else { parent_z };
        let order = behind.unwrap_or(i as f64);
        let (top_left, _) = self.local_rect(i);
        let at = origin + top_left * scale;
        let scale = scale * e.look.scale;
        let alpha = alpha * e.look.alpha;
        let size = self.size(i) * scale;
        let rgba = [
            e.look.rgba[0],
            e.look.rgba[1],
            e.look.rgba[2],
            e.look.rgba[3] * alpha,
        ];
        let mut children: Vec<usize> = e
            .children
            .iter()
            .copied()
            .filter(|c| !except.contains(c))
            .collect();
        children.sort_by(|a, b| {
            let z = |c: &usize| self.el(*c).map_or(0.0, |c| c.z);
            z(a).total_cmp(&z(b))
        });
        // Those drawn behind it first.
        for &c in &children {
            if self.el(c).is_some_and(|c| c.behind_parent) {
                self.draw_element(c, at, scale, alpha, (z, Some(order - 0.5)), out);
            }
        }
        match e.kind {
            Kind::Sprite => {
                if let Some(texture) = e.texture {
                    out.push((
                        z,
                        order,
                        Draw::Sprite {
                            texture,
                            rect: [at.x, at.y, at.x + size.x, at.y + size.y],
                            rgba,
                            angle: e.angle,
                        },
                    ));
                }
            }
            Kind::Text | Kind::TextBlock => {
                if let Some(font) = self.font_of(i) {
                    // The letters at their own size within it.
                    let scale = scale * e.internal_scale;
                    let font_name = self.font_name(i);
                    let lines = self.lines(i);
                    for (row, line) in lines.iter().enumerate() {
                        let line_w = self.text_width(font, line) * scale.x;
                        // A block's lines line up inside it.
                        let mut x = at.x
                            + match e.kind {
                                Kind::TextBlock => (size.x - line_w) * e.internal_just.x,
                                _ => 0.0,
                            };
                        let y = at.y + row as f32 * font.line_height as f32 * scale.y;
                        for ch in line.chars() {
                            // A controller button from the buttons font,
                            // in its own colours; the rest in the text's.
                            let (font, font_name, rgba) = match self.button_font(ch) {
                                Some(buttons) => (
                                    buttons,
                                    checksum(BUTTONS),
                                    [1.0, 1.0, 1.0, rgba[3].clamp(0.0, 1.0)],
                                ),
                                None => (font, font_name, rgba),
                            };
                            let Some(g) = font.glyph(ch) else {
                                x += font.advance(ch) * scale.x;
                                continue;
                            };
                            // The cell drawn so what shows starts at the pen.
                            let x0 = x - f32::from(g.ink_left) * scale.x;
                            let top =
                                y + (font.baseline as f32 - f32::from(g.top)).max(0.0) * scale.y;
                            out.push((
                                z,
                                order,
                                Draw::Glyph {
                                    font: font_name,
                                    source: [g.x, g.y, g.width, g.height],
                                    rect: [
                                        x0,
                                        top,
                                        x0 + f32::from(g.width) * scale.x,
                                        top + f32::from(g.height) * scale.y,
                                    ],
                                    rgba,
                                },
                            ));
                            x += font.advance(ch) * scale.x;
                        }
                    }
                }
            }
            _ => {}
        }
        // (Inside something drawn behind its parent, all of it is.)
        let inner = (z, behind.map(|_| order));
        if e.scrolling {
            self.draw_scrolled(i, &children, at, scale, alpha, inner, out);
            return;
        }
        for &c in &children {
            if self.el(c).is_some_and(|c| !c.behind_parent) {
                self.draw_element(c, at, scale, alpha, inner, out);
            }
        }
    }

    /// A scrolling menu's insides: moved by its scroll (kept so the
    /// selected item shows), and only what's inside its box.
    #[allow(clippy::too_many_arguments)]
    fn draw_scrolled(
        &self,
        i: usize,
        children: &[usize],
        at: Vec2,
        scale: Vec2,
        alpha: f32,
        layer: (f32, Option<f64>),
        out: &mut Vec<(f32, f64, Draw)>,
    ) {
        let Some(e) = self.el(i) else { return };
        let across = e.kind == Kind::HMenu;
        let span = e.dims.unwrap_or_else(|| self.size(i));
        let mut room = if across { span.x } else { span.y };
        // Where its window starts: its top, or with so many rows shown,
        // its menu's first row.
        let mut base = 0.0;
        // (Or only so many rows of its menu: `num_items_to_show`.)
        if let Some(n) = e.show {
            let rows = children
                .iter()
                .filter_map(|&m| self.el(m))
                .map(|m| m.children.as_slice())
                .next()
                .unwrap_or_default();
            if let (Some(&first), Some(&past)) = (rows.first(), rows.get(n)) {
                let start = |row: usize| {
                    let top_left = self.local_rect(row).0;
                    if across { top_left.x } else { top_left.y }
                };
                room = room.min(start(past) - start(first));
                let menu = children.first().map_or(0.0, |&m| {
                    let at = self.local_rect(m).0;
                    if across { at.x } else { at.y }
                });
                base = menu + start(first);
            }
        }
        // The selected item of the menu inside, where it is in that.
        let selected = children.iter().find_map(|&m| {
            let menu = self.el(m)?;
            let item = *menu.children.get(menu.selected?)?;
            let (top_left, size) = self.local_rect(item);
            let menu_at = self.local_rect(m).0;
            Some(if across {
                (menu_at.x + top_left.x, size.x)
            } else {
                (menu_at.y + top_left.y, size.y)
            })
        });
        let mut scroll = e.scroll.get();
        if let Some((start, length)) = selected {
            if start < scroll {
                scroll = start;
            } else if start + length > scroll + room {
                scroll = start + length - room;
            }
        }
        let scroll = scroll.max(base);
        e.scroll.set(scroll);
        let by = scroll - base;
        let shift = if across {
            Vec2::new(-by * scale.x, 0.0)
        } else {
            Vec2::new(0.0, -by * scale.y)
        };
        // Each menu's rows inside the box, whole.
        let slack = 2.0;
        for &c in children {
            let menu_at = self.local_rect(c).0;
            let outside: Vec<usize> = self.el(c).map_or(Vec::new(), |m| {
                m.children
                    .iter()
                    .copied()
                    .filter(|&row| {
                        let (top_left, size) = self.local_rect(row);
                        let (start, length) = if across {
                            (menu_at.x + top_left.x, size.x)
                        } else {
                            (menu_at.y + top_left.y, size.y)
                        };
                        start - scroll < -slack || start + length - scroll > room + slack
                    })
                    .collect()
            });
            // (What isn't a menu, its title, stays put.)
            let menu = self
                .el(c)
                .is_some_and(|m| matches!(m.kind, Kind::VMenu | Kind::HMenu));
            let moved = if menu { at + shift } else { at };
            self.draw_element_except(c, moved, scale, alpha, layer, out, &outside);
        }
    }

    /// The buttons font, for a controller button's character.
    fn button_font(&self, c: char) -> Option<&Font> {
        (u32::from(c) >= 0xffe0)
            .then(|| self.fonts.get(&checksum(BUTTONS)))
            .flatten()
    }

    /// A line's width, its buttons in the buttons font.
    fn text_width(&self, font: &Font, text: &str) -> f32 {
        text.chars()
            .map(|c| self.button_font(c).unwrap_or(font).advance(c))
            .sum()
    }

    fn font_name(&self, i: usize) -> u32 {
        let mut at = Some(i);
        while let Some(e) = at.and_then(|i| self.el(i)) {
            if let Some(f) = e.font.filter(|f| self.fonts.contains_key(f)) {
                return f;
            }
            at = e.parent;
        }
        checksum("small")
    }

    // ---- Changes ----

    fn create(&mut self, kind: Kind, parent: usize, id: Option<u32>) -> usize {
        let id = id.unwrap_or_else(|| {
            self.next_id += 1;
            checksum(&format!("screen_element_{}", self.next_id))
        });
        // An element by that id already: it's replaced.
        if let Some(old) = self.by_id.get(&id).copied() {
            self.destroy(old);
        }
        let i = self.elements.len();
        self.elements
            .push(Some(Element::new(kind, id, Some(parent))));
        self.by_id.insert(id, i);
        if let Some(p) = self.el_mut(parent) {
            p.children.push(i);
        }
        i
    }

    fn destroy(&mut self, i: usize) {
        if i == ROOT {
            // The root stays; its children go.
            let children = self
                .el(ROOT)
                .map(|r| r.children.clone())
                .unwrap_or_default();
            for c in children {
                self.destroy(c);
            }
            return;
        }
        let Some(e) = self.elements.get_mut(i).and_then(Option::take) else {
            return;
        };
        if self.by_id.get(&e.id) == Some(&i) {
            self.by_id.remove(&e.id);
        }
        self.aliases.retain(|_, v| *v != i);
        if let Some(p) = e.parent.and_then(|p| self.el_mut(p)) {
            p.children.retain(|c| *c != i);
            if let Some(s) = p.selected {
                p.selected = Some(s.min(p.children.len().saturating_sub(1)));
            }
        }
        for c in e.children {
            self.destroy(c);
        }
    }

    /// Sets what `props` says (the properties of `CreateScreenElement` and
    /// `SetScreenElementProps`), the colours and places as they are now if
    /// `morph` (to move to over `time`).
    fn apply(&mut self, i: usize, props: &Value, program: &Program) {
        let later = self.apply_props(i, props, program);
        self.place_after(i, later);
    }

    /// [`Screen::apply`]'s work, a proportional or relative place left for
    /// when the rest's in.
    fn apply_props(&mut self, i: usize, props: &Value, program: &Program) -> Option<Place> {
        let Value::Struct(items) = props else {
            return None;
        };
        // A new `just` alone re-anchors it where it is (`menu_onscreen`'s
        // `SetProps just = [center center]` doesn't move the menu): its
        // top left stays.
        let rejust = match (
            props.get(checksum("just")).and_then(just),
            props.get(checksum("pos")),
        ) {
            (Some(new), None) => self.el(i).map(|e| (e.just, new)),
            _ => None,
        };
        if let Some((old, new)) = rejust {
            let size = self.size(i) * self.el(i).map_or(Vec2::ONE, |e| e.look.scale);
            if let Some(e) = self.el_mut(i) {
                e.look.pos += (new - old) * size;
                e.just = new;
            }
        }
        let e = self.elements.get_mut(i).and_then(Option::as_mut)?;
        let replace = props.has_flag(checksum("replace_handlers"));
        let mut handlers_set = false;
        let mut pos_later = None;
        for (k, v) in items {
            match (*k, v) {
                (None, Value::Name(f)) => {
                    let f = *f;
                    if f == checksum("hide") {
                        e.hidden = true;
                    } else if f == checksum("unhide") {
                        e.hidden = false;
                    } else if f == checksum("not_focusable") {
                        e.not_focusable = true;
                    } else if f == checksum("focusable") {
                        e.not_focusable = false;
                    } else if f == checksum("dont_allow_wrap") {
                        e.wrap = false;
                    } else if f == checksum("draw_behind_parent") {
                        e.behind_parent = true;
                    }
                }
                (Some(k), v) => {
                    if k == checksum("pos") {
                        if let Some(p) = vec2(v) {
                            e.look.pos = p;
                        } else if let Some(place) = place(v) {
                            pos_later = Some(place);
                        }
                    } else if k == checksum("scale") {
                        if let Some(s) = scale(v) {
                            e.look.scale = s;
                        }
                    } else if k == checksum("rgba") {
                        if let Some(c) = rgba(v) {
                            e.look.rgba = c;
                        }
                    } else if k == checksum("alpha") {
                        if let Some(a) = v.as_f32() {
                            e.look.alpha = a;
                        }
                    } else if k == checksum("just") {
                        if let Some(j) = just(v) {
                            e.just = j;
                        }
                    } else if k == checksum("internal_just") {
                        if let Some(j) = just(v) {
                            e.internal_just = j;
                        }
                    } else if k == checksum("texture") {
                        e.texture = v.as_name();
                    } else if k == checksum("text") {
                        e.text = text(v, program);
                    } else if k == checksum("font") {
                        e.font = v.as_name();
                    } else if k == checksum("Dims") {
                        e.dims = vec2(v);
                    } else if k == checksum("z_priority") {
                        e.z = v.as_f32().unwrap_or(0.0);
                    } else if k == checksum("focusable_child") {
                        e.focusable_child = v.as_name();
                    } else if k == checksum("padding_scale") {
                        e.padding = v.as_f32().unwrap_or(1.0);
                    } else if k == checksum("rot_angle") {
                        e.angle = v.as_f32().unwrap_or(0.0);
                    } else if k == checksum("internal_scale") {
                        e.internal_scale = v.as_f32().unwrap_or(1.0);
                    } else if k == checksum("spacing_between") {
                        e.spacing = v.as_f32().unwrap_or(0.0);
                    } else if k == checksum("regular_space_amount") {
                        e.regular = v.as_f32();
                    } else if k == checksum("num_items_to_show") {
                        e.show = v.as_int().and_then(|n| usize::try_from(n).ok());
                    } else if k == checksum("tags") {
                        if let Value::Struct(tags) = v {
                            for (tk, tv) in tags.iter().filter(|(k, _)| k.is_some()) {
                                e.tags.retain(|(k, _)| k != tk);
                                e.tags.push((*tk, tv.clone()));
                            }
                        }
                    } else if k == checksum("event_handlers") {
                        let new: Vec<(u32, u32, Params)> = match v {
                            Value::Array(list) => list.iter().filter_map(handler).collect(),
                            _ => Vec::new(),
                        };
                        // Replacing: those for the events named go, the
                        // rest stay (an item's focus handlers outlive
                        // replacing its pad_choose ones).
                        if replace && !handlers_set {
                            e.handlers
                                .retain(|(ev, ..)| !new.iter().any(|h| h.0 == *ev));
                        }
                        handlers_set = true;
                        e.handlers.extend(new);
                    }
                }
                _ => {}
            }
        }
        pos_later
    }

    /// The proportional or relative place `apply` met, now it's sized.
    fn place_after(&mut self, i: usize, place: Option<Place>) {
        if let Some(place) = place {
            let p = self.resolve_place(i, place);
            if let Some(e) = self.el_mut(i) {
                e.look.pos = p;
            }
        }
    }

    /// Where a place puts element `i`, in its parent's units.
    fn resolve_place(&self, i: usize, place: Place) -> Vec2 {
        match place {
            Place::At(p) => p,
            Place::Share(f) => {
                let parent = self
                    .el(i)
                    .and_then(|e| e.parent)
                    .map_or(Vec2::ZERO, |p| self.size(p));
                f * parent
            }
            Place::By(d) => self.el(i).map_or(d, |e| e.look.pos + d),
        }
    }

    /// Moves to `props` (pos, scale, rgba, alpha) over `time` seconds.
    fn morph(&mut self, i: usize, props: &Value) -> f32 {
        let time = props
            .get(checksum("time"))
            .and_then(Value::as_f32)
            .unwrap_or(0.0);
        let pos = props
            .get(checksum("pos"))
            .and_then(place)
            .map(|p| self.resolve_place(i, p));
        let Some(e) = self.el_mut(i) else { return 0.0 };
        // What doesn't move over time is set now.
        if let Some(j) = props.get(checksum("just")).and_then(just) {
            e.just = j;
        }
        if let Some(j) = props.get(checksum("internal_just")).and_then(just) {
            e.internal_just = j;
        }
        if let Some(k) = props
            .get(checksum("internal_scale"))
            .and_then(Value::as_f32)
        {
            e.internal_scale = k;
        }
        let from = e.look;
        let mut to = from;
        if let Some(p) = pos {
            to.pos = p;
        }
        if let Some(s) = props.get(checksum("scale")).and_then(scale) {
            to.scale = s;
        }
        if let Some(c) = props.get(checksum("rgba")).and_then(rgba) {
            to.rgba = c;
        }
        if let Some(a) = props.get(checksum("alpha")).and_then(Value::as_f32) {
            to.alpha = a;
        }
        if time <= 0.0 {
            e.look = to;
            e.morph = None;
        } else {
            e.morph = Some(Morph {
                from,
                to,
                t: 0.0,
                time,
            });
        }
        time
    }

    // ---- Events ----

    /// An event to an element: its built-in doing (focus passed on), then
    /// its handlers. True if anything took it.
    fn fire(&mut self, i: usize, event: u32, data: Params) -> bool {
        let mut taken = false;
        if event == checksum("focus") {
            self.focus(i, &data);
            taken = true;
        } else if event == checksum("unfocus") {
            self.unfocus(i);
            taken = true;
        }
        taken | self.handle(i, event, data)
    }

    /// Runs its handlers for the event; true if it has any.
    fn handle(&mut self, i: usize, event: u32, data: Params) -> bool {
        let Some(e) = self.el(i) else { return false };
        let handlers: Vec<(u32, Params)> = e
            .handlers
            .iter()
            .filter(|(ev, ..)| *ev == event)
            .map(|(_, s, p)| (*s, p.clone()))
            .collect();
        for (script, mut params) in handlers.iter().cloned() {
            params.extend(data.iter().cloned());
            // A script the viewer does itself (View Goals' start a goal).
            if self.listening.contains(&script) {
                self.requests.push((script, Value::Struct(params)));
                continue;
            }
            self.starting.push(Running {
                element: i,
                thread: Thread::new(script, params),
                then: None,
            });
        }
        !handlers.is_empty()
    }

    fn focusable(&self, i: usize) -> bool {
        self.el(i).is_some_and(|e| !e.not_focusable && !e.hidden)
    }

    fn focus(&mut self, i: usize, data: &Params) {
        let Some(e) = self.el_mut(i) else { return };
        e.focused = true;
        let (kind, focusable_child) = (e.kind, e.focusable_child);
        match kind {
            Kind::VMenu | Kind::HMenu => {
                let Some(e) = self.el(i) else { return };
                let wanted = data
                    .iter()
                    .find(|(k, _)| *k == Some(checksum("child_id")))
                    .and_then(|(_, v)| self.resolve(v))
                    .and_then(|c| e.children.iter().position(|x| *x == c));
                let pick = wanted
                    .or(e.selected)
                    .filter(|s| e.children.get(*s).is_some_and(|c| self.focusable(*c)));
                let pick = pick.or_else(|| e.children.iter().position(|c| self.focusable(*c)));
                if let Some(s) = pick {
                    let child = e.children[s];
                    if let Some(e) = self.el_mut(i) {
                        e.selected = Some(s);
                    }
                    self.fire(child, checksum("focus"), Vec::new());
                }
            }
            _ => {
                if let Some(c) = focusable_child.and_then(|c| self.find(c)) {
                    self.fire(c, checksum("focus"), Vec::new());
                }
            }
        }
    }

    fn unfocus(&mut self, i: usize) {
        let Some(e) = self.el_mut(i) else { return };
        e.focused = false;
        let selected = e.selected.and_then(|s| e.children.get(s).copied());
        let child = e.focusable_child;
        if let Some(c) = selected.or_else(|| child.and_then(|c| self.find(c))) {
            if self.el(c).is_some_and(|c| c.focused) {
                self.fire(c, checksum("unfocus"), Vec::new());
            }
        }
    }

    /// A menu's own way with the pad: the focus moved up or down (or
    /// along). True if it took the event.
    fn navigate(&mut self, i: usize, pad: Pad) -> bool {
        let Some(e) = self.el(i) else { return false };
        let step: isize = match (e.kind, pad) {
            (Kind::VMenu, Pad::Up) | (Kind::HMenu, Pad::Left) => -1,
            (Kind::VMenu, Pad::Down) | (Kind::HMenu, Pad::Right) => 1,
            _ => return false,
        };
        let n = e.children.len() as isize;
        let Some(from) = e.selected else { return true };
        let mut at = from as isize;
        for _ in 0..n {
            at += step;
            if at < 0 || at >= n {
                if !e.wrap {
                    return true;
                }
                at = at.rem_euclid(n);
            }
            if self.focusable(e.children[at as usize]) {
                break;
            }
        }
        let (old, new) = (e.children[from], e.children[at as usize]);
        if old != new {
            self.fire(old, checksum("unfocus"), Vec::new());
            if let Some(e) = self.el_mut(i) {
                e.selected = Some(at as usize);
            }
            self.fire(new, checksum("focus"), Vec::new());
        }
        true
    }
}

fn lerp(a: Look, b: Look, k: f32) -> Look {
    let mix = |x: f32, y: f32| x + (y - x) * k;
    Look {
        pos: a.pos.lerp(b.pos, k),
        scale: a.scale.lerp(b.scale, k),
        rgba: [
            mix(a.rgba[0], b.rgba[0]),
            mix(a.rgba[1], b.rgba[1]),
            mix(a.rgba[2], b.rgba[2]),
            mix(a.rgba[3], b.rgba[3]),
        ],
        alpha: mix(a.alpha, b.alpha),
    }
}

/// A place for an element: a pair, `{ (0.5, 0.0) proportional }` (a share
/// of its parent's size) or `{ (0.0, 25.0) relative }` (from where it is).
enum Place {
    At(Vec2),
    Share(Vec2),
    By(Vec2),
}

fn place(v: &Value) -> Option<Place> {
    if let Some(p) = vec2(v) {
        return Some(Place::At(p));
    }
    let Value::Struct(items) = v else { return None };
    let p = items.iter().find_map(|(k, v)| match k {
        None => vec2(v),
        _ => None,
    })?;
    if v.has_flag(checksum("proportional")) {
        Some(Place::Share(p))
    } else if v.has_flag(checksum("relative")) {
        Some(Place::By(p))
    } else {
        Some(Place::At(p))
    }
}

fn vec2(v: &Value) -> Option<Vec2> {
    match v {
        Value::Pair([x, y]) => Some(Vec2::new(*x, *y)),
        Value::Vector([x, y, _]) => Some(Vec2::new(*x, *y)),
        _ => None,
    }
}

/// A scale: one number for both ways, or a pair.
fn scale(v: &Value) -> Option<Vec2> {
    vec2(v).or_else(|| v.as_f32().map(Vec2::splat))
}

/// `[r g b a]`, 128 full.
fn rgba(v: &Value) -> Option<[f32; 4]> {
    let Value::Array(items) = v else { return None };
    let c: Vec<f32> = items.iter().filter_map(Value::as_f32).collect();
    (c.len() == 4).then(|| [c[0] / 128.0, c[1] / 128.0, c[2] / 128.0, c[3] / 128.0])
}

/// `[center top]`: across then up and down (in either order, as named).
fn just(v: &Value) -> Option<Vec2> {
    let Value::Array(items) = v else { return None };
    let mut out = Vec2::splat(0.5);
    let names: Vec<u32> = items.iter().filter_map(Value::as_name).collect();
    for (n, name) in names.iter().enumerate() {
        if *name == checksum("left") {
            out.x = 0.0;
        } else if *name == checksum("right") {
            out.x = 1.0;
        } else if *name == checksum("top") {
            out.y = 0.0;
        } else if *name == checksum("bottom") {
            out.y = 1.0;
        } else if *name == checksum("center") {
            if n == 0 {
                out.x = 0.5;
            } else {
                out.y = 0.5;
            }
        }
    }
    Some(out)
}

/// The GameCube's controller-button font.
const BUTTONS: &str = "buttonsngc";

/// Text: a string, or a global naming one (the localized strings), its
/// escapes turned to characters (see [`unescape`]).
fn text(v: &Value, program: &Program) -> String {
    let raw = match v {
        Value::String(s) | Value::LocalString(s) => s.clone(),
        Value::Name(n) => match program.value(*n) {
            Some(Value::String(s) | Value::LocalString(s)) => s.clone(),
            _ => String::new(),
        },
        Value::Integer(i) => i.to_string(),
        _ => String::new(),
    };
    unescape(&raw)
}

/// `\n` a new line; `\bN` controller button N (the buttons font's
/// characters count down from U+FFFF: Y, B, X, A, then the d-pad's down,
/// right, left, up...); `\mN` the menus' buttons (0 accept: A, 1 back:
/// B).
fn unescape(raw: &str) -> String {
    let button = |n: u32| char::from_u32(0xffff - n.min(31)).unwrap_or(' ');
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            // `\cN`: a colour from the font's set; drawn in the text's own.
            Some('c') => {
                chars.next();
            }
            Some('b') => {
                if let Some(n) = chars.next().and_then(|d| d.to_digit(16)) {
                    out.push(button(n));
                }
            }
            Some('m') => {
                if let Some(n) = chars.next().and_then(|d| d.to_digit(16)) {
                    out.push(button(match n {
                        0 => 3,
                        1 => 1,
                        n => n,
                    }));
                }
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// `{ pad_choose script params = {...} }`: (event, script, params).
fn handler(v: &Value) -> Option<(u32, u32, Params)> {
    let Value::Struct(items) = v else { return None };
    let mut names = items.iter().filter_map(|(k, v)| match (k, v) {
        (None, Value::Name(n)) => Some(*n),
        _ => None,
    });
    let (event, script) = (names.next()?, names.next()?);
    let params = match v.get(checksum("params")) {
        Some(Value::Struct(p)) => p.clone(),
        _ => Vec::new(),
    };
    Some((event, script, params))
}

/// The commands scripts run on screen elements.
struct Ui<'a> {
    screen: &'a mut Screen,
    /// The element the script runs on.
    element: usize,
    program: &'a Program,
}

impl Ui<'_> {
    /// The arguments, unwrapped if they're one struct of their own
    /// (`CreateScreenElement { ... }`).
    fn props(args: &Value) -> Value {
        match args {
            Value::Struct(items) => {
                let inner = items.iter().find_map(|(k, v)| match (k, v) {
                    (None, inner @ Value::Struct(_)) => Some(inner),
                    _ => None,
                });
                match inner {
                    Some(Value::Struct(inner_items)) => {
                        // The struct's and any beside it.
                        let mut all = inner_items.clone();
                        all.extend(
                            items
                                .iter()
                                .filter(|(k, v)| k.is_some() || !matches!(v, Value::Struct(_)))
                                .cloned(),
                        );
                        Value::Struct(all)
                    }
                    _ => args.clone(),
                }
            }
            _ => args.clone(),
        }
    }
}

impl Ui<'_> {
    /// The character's profile (its name, stats...), its three special
    /// tricks assigned as the career gives them (lip, manual, air:
    /// `goal_get_special_trick_display_text`). (On a kid in the Skate
    /// Shop's skater select, `disney_skater` up to `LAST_CAK`, the profile
    /// asked about is the created skater's, as the game has it, with what
    /// the menus have changed in it.)
    fn current_profile(&self) -> Option<Params> {
        let c = checksum;
        let s = &*self.screen;
        let int = |name: &str| self.program.value(c(name)).and_then(Value::as_int);
        let on_kid = matches!(
            (int("disney_skater"), int("LAST_CAK")),
            (Some(i), Some(last)) if (0..=last).contains(&i)
        ) && s.level == Some(c("load_skateshop"));
        let id = if on_kid {
            Some("kid")
        } else {
            s.character.as_deref()
        };
        let Some(Value::Struct(profile)) =
            id.and_then(|id| skate::constants::profile(self.program, id))
        else {
            return None;
        };
        let special = Value::Struct(vec![
            (Some(c("TrickSlot")), Value::Name(c("Assigned"))),
            (Some(c("trickname")), Value::Name(c("Assigned"))),
        ]);
        let mut params = profile.clone();
        // (The girl kid's the kid's profile, as a girl.)
        let kid = matches!(s.character.as_deref(), Some("kid" | "kid_girl"));
        if s.character.as_deref() == Some("kid_girl") {
            params.retain(|(k, _)| *k != Some(c("is_male")));
            params.push((Some(c("is_male")), Value::Integer(0)));
        }
        if kid {
            for (k, v) in &s.profile_overrides {
                params.retain(|(o, _)| o != k);
                params.push((*k, v.clone()));
            }
        }
        params.retain(|(k, _)| *k != Some(c("specials")));
        params.push((
            Some(c("specials")),
            Value::Struct(vec![(None, Value::Array(vec![special; 3]))]),
        ));
        Some(params)
    }

    /// A temporary profile: as remembered, else a created skater's slot as
    /// saved (or empty, `info_cak`'s), else the kid on the stage.
    fn temp_profile(&self, profile: u32) -> Option<Params> {
        let c = checksum;
        if let Some(info) = self.screen.temp_profiles.get(&profile) {
            return Some(info.clone());
        }
        let slots = self
            .program
            .value(c("info_cak"))
            .and_then(Value::as_array)
            .unwrap_or_default();
        for (i, slot) in slots.iter().enumerate() {
            if c(&format!("cak_profile{i}")) != profile {
                continue;
            }
            if let Some(&(_, girl)) = self.screen.saved_kids.iter().find(|(s, _)| *s == i) {
                return Some(vec![
                    (
                        Some(c("display_name")),
                        Value::String(format!("Kid {}", i + 1)),
                    ),
                    (Some(c("is_male")), Value::Integer(i32::from(!girl))),
                    (Some(c("is_locked")), Value::Integer(-(i as i32))),
                ]);
            }
            return match slot.get(c("profile")) {
                Some(Value::Struct(info)) => Some(info.clone()),
                _ => None,
            };
        }
        self.current_profile()
    }
}

impl Host for Ui<'_> {
    fn is_self(&self, target: u32) -> bool {
        self.screen.find(target) == Some(self.element)
    }

    fn tags(&mut self) -> Option<&mut Params> {
        self.screen.el_mut(self.element).map(|e| &mut e.tags)
    }

    /// Scripts the viewer listens for are its to do (the screens it
    /// does itself), with their arguments.
    fn calling(&mut self, script: u32, args: &Value) -> bool {
        if self.screen.listening.contains(&script) {
            self.screen.requests.push((script, args.clone()));
            return true;
        }
        false
    }

    fn command(&mut self, target: Option<u32>, name: u32, args: &Value) -> Outcome {
        let c = checksum;
        let s = &mut *self.screen;
        // The skater moved for a menu (the Edit Skater set).
        if target == Some(c("skater")) && name == c("Obj_MoveToNode") {
            let props = globals_in(self.program, Ui::props(args));
            s.skater_node = props.get(c("Name")).and_then(Value::as_name);
            return Outcome::Done(true);
        }
        // `element:command`: on that element.
        let this = match target {
            Some(t) => match s.find(t) {
                Some(i) => i,
                None => return Outcome::Done(false),
            },
            None => self.element,
        };
        let props = globals_in(self.program, Ui::props(args));
        let named = |key: &str| props.get(c(key)).cloned();
        let target_of = |s: &Screen| -> Option<usize> {
            match named("id") {
                Some(v) => s.resolve(&v),
                None => Some(this),
            }
        };
        if name == c("CreateScreenElement") {
            let kind = match props.get(c("Type")).and_then(Value::as_name) {
                Some(t) if t == c("SpriteElement") => Kind::Sprite,
                Some(t) if t == c("TextElement") => Kind::Text,
                Some(t) if t == c("TextBlockElement") => Kind::TextBlock,
                Some(t) if t == c("VMenu") || t == c("vscrollingmenu") => Kind::VMenu,
                Some(t) if t == c("HMenu") || t == c("hscrollingmenu") => Kind::HMenu,
                _ => Kind::Container,
            };
            let parent = named("parent").and_then(|p| s.resolve(&p)).unwrap_or(ROOT);
            let id = named("id").and_then(|v| v.as_name()).filter(|id| *id != 0);
            let i = s.create(kind, parent, id);
            let scrolling = props
                .get(c("Type"))
                .and_then(Value::as_name)
                .is_some_and(|t| t == c("vscrollingmenu") || t == c("hscrollingmenu"));
            if let Some(e) = s.el_mut(i) {
                e.scrolling = scrolling;
            }
            s.apply(i, &props, self.program);
            let id = s.el(i).map_or(0, |e| e.id);
            return Outcome::Params(vec![(Some(c("id")), Value::Name(id))]);
        } else if name == c("SetScreenElementProps") || name == c("SetProps") {
            if let Some(i) = target_of(s) {
                s.apply(i, &props, self.program);
            }
        } else if name == c("DoScreenElementMorph") || name == c("DoMorph") {
            if let Some(i) = target_of(s) {
                let time = s.morph(i, &props);
                // DoMorph waits for it.
                if name == c("DoMorph") && time > 0.0 {
                    return Outcome::Wait(time);
                }
            }
        } else if name == c("DestroyScreenElement") {
            if let Some(i) = target_of(s) {
                s.destroy(i);
            }
        } else if name == c("Die") {
            s.destroy(this);
        } else if name == c("ScreenElementExists") || name == c("ObjectExists") {
            return Outcome::Done(named("id").and_then(|v| s.resolve(&v)).is_some());
        } else if name == c("AssignAlias") {
            if let (Some(i), Some(alias)) = (target_of(s), named("alias").and_then(|v| v.as_name()))
            {
                s.aliases.insert(alias, i);
            }
        } else if name == c("GetTags") {
            // (On another element: `root_window:GetTags`.)
            return Outcome::Params(s.el(this).map(|e| e.tags.clone()).unwrap_or_default());
        } else if name == c("SetTags") {
            if let (Some(e), Value::Struct(items)) = (s.el_mut(this), &props) {
                for (k, v) in items.iter().filter(|(k, _)| k.is_some()) {
                    e.tags.retain(|(o, _)| o != k);
                    e.tags.push((*k, v.clone()));
                }
            }
        } else if name == c("TextElementConcatenate")
            || name == c("TextElementBackspace")
            || name == c("GetTextElementLength")
            || name == c("GetTextElementString")
        {
            // A text element typed into (the game's on-screen keyboard):
            // a letter added (`TextElementConcatenate id = x "a"`), the
            // last taken off, how long it is (`length`), what it says
            // (`string`).
            let Some(i) = target_of(s) else {
                return Outcome::Done(false);
            };
            let added = match &props {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::String(t) | Value::LocalString(t)) => Some(t.clone()),
                    _ => None,
                }),
                _ => None,
            };
            let Some(e) = s.el_mut(i) else {
                return Outcome::Done(false);
            };
            if name == c("TextElementConcatenate") {
                if let Some(added) = added {
                    e.text.push_str(&added);
                }
            } else if name == c("TextElementBackspace") {
                e.text.pop();
            } else if name == c("GetTextElementLength") {
                let length = e.text.chars().count() as i32;
                return Outcome::Params(vec![(Some(c("length")), Value::Integer(length))]);
            } else {
                return Outcome::Params(vec![(Some(c("string")), Value::String(e.text.clone()))]);
            }
            return Outcome::Done(true);
        } else if name == c("GetScreenElementPosition") || name == c("GetScreenElementDims") {
            let Some(i) = target_of(s) else {
                return Outcome::Done(false);
            };
            let (top_left, size) = s.local_rect(i);
            // (Its place, or its size: not both, as a position's caller
            // may have a `width` of its own, the keyboard's wide keys.)
            return Outcome::Params(if name == c("GetScreenElementDims") {
                vec![
                    (Some(c("width")), Value::Float(size.x)),
                    (Some(c("height")), Value::Float(size.y)),
                ]
            } else {
                vec![(
                    Some(c("ScreenElementPos")),
                    Value::Pair(top_left.to_array()),
                )]
            });
        } else if name == c("GetStackedScreenElementPos") {
            let Some(i) = target_of(s) else {
                return Outcome::Done(false);
            };
            let (top_left, size) = s.local_rect(i);
            let offset = named("offset")
                .as_ref()
                .and_then(vec2)
                .unwrap_or(Vec2::ZERO);
            let along = if props.has_flag(c("y")) {
                Vec2::new(0.0, size.y)
            } else {
                Vec2::new(size.x, 0.0)
            };
            return Outcome::Params(vec![(
                Some(c("pos")),
                Value::Pair((top_left + along + offset).to_array()),
            )]);
        } else if name == c("RunScriptOnScreenElement") {
            let Some(i) = target_of(s) else {
                return Outcome::Done(false);
            };
            let script = match &props {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Name(n)) => Some(*n),
                    _ => None,
                }),
                _ => None,
            };
            let Some(script) = script else {
                return Outcome::Done(false);
            };
            let params = match props.get(c("params")) {
                Some(Value::Struct(p)) => p.clone(),
                _ => match &props {
                    // The rest passed on (`callback = ...`).
                    Value::Struct(items) => items
                        .iter()
                        .filter(|(k, _)| k.is_some_and(|k| k != c("id")))
                        .cloned()
                        .collect(),
                    _ => Vec::new(),
                },
            };
            // In place of what the element was running (an element runs a
            // script at a time: `do_scale_down` stops the focus's
            // `do_random_effect` flicker).
            s.stopping.push((i, None));
            let then = props.get(c("callback")).and_then(Value::as_name).map(|cb| {
                let p = match props.get(c("callback_params")) {
                    Some(Value::Struct(p)) => p.clone(),
                    _ => Vec::new(),
                };
                (cb, p)
            });
            s.starting.push(Running {
                element: i,
                thread: Thread::new(script, params),
                then,
            });
        } else if name == c("SpawnScript") {
            // Another script run alongside, on the same element
            // (`spawnscript control_menu_vibrate_controller params = ...`).
            let script = match &props {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Name(n)) => Some(*n),
                    _ => None,
                }),
                _ => None,
            };
            let Some(script) = script.filter(|n| self.program.has_script(*n)) else {
                return Outcome::Done(false);
            };
            let params = match props.get(c("params")) {
                Some(Value::Struct(p)) => p.clone(),
                _ => Vec::new(),
            };
            s.starting.push(Running {
                element: self.element,
                thread: Thread::new(script, params),
                then: None,
            });
        } else if name == c("TerminateObjectsScripts") {
            // An element's scripts stopped (those of one name, if named).
            if let Some(i) = target_of(s) {
                let only = named("script_name").and_then(|v| v.as_name());
                let doomed = |r: &Running| {
                    r.element == i && only.is_none_or(|n| r.thread.script() == Some(n))
                };
                s.starting.retain(|r| !doomed(r));
                s.stopping.push((i, only));
            }
        } else if name == c("FireEvent") || name == c("LaunchEvent") {
            let event = named("Type").and_then(|v| v.as_name());
            // (No target: the element the script runs on. `animate_in`,
            // run on a menu's anchor, focuses it so.)
            let to = match named("target") {
                Some(v) => s.resolve(&v),
                None => Some(self.element),
            };
            let data = match props.get(c("data")) {
                Some(Value::Struct(d)) => d.clone(),
                _ => Vec::new(),
            };
            if let (Some(event), Some(to)) = (event, to) {
                s.fire(to, event, data);
            }
        } else if name == c("GoalManager_GetGoalParams") {
            return match (&s.goal, named("Name").and_then(|v| v.as_name())) {
                (Some((id, params)), Some(asked)) if *id == asked => {
                    Outcome::Params(params.clone())
                }
                _ => Outcome::Done(false),
            };
        } else if name == c("GetGameMode") {
            // The career, as `GameModeEquals` has it.
            return Outcome::Params(vec![(Some(c("GameMode")), Value::Name(c("career")))]);
        } else if name == c("GameModeEquals") {
            // The levels are played as the career has them.
            let asked = match &props {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Name(n)) => Some(*n),
                    _ => None,
                }),
                _ => None,
            };
            return Outcome::Done(asked == Some(c("is_career")));
        } else if name == c("GoalManager_AddViewGoalsList") {
            // View Goals' rows: `view_goals_menu_add_item` for each goal.
            for (id, text, _) in s.goal_list.clone() {
                s.starting.push(Running {
                    element: self.element,
                    thread: Thread::new(
                        c("view_goals_menu_add_item"),
                        vec![
                            (Some(c("goal_ID")), Value::Name(id)),
                            (Some(c("text")), Value::String(text)),
                        ],
                    ),
                    then: None,
                });
            }
        } else if name == c("GetGlobalFlag") {
            // The levels' unlock flags (`LEVEL_UNLOCKED_PIZZA_PLANET`, as
            // the level list names them): every level's open in the
            // viewer. (Others, cheats and the like, are off.)
            let flag = named("flag").and_then(|v| v.as_name());
            // (And the Skate Shop's locked parts, `CAS_UNLOCK_063`: open
            // too, as everything is.)
            if flag.is_some_and(|f| cas_unlock_flags(self.program).contains(&f)) {
                return Outcome::Done(true);
            }
            let unlock = match self.program.value(c("level_select_menu_level_info")) {
                Some(Value::Array(levels)) => levels
                    .iter()
                    .any(|l| flag.is_some() && l.get(c("flag")).and_then(Value::as_name) == flag),
                _ => false,
            };
            return Outcome::Done(unlock);
        } else if name == c("GetLevelRecords") {
            // `GetLevelRecords level = 3`: the level's records, as the
            // records screen reads them (tables of one, and values).
            let level_num = props.get(c("level")).and_then(Value::as_int);
            let level = match self.program.value(c("level_select_menu_level_info")) {
                Some(Value::Array(levels)) => levels.iter().find_map(|l| {
                    (l.get(c("level_num")).and_then(Value::as_int) == level_num)
                        .then(|| l.get(c("level")).and_then(Value::as_name))
                        .flatten()
                }),
                _ => None,
            };
            let r = level
                .and_then(|l| s.records.get(&l))
                .copied()
                .unwrap_or([0; 6]);
            let table = |v: i32| {
                Value::Struct(vec![(
                    Some(c("RecordTable")),
                    Value::Array(vec![Value::Struct(vec![(
                        Some(c("value")),
                        Value::Integer(v),
                    )])]),
                )])
            };
            let single = |v: i32| {
                Value::Struct(vec![
                    (Some(c("value")), Value::Integer(v)),
                    (Some(c("initials")), Value::String(String::new())),
                ])
            };
            return Outcome::Params(vec![
                (Some(c("highscores")), table(r[0])),
                (Some(c("bestcombos")), table(r[1])),
                (Some(c("longestgrind")), single(r[2])),
                (Some(c("longestmanual")), single(r[3])),
                (Some(c("longestliptrick")), single(r[4])),
                (Some(c("longestcombo")), single(r[5])),
            ]);
        } else if name == c("HawkControlsIsOn") {
            return Outcome::Done(s.hawk_controls);
        } else if name == c("HawkControlsOn") || name == c("HawkControlsOff") {
            s.hawk_controls = name == c("HawkControlsOn");
            s.hawk_controls_changed = true;
        } else if name == c("AddMusicTrack") {
            let track = match &props {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::String(t) | Value::LocalString(t)) => Some(t.clone()),
                    _ => None,
                }),
                _ => None,
            };
            if let Some(track) = track {
                let name = track
                    .rsplit(['\\', '/'])
                    .next()
                    .unwrap_or(&track)
                    .to_string();
                s.music = Some(Some(name));
            }
        } else if name == c("PlayMovie") && named("mesh").is_none() {
            if let Some((movie, _)) = crate::behaviour::screen_movie(&props) {
                if s.movies.len() < 4 {
                    s.movies.push(movie);
                }
            }
        } else if name == c("PlayMovie") && named("mesh").is_some() {
            if let Some(movie) = crate::behaviour::screen_movie(&props) {
                s.screen_movie = Some(Some(movie));
            }
        } else if name == c("StopMovie") {
            s.screen_movie = Some(None);
        } else if name == c("MenuSelectedIndexIs") {
            // `id = menu first` (or `last`, or `index = n`): whether its
            // focused item is that one (the menus' up and down arrows).
            let Some(menu) = target_of(s).and_then(|i| s.el(i)) else {
                return Outcome::Done(false);
            };
            let Some(selected) = menu.selected else {
                return Outcome::Done(false);
            };
            let shown: Vec<usize> = menu
                .children
                .iter()
                .enumerate()
                .filter(|&(_, &c)| s.el(c).is_some_and(|e| !e.hidden))
                .map(|(i, _)| i)
                .collect();
            let is = if props.has_flag(c("first")) {
                shown.first() == Some(&selected)
            } else if props.has_flag(c("last")) {
                shown.last() == Some(&selected)
            } else {
                named("index").and_then(|v| v.as_int()) == Some(selected as i32)
            };
            return Outcome::Done(is);
        } else if name == c("GetPlayerAppearancePart") {
            // `if GetPlayerAppearancePart part = skater_m_hair`: its
            // `desc_id`, if the kid wears one.
            let part = named("part").and_then(|v| v.as_name());
            let worn = s.kid_appearance.iter().find(|(p, ..)| Some(*p) == part);
            return match worn {
                Some(&(_, desc, color)) => {
                    let mut out = vec![(Some(c("desc_id")), Value::Name(desc))];
                    // (Its colour, as the colour menu reads it.)
                    let [h, sat, v] = color.unwrap_or([0.0, 50.0, 50.0]);
                    out.extend([
                        (Some(c("h")), Value::Integer(h.round() as i32)),
                        (Some(c("S")), Value::Integer(sat.round() as i32)),
                        (Some(c("v")), Value::Integer(v.round() as i32)),
                        (
                            Some(c("use_default_hsv")),
                            Value::Integer(i32::from(color.is_none())),
                        ),
                    ]);
                    Outcome::Params(out)
                }
                None => Outcome::Done(false),
            };
        } else if name == c("GetActualCASOptionStruct") {
            // That part's entry in its list (`frontend_desc`, `no_color`...).
            let part = named("part").and_then(|v| v.as_name());
            let desc = named("desc_id").and_then(|v| v.as_name());
            let entry = part
                .and_then(|p| self.program.value(p))
                .and_then(Value::as_array)
                .and_then(|items| {
                    items
                        .iter()
                        .find(|e| e.get(c("desc_id")).and_then(Value::as_name) == desc)
                });
            return match entry {
                Some(Value::Struct(fields)) => Outcome::Params(fields.clone()),
                _ => Outcome::Done(false),
            };
        } else if name == c("RememberTemporaryAppearance")
            && named("appearance_structure").is_some()
        {
            let made = named("appearance_structure").and_then(|v| v.as_name());
            if made == Some(c("appearance_reset_girl")) {
                s.appearance_edits.push(AppearanceEdit::Reset(true));
            } else if made == Some(c("appearance_reset_boy")) {
                s.appearance_edits.push(AppearanceEdit::Reset(false));
            }
        } else if name == c("SetPlayerAppearance") {
            if let Some(app) =
                named("appearance_structure").and_then(|v| Appearance::of(&v, self.program))
            {
                s.appearance_edits.push(AppearanceEdit::Apply(app));
            }
        } else if name == c("RememberTemporaryProfile") || name == c("RememberTemporaryAppearance")
        {
            let profile = named("Name").and_then(|v| v.as_name());
            if name == c("RememberTemporaryProfile") {
                if let (Some(profile), Some(info)) = (profile, self.current_profile()) {
                    self.screen.temp_profiles.insert(profile, info);
                }
            }
            self.screen
                .appearance_edits
                .push(AppearanceEdit::Keep(profile));
        } else if name == c("RestoreTemporaryAppearance") {
            let profile = named("Name").and_then(|v| v.as_name());
            s.appearance_edits.push(AppearanceEdit::Restore(profile));
        } else if name == c("GetTemporaryProfileInfo") {
            let Some(profile) = named("Name").and_then(|v| v.as_name()) else {
                return Outcome::Done(false);
            };
            return match self.temp_profile(profile) {
                Some(info) => Outcome::Params(info),
                None => Outcome::Done(false),
            };
        } else if name == c("SetSkaterProfileInfo") {
            if let Some(Value::Struct(items)) = named("params") {
                for (k, v) in items {
                    if let Some(k) = k {
                        s.profile_overrides.retain(|(o, _)| *o != Some(k));
                        s.profile_overrides.push((Some(k), v));
                    }
                }
            }
        } else if name == c("SetPlayerAppearanceColor") {
            // `part = hat h = 220 S = 90 v = 60 use_default_hsv = 0`.
            let part = named("part").and_then(|v| v.as_name());
            let n = |k: &str| named(k).and_then(|v| v.as_f32());
            let own = named("use_default_hsv").and_then(|v| v.as_int()) == Some(1);
            let color = match (n("h"), n("S"), n("v")) {
                (Some(h), Some(sat), Some(v)) if !own => Some([h, sat, v]),
                _ => None,
            };
            if let Some(part) = part {
                s.appearance_edits.push(AppearanceEdit::Color(part, color));
            }
        } else if name == c("EditPlayerAppearance") {
            // `target = SetPart targetParams = { part = hat desc_id = x }`.
            let target = named("target").and_then(|v| v.as_name());
            let params = named("targetParams");
            let part = params
                .as_ref()
                .and_then(|p| p.get(c("part")))
                .and_then(Value::as_name);
            let desc = params
                .as_ref()
                .and_then(|p| p.get(c("desc_id")))
                .and_then(Value::as_name);
            if let Some(part) = part {
                if target == Some(c("SetPart")) {
                    if let Some(desc) = desc {
                        s.appearance_edits
                            .push(AppearanceEdit::Part(part, Some(desc)));
                    }
                } else if target == Some(c("ClearPart")) {
                    s.appearance_edits.push(AppearanceEdit::Part(part, None));
                }
            }
        } else if name == c("PauseMusic") || name == c("PauseStream") {
            let on = match &props {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Integer(n)) => Some(*n != 0),
                    _ => None,
                }),
                _ => None,
            };
            s.music_paused = Some(on.unwrap_or(true));
        } else if name == c("LocalSkaterExists") {
            return Outcome::Done(true);
        } else if [
            "InNetGame",
            "InSplitScreenGame",
            "InMultiPlayerGame",
            "RunningReplay",
        ]
        .iter()
        .any(|n| name == c(n))
        {
            return Outcome::Done(false);
        } else if name == c("StopMusic") || name == c("ClearMusicTrackList") {
            s.music = Some(None);
        } else if name == c("VibrateController") {
            // `Actuator = 1 percent = 70`: that motor at that strength
            // (1 the strong one), for the viewer to rumble; 0 stops it.
            let actuator = named("Actuator").and_then(|v| v.as_int()).unwrap_or(1);
            let percent = named("percent").and_then(|v| v.as_f32()).unwrap_or(0.0);
            if percent > 0.0 && s.vibrations.len() < 8 {
                s.vibrations.push((actuator == 1, percent / 100.0));
            }
        } else if name == c("VibrationIsOn") {
            return Outcome::Done(s.rumble);
        } else if name == c("VibrationOn") || name == c("VibrationOff") {
            s.rumble = name == c("VibrationOn");
            s.rumble_changed = true;
        } else if name == c("GetValueFromVolume") {
            let which = match &props {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Name(n)) => Some(*n),
                    _ => None,
                }),
                _ => None,
            };
            let value = if which == Some(c("sfxvol")) {
                s.sound_level
            } else {
                s.music_level
            };
            return Outcome::Params(vec![(Some(c("value")), Value::Integer(value))]);
        } else if name == c("SetMusicVolume") || name == c("SetSfxVolume") {
            // `SetMusicVolume 70`: out of 100.
            let level = match &props {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, v) => v.as_f32(),
                    _ => None,
                }),
                v => v.as_f32(),
            };
            if let Some(level) = level {
                let level = (level / 10.0).round().clamp(0.0, 10.0) as i32;
                if name == c("SetMusicVolume") {
                    s.music_level = level;
                } else {
                    s.sound_level = level;
                }
                s.volumes_changed = true;
            }
        } else if name == c("GoalManager_NumGoalsBeatenInLevel") || name == c("GetLevelGapTotals") {
            // The level select's and game progress' counts: the level on's
            // goals won and gaps landed (the others' aren't known here).
            let level_num = match &props {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Integer(n)) => Some(*n),
                    (Some(k), Value::Integer(n)) if *k == c("level") => Some(*n),
                    _ => None,
                }),
                _ => None,
            };
            let level = match self.program.value(c("level_select_menu_level_info")) {
                Some(Value::Array(levels)) => levels.iter().find_map(|l| {
                    (l.get(c("level_num")).and_then(Value::as_int) == level_num)
                        .then(|| l.get(c("level")).and_then(Value::as_name))
                        .flatten()
                }),
                _ => None,
            };
            let here = level.is_some() && level == s.level;
            let saved = level
                .and_then(|l| s.progress.get(&l))
                .copied()
                .unwrap_or((0, 0));
            if name == c("GetLevelGapTotals") {
                let landed = if here {
                    s.gaps.iter().filter(|(_, _, times)| *times > 0).count() as i32
                } else {
                    saved.1
                };
                return Outcome::Params(vec![(
                    Some(c("num_collected_gaps")),
                    Value::Integer(landed),
                )]);
            }
            let beaten = if here {
                s.goal_list.iter().filter(|(_, _, won)| *won).count() as i32
            } else {
                saved.0
            };
            return Outcome::Params(vec![(Some(c("num_beaten")), Value::Integer(beaten))]);
        } else if name == c("GoalManager_HasWonGoal") {
            let asked = named("Name").and_then(|v| v.as_name());
            return Outcome::Done(
                s.goal_list
                    .iter()
                    .any(|(id, _, won)| *won && Some(*id) == asked),
            );
        } else if name == c("GoalManager_GoalIsLocked") {
            // (Every goal's open in the viewer.)
            return Outcome::Done(false);
        } else if name == c("GoalManager_HasSeenGoal") {
            return Outcome::Done(true);
        } else if name == c("AddGapsToMenu") {
            // The gap checklist's rows: `gap_menu_add_item` for each gap.
            let count = s.gaps.len();
            for (n, (gap, score, times)) in s.gaps.clone().into_iter().enumerate() {
                let mut params = vec![
                    (Some(c("gap_name")), Value::String(gap)),
                    (Some(c("gap_score")), Value::Integer(score as i32)),
                    (Some(c("times")), Value::Integer(times as i32)),
                ];
                if n == 0 {
                    params.push((None, Value::Name(c("first_item"))));
                }
                if n + 1 == count {
                    params.push((None, Value::Name(c("last_item"))));
                }
                s.starting.push(Running {
                    element: self.element,
                    thread: Thread::new(c("gap_menu_add_item"), params),
                    then: None,
                });
            }
        } else if name == c("GetSkaterProfileInfoByName") {
            // Another character's profile (the skater select's), none of
            // them hidden: the viewer lets every character skate, where
            // the career unlocks some (`is_hidden` till a goal's won).
            let Some(Value::Struct(profile)) = named("Name")
                .and_then(|v| v.as_name())
                .and_then(|n| skate::constants::profile_named(self.program, n))
            else {
                return Outcome::Done(false);
            };
            let mut profile = profile.clone();
            profile.retain(|(k, _)| *k != Some(c("is_hidden")));
            return Outcome::Params(profile);
        } else if name == c("GetStatValue") {
            // `GetStatValue air skater = jessie`: a stat of that skater's
            // profile (or the one picked), as `stat_value`.
            let stat = match &props {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Name(n)) => Some(*n),
                    _ => None,
                }),
                _ => None,
            };
            let skater = named("skater")
                .and_then(|v| v.as_name())
                .or(s.selected_skater)
                .or_else(|| s.character.as_deref().map(c));
            let value = skater
                .and_then(|n| skate::constants::profile_named(self.program, n))
                .zip(stat)
                .and_then(|(p, stat)| p.get(stat))
                .and_then(Value::as_int)
                .unwrap_or(0);
            return Outcome::Params(vec![(Some(c("stat_value")), Value::Integer(value))]);
        } else if name == c("GetCurrentSkaterProfileIndex") {
            return Outcome::Params(vec![(
                Some(c("currentSkaterProfileIndex")),
                Value::Integer(0),
            )]);
        } else if name == c("SelectCurrentSkater") {
            // The skater picked: asked about (`if SelectCurrentSkater
            // Name = kid`), or chosen, for the viewer to load.
            let picked = named("Name").and_then(|v| v.as_name());
            if let Some(picked) = picked {
                let was = s.selected_skater.replace(picked);
                if was != Some(picked) && s.listening.contains(&name) {
                    s.requests.push((name, props.clone()));
                }
            }
            return Outcome::Done(true);
        } else if name == c("PlaySkaterCamAnim") {
            if let (Some(path), false) = (
                named("Name").and_then(|v| v.as_name()),
                props.has_flag(c("STOP")),
            ) {
                s.camera = Some(path);
            }
        } else if name == c("SkaterCamAnimFinished") {
            return Outcome::Done(true);
        } else if name == c("GetSkaterProfileInfo") {
            return match self.current_profile() {
                Some(params) => Outcome::Params(params),
                None => Outcome::Done(false),
            };
        } else if name == c("GoalManager_HasActiveGoals") {
            return Outcome::Done(s.goal.is_some());
        } else if name == c("GoalManager_CanRetryGoal") {
            return Outcome::Done(s.can_retry);
        } else if name == c("GoalManager_IsInCompetition") {
            return Outcome::Done(false);
        } else if name == c("LevelIs") {
            let asked = match &props {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Name(n)) => Some(*n),
                    _ => None,
                }),
                _ => None,
            };
            return Outcome::Done(asked.is_some() && asked == s.level);
        } else if name == c("IsTrue") {
            // A global flag's value (`IsTrue Bootstrap_Build`).
            let set = match &props {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Name(n)) => self.program.value(*n).and_then(Value::as_int),
                    _ => None,
                }),
                _ => None,
            };
            return Outcome::Done(set.is_some_and(|v| v != 0));
        } else if name == c("JoinServerComplete") {
            // (Back to the stage, the game joins its own one-player
            // server first, and waits on it: there's none to wait on.)
            return Outcome::Done(true);
        } else if name == c("English") {
            // The USA disc's language (the menus lay out wider for others).
            return Outcome::Done(true);
        } else if name == c("isngc") || name == c("CD") {
            // The GameCube disc's.
            return Outcome::Done(true);
        } else if name == c("isxbox") || name == c("IsPs2") || name == c("IsPC") {
            return Outcome::Done(false);
        } else if name == c("GetPlatform") {
            return Outcome::Params(vec![(Some(c("Platform")), Value::Name(c("NGC")))]);
        } else if name == c("playsound") {
            if let Some(sound) = match &props {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Name(n)) => Some(*n),
                    _ => None,
                }),
                _ => None,
            } {
                s.sounds.push(sound);
            }
        } else if [
            "SetScreenElementLock",
            "AddTextureToVram",
            "RemoveTextureFromVram",
            "KillSpawnedScript",
            "SetMenuPadMappings",
        ]
        .iter()
        .any(|n| name == c(n))
        {
            // Nothing to do here.
        } else if s.listening.contains(&name) {
            s.requests.push((name, props));
        } else {
            *s.unknown.entry(name).or_default() += 1;
            return Outcome::Done(false);
        }
        Outcome::Done(true)
    }
}

/// Settings given by a global's name (`scale = keyboard_button_scale`,
/// a pair): the global's value in its place. Only for sizes, places
/// and colours: other names (ids, textures, scripts) stay names.
fn globals_in(program: &qb::vm::Program, props: Value) -> Value {
    let Value::Struct(items) = props else {
        return props;
    };
    let keys = [
        "scale",
        "pos",
        "Dims",
        "rgba",
        "text_scale",
        "dims",
        "spacing_between",
    ]
    .map(checksum);
    Value::Struct(
        items
            .into_iter()
            .map(|(k, v)| match (k, &v) {
                (Some(key), Value::Name(n)) if keys.contains(&key) => match program.value(*n) {
                    Some(
                        g @ (Value::Pair(_)
                        | Value::Vector(_)
                        | Value::Float(_)
                        | Value::Integer(_)
                        | Value::Array(_)),
                    ) => (k, g.clone()),
                    _ => (k, v),
                },
                _ => (k, v),
            })
            .collect(),
    )
}

/// The unlock flags of the Skate Shop's parts (`unlock_flag =
/// CAS_UNLOCK_063` in its lists, as `master_editable_list` names them,
/// and the decks' `flag`).
fn cas_unlock_flags(program: &Program) -> Vec<u32> {
    let c = checksum;
    let mut flags = Vec::new();
    let lists = program
        .value(c("master_editable_list"))
        .and_then(Value::as_array)
        .unwrap_or_default();
    for list in lists {
        let Some(items) = list
            .get(c("part"))
            .and_then(Value::as_name)
            .and_then(|p| program.value(p))
            .and_then(Value::as_array)
        else {
            continue;
        };
        // (The decks' are `flag = BOARD_UNLOCKED_CASB01`.)
        flags.extend(items.iter().flat_map(|i| {
            ["unlock_flag", "flag"]
                .into_iter()
                .filter_map(|k| i.get(c(k)).and_then(Value::as_name))
        }));
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;
    use qb::Token;

    fn n(name: &str) -> Token {
        Token::Name(checksum(name))
    }

    /// A script of lines of tokens.
    fn script(lines: Vec<Vec<Token>>) -> Vec<Token> {
        let mut out = Vec::new();
        for line in lines {
            out.extend(line);
            out.push(Token::EndOfLine);
        }
        out
    }

    #[test]
    fn button_escapes() {
        assert_eq!(unescape(r"\b7/\b4 = Select"), "\u{fff8}/\u{fffb} = Select");
        assert_eq!(unescape(r"\m0 = Accept\nNext"), "\u{fffc} = Accept\nNext");
    }

    #[test]
    fn menus_take_the_pad_and_items_their_choice() {
        let mut program = Program::new();
        // A VMenu of three items; choosing the second runs Chosen.
        let item = |id: &str, chosen: bool| {
            let mut line = vec![
                n("CreateScreenElement"),
                n("Type"),
                Token::Equals,
                n("ContainerElement"),
                n("parent"),
                Token::Equals,
                n("menu"),
                n("id"),
                Token::Equals,
                n(id),
                n("Dims"),
                Token::Equals,
                Token::Pair([100.0, 20.0]),
            ];
            if chosen {
                line.extend([
                    n("event_handlers"),
                    Token::Equals,
                    Token::StartArray,
                    Token::StartStruct,
                    n("pad_choose"),
                    n("Chosen"),
                    Token::EndStruct,
                    Token::EndArray,
                ]);
            }
            line
        };
        program.add_script(
            checksum("Make"),
            script(vec![
                vec![
                    n("CreateScreenElement"),
                    n("Type"),
                    Token::Equals,
                    n("VMenu"),
                    n("id"),
                    Token::Equals,
                    n("menu"),
                ],
                item("a", false),
                item("b", true),
                item("c", false),
                vec![
                    n("FireEvent"),
                    n("Type"),
                    Token::Equals,
                    n("focus"),
                    n("target"),
                    Token::Equals,
                    n("menu"),
                ],
            ]),
        );
        program.add_script(checksum("Chosen"), script(vec![vec![n("Picked")]]));
        let mut screen = Screen::new(HashMap::new(), HashMap::new());
        screen.listen(&["Picked"]);
        screen.run(checksum("Make"), Vec::new());
        screen.update(&program, 0.0);
        assert!(screen.exists("b"));
        // The first's focused; down once, and choose.
        let focused = |s: &Screen, id: &str| s.el(s.find(checksum(id)).unwrap()).unwrap().focused;
        assert!(focused(&screen, "a"));
        screen.pad(Pad::Down);
        assert!(focused(&screen, "b") && !focused(&screen, "a"));
        screen.pad(Pad::Choose);
        screen.update(&program, 0.0);
        assert_eq!(screen.requests.len(), 1);
        // Stacked: b below a.
        let (a, _) = screen.local_rect(screen.find(checksum("a")).unwrap());
        let (b, _) = screen.local_rect(screen.find(checksum("b")).unwrap());
        assert_eq!(b.y - a.y, 20.0);
        // Up twice wraps round to c.
        screen.pad(Pad::Up);
        screen.pad(Pad::Up);
        assert!(focused(&screen, "c"));
    }
}
