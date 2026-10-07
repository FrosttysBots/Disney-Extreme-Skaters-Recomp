//! Goals' settings, read by running a level's goal scripts with the QB
//! interpreter and catching what they hand the game's goal manager
//! (`GoalManager_AddGoal Name = ... params = {...}`).
//!
//! Only the S-K-A-T-E letters so far: `<level>_AddGoal_SKATE` calls
//! `AddGoal_Skate { ... }`, which adds `Goal_SkateLetters_genericParams`
//! with the level's own settings over them (`time`, the letters' objects).

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

/// Catches the goal manager's goals.
#[derive(Default)]
struct Catch {
    goals: Vec<Value>,
}

impl Host for Catch {
    fn command(&mut self, _target: Option<u32>, name: u32, args: &Value) -> Outcome {
        // `GoalManager_AddGoal Name = id { params = {...} }`: the params
        // are in a struct of their own.
        if name == checksum("GoalManager_AddGoal") {
            let params = args.get(checksum("params")).or_else(|| match args {
                Value::Struct(items) => items
                    .iter()
                    .find_map(|(k, v)| k.is_none().then(|| v.get(checksum("params"))).flatten()),
                _ => None,
            });
            if let Some(params) = params {
                self.goals.push(params.clone());
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

/// The level's S-K-A-T-E letters goal, if it has one (`level` is the
/// archive name; Pride Rock's script says `pride`).
pub fn skate_letters(program: &Program, level: &str) -> Option<SkateLetters> {
    let script = [level, "pride"]
        .iter()
        .map(|l| checksum(&format!("{l}_AddGoal_SKATE")))
        .find(|s| program.has_script(*s));
    let mut out = Vec::new();
    match script {
        Some(script) => {
            let mut host = Catch::default();
            let mut thread = Thread::new(script, Vec::new());
            thread.run(program, &mut host, 0.0);
            flatten(host.goals.first()?, program, &mut out);
        }
        // No script found: the generic settings.
        None => flatten(
            &Value::Struct(vec![(
                None,
                Value::Name(checksum("Goal_SkateLetters_genericParams")),
            )]),
            program,
            &mut out,
        ),
    }
    let get = |key: &str| {
        out.iter()
            .find(|(k, _)| *k == checksum(key))
            .map(|(_, v)| v)
    };
    let name = |key: &str| get(key).and_then(Value::as_name);
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
