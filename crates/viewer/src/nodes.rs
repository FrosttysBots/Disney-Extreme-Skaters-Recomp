//! Rails, spawn points and objects from a level's `NodeArray` (in
//! `<level>.qb`).
//!
//! **Coordinates.** Node positions use the opposite Z direction from the
//! level meshes: mirroring Z puts 96% of the hub's rail nodes within 60
//! units of a collision surface, against 0% as stored. Everything here is
//! converted to mesh space.
//!
//! Rail nodes (`Class = RailNode`) list the nodes they connect to in
//! `Links` (indices into the array); each link is one straight rail
//! segment. Spawn points are `Class = Restart` nodes with a position, a
//! heading (`Angles` y, in radians) and usually a `RestartName`.
//!
//! `GameObject`, `Vehicle` and `Pedestrian` nodes place a `Model` (and
//! pedestrians a `SkeletonName`). `LevelGeometry` and `LevelObject` nodes
//! name sectors of the level scene. Nodes without the `CreatedAtStart` flag
//! aren't there when the level starts: goal pickups, warp portals that
//! open later, invisible trigger boxes and most pedestrians, which goals
//! create.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use glam::{Quat, Vec3};
use qb::{Definition, Symbols, Value, checksum, parse_definitions, tokenize};

use crate::camera::FlyCamera;
use crate::collision::ColorVertex;

pub struct RailSegment {
    pub start: Vec3,
    pub end: Vec3,
    /// The rail's `Type` (Metal, Wood, Concrete...), as a name.
    pub kind: String,
    /// Its `TerrainType` (`TERRAIN_METALSMOOTH`...), as a name's checksum.
    pub terrain: Option<u32>,
}

pub struct Spawn {
    /// `RestartName` if present, otherwise the node's `Name`.
    pub label: String,
    pub position: Vec3,
    /// Heading around the vertical axis, in radians, as stored in `Angles`.
    pub heading: f32,
    /// The restart `Type` (Player1, Multiplayer, Generic...).
    pub kind: String,
    /// The node's `Name` checksum.
    pub name: u32,
}

impl Spawn {
    /// The direction a skater placed here faces, in mesh space. In script
    /// space heading 0 faces +Z (the engine's forward axis) and positive
    /// headings turn toward +X; mirroring Z for mesh space flips the Z
    /// component. This follows the engine's conventions and hasn't been
    /// checked against the running game yet.
    pub fn facing(&self) -> Vec3 {
        let forward = Quat::from_rotation_y(self.heading) * Vec3::Z;
        Vec3::new(forward.x, forward.y, -forward.z)
    }

    /// A camera just behind and above the spawn, looking the way it faces.
    pub fn camera(&self) -> FlyCamera {
        self.camera_clear_of(&[])
    }

    /// Like [`camera`](Self::camera), but pulled in toward the spawn if a
    /// collision triangle (three vertices each) is in the way, so tight
    /// rooms don't put the camera behind a wall.
    pub fn camera_clear_of(&self, triangles: &[ColorVertex]) -> FlyCamera {
        let forward = self.facing();
        let eye_from = self.position + Vec3::Y * 100.0;
        let wanted = self.position + Vec3::Y * 140.0 - forward * 260.0;
        let offset = wanted - eye_from;
        let length = offset.length();
        let dir = offset / length;
        let hit = triangles
            .chunks_exact(3)
            .filter_map(|t| ray_triangle(eye_from, dir, t.iter().map(|v| Vec3::from(v.position))))
            .filter(|&d| d < length)
            .fold(length, f32::min);
        // Stay a little in front of whatever was hit.
        let distance = if hit < length {
            (hit - 25.0).max(0.0)
        } else {
            length
        };
        let eye = eye_from + dir * distance;
        FlyCamera::looking_at(eye, self.position + Vec3::Y * 60.0 + forward * 400.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    GameObject,
    Vehicle,
    Pedestrian,
}

/// A node that places a model.
#[derive(Clone, Debug)]
pub struct ObjectNode {
    pub label: String,
    pub kind: ObjectKind,
    /// In mesh space.
    pub position: Vec3,
    /// `Angles` y as stored (none of these nodes is tilted on the disc).
    pub heading: f32,
    /// Whether it's there when the level starts (see the module docs).
    pub created_at_start: bool,
    /// The model's path as in the node array, lowercase with `/`
    /// (`gameobjects/milk/milk.mdl`).
    pub model: String,
    /// For pedestrians, the skeleton's name, lowercase.
    pub skeleton: Option<String>,
    /// For pedestrians, the script that loads their animations (`AnimName`).
    pub animations: Option<u32>,
    /// The node's name and its index in the node array (paths start here).
    pub name: u32,
    pub node: usize,
    /// The script the object runs when it's created (`TriggerScript`).
    pub script: Option<u32>,
}

/// Any node's place and links, for following paths (`Links` are indices
/// into the node array).
#[derive(Clone, Debug, Default)]
pub struct PathNode {
    pub name: u32,
    /// In mesh space; `None` for nodes without a position.
    pub position: Option<Vec3>,
    pub links: Vec<usize>,
}

impl ObjectNode {
    /// The model's orientation in mesh space: mirroring Z turns a turn of
    /// `heading` about Y into one of `-heading`. This is the opposite way
    /// round from [`Spawn::facing`], and neither has been checked against
    /// the game: goal intro cameras don't settle it (the 40 that frame
    /// their goal's pedestrian see it from the front 19 times this way and
    /// 21 times the other way), and neither do walls in front of spawns.
    pub fn rotation(&self) -> Quat {
        Quat::from_rotation_y(-self.heading)
    }
}

#[derive(Default)]
pub struct LevelNodes {
    pub rails: Vec<RailSegment>,
    pub spawns: Vec<Spawn>,
    pub objects: Vec<ObjectNode>,
    /// Name checksums of scene sectors that aren't there at the start.
    pub hidden_sectors: HashSet<u32>,
    /// Every node in the array, in order.
    pub nodes: Vec<PathNode>,
    /// Level geometry that runs a script when the skater touches its
    /// trigger faces: the node's name (its collision object's checksum)
    /// and the `TriggerScript`.
    pub geometry_scripts: Vec<(u32, u32)>,
    /// Particle emitters (`ParticleEmitter`): where, their `TriggerScript`
    /// (which makes the particle system), whether there at the start, and
    /// how far off they're drawn (`CutOff`).
    pub emitters: Vec<Emitter>,
}

/// A `ParticleEmitter` node.
#[derive(Clone, Debug)]
pub struct Emitter {
    pub position: Vec3,
    pub script: u32,
    pub created_at_start: bool,
    pub cutoff: f32,
}

impl LevelNodes {
    pub fn load(path: &Path) -> Result<Self> {
        let data = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
        Self::from_bytes(&data).with_context(|| format!("could not load {}", path.display()))
    }

    /// Reads the `NodeArray` from the contents of a level's `.qb`.
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let tokens = tokenize(data).context("could not read the script")?;
        let mut symbols = Symbols::new();
        symbols.add_tokens(&tokens);
        let defs = parse_definitions(&tokens).context("could not parse the script")?;
        let Some(nodes) = defs.iter().find_map(|d| match d {
            Definition::Value { name, value } if *name == checksum("NodeArray") => value.as_array(),
            _ => None,
        }) else {
            bail!("the script has no NodeArray");
        };

        let key = |name: &str| checksum(name);
        let class_of = |node: &Value| node.get(key("Class")).and_then(Value::as_name);
        // Script space to mesh space: mirror Z (see the module docs).
        let position = |node: &Value| {
            node.get(key("Position"))
                .or_else(|| node.get(key("Pos")))
                .and_then(Value::as_vector)
                .map(|[x, y, z]| [x, y, -z])
        };
        let name_of = |node: &Value, field: &str| {
            node.get(key(field))
                .and_then(Value::as_name)
                .map(|n| symbols.name(n))
        };

        let mut out = LevelNodes {
            nodes: nodes
                .iter()
                .map(|node| PathNode {
                    name: node.get(key("Name")).and_then(Value::as_name).unwrap_or(0),
                    position: position(node).map(Vec3::from),
                    links: node
                        .get(key("Links"))
                        .and_then(Value::as_array)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|l| l.as_int())
                        .filter_map(|l| usize::try_from(l).ok())
                        .filter(|&l| l < nodes.len())
                        .collect(),
                })
                .collect(),
            ..LevelNodes::default()
        };
        for (index, node) in nodes.iter().enumerate() {
            let Some(pos) = position(node).map(Vec3::from) else {
                continue;
            };
            match class_of(node) {
                Some(c) if c == key("RailNode") => {
                    let kind = name_of(node, "Type").unwrap_or_default();
                    let links = node
                        .get(key("Links"))
                        .and_then(Value::as_array)
                        .unwrap_or_default();
                    for link in links {
                        let Some(target) = link.as_int().and_then(|i| nodes.get(i as usize)) else {
                            continue;
                        };
                        if class_of(target) != Some(key("RailNode")) {
                            continue;
                        }
                        if let Some(end) = position(target) {
                            out.rails.push(RailSegment {
                                start: pos,
                                end: end.into(),
                                kind: kind.clone(),
                                terrain: node.get(key("TerrainType")).and_then(Value::as_name),
                            });
                        }
                    }
                }
                Some(c) if c == key("LevelGeometry") || c == key("LevelObject") => {
                    if let (Some(name), Some(script)) = (
                        node.get(key("Name")).and_then(Value::as_name),
                        node.get(key("TriggerScript")).and_then(Value::as_name),
                    ) {
                        out.geometry_scripts.push((name, script));
                    }
                    if !node.has_flag(key("CreatedAtStart")) {
                        if let Some(name) = node.get(key("Name")).and_then(Value::as_name) {
                            out.hidden_sectors.insert(name);
                        }
                    }
                }
                Some(c) if [key("GameObject"), key("Vehicle"), key("Pedestrian")].contains(&c) => {
                    let Some(Value::String(model)) = node.get(key("Model")) else {
                        continue;
                    };
                    let kind = if c == key("GameObject") {
                        ObjectKind::GameObject
                    } else if c == key("Vehicle") {
                        ObjectKind::Vehicle
                    } else {
                        ObjectKind::Pedestrian
                    };
                    out.objects.push(ObjectNode {
                        label: name_of(node, "Name").unwrap_or_default(),
                        kind,
                        position: pos,
                        heading: node
                            .get(key("Angles"))
                            .and_then(Value::as_vector)
                            .map_or(0.0, |a| a[1]),
                        created_at_start: node.has_flag(key("CreatedAtStart")),
                        model: model.replace('\\', "/").to_ascii_lowercase(),
                        skeleton: name_of(node, "SkeletonName").map(|s| s.to_ascii_lowercase()),
                        animations: node.get(key("AnimName")).and_then(Value::as_name),
                        name: node.get(key("Name")).and_then(Value::as_name).unwrap_or(0),
                        node: index,
                        script: node.get(key("TriggerScript")).and_then(Value::as_name),
                    });
                }
                Some(c) if c == key("ParticleEmitter") => {
                    if let Some(script) = node.get(key("TriggerScript")).and_then(Value::as_name) {
                        out.emitters.push(Emitter {
                            position: pos,
                            script,
                            created_at_start: node.has_flag(key("CreatedAtStart")),
                            cutoff: node
                                .get(key("CutOff"))
                                .and_then(Value::as_f32)
                                .unwrap_or(500.0),
                        });
                    }
                }
                Some(c) if c == key("Restart") => {
                    let heading = node
                        .get(key("Angles"))
                        .and_then(Value::as_vector)
                        .map_or(0.0, |a| a[1]);
                    let label = match node.get(key("RestartName")) {
                        Some(Value::String(s)) => s.clone(),
                        _ => name_of(node, "Name").unwrap_or_else(|| "Restart".into()),
                    };
                    let kind = name_of(node, "Type").unwrap_or_default();
                    out.spawns.push(Spawn {
                        label,
                        position: pos,
                        heading,
                        kind,
                        name: node.get(key("Name")).and_then(Value::as_name).unwrap_or(0),
                    });
                }
                _ => {}
            }
        }
        Ok(out)
    }

    /// The spawn a level starts at: the first `Player1` restart, otherwise
    /// the first restart.
    pub fn start(&self) -> Option<&Spawn> {
        self.spawns
            .iter()
            .find(|s| s.kind.eq_ignore_ascii_case("player1"))
            .or_else(|| self.spawns.first())
    }

    /// Rails as square tubes and spawn points as posts with a direction
    /// arrow, as colored triangles.
    pub fn geometry(&self, show_rails: bool, show_spawns: bool) -> Vec<ColorVertex> {
        let mut out = Vec::new();
        if show_rails {
            for rail in &self.rails {
                let color = match rail.kind.to_ascii_lowercase().as_str() {
                    "wood" => [190, 120, 60, 255],
                    "concrete" => [200, 200, 210, 255],
                    _ => [255, 40, 220, 255],
                };
                tube(&mut out, rail.start, rail.end, 2.5, color);
            }
        }
        if show_spawns {
            for spawn in &self.spawns {
                let color = if spawn.kind.eq_ignore_ascii_case("player1") {
                    [40, 230, 90, 255]
                } else {
                    [40, 200, 255, 255]
                };
                let base = spawn.position;
                tube(&mut out, base, base + Vec3::Y * 90.0, 3.0, color);
                // A flat arrow on the ground pointing the way the skater faces.
                let forward = spawn.facing();
                let side = forward.cross(Vec3::Y).normalize_or_zero();
                let lift = Vec3::Y * 2.0;
                let tip = base + forward * 60.0 + lift;
                let left = base - side * 22.0 + lift;
                let right = base + side * 22.0 + lift;
                for corner in [left, right, tip] {
                    out.push(ColorVertex {
                        position: corner.into(),
                        color,
                    });
                }
            }
        }
        out
    }
}

/// Distance along the ray to the triangle, if it's hit (Moller-Trumbore).
fn ray_triangle(origin: Vec3, dir: Vec3, mut corners: impl Iterator<Item = Vec3>) -> Option<f32> {
    let (a, b, c) = (corners.next()?, corners.next()?, corners.next()?);
    let (e1, e2) = (b - a, c - a);
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-6 {
        return None;
    }
    let inv = 1.0 / det;
    let s = origin - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 0.0).then_some(t)
}

/// Appends a square tube from `a` to `b` with half-width `radius`.
fn tube(out: &mut Vec<ColorVertex>, a: Vec3, b: Vec3, radius: f32, color: [u8; 4]) {
    let dir = (b - a).normalize_or_zero();
    if dir == Vec3::ZERO {
        return;
    }
    let helper = if dir.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
    let u = dir.cross(helper).normalize() * radius;
    let v = dir.cross(u).normalize() * radius;
    let corners = [u + v, u - v, -u - v, -u + v];
    for side in 0..4 {
        let (c0, c1) = (corners[side], corners[(side + 1) % 4]);
        // Shade each side differently so the tube reads as 3D.
        let shade = [1.0, 0.8, 0.6, 0.85][side];
        let color = [
            (f32::from(color[0]) * shade) as u8,
            (f32::from(color[1]) * shade) as u8,
            (f32::from(color[2]) * shade) as u8,
            color[3],
        ];
        for p in [a + c0, a + c1, b + c1, a + c0, b + c1, b + c0] {
            out.push(ColorVertex {
                position: p.into(),
                color,
            });
        }
    }
}

/// `<root>/<X>Scn/Levels/<name>/<name>.scn.ngc` has its nodes in
/// `<root>/<X>/levels/<name>/<name>.qb`.
pub fn find(scene_path: &Path) -> Option<std::path::PathBuf> {
    use crate::level::find_ignoring_case;
    let file = scene_path.file_name()?.to_str()?;
    let stem = &file[..file.to_ascii_lowercase().strip_suffix(".scn.ngc")?.len()];
    let level_dir = scene_path.parent()?;
    let scn_dir = level_dir.parent()?.parent()?;
    let scn_name = scn_dir.file_name()?.to_str()?;
    let base = &scn_name[..scn_name.to_ascii_lowercase().strip_suffix("scn")?.len()];
    let dir = find_ignoring_case(scn_dir.parent()?, base)?;
    let levels = find_ignoring_case(&dir, "levels")?;
    let level = find_ignoring_case(&levels, level_dir.file_name()?.to_str()?)?;
    find_ignoring_case(&level, &format!("{stem}.qb"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_stops_in_front_of_walls() {
        // A spawn at the origin facing -Z: the camera wants to sit 260 units
        // behind it (+Z). A wall at z = 100 should stop it short.
        let spawn = Spawn {
            label: String::new(),
            position: Vec3::ZERO,
            heading: 0.0,
            kind: String::new(),
            name: 0,
        };
        let free = spawn.camera();
        assert!(free.position.z > 250.0);
        let wall: Vec<ColorVertex> = [
            [-500.0, -500.0, 100.0],
            [500.0, -500.0, 100.0],
            [0.0, 1000.0, 100.0],
        ]
        .into_iter()
        .map(|p| ColorVertex {
            position: p,
            color: [0; 4],
        })
        .collect();
        let blocked = spawn.camera_clear_of(&wall);
        assert!(
            blocked.position.z < 100.0 && blocked.position.z > 50.0,
            "{:?}",
            blocked.position
        );
    }

    #[test]
    fn tube_makes_four_sides() {
        let mut out = Vec::new();
        tube(&mut out, Vec3::ZERO, Vec3::X * 10.0, 1.0, [255; 4]);
        assert_eq!(out.len(), 24);
        tube(&mut out, Vec3::ONE, Vec3::ONE, 1.0, [255; 4]);
        assert_eq!(out.len(), 24, "zero-length tubes are skipped");
    }

    #[test]
    fn headings_are_mirrored_into_mesh_space() {
        let spawn = |heading| Spawn {
            label: String::new(),
            position: Vec3::ZERO,
            heading,
            kind: String::new(),
            name: 0,
        };
        // Script +Z is mesh -Z; a quarter turn still faces +X.
        assert!(spawn(0.0).facing().abs_diff_eq(Vec3::NEG_Z, 1e-6));
        assert!(
            spawn(std::f32::consts::FRAC_PI_2)
                .facing()
                .abs_diff_eq(Vec3::X, 1e-6)
        );
    }
}
