//! Tricks: what the trick buttons do, read from the game's scripts.
//!
//! The scripts list the button combinations (`AirTricks` in
//! `airtricks.q`: a button with a direction, `AirTrickLogic`) and the slot
//! each one fills (`Air_SquareD`, `Air_CircleL`...); each character's
//! profile names a trick table (`default_trick_mapping = JessieTricks`)
//! that puts a trick in every slot: a script (`FlipTrick`, `GrabTrick`),
//! its name, score and animations. The scripts then play it: a flip plays
//! its animation once, a grab plays its way in, holds while the button is
//! held and plays back out. Both turn bailing on (`BailOn`): landing before
//! the trick is nearly over (`trickslack` frames from the end) is a bail.
//!
//! Only the default ("Hawk") controls are read.

use qb::vm::Program;
use qb::{Value, checksum};

use crate::constants::{Stats, profile, scale};

/// The trick buttons: Square flips, Circle grabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Flip,
    Grab,
}

/// A direction held with a trick button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
    UpLeft,
    UpRight,
    DownLeft,
    DownRight,
}

impl Dir {
    /// The direction from up/down and left/right held, if any.
    pub fn from_held(up: bool, down: bool, left: bool, right: bool) -> Option<Dir> {
        let v = i32::from(up) - i32::from(down);
        let h = i32::from(right) - i32::from(left);
        Some(match (v, h) {
            (1, 0) => Dir::Up,
            (-1, 0) => Dir::Down,
            (0, -1) => Dir::Left,
            (0, 1) => Dir::Right,
            (1, -1) => Dir::UpLeft,
            (1, 1) => Dir::UpRight,
            (-1, -1) => Dir::DownLeft,
            (-1, 1) => Dir::DownRight,
            _ => return None,
        })
    }

    fn from_name(name: u32) -> Option<Dir> {
        [
            ("Up", Dir::Up),
            ("Down", Dir::Down),
            ("Left", Dir::Left),
            ("Right", Dir::Right),
            ("UpLeft", Dir::UpLeft),
            ("UpRight", Dir::UpRight),
            ("DownLeft", Dir::DownLeft),
            ("DownRight", Dir::DownRight),
        ]
        .into_iter()
        .find_map(|(n, d)| (checksum(n) == name).then_some(d))
    }
}

/// How a trick plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Flip,
    Grab,
}

/// One trick, as a character's trick table gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Trick {
    pub name: String,
    pub score: u32,
    pub kind: Kind,
    /// The animation (its name's checksum), and for a grab the one held.
    pub anim: u32,
    pub idle: Option<u32>,
    /// Playback speed (flips: times the flip speed stat, at most 1.3).
    pub speed: f32,
    /// How many frames from the end bailing turns off.
    pub trickslack: f32,
    /// Points a frame while a grab is held (`GrabTweak`).
    pub tweak: u32,
}

/// A character's tricks and the combinations that do them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrickBook {
    pub tricks: Vec<Trick>,
    air: Vec<(Button, Dir, usize)>,
    /// The trick for each button pressed with no direction (not in the
    /// default controls; the viewer's convenience, using the slot the
    /// simplified controls give a bare press).
    neutral: Vec<(Button, usize)>,
    /// The names and scores of the character's first grind and manual.
    pub grind: Option<(String, u32)>,
    pub manual: Option<(String, u32)>,
    /// How long each animation runs, in seconds, from whoever has the
    /// character's animations loaded (see [`TrickBook::set_durations`]).
    durations: Vec<(u32, f32)>,
}

impl TrickBook {
    /// The tricks of `character` (such as `jessie`), from the scripts in
    /// `program`, with its stats.
    pub fn new(program: &Program, character: &str, stats: &Stats) -> Self {
        let mut book = TrickBook::default();
        let Some(table) = profile(program, character)
            .and_then(|p| p.get(checksum("default_trick_mapping")))
            .and_then(Value::as_name)
            .and_then(|n| resolve(program, n))
        else {
            return book;
        };
        let flip_speed = program
            .value(checksum("Skater_Flip_Speed_Stat"))
            .and_then(|v| scale(program, v, stats))
            .unwrap_or(1.0);
        let slot = |book: &mut TrickBook, slot: u32| -> Option<usize> {
            let trick = table.get(slot).and_then(Value::as_name)?;
            let trick = program.value(trick)?;
            let index = book
                .tricks
                .iter()
                .position(|t| t.name == trick_name(trick))?;
            Some(index)
        };
        // Every air trick the table can reach, once.
        let add = |book: &mut TrickBook, slot_name: u32| -> Option<usize> {
            if let Some(i) = slot(book, slot_name) {
                return Some(i);
            }
            let trick = program.value(table.get(slot_name)?.as_name()?)?;
            let parsed = parse_trick(trick, flip_speed)?;
            book.tricks.push(parsed);
            Some(book.tricks.len() - 1)
        };
        let triggers = program
            .value(checksum("AirTricks"))
            .and_then(Value::as_array)
            .unwrap_or_default();
        for entry in triggers {
            let hawk = entry.has_flag(checksum("Hawk"));
            let Some(trigger) = entry.get(checksum("trigger")) else {
                continue;
            };
            let Some(slot_name) = entry.get(checksum("TrickSlot")).and_then(Value::as_name) else {
                continue;
            };
            let Value::Struct(items) = trigger else {
                continue;
            };
            let names: Vec<u32> = items.iter().filter_map(|(_, v)| v.as_name()).collect();
            if names.first() != Some(&checksum("AirTrickLogic")) {
                continue;
            }
            let button = match names.get(1) {
                Some(&n) if n == checksum("Square") => Button::Flip,
                Some(&n) if n == checksum("circle") => Button::Grab,
                // The simplified controls' bare double press.
                Some(&n) if n == checksum("Squirgle") && names.get(2) == Some(&n) => {
                    if let Some(i) = add(&mut book, slot_name) {
                        book.neutral.push((Button::Flip, i));
                    }
                    continue;
                }
                _ => continue,
            };
            if !hawk {
                continue;
            }
            let Some(dir) = names.get(2).and_then(|&n| Dir::from_name(n)) else {
                continue;
            };
            if let Some(i) = add(&mut book, slot_name) {
                book.air.push((button, dir, i));
            }
        }
        // A bare grab: the up grab.
        if let Some(&(_, _, i)) = book
            .air
            .iter()
            .find(|(b, d, _)| *b == Button::Grab && *d == Dir::Up)
        {
            book.neutral.push((Button::Grab, i));
        }
        let named = |slot_name: &str| {
            let trick = program.value(table.get(checksum(slot_name))?.as_name()?)?;
            let params = trick.get(checksum("params"))?;
            let score = params
                .get(checksum("score"))
                .and_then(Value::as_int)
                .unwrap_or(0);
            Some((trick_name(trick), score.max(0) as u32))
        };
        book.grind = named("Grind1");
        book.manual = named("Manual1");
        book
    }

    /// Gives each trick animation's length, by the checksum of its name.
    pub fn set_durations(&mut self, duration: impl Fn(u32) -> Option<f32>) {
        let anims: Vec<u32> = self
            .tricks
            .iter()
            .flat_map(|t| [Some(t.anim), t.idle])
            .flatten()
            .collect();
        self.durations = anims
            .into_iter()
            .filter_map(|a| duration(a).map(|d| (a, d)))
            .collect();
    }

    /// An animation's length in seconds (1 if it isn't known).
    pub fn duration(&self, anim: u32) -> f32 {
        self.durations
            .iter()
            .find(|(a, _)| *a == anim)
            .map_or(1.0, |&(_, d)| d.max(0.01))
    }

    /// The trick for a button pressed with a direction held (or none).
    pub fn air_trick(&self, button: Button, dir: Option<Dir>) -> Option<usize> {
        match dir {
            Some(dir) => self
                .air
                .iter()
                .find(|(b, d, _)| *b == button && *d == dir)
                .map(|&(_, _, i)| i),
            None => self
                .neutral
                .iter()
                .find(|(b, _)| *b == button)
                .map(|&(_, i)| i),
        }
    }
}

/// A name's value, following names that stand for other values
/// (`JessieTricks = JessieTricks_default`).
fn resolve(program: &Program, mut name: u32) -> Option<&Value> {
    for _ in 0..8 {
        let value = program.value(name)?;
        match value {
            Value::Name(next) => name = *next,
            _ => return Some(value),
        }
    }
    None
}

fn trick_name(trick: &Value) -> String {
    match trick
        .get(checksum("params"))
        .and_then(|p| p.get(checksum("Name")))
    {
        Some(Value::String(s) | Value::LocalString(s)) => s.clone(),
        _ => String::new(),
    }
}

fn parse_trick(trick: &Value, flip_speed: f32) -> Option<Trick> {
    let script = trick.get(checksum("scr")).and_then(Value::as_name)?;
    let kind = if script == checksum("FlipTrick") {
        Kind::Flip
    } else if script == checksum("GrabTrick") {
        Kind::Grab
    } else {
        return None;
    };
    let params = trick.get(checksum("params"))?;
    let number = |key: &str| params.get(checksum(key)).and_then(Value::as_f32);
    let speed = number("speed").unwrap_or(1.0);
    let speed = match kind {
        // `FlipTrick`: speed times the flip speed stat, at most `MaxSpeed`
        // or 1.3.
        Kind::Flip => (speed * flip_speed).min(number("MaxSpeed").unwrap_or(1.3)),
        Kind::Grab => speed,
    };
    Some(Trick {
        name: trick_name(trick),
        score: params
            .get(checksum("score"))
            .and_then(Value::as_int)
            .unwrap_or(0)
            .max(0) as u32,
        kind,
        anim: params.get(checksum("anim")).and_then(Value::as_name)?,
        idle: params.get(checksum("idle")).and_then(Value::as_name),
        speed,
        // The scripts' defaults.
        trickslack: number("trickslack").unwrap_or(match kind {
            Kind::Flip => 10.0,
            Kind::Grab => 0.0,
        }),
        // `GrabTrick`'s default, `GRABTWEAK_MEDIUM`.
        tweak: number("GrabTweak").unwrap_or(20.0) as u32,
    })
}
