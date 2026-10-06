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

/// A lip trick (`LipMacro2`): its name and score, its animations in, held
/// (balanced along the meter) and out, and whether ollying ends it
/// without an ollie (`NoOllie`).
#[derive(Clone, Debug, PartialEq)]
pub struct LipTrick {
    pub name: String,
    pub score: u32,
    pub init: Option<u32>,
    pub range: u32,
    pub out: Option<u32>,
    pub no_ollie: bool,
}

/// A grind or manual: its name and score, and its animations in and
/// held (balanced along the meter).
#[derive(Clone, Debug, PartialEq)]
pub struct BalanceTrick {
    pub name: String,
    pub score: u32,
    pub init: Option<u32>,
    pub range: u32,
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
    /// The character's special grab (two directions in order, then flip)
    /// and special manual (two directions, then grind), once the special
    /// meter is full.
    pub special_air: Option<(Dir, Dir, usize)>,
    pub special_manual: Option<(Dir, Dir, (String, u32))>,
    /// Lip tricks: by the direction pressed before reaching the coping
    /// (`LipTricks`: `{ Press, Left, 500 }` and so on), the plain one with
    /// none (`Lip_Triangle`), and the special (two directions then grind,
    /// within a second, `SpecialLipTricks`).
    pub lips: Vec<(Option<Dir>, LipTrick)>,
    /// Grinds by the direction pressed with the grind button
    /// (`GrindTricks`: none `Grind1`, up `Grind2`, right `Grind3`, down
    /// `Grind4`, left `Grind5`), and manuals by the direction with Circle
    /// (`ManualTricks`, `GroundManualTrickBranches`: `Manual2` to `5`; the
    /// up-down press is `Manual1`).
    pub grinds: Vec<(Option<Dir>, BalanceTrick)>,
    pub manuals: Vec<(Option<Dir>, BalanceTrick)>,
    /// The special manual's animations and name.
    pub special_manual_trick: Option<BalanceTrick>,
    pub special_lip: Option<(Dir, Dir, LipTrick)>,
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

        // Grinds and manuals, by slot.
        let balance = |slot_name: &str, init: &str, range: &str| -> Option<BalanceTrick> {
            let trick = program.value(table.get(checksum(slot_name))?.as_name()?)?;
            parse_balance(trick, init, range)
        };
        for (slot_name, dirs) in [
            ("Grind1", vec![None]),
            (
                "Grind2",
                vec![Some(Dir::Up), Some(Dir::UpLeft), Some(Dir::UpRight)],
            ),
            ("Grind3", vec![Some(Dir::Right)]),
            (
                "Grind4",
                vec![Some(Dir::Down), Some(Dir::DownLeft), Some(Dir::DownRight)],
            ),
            ("Grind5", vec![Some(Dir::Left)]),
        ] {
            if let Some(trick) = balance(slot_name, "InitAnim", "anim") {
                for dir in dirs {
                    book.grinds.push((dir, trick.clone()));
                }
            }
        }
        for (slot_name, dir) in [
            ("Manual1", None),
            ("Manual2", Some(Dir::Up)),
            ("Manual3", Some(Dir::Right)),
            ("Manual4", Some(Dir::Down)),
            ("Manual5", Some(Dir::Left)),
        ] {
            if let Some(trick) = balance(slot_name, "InitAnim", "BalanceAnim") {
                book.manuals.push((dir, trick));
            }
        }

        // Lips, by slot.
        for (slot_name, dir) in [
            ("Lip_Triangle", None),
            ("Lip_TriangleU", Some(Dir::Up)),
            ("Lip_TriangleD", Some(Dir::Down)),
            ("Lip_TriangleL", Some(Dir::Left)),
            ("Lip_TriangleR", Some(Dir::Right)),
            ("Lip_TriangleUL", Some(Dir::UpLeft)),
            ("Lip_TriangleUR", Some(Dir::UpRight)),
            ("Lip_TriangleDL", Some(Dir::DownLeft)),
            ("Lip_TriangleDR", Some(Dir::DownRight)),
        ] {
            let lip = table
                .get(checksum(slot_name))
                .and_then(Value::as_name)
                .and_then(|n| program.value(n))
                .and_then(parse_lip);
            if let Some(lip) = lip {
                book.lips.push((dir, lip));
            }
        }

        // Specials: the game gives each character its own slot when a goal
        // unlocks it; here they're all unlocked.
        if let Some((air, manual)) = special_slots(character) {
            let trick = |kind: &str| program.value(checksum(&format!("Trick_{character}Sp{kind}")));
            if let Some(trick) = trick("Grab").and_then(|t| parse_trick(t, flip_speed)) {
                book.tricks.push(trick);
                book.special_air = Some((air.0, air.1, book.tricks.len() - 1));
            }
            book.special_manual_trick =
                trick("Manual").and_then(|t| parse_balance(t, "InitAnim", "BalanceAnim"));
            if let Some(trick) = trick("Manual") {
                let params = trick.get(checksum("params"));
                let score = params
                    .and_then(|p| p.get(checksum("score")))
                    .and_then(Value::as_int)
                    .unwrap_or(0);
                book.special_manual =
                    Some((manual.0, manual.1, (trick_name(trick), score.max(0) as u32)));
            }
            if let (Some(trick), Some(slot)) = (trick("Lip"), special_lip_slot(character)) {
                if let Some(lip) = parse_lip(trick) {
                    book.special_lip = Some((slot.0, slot.1, lip));
                }
            }
        }
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

    /// The grind or manual for a direction (or the plain one).
    pub fn balance_trick(
        list: &[(Option<Dir>, BalanceTrick)],
        dir: Option<Dir>,
    ) -> Option<BalanceTrick> {
        list.iter()
            .find(|(d, _)| dir.is_some() && *d == dir)
            .or_else(|| list.iter().find(|(d, _)| d.is_none()))
            .map(|(_, t)| t.clone())
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

/// Each character's special grab and special manual slots: the two
/// directions before the button (Square, then Triangle). The game assigns
/// them as goals unlock them (`goal_get_special_trick_display_text` in
/// `GOAL_UTILITIES.q`, which spells them `SpAir_D_R_Square`,
/// `SpMan_R_L_Triangle`...).
fn special_slots(character: &str) -> Option<((Dir, Dir), (Dir, Dir))> {
    use Dir::{Down as D, Left as L, Right as R, Up as U};
    Some(match character.to_ascii_lowercase().as_str() {
        "buzz" => ((U, R), (L, U)),
        "woody" => ((L, R), (D, U)),
        "jessie" => ((D, R), (R, L)),
        "zurg" => ((R, U), (U, R)),
        "tarzan" => ((U, D), (L, L)),
        "tantor" => ((U, U), (R, D)),
        "jane" => ((R, L), (L, U)),
        "terk" => ((U, R), (D, D)),
        "simba" => ((U, L), (L, R)),
        "timon" => ((R, U), (L, L)),
        "rafiki" => ((L, U), (R, R)),
        "nala" => ((R, L), (D, D)),
        "kid" => ((L, R), (U, U)),
        _ => return None,
    })
}

/// Each character's special lip slot (`SpLip_L_D_Triangle` for Jessie...),
/// from the same goal script.
fn special_lip_slot(character: &str) -> Option<(Dir, Dir)> {
    use Dir::{Down as D, Left as L, Right as R, Up as U};
    Some(match character.to_ascii_lowercase().as_str() {
        "buzz" => (R, D),
        "woody" => (U, D),
        "jessie" => (L, D),
        "zurg" => (D, D),
        "tarzan" => (D, R),
        "tantor" => (D, L),
        "jane" => (R, R),
        "terk" => (L, D),
        "simba" => (D, U),
        "timon" => (D, R),
        "rafiki" => (U, D),
        "nala" => (U, R),
        "kid" => (D, L),
        _ => return None,
    })
}

/// A `Grind` or `Manual` trick, with the keys its animations are under.
fn parse_balance(trick: &Value, init: &str, range: &str) -> Option<BalanceTrick> {
    let params = trick.get(checksum("params"))?;
    Some(BalanceTrick {
        name: trick_name(trick),
        score: params
            .get(checksum("score"))
            .and_then(Value::as_int)
            .unwrap_or(0)
            .max(0) as u32,
        init: params.get(checksum(init)).and_then(Value::as_name),
        range: params.get(checksum(range)).and_then(Value::as_name)?,
    })
}

/// A `LipMacro2` trick.
fn parse_lip(trick: &Value) -> Option<LipTrick> {
    if trick.get(checksum("scr")).and_then(Value::as_name) != Some(checksum("LipMacro2")) {
        return None;
    }
    let params = trick.get(checksum("params"))?;
    let anim = |key: &str| params.get(checksum(key)).and_then(Value::as_name);
    Some(LipTrick {
        name: trick_name(trick),
        score: params
            .get(checksum("score"))
            .and_then(Value::as_int)
            .unwrap_or(0)
            .max(0) as u32,
        init: anim("InitAnim"),
        range: anim("anim")?,
        out: anim("OutAnim"),
        no_ollie: params.has_flag(checksum("NoOllie")),
    })
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
        // `GrabTrick`'s default, `GRABTWEAK_MEDIUM`; `GRABTWEAK_SPECIAL` for
        // specials.
        tweak: number("GrabTweak").unwrap_or(if params.has_flag(checksum("IsSpecial")) {
            30.0
        } else {
            20.0
        }) as u32,
    })
}
