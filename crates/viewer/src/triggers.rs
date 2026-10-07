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

use std::collections::HashMap;

use qb::checksum;
use qb::token::Token;
use qb::vm::Program;

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
