//! The models and pedestrians a level's node array places.
//!
//! Model objects (goal pickups, letters, vehicles) are copied into one mesh
//! per layer; a copy that moves is re-placed by rewriting its vertices.
//! Pedestrians are skinned characters with every animation from their set
//! (see `source`), posed on the CPU every frame like the player's
//! character. Both are split by whether the object is there when the level
//! starts (see `nodes`). Objects' scripts move them and pick their
//! animations (see `behaviour`).

use std::collections::HashMap;

use glam::{Mat4, Vec3};

use crate::character::Character;
use crate::level::{Level, Vertex};
use crate::nodes::{LevelNodes, ObjectKind, ObjectNode};
use crate::source::{CharacterFiles, LevelFiles};

/// The roles a pedestrian plays until its script says otherwise, in order
/// of preference. Every set on the disc has one of these two.
const IDLE_ROLES: [&str; 2] = ["Ped_Guide_Idle1", "Ped_M_Idle1"];

/// Animation sets by script checksum: (path, role) for each animation.
pub type AnimationSets = HashMap<u32, Vec<Load>>;

/// One animation in a set: (path, role).
type Load = (String, String);

/// Pedestrians: one posable character per model, and where each stands.
pub struct Crowd {
    kinds: Vec<Kind>,
    members: Vec<Member>,
    /// Every member's mesh, in member order, in the rest pose.
    pub mesh: Level,
}

struct Kind {
    character: Character,
    /// Animation index by role checksum.
    roles: HashMap<u32, usize>,
    /// How far it reaches out from its feet (most of its vertices: not a
    /// tail or an outstretched arm) and how tall it is.
    radius: f32,
    height: f32,
    /// Something the skater bumps into: not a bird or a bat, which fly
    /// off instead.
    solid: bool,
}

struct Member {
    kind: usize,
    placement: Mat4,
    animation: usize,
    /// When the animation started, and whether it loops.
    started: f32,
    cycle: bool,
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

    /// Where each solid member stands now (not those gone), how far it
    /// reaches and how tall it is.
    pub fn footprints(&self) -> impl Iterator<Item = (Vec3, f32, f32)> + '_ {
        self.members.iter().filter_map(|m| {
            let (scale, _, position) = m.placement.to_scale_rotation_translation();
            if scale.x < 0.01 {
                return None;
            }
            let kind = &self.kinds[m.kind];
            if !kind.solid {
                return None;
            }
            Some((position, kind.radius * scale.x, kind.height * scale.y))
        })
    }

    pub fn set_placement(&mut self, member: usize, placement: Mat4) {
        if let Some(m) = self.members.get_mut(member) {
            m.placement = placement;
        }
    }

    /// Plays the animation with role `role` (a checksum) from `now`;
    /// returns its length, or `None` if the member hasn't got it.
    pub fn play(&mut self, member: usize, role: u32, cycle: bool, now: f32) -> Option<f32> {
        let m = self.members.get_mut(member)?;
        let kind = &self.kinds[m.kind];
        let animation = *kind.roles.get(&role)?;
        m.animation = animation;
        m.started = now;
        m.cycle = cycle;
        Some(kind.character.animations[animation].1.duration)
    }

    /// Seconds left in a member's current animation (0 if it loops).
    pub fn remaining(&self, member: usize, now: f32) -> f32 {
        let Some(m) = self.members.get(member) else {
            return 0.0;
        };
        if m.cycle {
            return 0.0;
        }
        let duration = self.kinds[m.kind].character.animations[m.animation]
            .1
            .duration;
        (duration - (now - m.started)).max(0.0)
    }

    /// Every member's vertices at `now`, in the order of `mesh`.
    pub fn pose(&self, now: f32) -> Vec<Vertex> {
        let mut out = Vec::with_capacity(self.mesh.vertices.len());
        for m in &self.members {
            let character = &self.kinds[m.kind].character;
            let duration = character.animations[m.animation].1.duration.max(1e-3);
            let t = (now - m.started).max(0.0);
            let t = if m.cycle {
                t % duration
            } else {
                t.min(duration)
            };
            out.extend(character.pose(m.animation, t, m.placement));
        }
        out
    }
}

/// Model objects in one mesh, each copy movable.
pub struct Props {
    pub mesh: Level,
    models: Vec<Level>,
    /// Each copy: its model, and where its vertices start in `mesh`.
    copies: Vec<(usize, usize)>,
}

impl Props {
    fn new() -> Self {
        Props {
            mesh: Level::empty(),
            models: Vec::new(),
            copies: Vec::new(),
        }
    }

    /// Moves copy `copy` to `placement`; returns the vertices to write and
    /// where they start.
    /// Half a copy's length along its facing (its model's furthest point
    /// fore or aft).
    pub fn half_length(&self, copy: usize) -> f32 {
        self.copies.get(copy).map_or(0.0, |&(model, _)| {
            self.models[model]
                .vertices
                .iter()
                .map(|v| v.position[2].abs())
                .fold(0.0, f32::max)
        })
    }

    pub fn place(&self, copy: usize, placement: Mat4) -> (usize, Vec<Vertex>) {
        let Some(&(model, first)) = self.copies.get(copy) else {
            return (0, Vec::new());
        };
        let vertices = self.models[model]
            .vertices
            .iter()
            .map(|v| Vertex {
                position: placement.transform_point3(v.position.into()).into(),
                normal: placement
                    .transform_vector3(v.normal.into())
                    .normalize_or_zero()
                    .into(),
                ..*v
            })
            .collect();
        (first, vertices)
    }
}

/// Where an object node's model ended up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placed {
    Prop { goal: bool, copy: usize },
    Pedestrian { goal: bool, member: usize },
}

pub struct LevelObjects {
    /// Model objects there at the start, and the ones goals create.
    pub props: Props,
    pub goal_props: Props,
    /// Pedestrians, split the same way.
    pub crowd: Crowd,
    pub goal_crowd: Crowd,
    /// For each of `LevelNodes::objects`, where its model is (if loaded).
    pub placed: Vec<Option<Placed>>,
    /// Models the node array names that couldn't be loaded, with why.
    pub missing: Vec<String>,
}

/// The animations the level's pedestrians need, to fetch any that aren't
/// in the level's archive.
pub fn needed_animations(nodes: &LevelNodes, sets: &AnimationSets) -> Vec<String> {
    nodes
        .objects
        .iter()
        .filter_map(|n| sets.get(&n.animations?))
        .flatten()
        .map(|(path, _)| path.clone())
        .collect()
}

impl LevelObjects {
    pub fn build(nodes: &LevelNodes, files: &LevelFiles, sets: &AnimationSets) -> Self {
        let mut out = LevelObjects {
            props: Props::new(),
            goal_props: Props::new(),
            crowd: Crowd::new(),
            goal_crowd: Crowd::new(),
            placed: vec![None; nodes.objects.len()],
            missing: Vec::new(),
        };
        // Loaded models and pedestrian kinds, per layer.
        let mut models: HashMap<(&str, bool), Option<(usize, usize)>> = HashMap::new();
        let mut kinds: HashMap<(&str, bool), Option<usize>> = HashMap::new();

        for (i, node) in nodes.objects.iter().enumerate() {
            // Some nodes say "none" or name a model without a path.
            if !node.model.contains('/') {
                continue;
            }
            let goal = !node.created_at_start;
            let placement = Mat4::from_rotation_translation(node.rotation(), node.position);
            if node.kind == ObjectKind::Pedestrian {
                let crowd = if goal {
                    &mut out.goal_crowd
                } else {
                    &mut out.crowd
                };
                let kind = *kinds.entry((&node.model, goal)).or_insert_with(|| {
                    match pedestrian(node, files, sets) {
                        Ok(kind) => {
                            crowd.kinds.push(kind);
                            Some(crowd.kinds.len() - 1)
                        }
                        Err(why) => {
                            out.missing.push(format!("{} ({why})", node.model));
                            None
                        }
                    }
                });
                let Some(kind) = kind else { continue };
                let character = &crowd.kinds[kind].character;
                let animation = IDLE_ROLES
                    .iter()
                    .find_map(|r| character.animation(r))
                    .or_else(|| {
                        character
                            .animations
                            .iter()
                            .position(|(n, _)| !n.eq_ignore_ascii_case("default"))
                    })
                    .unwrap_or(0);
                crowd
                    .mesh
                    .add_instance(&character.mesh, Mat4::IDENTITY, None);
                crowd.members.push(Member {
                    kind,
                    placement,
                    animation,
                    // Spread members out so a kind doesn't move in step.
                    started: -(i as f32 * 0.37),
                    cycle: true,
                });
                out.placed[i] = Some(Placed::Pedestrian {
                    goal,
                    member: crowd.members.len() - 1,
                });
                continue;
            }

            let props = if goal {
                &mut out.goal_props
            } else {
                &mut out.props
            };
            let loaded = *models.entry((&node.model, goal)).or_insert_with(|| {
                match model(&node.model, files) {
                    Ok(level) => {
                        props.models.push(level);
                        Some((props.models.len() - 1, usize::MAX))
                    }
                    Err(why) => {
                        out.missing.push(format!("{} ({why})", node.model));
                        None
                    }
                }
            });
            let Some((index, slots)) = loaded else {
                continue;
            };
            let first = props.mesh.vertices.len();
            let textures = (slots != usize::MAX).then_some(slots);
            let base = props
                .mesh
                .add_instance(&props.models[index], placement, textures);
            models.insert((&node.model, goal), Some((index, base)));
            props.copies.push((index, first));
            out.placed[i] = Some(Placed::Prop {
                goal,
                copy: props.copies.len() - 1,
            });
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

/// A pedestrian's skinned model, skeleton and every animation in its set,
/// named by role.
fn pedestrian(node: &ObjectNode, files: &LevelFiles, sets: &AnimationSets) -> Result<Kind, String> {
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
    let mut animations: Vec<(String, Vec<u8>)> = Vec::new();
    for (path, role) in set {
        if animations.iter().any(|(r, _)| r.eq_ignore_ascii_case(role)) {
            continue;
        }
        if let Some(data) = files.animations.get(path) {
            animations.push((role.clone(), data.clone()));
        }
    }
    if !animations
        .iter()
        .any(|(r, _)| r.eq_ignore_ascii_case("default"))
    {
        return Err("no default animation".into());
    }
    let files = CharacterFiles {
        skin: skin.clone(),
        textures: texture_path(&node.model).and_then(|t| files.models.get(&t).cloned()),
        board: None,
        skeleton: skeleton.clone(),
        key_tables,
        animations,
    };
    let character = Character::from_files(&files).map_err(|e| format!("{e:#}"))?;
    let roles = character
        .animations
        .iter()
        .enumerate()
        .map(|(i, (name, _))| (qb::checksum(name), i))
        .collect();
    let mut reach: Vec<f32> = character
        .mesh
        .vertices
        .iter()
        .map(|v| glam::Vec2::new(v.position[0], v.position[2]).length())
        .collect();
    reach.sort_by(f32::total_cmp);
    let radius = reach
        .get(reach.len() * 8 / 10)
        .copied()
        .unwrap_or(15.0)
        .clamp(8.0, 120.0);
    let height = character
        .mesh
        .vertices
        .iter()
        .map(|v| v.position[1])
        .fold(0.0, f32::max)
        .max(20.0);
    let solid = !["bird", "crow", "bat", "gull"]
        .iter()
        .any(|flier| skeleton_name.contains(flier));
    Ok(Kind {
        character,
        roles,
        radius,
        height,
        solid,
    })
}

/// Where to look at an object from: in front of it and a little above.
pub fn view_of(node: &ObjectNode) -> (Vec3, Vec3) {
    let forward = node.rotation() * Vec3::Z;
    let target = node.position + Vec3::Y * 40.0;
    (target + forward * 250.0 + Vec3::Y * 80.0, target)
}
