//! Objects' scripts: each object there at the start runs its
//! `TriggerScript` with the QB interpreter (`qb::vm`), and the commands it
//! uses move and animate the object.
//!
//! Commands understood so far:
//!
//! - `Obj_FollowPathLinked [orient]`: head along the object's node links,
//!   from waypoint to waypoint, at the path velocity; at a fork take the
//!   first link, or a random one after `Obj_RandomPathMode On`. The object
//!   turns to face where it's going unless `Obj_PathHeading off`; `orient`
//!   also pitches it (aircraft).
//! - `Obj_SetPathVelocity v [mph | fps]`, `Obj_SetPathAcceleration` and
//!   `Obj_SetPathDeceleration` (in `mphps`): speeds in miles per hour
//!   become 17.6 units a second, taking a unit as an inch.
//! - `Obj_MoveToNode Name = node`, `Obj_PlayAnim Anim = role [Cycle]`,
//!   `Obj_WaitAnimFinished`, `Obj_WaitMove`.
//! - `create` / `kill Name = object`, `Die`, and `IsAlive Name = object`.
//!
//! Everything else does nothing yet, and conditions about the game
//! (`IsCareerMode`, goals, the skater) are false: the viewer has no skater
//! or career. Units and turning are this module's best reading of the
//! command names, not checked against the running game.

use std::collections::HashMap;

use glam::{Mat4, Quat, Vec3};
use qb::vm::{Host, Outcome, Program, Thread};
use qb::{Value, checksum};

use crate::nodes::{LevelNodes, PathNode};
use crate::objects::{LevelObjects, Placed};

/// Units a second per mile an hour, taking a unit as an inch.
const MPH: f32 = 17.6;
/// Units a second per foot a second.
const FPS: f32 = 12.0;

/// One object's state.
struct State {
    position: Vec3,
    rotation: Quat,
    alive: bool,
    /// Whether its model needs re-placing.
    dirty: bool,
    path: Path,
}

#[derive(Default)]
struct Path {
    /// The node being headed for.
    target: Option<usize>,
    speed: f32,
    top_speed: f32,
    acceleration: f32,
    deceleration: f32,
    random: bool,
    turn: bool,
    orient: bool,
}

pub struct Behaviour {
    program: Program,
    threads: Vec<(usize, Thread)>,
    states: Vec<State>,
    nodes: Vec<PathNode>,
    /// Object index by node name.
    by_name: HashMap<u32, usize>,
    /// Node index by name (for `Obj_MoveToNode`).
    node_by_name: HashMap<u32, usize>,
    /// Each object's own node (where its paths start).
    object_node: Vec<usize>,
    random: u32,
    /// Commands scripts used that aren't understood, by name, with counts.
    pub unknown: HashMap<u32, usize>,
}

impl Behaviour {
    /// Starts the scripts of the objects there at the start. `scripts` are
    /// the `.qb` files to load (the game's shared ones, then the level's).
    pub fn new(nodes: &LevelNodes, scripts: &[Vec<u8>]) -> Self {
        let mut program = Program::new();
        for file in scripts {
            if let Err(e) = program.add(file) {
                eprintln!("warning: a script file didn't load: {e}");
            }
        }
        let states = nodes
            .objects
            .iter()
            .map(|o| State {
                position: o.position,
                rotation: o.rotation(),
                alive: o.created_at_start,
                dirty: false,
                path: Path {
                    turn: true,
                    ..Path::default()
                },
            })
            .collect();
        let threads = nodes
            .objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.created_at_start)
            .filter_map(|(i, o)| Some((i, Thread::new(o.script?, Vec::new()))))
            .collect();
        Behaviour {
            program,
            threads,
            states,
            nodes: nodes.nodes.clone(),
            by_name: nodes
                .objects
                .iter()
                .enumerate()
                .map(|(i, o)| (o.name, i))
                .collect(),
            node_by_name: nodes
                .nodes
                .iter()
                .enumerate()
                .map(|(i, n)| (n.name, i))
                .collect(),
            object_node: nodes.objects.iter().map(|o| o.node).collect(),
            random: 0x2545_F491,
            unknown: HashMap::new(),
        }
    }

    /// Where an object is now (in mesh space).
    pub fn position(&self, object: usize) -> Vec3 {
        self.states[object].position
    }

    /// Whether an object exists now (created and not killed).
    pub fn alive(&self, object: usize) -> bool {
        self.states[object].alive
    }

    /// How many scripts are still running.
    pub fn running(&self) -> usize {
        self.threads
            .iter()
            .filter(|(_, t)| !t.is_finished())
            .count()
    }

    /// Runs the scripts for `dt` seconds and moves objects along their
    /// paths; `now` is the clock pedestrians' animations use. Returns the
    /// props to re-place: (goal layer?, copy, placement).
    pub fn update(
        &mut self,
        objects: &mut LevelObjects,
        now: f32,
        dt: f32,
    ) -> Vec<(bool, usize, Mat4)> {
        let program = std::mem::take(&mut self.program);
        let mut threads = std::mem::take(&mut self.threads);
        for (object, thread) in &mut threads {
            if thread.is_finished() {
                continue;
            }
            let mut host = Commands {
                behaviour: self,
                objects,
                object: *object,
                now,
            };
            thread.run(&program, &mut host, dt);
        }
        self.threads = threads;
        self.program = program;

        for i in 0..self.states.len() {
            self.follow_path(i, dt);
        }

        // Push changes to the models.
        let mut moved = Vec::new();
        for (i, state) in self.states.iter_mut().enumerate() {
            if !std::mem::take(&mut state.dirty) {
                continue;
            }
            let placement = if state.alive {
                Mat4::from_rotation_translation(state.rotation, state.position)
            } else {
                // Gone: shrink it to nothing.
                Mat4::from_scale(Vec3::ZERO)
            };
            match objects.placed.get(i).copied().flatten() {
                Some(Placed::Prop { goal, copy }) => moved.push((goal, copy, placement)),
                Some(Placed::Pedestrian { goal, member }) => {
                    let crowd = if goal {
                        &mut objects.goal_crowd
                    } else {
                        &mut objects.crowd
                    };
                    crowd.set_placement(member, placement);
                }
                None => {}
            }
        }
        moved
    }

    fn follow_path(&mut self, i: usize, dt: f32) {
        let Some(target) = self.states[i].path.target else {
            return;
        };
        let Some(goal) = self.nodes.get(target).and_then(|n| n.position) else {
            self.states[i].path.target = None;
            return;
        };
        let state = &mut self.states[i];
        let path = &mut state.path;
        // Speed up or slow down toward the path velocity.
        if path.speed < path.top_speed {
            let rate = if path.acceleration > 0.0 {
                path.acceleration
            } else {
                f32::INFINITY
            };
            path.speed = (path.speed + rate * dt).min(path.top_speed);
        } else if path.speed > path.top_speed {
            let rate = if path.deceleration > 0.0 {
                path.deceleration
            } else {
                f32::INFINITY
            };
            path.speed = (path.speed - rate * dt).max(path.top_speed);
        }
        let mut step = path.speed * dt;
        let to = goal - state.position;
        let distance = to.length();
        if distance > 1e-3 && (path.turn || path.orient) {
            let direction = if path.orient {
                to / distance
            } else {
                Vec3::new(to.x, 0.0, to.z).normalize_or_zero()
            };
            if direction != Vec3::ZERO {
                // Models face +Z (see `ObjectNode::rotation`).
                state.rotation = Quat::from_rotation_arc(Vec3::Z, direction);
            }
        }
        if step < distance {
            state.position += to / distance * step;
        } else {
            state.position = goal;
            step -= distance;
            let _ = step;
            let links = &self.nodes[target].links;
            let next = match links.len() {
                0 => None,
                1 => Some(links[0]),
                n if path.random => {
                    self.random ^= self.random << 13;
                    self.random ^= self.random >> 17;
                    self.random ^= self.random << 5;
                    Some(links[self.random as usize % n])
                }
                _ => Some(links[0]),
            };
            state.path.target = next;
        }
        state.dirty = true;
    }

    fn object_named(&self, name: u32) -> Option<usize> {
        self.by_name.get(&name).copied()
    }
}

/// The host commands for one object's script.
struct Commands<'a> {
    behaviour: &'a mut Behaviour,
    objects: &'a mut LevelObjects,
    object: usize,
    now: f32,
}

impl Commands<'_> {
    fn number(args: &Value) -> Option<f32> {
        match args {
            Value::Struct(items) => items
                .iter()
                .find(|(k, _)| k.is_none())
                .and_then(|(_, v)| v.as_f32()),
            _ => None,
        }
    }

    /// A speed in units a second (`mph` by default, or `fps`).
    fn speed(args: &Value) -> Option<f32> {
        let v = Self::number(args)?;
        Some(if args.has_flag(checksum("fps")) {
            v * FPS
        } else {
            v * MPH
        })
    }

    fn crowd_member(&mut self, object: usize) -> Option<(&mut crate::objects::Crowd, usize)> {
        match self.objects.placed.get(object).copied().flatten()? {
            Placed::Pedestrian { goal, member } => Some((
                if goal {
                    &mut self.objects.goal_crowd
                } else {
                    &mut self.objects.crowd
                },
                member,
            )),
            Placed::Prop { .. } => None,
        }
    }
}

impl Host for Commands<'_> {
    fn command(&mut self, target: Option<u32>, name: u32, args: &Value) -> Outcome {
        let object = match target {
            Some(t) => match self.behaviour.object_named(t) {
                Some(o) => o,
                None => return Outcome::Done(false),
            },
            None => self.object,
        };
        let named = |key: &str| args.get(checksum(key)).and_then(Value::as_name);
        let c = |s: &str| checksum(s);
        let b = &mut *self.behaviour;

        if name == c("Obj_FollowPathLinked") {
            let state = &mut b.states[object];
            let node = b.object_node[object];
            state.path.target = b.nodes[node].links.first().copied();
            state.path.orient = args.has_flag(c("orient"));
            if state.path.top_speed == 0.0 {
                state.path.top_speed = 10.0 * MPH;
            }
        } else if name == c("Obj_SetPathVelocity") {
            if let Some(v) = Self::speed(args) {
                b.states[object].path.top_speed = v;
            }
        } else if name == c("Obj_SetPathAcceleration") {
            if let Some(v) = Self::number(args) {
                b.states[object].path.acceleration = v * MPH;
            }
        } else if name == c("Obj_SetPathDeceleration") {
            if let Some(v) = Self::number(args) {
                b.states[object].path.deceleration = v * MPH;
            }
        } else if name == c("Obj_RandomPathMode") {
            b.states[object].path.random = !args.has_flag(c("Off"));
        } else if name == c("Obj_PathHeading") {
            b.states[object].path.turn = !args.has_flag(c("off"));
        } else if name == c("Obj_MoveToNode") {
            let node = named("Name").and_then(|n| b.node_by_name.get(&n).copied());
            if let Some(p) = node.and_then(|n| b.nodes[n].position) {
                let state = &mut b.states[object];
                state.position = p;
                state.dirty = true;
            }
        } else if name == c("Obj_PlayAnim") {
            let role = named("Anim");
            let cycle = args.has_flag(c("Cycle"));
            let now = self.now;
            if let (Some(role), Some((crowd, member))) = (role, self.crowd_member(object)) {
                crowd.play(member, role, cycle, now);
            }
        } else if name == c("Obj_WaitAnimFinished") {
            let now = self.now;
            if let Some((crowd, member)) = self.crowd_member(object) {
                return Outcome::Wait(crowd.remaining(member, now));
            }
        } else if name == c("Obj_WaitMove") {
            let state = &b.states[object];
            if let (Some(t), true) = (state.path.target, state.path.speed > 0.0) {
                if let Some(p) = b.nodes[t].position {
                    return Outcome::Wait((p - state.position).length() / state.path.speed);
                }
            }
        } else if name == c("create") || name == c("kill") {
            if let Some(o) = named("Name").and_then(|n| b.object_named(n)) {
                b.states[o].alive = name == c("create");
                b.states[o].dirty = true;
            }
        } else if name == c("Die") {
            b.states[object].alive = false;
            b.states[object].dirty = true;
        } else if name == c("IsAlive") {
            let alive = named("Name")
                .and_then(|n| b.object_named(n))
                .is_some_and(|o| b.states[o].alive);
            return Outcome::Done(alive);
        } else {
            // Not understood (yet): does nothing, false in an `if`.
            *b.unknown.entry(name).or_default() += 1;
            return Outcome::Done(false);
        }
        Outcome::Done(true)
    }
}
