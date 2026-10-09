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
//! - `create` / `kill Name = object` (a created object starts its
//!   `TriggerScript`), `Die`, and `IsAlive Name = object`.
//! - The skater coming near: `Obj_SetInnerRadius` / `Obj_SetOuterRadius`
//!   (feet) and `Obj_SetException ex = SkaterInRadius | SkaterOutOfRadius
//!   scr = script [params = {...}]`, which, when the skater crosses the
//!   radius, runs that script in place of the object's own (until
//!   `Obj_ClearException(s)`). Birds take off this way.
//! - Moving straight to a spot: `Obj_MoveToNode Name = node [speed = mph]
//!   [orient]` (at once without a speed), `Obj_MoveToRelPos (x, y, z) time
//!   = seconds` (relative to the object's facing), `Obj_LookAtNode`,
//!   `Obj_WaitMove` and `Obj_IsMoving`.
//! - `Obj_StickToGround distAbove distBelow [pitch]` (feet; `off`): kept
//!   on the ground below as it goes, tipped with the slope.
//! - Object flags: `Obj_SetFlag`, `Obj_ClearFlag`, `Obj_FlagSet`,
//!   `Obj_FlagNotSet`.
//! - Looking at the skater: `Obj_ObjectInRadius radius = n feet Type =
//!   skater`, `Obj_AngleToNearestSkaterGreaterThan degrees`,
//!   `Obj_LookAtObject Type = skater time = seconds` (turning smoothly)
//!   and `Obj_WaitRotate`.
//! - `GoalManager_HasWonGoal Name = goal`, from the goals won.
//! - The goal on, for scripts that aren't any object's (a gap's
//!   `Gapscript`, a goal's own): `GoalManager_GoalIsActive`,
//!   `GoalManager_SetGoalFlag Name = goal flag 1`, `GoalManager_GoalFlagSet`,
//!   `GoalManager_AllFlagsSet`, `GoalManager_GotCounterObject` (a counter
//!   goal's), `GoalManager_WinGoal`, `GoalManager_GoalExists`, and
//!   `IsCareerMode` (always: the levels are as the career has them).
//! - `LocalSkaterExists` (skating), `Obj_LookAtObject Name = object`.
//! - `Obj_RotY speed = degrees a second`, `Obj_StopRotating` and
//!   `Obj_Hover Amp = units Freq = hertz`.
//! - `playsound` / `obj_playsound name [Vol = percent]`: collected in
//!   [`Behaviour::sounds`] for the viewer to play.
//!
//! Everything else does nothing yet, and conditions about the game
//! (`IsCareerMode`, goals) are false: the viewer has no career. Units and
//! turning are this module's best reading of the command names, not
//! checked against the running game.

use std::collections::HashMap;

use glam::{Mat4, Quat, Vec3};
use qb::vm::{Host, Outcome, Params, Program, Thread};
use qb::{Value, checksum};

use crate::nodes::{LevelNodes, PathNode};
use crate::objects::{LevelObjects, Placed};

/// Units a second per mile an hour, taking a unit as an inch.
const MPH: f32 = 17.6;
/// Units a second per foot a second.
const FPS: f32 = 12.0;
/// Units per foot (radii are in feet).
const FOOT: f32 = 12.0;

/// One object's state.
struct State {
    position: Vec3,
    rotation: Quat,
    alive: bool,
    /// Whether its model needs re-placing.
    dirty: bool,
    path: Path,
    /// Moving straight to a spot: where, how fast, and whether to pitch to
    /// face it.
    moving: Option<(Vec3, f32, bool)>,
    /// Radii for the skater exceptions (units), and where the skater was
    /// last frame: inside the inner one, outside the outer one.
    inner: f32,
    outer: f32,
    was_inside: bool,
    was_outside: bool,
    /// Exceptions set: (exception, script, params).
    exceptions: Vec<(u32, u32, Params)>,
    /// Turning about Y (radians a second; `Obj_RotY`).
    spin: f32,
    /// Bobbing up and down (`Obj_Hover Amp = units Freq = hertz`), shown
    /// only: where it is stays put.
    hover: Option<(f32, f32)>,
    /// Its flags (`Obj_SetFlag`, `Obj_ClearFlag`; `Obj_FlagSet` asks).
    flags: Vec<u32>,
    /// Turning to face something: from, to, seconds in and how long.
    turn: Option<(Quat, Quat, f32, f32)>,
    /// Kept on the ground below as it moves (`Obj_StickToGround distAbove
    /// distBelow [pitch]`): how far up and down to look (units), and
    /// whether to tip with the slope.
    stick: Option<(f32, f32, bool)>,
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
    /// Each object's `TriggerScript`, run when it's created.
    scripts: Vec<Option<u32>>,
    /// Whether each object is one goals add (not there at the start).
    goal: Vec<bool>,
    /// Whether goals' objects are being shown whether made or not, as last
    /// placed.
    goal_shown: Option<bool>,
    /// Where the skater is (none when not skating).
    skater: Option<Vec3>,
    /// Scripts to start once the running ones have had their turn: an
    /// object's script on `create`.
    starting: Vec<(usize, Thread)>,
    /// Sounds the scripts played since the viewer last took them: the
    /// sound's name (checksum), where, and the volume (1 is full).
    pub sounds: Vec<(u32, Vec3, f32)>,
    /// Names scripts created or killed that aren't objects (particle
    /// emitters, sectors), for the viewer: (name, created?).
    pub other_creates: Vec<(u32, bool)>,
    /// Voice lines scripts asked for (`midgoalvoiceover stream = name`).
    pub voice_lines: Vec<u32>,
    /// The goals won (their ids), for `GoalManager_HasWonGoal`.
    pub won_goals: std::collections::HashSet<u32>,
    /// The level's own runner (an extra state past the objects).
    level_runner: usize,
    /// Every named node's name as written (for `create prefix = "..."`).
    labels: Vec<(u32, String)>,
    /// The goal manager: the goal on (its id), the flags its scripts have
    /// set (`GoalManager_SetGoalFlag`), how many win it, and whether a
    /// script has won it (`GoalManager_WinGoal`).
    pub active_goal: Option<u32>,
    pub goal_flags: std::collections::HashSet<u32>,
    /// A counter goal's things got (`GoalManager_GotCounterObject`).
    pub goal_count: usize,
    pub goal_needed: usize,
    pub goal_won: bool,
    /// The goal on's parameters, as `GoalManager_GetGoalParams` gives
    /// them (and `GoalManager_EditGoal` changes them).
    pub goal_params: Params,
    /// Scripts to run alongside what objects are running
    /// (`RunScriptOnObject`, `SpawnScript`).
    spawning: Vec<(usize, Thread)>,
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
                moving: None,
                inner: 0.0,
                outer: 0.0,
                was_inside: false,
                was_outside: true,
                exceptions: Vec::new(),
                spin: 0.0,
                hover: None,
                stick: None,
                flags: Vec::new(),
                turn: None,
            })
            .collect();
        // One more, the level's own: what runs scripts that are nobody's
        // (a gap's, a goal's).
        let mut states: Vec<State> = states;
        let level_runner = states.len();
        states.push(State {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            alive: true,
            dirty: false,
            path: Path::default(),
            moving: None,
            inner: 0.0,
            outer: 0.0,
            was_inside: false,
            was_outside: true,
            exceptions: Vec::new(),
            spin: 0.0,
            hover: None,
            stick: None,
            flags: Vec::new(),
            turn: None,
        });
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
            object_node: nodes.objects.iter().map(|o| o.node).chain([0]).collect(),
            level_runner,
            labels: nodes
                .labels
                .iter()
                .map(|(n, l)| (*n, l.to_ascii_lowercase()))
                .collect(),
            active_goal: None,
            goal_flags: Default::default(),
            goal_count: 0,
            goal_needed: 0,
            goal_won: false,
            goal_params: Vec::new(),
            spawning: Vec::new(),
            random: 0x2545_F491,
            unknown: HashMap::new(),
            scripts: nodes
                .objects
                .iter()
                .map(|o| o.script)
                .chain([None])
                .collect(),
            goal: nodes
                .objects
                .iter()
                .map(|o| !o.created_at_start)
                .chain([false])
                .collect(),
            goal_shown: None,
            skater: None,
            starting: Vec::new(),
            sounds: Vec::new(),
            other_creates: Vec::new(),
            voice_lines: Vec::new(),
            won_goals: Default::default(),
        }
    }

    /// The object with this node name.
    pub fn object(&self, name: u32) -> Option<usize> {
        self.object_named(name)
    }

    /// Makes an object exist or not (its own script isn't run), stopping
    /// what it was doing.
    pub fn set_alive(&mut self, object: usize, alive: bool) {
        let state = &mut self.states[object];
        state.alive = alive;
        state.dirty = true;
        state.moving = None;
        state.spin = 0.0;
        state.hover = None;
        state.exceptions.clear();
        self.threads.retain(|(o, _)| *o != object);
    }

    /// A vehicle the skater's skitching on: no more stopping for the
    /// skater, off at its `SkitchSpeed` (the car script's header default,
    /// in miles an hour).
    pub fn skitch(&mut self, object: usize) {
        let speed = self.scripts[object]
            .and_then(|s| self.program.script(s))
            .and_then(|body| {
                body.iter().find_map(|t| match t {
                    qb::Token::Name(n) if self.program.has_script(*n) => Some(*n),
                    _ => None,
                })
            })
            .and_then(|called| self.program.default_param(called, checksum("SkitchSpeed")))
            .and_then(Value::as_f32)
            .unwrap_or(30.0);
        let state = &mut self.states[object];
        state.exceptions.clear();
        state.path.top_speed = speed * MPH;
    }

    /// Lets the vehicle go back to what its own script has it do.
    pub fn unskitch(&mut self, object: usize) {
        if let Some(script) = self.scripts[object] {
            self.start(object, Thread::new(script, Vec::new()));
        }
    }

    /// Runs `script` as the object's script.
    pub fn run_script(&mut self, object: usize, script: u32) {
        self.start(object, Thread::new(script, Vec::new()));
    }

    /// Turns an object to face `at` (across), over `seconds`.
    pub fn look_at(&mut self, object: usize, at: Vec3, seconds: f32) {
        let state = &mut self.states[object];
        let flat = (at - state.position).with_y(0.0).normalize_or_zero();
        if flat != Vec3::ZERO {
            let to = Quat::from_rotation_arc(Vec3::Z, flat);
            state.turn = Some((state.rotation, to, 0.0, seconds.max(1e-3)));
        }
    }

    /// Sets an object bobbing up and down so far, so often a second.
    pub fn set_hover(&mut self, object: usize, amplitude: f32, frequency: f32) {
        self.states[object].hover = Some((amplitude, frequency));
    }

    /// Sets an object spinning about Y (radians a second).
    pub fn set_spin(&mut self, object: usize, spin: f32) {
        self.states[object].spin = spin;
    }

    /// Where the skater is now, for the scripts' radii (`None` when not
    /// skating).
    pub fn set_skater(&mut self, position: Option<Vec3>) {
        self.skater = position;
    }

    /// Runs the exception scripts of objects the skater just came inside
    /// the inner radius of, or went outside the outer radius of.
    fn check_radii(&mut self) {
        let Some(skater) = self.skater else {
            return;
        };
        let inside_key = checksum("SkaterInRadius");
        let outside_key = checksum("SkaterOutOfRadius");
        for i in 0..self.states.len() {
            let state = &mut self.states[i];
            if !state.alive || state.exceptions.is_empty() {
                continue;
            }
            let distance = state.position.distance(skater);
            let inside = state.inner > 0.0 && distance < state.inner;
            let outside = state.outer > 0.0 && distance > state.outer;
            let mut fire = None;
            if inside && !state.was_inside {
                fire = Some(inside_key);
            } else if outside && !state.was_outside {
                fire = Some(outside_key);
            }
            state.was_inside = inside;
            state.was_outside = outside;
            let Some(key) = fire else { continue };
            let Some((_, script, params)) =
                state.exceptions.iter().find(|(ex, ..)| *ex == key).cloned()
            else {
                continue;
            };
            self.start(i, Thread::new(script, params));
        }
    }

    /// Where the objects are that do something when the skater comes
    /// near (their `SkaterInRadius` exception set), now.
    pub fn radius_triggers(&self) -> Vec<Vec3> {
        let key = checksum("SkaterInRadius");
        self.states
            .iter()
            .filter(|s| s.alive && s.inner > 0.0 && s.exceptions.iter().any(|(e, ..)| *e == key))
            .map(|s| s.position)
            .collect()
    }

    /// How far the goal on's got: its flags set and things counted.
    pub fn goal_progress(&self) -> usize {
        self.goal_flags.len() + self.goal_count
    }

    /// Runs a script that's nobody's (a gap's `Gapscript`, a goal's
    /// scripts) alongside whatever else the level's running.
    pub fn run_level_script(&mut self, script: u32, params: Params) {
        self.starting
            .push((self.level_runner, Thread::new(script, params)));
    }

    /// Runs `thread` as object `i`'s script, in place of what it was
    /// running.
    fn start(&mut self, i: usize, thread: Thread) {
        // The level's runner runs any number side by side.
        if i == self.level_runner {
            self.threads.push((i, thread));
            return;
        }
        match self.threads.iter_mut().find(|(o, _)| *o == i) {
            Some((_, t)) => *t = thread,
            None => self.threads.push((i, thread)),
        }
    }

    /// Keeps an object that sticks to the ground on it, tipped with the
    /// slope if it says so.
    fn stick_to_ground(&mut self, i: usize, world: &skate::World) {
        let state = &mut self.states[i];
        let (Some((above, below, pitch)), true, true) = (state.stick, state.alive, state.dirty)
        else {
            return;
        };
        let from = state.position + Vec3::Y * above;
        let Some(hit) = world.ray(from, state.position - Vec3::Y * below) else {
            return;
        };
        state.position = hit.point;
        if pitch && hit.normal.y > 0.3 {
            // Facing the same way round, along the slope.
            let forward = state.rotation * Vec3::Z;
            let along = (forward - hit.normal * forward.dot(hit.normal)).normalize_or(forward);
            state.rotation = Quat::from_mat3(&glam::Mat3::from_cols(
                hit.normal.cross(along).normalize_or(Vec3::X),
                hit.normal,
                along,
            ));
        }
    }

    /// Moves objects heading straight for a spot.
    fn move_straight(&mut self, i: usize, dt: f32) {
        let state = &mut self.states[i];
        if let Some((from, to, t, length)) = &mut state.turn {
            *t += dt;
            let k = (*t / length.max(1e-3)).min(1.0);
            state.rotation = from.slerp(*to, k);
            state.dirty = true;
            if k >= 1.0 {
                state.turn = None;
            }
        }
        if state.hover.is_some() && state.alive {
            state.dirty = true;
        }
        if state.spin != 0.0 && state.alive {
            state.rotation = Quat::from_rotation_y(state.spin * dt) * state.rotation;
            state.dirty = true;
        }
        let Some((to, speed, orient)) = state.moving else {
            return;
        };
        let along = to - state.position;
        let distance = along.length();
        let step = speed * dt;
        if distance > 1e-3 {
            let direction = if orient {
                along / distance
            } else {
                Vec3::new(along.x, 0.0, along.z).normalize_or_zero()
            };
            if direction != Vec3::ZERO {
                state.rotation = Quat::from_rotation_arc(Vec3::Z, direction);
            }
        }
        if step >= distance {
            state.position = to;
            state.moving = None;
        } else {
            state.position += along / distance * step;
        }
        state.dirty = true;
    }

    /// Where an object is now (in mesh space).
    pub fn position(&self, object: usize) -> Vec3 {
        self.states[object].position
    }

    /// Which way an object faces (models face +Z) and how fast it's going
    /// (along a path or straight to a spot).
    pub fn motion(&self, object: usize) -> (Vec3, f32) {
        let state = &self.states[object];
        let speed = match state.moving {
            Some((_, speed, _)) => speed,
            None if state.path.target.is_some() => state.path.speed,
            None => 0.0,
        };
        (state.rotation * Vec3::Z, speed)
    }

    /// Whether an object exists now (created and not killed).
    pub fn alive(&self, object: usize) -> bool {
        self.states[object].alive
    }

    /// The loaded scripts and global values (the game's constants too).
    pub fn program(&self) -> &Program {
        &self.program
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
        show_goal: bool,
        ground: Option<&skate::World>,
    ) -> Vec<(bool, usize, Mat4)> {
        // Goals' objects show when made (or all of them, previewing).
        if self.goal_shown != Some(show_goal) {
            self.goal_shown = Some(show_goal);
            for (state, goal) in self.states.iter_mut().zip(&self.goal) {
                state.dirty |= *goal;
            }
        }
        self.check_radii();
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
        // Scripts started meanwhile (by `create`) join in, and those of
        // objects gone stop.
        threads.retain(|(o, _)| self.states[*o].alive);
        self.threads = threads;
        for (i, thread) in std::mem::take(&mut self.starting) {
            self.start(i, thread);
        }
        self.threads.append(&mut self.spawning);
        self.program = program;

        for i in 0..self.states.len() {
            self.follow_path(i, dt);
            self.move_straight(i, dt);
            if let Some(world) = ground {
                self.stick_to_ground(i, world);
            }
        }

        // Push changes to the models.
        let mut moved = Vec::new();
        for (i, state) in self.states.iter_mut().enumerate() {
            if !std::mem::take(&mut state.dirty) {
                continue;
            }
            let shown = state.alive || (self.goal[i] && show_goal);
            let bob = state.hover.map_or(0.0, |(amplitude, frequency)| {
                amplitude * (now * frequency * std::f32::consts::TAU).sin()
            });
            let placement = if shown {
                Mat4::from_rotation_translation(state.rotation, state.position + Vec3::Y * bob)
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
                match args.get(c("speed")).and_then(Value::as_f32) {
                    Some(speed) if speed > 0.0 => {
                        state.moving = Some((p, speed * MPH, args.has_flag(c("orient"))));
                    }
                    _ => {
                        state.position = p;
                        state.moving = None;
                        state.dirty = true;
                    }
                }
            }
        } else if name == c("Obj_MoveToRelPos") {
            let offset = match args {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Vector(v)) => Some(Vec3::from(*v)),
                    _ => None,
                }),
                _ => None,
            };
            if let Some(offset) = offset {
                let state = &mut b.states[object];
                let to = state.position + state.rotation * offset;
                let time = args
                    .get(c("time"))
                    .and_then(Value::as_f32)
                    .filter(|t| *t > 0.0)
                    .unwrap_or(10.0);
                state.moving = Some((to, offset.length() / time, true));
            }
        } else if name == c("Obj_LookAtNode") {
            let node = named("Name").and_then(|n| b.node_by_name.get(&n).copied());
            if let Some(p) = node.and_then(|n| b.nodes[n].position) {
                let state = &mut b.states[object];
                let along = p - state.position;
                let flat = Vec3::new(along.x, 0.0, along.z).normalize_or_zero();
                if flat != Vec3::ZERO {
                    state.rotation = Quat::from_rotation_arc(Vec3::Z, flat);
                    state.dirty = true;
                }
            }
        } else if name == c("Obj_IsMoving") {
            let state = &b.states[object];
            return Outcome::Done(state.moving.is_some() || state.path.target.is_some());
        } else if name == c("Obj_SetInnerRadius") || name == c("Obj_SetOuterRadius") {
            if let Some(r) = Self::number(args) {
                let state = &mut b.states[object];
                if name == c("Obj_SetInnerRadius") {
                    state.inner = r * FOOT;
                    state.was_inside = false;
                } else {
                    state.outer = r * FOOT;
                    state.was_outside = true;
                }
            }
        } else if name == c("Obj_SetException") {
            if let (Some(ex), Some(scr)) = (named("ex"), named("scr")) {
                let params = match args.get(c("params")) {
                    Some(Value::Struct(items)) => items.clone(),
                    _ => Vec::new(),
                };
                let state = &mut b.states[object];
                state.exceptions.retain(|(e, ..)| *e != ex);
                state.exceptions.push((ex, scr, params));
            }
        } else if name == c("Obj_ClearException") {
            if let Some(ex) = named("ex") {
                b.states[object].exceptions.retain(|(e, ..)| *e != ex);
            }
        } else if name == c("Obj_ClearExceptions") {
            b.states[object].exceptions.clear();
        } else if name == c("playsound") || name == c("obj_playsound") {
            let sound = match args {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Name(n)) => Some(*n),
                    _ => None,
                }),
                _ => None,
            };
            if let Some(sound) = sound {
                let volume = args
                    .get(c("Vol"))
                    .and_then(Value::as_f32)
                    .map_or(1.0, |v| v / 100.0);
                b.sounds.push((sound, b.states[object].position, volume));
            }
        } else if name == c("Obj_RotY") {
            let speed = args.get(c("speed")).and_then(Value::as_f32).unwrap_or(0.0);
            b.states[object].spin = speed.to_radians();
        } else if name == c("Obj_Hover") {
            let amp = args.get(c("Amp")).and_then(Value::as_f32).unwrap_or(0.0);
            let freq = args.get(c("Freq")).and_then(Value::as_f32).unwrap_or(1.0);
            b.states[object].hover = Some((amp, freq));
        } else if name == c("Obj_StickToGround") {
            b.states[object].stick = if args.has_flag(c("off")) {
                None
            } else {
                Some((
                    args.get(c("distAbove"))
                        .and_then(Value::as_f32)
                        .unwrap_or(10.0)
                        * FOOT,
                    args.get(c("distBelow"))
                        .and_then(Value::as_f32)
                        .unwrap_or(30.0)
                        * FOOT,
                    args.has_flag(c("pitch")),
                ))
            };
        } else if name == c("Obj_StopRotating") {
            b.states[object].spin = 0.0;
        } else if [
            "Obj_SetFlag",
            "Obj_ClearFlag",
            "Obj_FlagSet",
            "Obj_FlagNotSet",
        ]
        .iter()
        .any(|n| name == c(n))
        {
            // The flag's the first bare name (`Obj_FlagSet Expired`).
            let flag = match args {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Name(n)) => Some(*n),
                    (Some(k), Value::Name(n)) if *k == c("flag") => Some(*n),
                    _ => None,
                }),
                _ => None,
            };
            let Some(flag) = flag else {
                return Outcome::Done(false);
            };
            let flags = &mut b.states[object].flags;
            if name == c("Obj_SetFlag") {
                if !flags.contains(&flag) {
                    flags.push(flag);
                }
            } else if name == c("Obj_ClearFlag") {
                flags.retain(|f| *f != flag);
            } else {
                let set = flags.contains(&flag);
                return Outcome::Done(if name == c("Obj_FlagSet") { set } else { !set });
            }
        } else if name == c("midgoalvoiceover") || name == c("obj_playstream") {
            // `midgoalvoiceover stream = name`, `obj_playstream name`.
            let stream = named("stream").or_else(|| match args {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Name(n)) => Some(*n),
                    _ => None,
                }),
                _ => None,
            });
            if let Some(stream) = stream {
                b.voice_lines.push(stream);
            }
        } else if name == c("LocalSkaterExists") {
            return Outcome::Done(b.skater.is_some());
        } else if name == c("Obj_LookAtObject") {
            // At another object, or the skater (`Type = skater`), over
            // `time` seconds.
            let at = if named("Type") == Some(c("skater")) {
                b.skater
            } else {
                named("Name")
                    .and_then(|n| b.object_named(n))
                    .map(|o| b.states[o].position)
            };
            if let Some(at) = at {
                let time = args.get(c("time")).and_then(Value::as_f32).unwrap_or(0.0);
                let state = &mut b.states[object];
                let flat = (at - state.position).with_y(0.0).normalize_or_zero();
                if flat != Vec3::ZERO {
                    let to = Quat::from_rotation_arc(Vec3::Z, flat);
                    if time > 0.0 {
                        state.turn = Some((state.rotation, to, 0.0, time));
                    } else {
                        state.rotation = to;
                        state.dirty = true;
                    }
                }
            }
        } else if name == c("Obj_WaitRotate") {
            if let Some((_, _, t, length)) = b.states[object].turn {
                return Outcome::Wait((length - t).max(0.0));
            }
        } else if name == c("Obj_ObjectInRadius") {
            // `radius = 80 feet Type = skater`: the skater that near.
            let radius = args.get(c("radius")).and_then(Value::as_f32).unwrap_or(0.0);
            let radius = if args.has_flag(c("feet")) {
                radius * FOOT
            } else {
                radius
            };
            let near = b
                .skater
                .is_some_and(|s| s.distance(b.states[object].position) < radius);
            return Outcome::Done(near);
        } else if name == c("Obj_AngleToNearestSkaterGreaterThan") {
            let Some(skater) = b.skater else {
                return Outcome::Done(false);
            };
            let degrees = Self::number(args).unwrap_or(0.0);
            let state = &b.states[object];
            let facing = (state.rotation * Vec3::Z).with_y(0.0).normalize_or_zero();
            let to = (skater - state.position).with_y(0.0).normalize_or_zero();
            let angle = facing.dot(to).clamp(-1.0, 1.0).acos().to_degrees();
            return Outcome::Done(angle > degrees);
        } else if name == c("GoalManager_GoalIsActive") {
            return Outcome::Done(named("Name").is_some() && named("Name") == b.active_goal);
        } else if name == c("GoalManager_HasSeenGoal") {
            return Outcome::Done(true);
        } else if name == c("IsCareerMode") || name == c("GoalManager_GoalExists") {
            // The levels are played as the career has them (their set-up
            // scripts leave things as the goals won say), with all their
            // goals.
            return Outcome::Done(true);
        } else if name == c("GoalManager_SetGoalFlag") || name == c("GoalManager_GoalFlagSet") {
            // `GoalManager_SetGoalFlag Name = goal Got_1 1`.
            let (mut flag, mut value) = (None, 1);
            if let Value::Struct(items) = args {
                for (k, v) in items {
                    match (k, v) {
                        (None, Value::Name(n)) => flag = flag.or(Some(*n)),
                        (None, Value::Integer(i)) => value = *i,
                        (Some(k), Value::Name(n)) if *k == c("flag") => flag = Some(*n),
                        _ => {}
                    }
                }
            }
            let ours = named("Name").is_some() && named("Name") == b.active_goal;
            let Some(flag) = flag.filter(|_| ours) else {
                return Outcome::Done(false);
            };
            if name == c("GoalManager_GoalFlagSet") {
                return Outcome::Done(b.goal_flags.contains(&flag));
            }
            if value != 0 {
                b.goal_flags.insert(flag);
            } else {
                b.goal_flags.remove(&flag);
            }
        } else if name == c("GoalManager_GotCounterObject") {
            let ours = named("Name").is_some() && named("Name") == b.active_goal;
            if ours {
                b.goal_count += 1;
            }
        } else if name == c("GoalManager_CanStartGoal") {
            // The goal on starts (`goal_start` then makes its things).
            return Outcome::Done(named("Name").is_some() && named("Name") == b.active_goal);
        } else if name == c("GoalManager_GetGoalParams")
            || name == c("GoalManager_GetNumberCollected")
        {
            // The goal on's settings, with how far it's got.
            let ours = named("Name").is_some() && named("Name") == b.active_goal;
            if !ours {
                return Outcome::Done(false);
            }
            let mut params = b.goal_params.clone();
            params.push((
                Some(c("num_flags_set")),
                Value::Integer(b.goal_flags.len() as i32),
            ));
            params.push((
                Some(c("number_collected")),
                Value::Integer(b.goal_count as i32),
            ));
            return Outcome::Params(params);
        } else if name == c("GoalManager_EditGoal") {
            let ours = named("Name").is_some() && named("Name") == b.active_goal;
            if let (true, Some(Value::Struct(items))) = (ours, args.get(c("params"))) {
                for (k, v) in items.iter().filter(|(k, _)| k.is_some()) {
                    b.goal_params.retain(|(o, _)| o != k);
                    b.goal_params.push((*k, v.clone()));
                }
            }
        } else if name == c("RunScriptOnObject")
            || name == c("SpawnScript")
            || name == c("Obj_SpawnScript")
        {
            // A script run alongside: on the object named (`id`), or on
            // this one.
            let on = if name == c("RunScriptOnObject") {
                match named("id").and_then(|n| b.object_named(n)) {
                    Some(o) => o,
                    None => return Outcome::Done(false),
                }
            } else {
                object
            };
            let script = match args {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Name(n)) => Some(*n),
                    _ => None,
                }),
                _ => None,
            };
            let Some(script) = script else {
                return Outcome::Done(false);
            };
            let params = match args.get(c("params")) {
                Some(Value::Struct(p)) => p.clone(),
                _ => Vec::new(),
            };
            b.spawning.push((on, Thread::new(script, params)));
        } else if name == c("GoalManager_AllFlagsSet") {
            let ours = named("Name").is_some() && named("Name") == b.active_goal;
            return Outcome::Done(ours && b.goal_flags.len() >= b.goal_needed.max(1));
        } else if name == c("GoalManager_WinGoal") {
            let ours = named("Name").is_some() && named("Name") == b.active_goal;
            if ours {
                b.goal_won = true;
            }
            return Outcome::Done(ours);
        } else if name == c("GoalManager_HasWonGoal") {
            let won = named("Name").is_some_and(|g| b.won_goals.contains(&g));
            return Outcome::Done(won);
        } else if [
            "Obj_SetPathTurnDist",
            "Obj_SetPathMinStopVel",
            "Obj_SetGroundOffset",
        ]
        .iter()
        .any(|n| name == c(n))
        {
            // Fine points of following a path: close enough as it is.
        } else if name == c("Obj_ShadowOff") || name == c("Obj_ShadowOn") {
            // Pedestrians cast no shadows here anyway.
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
            if let Some((to, speed, _)) = state.moving {
                if speed > 0.0 {
                    return Outcome::Wait((to - state.position).length() / speed + 1.0 / 60.0);
                }
            }
            if let (Some(t), true) = (state.path.target, state.path.speed > 0.0) {
                if let Some(p) = b.nodes[t].position {
                    return Outcome::Wait((p - state.position).length() / state.path.speed);
                }
            }
        } else if name == c("create") || name == c("kill") {
            // By name, or every node whose name starts so
            // (`create prefix = "seaweed"`).
            let targets: Vec<u32> = match args.get(c("prefix")) {
                Some(Value::String(p) | Value::LocalString(p)) => {
                    let p = p.to_ascii_lowercase();
                    b.labels
                        .iter()
                        .filter(|(_, l)| l.starts_with(&p))
                        .map(|(n, _)| *n)
                        .collect()
                }
                _ => named("Name").into_iter().collect(),
            };
            let create = name == c("create");
            for target in targets {
                let Some(o) = b.object_named(target) else {
                    b.other_creates.push((target, create));
                    continue;
                };
                // A new object runs its own script.
                if create && !b.states[o].alive {
                    if let Some(script) = b.scripts[o] {
                        b.starting.push((o, Thread::new(script, Vec::new())));
                    }
                }
                b.states[o].alive = create;
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
