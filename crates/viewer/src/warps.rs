//! Warps between levels, read from the levels' scripts.
//!
//! - On the Hub, each level's portal is a film strip (`mesh_Warp_*`, a
//!   sector hidden at the start) whose script calls `LevelWarp level_num =
//!   N filmStrip = mesh_...`: within 60 feet it animates into place
//!   (`WarpAppears`), and touching it offers that level, the `N`th of
//!   `level_select_menu_level_info`.
//! - On the other levels, a `Warp_Master_Hub` object (`HubWarp`) is the way
//!   back to the Hub. The game shows it only to the Kid.

use glam::Vec3;
use qb::vm::{Host, Outcome, Program, Thread};
use qb::{Token, Value, checksum};

use crate::nodes::LevelNodes;

/// A way to another level.
#[derive(Clone, Debug, PartialEq)]
pub struct Warp {
    /// Where it stands (mesh space).
    pub position: Vec3,
    /// The level it goes to, as the level table names its loader
    /// (`load_PrideRock`), and its title ("Pride Rock").
    pub level: u32,
    pub title: String,
    /// The portal's sector, if it has one (the Hub's film strips).
    pub sector: Option<u32>,
    /// Its particle effect's emitter (`warpParticle`, or the way back's
    /// `TRG_Warp_Particle_Hub`).
    pub particle: Option<u32>,
    /// The camera path played going through (`warpCam`, or the way back's
    /// `Camera_Warp_Hub`).
    pub camera: Option<u32>,
}

/// Catches the parameters `LevelWarp` hands its exception.
#[derive(Default)]
struct Catch {
    params: Option<Value>,
}

impl Host for Catch {
    fn command(&mut self, _target: Option<u32>, name: u32, args: &Value) -> Outcome {
        if name == checksum("Obj_SetException") {
            if let Some(params) = args.get(checksum("params")) {
                self.params.get_or_insert_with(|| params.clone());
            }
        }
        Outcome::Done(false)
    }
}

/// The level table's entry `index`: its loader and title.
fn level_entry(program: &Program, index: usize) -> Option<(u32, String)> {
    let Some(Value::Array(levels)) = program.value(checksum("level_select_menu_level_info")) else {
        return None;
    };
    let entry = levels.get(index)?;
    let level = entry.get(checksum("level"))?.as_name()?;
    let title = match entry.get(checksum("text")) {
        Some(Value::String(s) | Value::LocalString(s)) => s.clone(),
        _ => String::new(),
    };
    Some((level, title))
}

/// The level's warps.
pub fn warps(program: &Program, nodes: &LevelNodes) -> Vec<Warp> {
    let mut out = Vec::new();
    // The Hub's portals: scripts that call `LevelWarp`.
    let level_warp = checksum("LevelWarp");
    for (name, tokens) in program.scripts() {
        if !tokens.contains(&Token::Name(level_warp)) {
            continue;
        }
        let mut host = Catch::default();
        let mut thread = Thread::new(name, Vec::new());
        thread.run(program, &mut host, 0.0);
        let Some(params) = host.params else { continue };
        let (Some(level_num), Some(strip)) = (
            params.get(checksum("level_num")).and_then(Value::as_int),
            params.get(checksum("filmStrip")).and_then(Value::as_name),
        ) else {
            continue;
        };
        let Some(position) = nodes
            .nodes
            .iter()
            .find(|n| n.name == strip)
            .and_then(|n| n.position)
        else {
            continue;
        };
        let Some((level, title)) = level_entry(program, level_num.max(0) as usize) else {
            continue;
        };
        out.push(Warp {
            position,
            level,
            title,
            sector: Some(strip),
            particle: params
                .get(checksum("warpParticle"))
                .and_then(Value::as_name),
            camera: params.get(checksum("warpCam")).and_then(Value::as_name),
        });
    }
    // The way back to the Hub.
    if let Some(master) = nodes
        .objects
        .iter()
        .find(|o| o.name == checksum("Warp_Master_Hub"))
    {
        let hub = checksum("load_HUB");
        let title = (0..16)
            .filter_map(|i| level_entry(program, i))
            .find(|(level, _)| *level == hub)
            .map_or_else(|| "Hub".to_string(), |(_, title)| title);
        out.push(Warp {
            position: master.position,
            level: hub,
            title,
            sector: None,
            particle: Some(checksum("TRG_Warp_Particle_Hub")),
            camera: Some(checksum("Camera_Warp_Hub")),
        });
    }
    out.sort_by(|a, b| a.title.cmp(&b.title));
    out
}
