//! What a level's trigger faces do to the skater, worked out from the
//! scripts without running them.
//!
//! Level geometry with a `TriggerScript` runs it when the skater touches
//! the object's trigger faces. Teleporters (the Hub's harbour water,
//! Beach's tubes, Graveyard's lava, Zurg's restart planes...) all end in a
//! call naming a restart node, like
//! `Teleporter_water node = TRG_WaterStart` or
//! `TeleporterTube node = Restart_TUBE_FL`, a few scripts down. Following
//! the calls and finding that `node =` (or `nodename =`) is enough to send
//! the skater there.
//!
//! Gaps work the same way: a trigger script ends in
//! `StartGap GapID = X flags = [ CANCEL_GROUND ]` or
//! `EndGap GapID = X text = "Chain Link Gap" score = 100`.

use std::collections::HashMap;

use qb::checksum;
use qb::token::Token;
use qb::vm::Program;
use skate::{GapFlags, GapTrigger};

use crate::nodes::LevelNodes;

/// How many scripts deep to follow calls.
const DEPTH: u32 = 6;

/// For each collision object whose trigger script teleports the skater:
/// the index of the restart (in `nodes.spawns`) it sends it to.
pub fn teleports(nodes: &LevelNodes, program: &Program) -> HashMap<u32, usize> {
    let restarts: HashMap<u32, usize> = nodes
        .spawns
        .iter()
        .enumerate()
        .filter(|(_, s)| s.name != 0)
        .map(|(i, s)| (s.name, i))
        .collect();
    nodes
        .geometry_scripts
        .iter()
        .filter_map(|&(object, script)| {
            let target = restart_named(program, script, &restarts, DEPTH)?;
            Some((object, target))
        })
        .collect()
}

/// For each collision object whose trigger script starts or ends a gap:
/// what it does. (`EndGap`s without a name and score belong to goals.)
pub fn gaps(nodes: &LevelNodes, program: &Program) -> HashMap<u32, GapTrigger> {
    nodes
        .geometry_scripts
        .iter()
        .filter_map(|&(object, script)| Some((object, gap_in(program, script, DEPTH)?)))
        .collect()
}

/// The first `StartGap` or `EndGap` a script (or one it calls) makes.
fn gap_in(program: &Program, script: u32, depth: u32) -> Option<GapTrigger> {
    let body = program.script(script)?;
    let (start, end) = (checksum("StartGap"), checksum("EndGap"));
    for (i, token) in body.iter().enumerate() {
        let at_line_start =
            i == 0 || matches!(body[i - 1], Token::EndOfLine | Token::LineNumber(_));
        let Token::Name(name) = token else { continue };
        if !at_line_start {
            continue;
        }
        let line: Vec<&Token> = body[i + 1..]
            .iter()
            .take_while(|t| !matches!(t, Token::EndOfLine | Token::LineNumber(_)))
            .collect();
        if *name == start || *name == end {
            if let Some(gap) = parse_gap(*name == start, &line) {
                return Some(gap);
            }
        } else if depth > 0 && *name != script && program.has_script(*name) {
            if let Some(gap) = gap_in(program, *name, depth - 1) {
                return Some(gap);
            }
        }
    }
    None
}

/// A `StartGap` or `EndGap` call's arguments.
fn parse_gap(start: bool, args: &[&Token]) -> Option<GapTrigger> {
    let key = |k: &str| checksum(k);
    let mut id = None;
    let mut flags = GapFlags::default();
    let mut text = None;
    let mut score = None;
    let mut i = 0;
    while i + 2 < args.len() + 1 {
        let (Some(Token::Name(k)), Some(Token::Equals)) = (args.get(i), args.get(i + 1)) else {
            i += 1;
            continue;
        };
        let value = args.get(i + 2);
        if *k == key("GapID") {
            if let Some(Token::Name(v)) = value {
                id = Some(*v);
            }
        } else if *k == key("flags") {
            // One flag, or a list of them.
            let names: Vec<u32> = match value {
                Some(Token::Name(v)) => vec![*v],
                Some(Token::StartArray) => args[i + 3..]
                    .iter()
                    .take_while(|t| !matches!(t, Token::EndArray))
                    .filter_map(|t| match t {
                        Token::Name(v) => Some(*v),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            for flag in names {
                if flag == key("CANCEL_GROUND") {
                    flags.cancel_ground = true;
                } else if flag == key("CANCEL_AIR") {
                    flags.cancel_air = true;
                } else if flag == key("PURE_AIR") {
                    flags.pure_air = true;
                } else if flag == key("REQUIRE_RAIL") {
                    flags.require_rail = true;
                } else if flag == key("REQUIRE_LIP") {
                    flags.require_lip = true;
                }
            }
        } else if *k == key("text") {
            if let Some(Token::String(s)) = value {
                text = Some(s.clone());
            }
        } else if *k == key("score") {
            score = match value {
                Some(Token::Integer(n)) => u32::try_from(*n).ok(),
                Some(Token::Float(f)) => Some(*f as u32),
                _ => None,
            };
        }
        i += 3;
    }
    let id = id?;
    if start {
        Some(GapTrigger::Start { id, flags })
    } else {
        Some(GapTrigger::End {
            id,
            text: text?,
            score: score?,
        })
    }
}

/// The first restart a script (or one it calls) names as `node` or
/// `nodename`.
fn restart_named(
    program: &Program,
    script: u32,
    restarts: &HashMap<u32, usize>,
    depth: u32,
) -> Option<usize> {
    let body = program.script(script)?;
    let keys = [checksum("node"), checksum("nodename")];
    for window in body.windows(3) {
        if let [Token::Name(key), Token::Equals, Token::Name(value)] = window {
            if keys.contains(key) {
                if let Some(&index) = restarts.get(value) {
                    return Some(index);
                }
            }
        }
    }
    if depth == 0 {
        return None;
    }
    // The scripts it calls: a name starting a line.
    let mut line_start = true;
    for token in body {
        match token {
            Token::EndOfLine | Token::LineNumber(_) => line_start = true,
            Token::Name(name) if line_start => {
                line_start = false;
                if *name != script && program.has_script(*name) {
                    if let Some(found) = restart_named(program, *name, restarts, depth - 1) {
                        return Some(found);
                    }
                }
            }
            Token::If | Token::Else | Token::ElseIf | Token::Not => {}
            _ => line_start = false,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nodes::Spawn;
    use glam::Vec3;

    fn line(tokens: &[Token]) -> Vec<Token> {
        let mut out = tokens.to_vec();
        out.push(Token::EndOfLine);
        out
    }

    #[test]
    fn reads_gap_starts_and_ends() {
        let n = |s: &str| Token::Name(checksum(s));
        let mut program = Program::new();
        program.add_script(
            checksum("StartScript"),
            line(&[
                n("StartGap"),
                n("GapID"),
                Token::Equals,
                n("ChainLinkGap"),
                n("flags"),
                Token::Equals,
                Token::StartArray,
                n("CANCEL_GROUND"),
                Token::EndArray,
            ]),
        );
        program.add_script(
            checksum("EndScript"),
            line(&[
                n("EndGap"),
                n("GapID"),
                Token::Equals,
                n("ChainLinkGap"),
                n("text"),
                Token::Equals,
                Token::String("Chain Link Gap".into()),
                n("score"),
                Token::Equals,
                Token::Integer(100),
            ]),
        );
        let nodes = LevelNodes {
            geometry_scripts: vec![(1, checksum("StartScript")), (2, checksum("EndScript"))],
            ..LevelNodes::default()
        };
        let found = gaps(&nodes, &program);
        assert_eq!(
            found.get(&1),
            Some(&GapTrigger::Start {
                id: checksum("ChainLinkGap"),
                flags: GapFlags {
                    cancel_ground: true,
                    ..GapFlags::default()
                }
            })
        );
        assert_eq!(
            found.get(&2),
            Some(&GapTrigger::End {
                id: checksum("ChainLinkGap"),
                text: "Chain Link Gap".into(),
                score: 100
            })
        );
    }

    #[test]
    fn follows_calls_to_the_restart_a_teleporter_names() {
        let mut program = Program::new();
        // Object03c01Script -> Object03c_DOIT -> Teleporter_water node = TRG_WaterStart
        program.add_script(
            checksum("Object03c01Script"),
            line(&[Token::Name(checksum("Object03c_DOIT"))]),
        );
        program.add_script(
            checksum("Object03c_DOIT"),
            line(&[
                Token::Name(checksum("Teleporter_water")),
                Token::Name(checksum("node")),
                Token::Equals,
                Token::Name(checksum("TRG_WaterStart")),
            ]),
        );
        program.add_script(
            checksum("BoxScript"),
            line(&[Token::Name(checksum("break_box"))]),
        );
        let nodes = LevelNodes {
            spawns: vec![Spawn {
                label: "P1: Restart".into(),
                position: Vec3::ZERO,
                heading: 0.0,
                kind: "Player1".into(),
                name: checksum("TRG_WaterStart"),
            }],
            geometry_scripts: vec![
                (checksum("Object03c01"), checksum("Object03c01Script")),
                (checksum("Box01"), checksum("BoxScript")),
            ],
            ..LevelNodes::default()
        };
        let found = teleports(&nodes, &program);
        assert_eq!(found.get(&checksum("Object03c01")), Some(&0));
        assert_eq!(found.len(), 1, "breaking a box teleports nowhere");
    }
}
