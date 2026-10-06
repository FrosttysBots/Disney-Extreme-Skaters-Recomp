//! The models and pedestrians a level's node array places.
//!
//! Model objects (goal pickups, letters, vehicles) are copied into one
//! static mesh. Pedestrians are skinned characters, each playing the idle
//! from its animation set (see `source`); they're posed on the CPU every
//! frame like the player's character. Both are split by whether the object
//! is there when the level starts (see `nodes`).

use std::collections::HashMap;

use glam::{Mat4, Vec3};

use crate::character::Character;
use crate::level::{Level, Vertex};
use crate::nodes::{LevelNodes, ObjectKind, ObjectNode};
use crate::source::{CharacterFiles, LevelFiles};

/// The roles a pedestrian plays, in order of preference. Every set on the
/// disc has one of these two.
const IDLE_ROLES: [&str; 2] = ["Ped_Guide_Idle1", "Ped_M_Idle1"];

/// Animation sets by script checksum: (path, role) for each animation.
pub type AnimationSets = HashMap<u32, Vec<Load>>;

/// One animation in a set: (path, role).
type Load = (String, String);

/// Pedestrians: one posable character per model, and where each stands.
pub struct Crowd {
    kinds: Vec<Character>,
    members: Vec<Member>,
    /// Every member's mesh, in member order, in the rest pose.
    pub mesh: Level,
}

struct Member {
    kind: usize,
    placement: Mat4,
    animation: usize,
    /// Spreads members out in time so the same kind doesn't move in step.
    offset: f32,
}

impl Crowd {
    fn new() -> Self {
        Crowd {
            kinds: Vec::new(),
            members: Vec::new(),
            mesh: Level::empty(),
        }
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Every member's vertices at `seconds`, in the order of `mesh`.
    pub fn pose(&self, seconds: f32) -> Vec<Vertex> {
        let mut out = Vec::with_capacity(self.mesh.vertices.len());
        for m in &self.members {
            let character = &self.kinds[m.kind];
            let duration = character.animations[m.animation].1.duration.max(1e-3);
            let t = (seconds + m.offset) % duration;
            out.extend(character.pose(m.animation, t, m.placement));
        }
        out
    }
}

pub struct LevelObjects {
    /// Model objects there at the start, and the ones goals create.
    pub props: Level,
    pub goal_props: Level,
    /// Pedestrians, split the same way.
    pub crowd: Crowd,
    pub goal_crowd: Crowd,
    /// Models the node array names that couldn't be loaded, with why.
    pub missing: Vec<String>,
}

/// The animations the level's pedestrians need (their rest pose and idle),
/// to fetch any that aren't in the level's archive.
pub fn needed_animations(nodes: &LevelNodes, sets: &AnimationSets) -> Vec<String> {
    nodes
        .objects
        .iter()
        .filter_map(|n| sets.get(&n.animations?))
        .flat_map(|set| {
            let (rest, idle) = pick(set);
            [rest, idle]
                .into_iter()
                .flatten()
                .map(|(path, _)| path.clone())
        })
        .collect()
}

/// A set's rest pose (`default`) and the idle to play, as (path, role).
fn pick(set: &[(String, String)]) -> (Option<&Load>, Option<&Load>) {
    let role = |name: &str| set.iter().find(|(_, r)| r.eq_ignore_ascii_case(name));
    let idle = IDLE_ROLES
        .iter()
        .find_map(|r| role(r))
        .or_else(|| set.iter().find(|(_, r)| !r.eq_ignore_ascii_case("default")));
    (role("default"), idle)
}

impl LevelObjects {
    pub fn build(nodes: &LevelNodes, files: &LevelFiles, sets: &AnimationSets) -> Self {
        let mut out = LevelObjects {
            props: Level::empty(),
            goal_props: Level::empty(),
            crowd: Crowd::new(),
            goal_crowd: Crowd::new(),
            missing: Vec::new(),
        };
        // Loaded models, and each one's texture slots in each target.
        let mut models: HashMap<&str, Option<Level>> = HashMap::new();
        let mut slots: HashMap<(&str, bool), usize> = HashMap::new();
        let mut kinds: HashMap<&str, Option<usize>> = HashMap::new();
        let mut goal_kinds: HashMap<&str, Option<usize>> = HashMap::new();

        for (i, node) in nodes.objects.iter().enumerate() {
            // Some nodes say "none" or name a model without a path.
            if !node.model.contains('/') {
                continue;
            }
            let placement = Mat4::from_rotation_translation(node.rotation(), node.position);
            if node.kind == ObjectKind::Pedestrian {
                let (crowd, kinds) = if node.created_at_start {
                    (&mut out.crowd, &mut kinds)
                } else {
                    (&mut out.goal_crowd, &mut goal_kinds)
                };
                let kind = *kinds.entry(&node.model).or_insert_with(|| {
                    match pedestrian(node, files, sets) {
                        Ok(character) => {
                            crowd.kinds.push(character);
                            Some(crowd.kinds.len() - 1)
                        }
                        Err(why) => {
                            out.missing.push(format!("{} ({why})", node.model));
                            None
                        }
                    }
                });
                let Some(kind) = kind else { continue };
                let character = &crowd.kinds[kind];
                // The idle if it loaded, else the rest pose.
                let animation = character
                    .animations
                    .iter()
                    .position(|(n, _)| !n.eq_ignore_ascii_case("default"))
                    .unwrap_or(0);
                crowd
                    .mesh
                    .add_instance(&character.mesh, Mat4::IDENTITY, None);
                crowd.members.push(Member {
                    kind,
                    placement,
                    animation,
                    offset: i as f32 * 0.37,
                });
                continue;
            }

            let model =
                models
                    .entry(&node.model)
                    .or_insert_with(|| match model(&node.model, files) {
                        Ok(level) => Some(level),
                        Err(why) => {
                            out.missing.push(format!("{} ({why})", node.model));
                            None
                        }
                    });
            let Some(model) = model else { continue };
            let target = if node.created_at_start {
                &mut out.props
            } else {
                &mut out.goal_props
            };
            let key = (node.model.as_str(), node.created_at_start);
            let base = target.add_instance(model, placement, slots.get(&key).copied());
            slots.insert(key, base);
        }
        out.missing.sort();
        out.missing.dedup();
        out
    }
}

/// A static model and its textures (`<path>.tex` beside it).
fn model(path: &str, files: &LevelFiles) -> Result<Level, String> {
    let data = files
        .models
        .get(path)
        .ok_or("not in the level's archives")?;
    let textures = texture_path(path).and_then(|t| files.models.get(&t));
    Level::from_bytes(data, textures.map(Vec::as_slice)).map_err(|e| format!("{e:#}"))
}

fn texture_path(model: &str) -> Option<String> {
    let stem = model
        .strip_suffix(".mdl")
        .or_else(|| model.strip_suffix(".skin"))?;
    Some(format!("{stem}.tex"))
}

/// A pedestrian's skinned model, skeleton, rest pose and idle.
fn pedestrian(
    node: &ObjectNode,
    files: &LevelFiles,
    sets: &AnimationSets,
) -> Result<Character, String> {
    let skin = files
        .models
        .get(&node.model)
        .ok_or("not in the level's archives")?;
    let skeleton_name = node.skeleton.as_deref().ok_or("no SkeletonName")?;
    let skeleton = files
        .skeletons
        .get(skeleton_name)
        .ok_or_else(|| format!("no skeleton {skeleton_name}"))?;
    let key_tables = files.key_tables.clone().ok_or("no animation key tables")?;
    let set = node
        .animations
        .and_then(|a| sets.get(&a))
        .ok_or("no animation set")?;
    let (rest, idle) = pick(set);
    let mut animations = Vec::new();
    for (path, role) in [rest, idle].into_iter().flatten() {
        let data = files
            .animations
            .get(path)
            .ok_or_else(|| format!("no animation {path}"))?;
        animations.push((role.clone(), data.clone()));
    }
    let files = CharacterFiles {
        skin: skin.clone(),
        textures: texture_path(&node.model).and_then(|t| files.models.get(&t).cloned()),
        board: None,
        skeleton: skeleton.clone(),
        key_tables,
        animations,
    };
    Character::from_files(&files).map_err(|e| format!("{e:#}"))
}

/// Where to look at an object from: in front of it and a little above.
pub fn view_of(node: &ObjectNode) -> (Vec3, Vec3) {
    let forward = node.rotation() * Vec3::Z;
    let target = node.position + Vec3::Y * 40.0;
    (target + forward * 250.0 + Vec3::Y * 80.0, target)
}
