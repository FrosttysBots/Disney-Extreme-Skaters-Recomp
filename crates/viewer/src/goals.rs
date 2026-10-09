//! Goals' settings, read by running a level's goal scripts with the QB
//! interpreter and catching what they hand the game's goal manager
//! (`GoalManager_AddGoal Name = ... params = {...}`).
//!
//! Each goal has a script per level, `<level>_AddGoal_<Kind>` (the level's
//! name as its scripts shorten it), calling `AddGoal_<Kind> { ... }`, which
//! adds the kind's generic parameters with the level's own over them. So
//! far:
//!
//! - the S-K-A-T-E letters (`AddGoal_Skate`: `time`, the letters'
//!   objects, the restart node);
//! - the High Score and Pro Score goals (`AddGoal_HighScore`,
//!   `AddGoal_ProScore`: the score, named by a global like
//!   `Beach_highscore_score`, and the time).

use qb::vm::{Host, Outcome, Program, Thread};
use qb::{Value, checksum};

/// The S-K-A-T-E letters goal on a level.
#[derive(Clone, Debug, PartialEq)]
pub struct SkateLetters {
    /// Seconds to collect them in.
    pub time: f32,
    /// The letters' objects (node names), S to E.
    pub letters: [u32; 5],
    /// Where the goal starts again (`restart_node`), if it says.
    pub restart: Option<u32>,
}

/// Catches the goal manager's goals (and each one's type, as
/// `GoalManager_CreateGoalName goal_type = ...` names it).
#[derive(Default)]
struct Catch {
    goals: Vec<Value>,
    kinds: Vec<String>,
    kind: String,
}

impl Host for Catch {
    fn command(&mut self, _target: Option<u32>, name: u32, args: &Value) -> Outcome {
        // `GoalManager_AddGoal Name = id { params = {...} }`: the params
        // are in a struct of their own.
        if name == checksum("GoalManager_CreateGoalName") {
            self.kind = match args.get(checksum("goal_type")) {
                Some(Value::String(s) | Value::LocalString(s)) => s.clone(),
                _ => String::new(),
            };
        }
        if name == checksum("GoalManager_AddGoal") {
            let params = args.get(checksum("params")).or_else(|| match args {
                Value::Struct(items) => items
                    .iter()
                    .find_map(|(k, v)| k.is_none().then(|| v.get(checksum("params"))).flatten()),
                _ => None,
            });
            if let Some(params) = params {
                self.goals.push(params.clone());
                self.kinds.push(self.kind.clone());
            }
        }
        Outcome::Done(false)
    }
}

/// A goal's parameters: global structs named bare (the generic ones)
/// first, then the named ones over them.
fn flatten(params: &Value, program: &Program, out: &mut Vec<(u32, Value)>) {
    let Value::Struct(items) = params else {
        return;
    };
    for (key, value) in items {
        match (key, value) {
            (None, Value::Name(n)) => {
                if let Some(global @ Value::Struct(_)) = program.value(*n) {
                    flatten(global, program, out);
                }
            }
            // A script's parameters passed on whole (`<...>` of a call
            // with a struct).
            (None, inner @ Value::Struct(_)) => flatten(inner, program, out),
            (Some(k), v) => {
                out.retain(|(o, _)| o != k);
                out.push((*k, v.clone()));
            }
            _ => {}
        }
    }
}

/// The names a level's goal scripts go by: its archive name, or the
/// short one its scripts use.
fn level_names(level: &str) -> Vec<&str> {
    let short = match level.to_ascii_lowercase().as_str() {
        "priderock" => "pride",
        "tarzan_treehouse" => "treehouse",
        "toystory_bedroom" => "bedroom",
        "zurghome" => "zurg",
        _ => level,
    };
    vec![level, short]
}

/// A goal's parameters as the level adds it (`<level>_AddGoal_<kind>`),
/// or `None` if the level hasn't that script.
fn goal_params(program: &Program, level: &str, kind: &str) -> Option<Vec<(u32, Value)>> {
    let script = level_names(level)
        .iter()
        .map(|l| checksum(&format!("{l}_AddGoal_{kind}")))
        .find(|s| program.has_script(*s))?;
    let mut host = Catch::default();
    let mut thread = Thread::new(script, Vec::new());
    thread.run(program, &mut host, 0.0);
    let mut out = Vec::new();
    flatten(host.goals.first()?, program, &mut out);
    Some(out)
}

/// A parameter, with a bare global's name read as its value.
fn param<'a>(params: &'a [(u32, Value)], program: &'a Program, key: &str) -> Option<&'a Value> {
    let value = params
        .iter()
        .find(|(k, _)| *k == checksum(key))
        .map(|(_, v)| v)?;
    match value {
        Value::Name(n) => program.value(*n).or(Some(value)),
        v => Some(v),
    }
}

/// A character's collectibles on a level (`AddGoal_Collect25`): what the
/// game calls them and their objects (node names), in order (their goal
/// flags `Got_1` to `got_25`).
#[derive(Clone, Debug, PartialEq)]
pub struct Collectibles {
    pub kind: String,
    pub objects: Vec<u32>,
    /// The world's special item (`AddGoal_Super`,
    /// `<first_name>_collect_super_objects`) and what it's called.
    pub special: Option<(u32, String)>,
}

/// The character's collectibles (`<first_name>_collect25_objects`), named
/// as `AddGoal_Collect25` names them for each character.
pub fn collectibles(program: &Program, character: &str) -> Option<Collectibles> {
    let id = character.to_ascii_lowercase();
    let kind = match id.as_str() {
        "woody" => "Badges",
        "buzz" => "PowerCells",
        "jessie" => "Cowgirl Boots",
        "zurg" => "RayGuns",
        "tarzan" => "Spearheads",
        "jane" => "Sketchbooks",
        "terk" => "Bananas",
        "tantor" => "Peanuts",
        "simba" | "nala" => "Haunches",
        "rafiki" => "Spirit Guides",
        "timon" => "Tasty Grubs",
        "kid" => "Medals",
        _ => return None,
    };
    let list = program.value(checksum(&format!("{id}_collect25_objects")))?;
    let Value::Array(items) = list else {
        return None;
    };
    let objects: Vec<u32> = items
        .iter()
        .filter_map(|item| item.get(checksum("id")).and_then(Value::as_name))
        .collect();
    let world = match id.as_str() {
        "woody" | "buzz" | "jessie" | "zurg" => "Toy Story Special",
        "tarzan" | "jane" | "terk" | "tantor" => "Tarzan Special",
        "simba" | "nala" | "rafiki" | "timon" => "Lion King Special",
        _ => "Kid Special",
    };
    let special = match program.value(checksum(&format!("{id}_collect_super_objects"))) {
        Some(Value::Array(items)) => items
            .first()
            .and_then(|item| item.get(checksum("id")))
            .and_then(Value::as_name)
            .map(|object| (object, world.to_string())),
        _ => None,
    };
    (!objects.is_empty()).then(|| Collectibles {
        kind: kind.to_string(),
        objects,
        special,
    })
}

/// One of the level's goals: its type (`Skate`, `HighScore`, `Gaps`...)
/// and what the goal list calls it.
#[derive(Clone, Debug, PartialEq)]
pub struct LevelGoal {
    pub kind: String,
    pub text: String,
}

/// The goals the level adds (`<level>_goals`, as the career has them, or
/// straight from `<level>_Startup` as Pizza Planet does).
pub fn level_goals(program: &Program, level: &str) -> Vec<LevelGoal> {
    let names = level_names(level);
    let Some(script) = ["goals", "Startup"]
        .iter()
        .flat_map(|suffix| {
            names
                .iter()
                .map(move |l| checksum(&format!("{l}_{suffix}")))
        })
        .find(|s| program.has_script(*s))
    else {
        return Vec::new();
    };
    let mut host = Catch::default();
    let mut thread = Thread::new(script, Vec::new());
    // (A start-up script is long: run it through, a slice at a time.)
    for _ in 0..50 {
        if thread.is_finished() {
            break;
        }
        thread.run(program, &mut host, 0.0);
    }
    host.goals
        .iter()
        .zip(&host.kinds)
        .filter_map(|(params, kind)| {
            let mut out = Vec::new();
            flatten(params, program, &mut out);
            let text = ["view_goals_text", "goal_text"].iter().find_map(|key| {
                match param(&out, program, key) {
                    Some(Value::String(s) | Value::LocalString(s)) if !s.is_empty() => {
                        Some(s.clone())
                    }
                    _ => None,
                }
            })?;
            Some(LevelGoal {
                kind: kind.clone(),
                text,
            })
        })
        .collect()
}

/// A race goal (`AddGoal_Race`): its name, the waypoints in order (each
/// one's object, the script run when it's next, and the seconds it adds
/// to the clock), where it starts, and the scripts run at the start and
/// the end (`goal_start_script`, `goal_deactivate_script`).
#[derive(Clone, Debug, PartialEq)]
pub struct Race {
    pub name: String,
    pub waypoints: Vec<(u32, Option<u32>, f32)>,
    pub restart: Option<u32>,
    pub start_script: Option<u32>,
    pub end_script: Option<u32>,
}

/// The level's race goal, if it has one.
pub fn race(program: &Program, level: &str) -> Option<Race> {
    let params = goal_params(program, level, "Race")?;
    let name = |key: &str| param(&params, program, key).and_then(Value::as_name);
    let Some(Value::Array(points)) = param(&params, program, "race_waypoints") else {
        return None;
    };
    let waypoints: Vec<(u32, Option<u32>, f32)> = points
        .iter()
        .filter_map(|p| {
            Some((
                p.get(checksum("id"))?.as_name()?,
                p.get(checksum("scr")).and_then(Value::as_name),
                p.get(checksum("time"))
                    .and_then(Value::as_f32)
                    .unwrap_or(10.0),
            ))
        })
        .collect();
    if waypoints.is_empty() {
        return None;
    }
    Some(Race {
        name: match param(&params, program, "view_goals_text") {
            Some(Value::String(s) | Value::LocalString(s)) => s.clone(),
            _ => "Race".to_string(),
        },
        waypoints,
        restart: name("restart_node"),
        start_script: name("goal_start_script"),
        end_script: name("goal_deactivate_script"),
    })
}

/// The goal's pro (`trigger_obj_id`, like `TRG_G_HS_Pro`): the pedestrian
/// who stands where the goal's offered. `kind` as in `<level>_AddGoal_<kind>`.
pub fn goal_pro(program: &Program, level: &str, kind: &str) -> Option<u32> {
    let params = goal_params(program, level, kind)?;
    param(&params, program, "trigger_obj_id").and_then(Value::as_name)
}

/// A goal to score so many points in the time.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreGoal {
    /// What the game calls it ("High score", "Extreme Score").
    pub name: String,
    pub score: u32,
    pub time: f32,
    pub restart: Option<u32>,
    /// What it says when won (`win_message_text`).
    pub win: String,
}

/// The level's High Score goal (`pro` false) or Pro Score goal.
pub fn score_goal(program: &Program, level: &str, pro: bool) -> Option<ScoreGoal> {
    let params = goal_params(program, level, if pro { "ProScore" } else { "HighScore" })?;
    let name = match param(&params, program, "view_goals_text") {
        Some(Value::String(s) | Value::LocalString(s)) => s.clone(),
        _ if pro => "Pro Score".to_string(),
        _ => "High Score".to_string(),
    };
    Some(ScoreGoal {
        name,
        score: param(&params, program, "score")?.as_int()?.max(0) as u32,
        time: param(&params, program, "time")
            .and_then(Value::as_f32)
            .unwrap_or(120.0),
        restart: param(&params, program, "restart_node").and_then(Value::as_name),
        win: match param(&params, program, "win_message_text") {
            Some(Value::String(s) | Value::LocalString(s)) => s.clone(),
            _ => "Goal complete!".to_string(),
        },
    })
}

/// The level's S-K-A-T-E letters goal, if it has one.
pub fn skate_letters(program: &Program, level: &str) -> Option<SkateLetters> {
    // No script found: the generic settings.
    let out = goal_params(program, level, "SKATE").unwrap_or_else(|| {
        let mut out = Vec::new();
        flatten(
            &Value::Struct(vec![(
                None,
                Value::Name(checksum("Goal_SkateLetters_genericParams")),
            )]),
            program,
            &mut out,
        );
        out
    });
    let get = |key: &str| param(&out, program, key);
    let name = |key: &str| {
        out.iter()
            .find(|(k, _)| *k == checksum(key))
            .and_then(|(_, v)| v.as_name())
    };
    Some(SkateLetters {
        time: get("time").and_then(Value::as_f32).unwrap_or(120.0),
        letters: [
            name("s_obj_id")?,
            name("k_obj_id")?,
            name("a_obj_id")?,
            name("t_obj_id")?,
            name("e_obj_id")?,
        ],
        restart: name("restart_node"),
    })
}
