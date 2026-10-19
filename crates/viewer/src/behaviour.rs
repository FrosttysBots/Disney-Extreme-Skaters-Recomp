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
    /// The axis it spins about, in its own frame (Y unless `Obj_RotX`,
    /// `Obj_RotZ`).
    spin_axis: Vec3,
    /// Its size, times its model's (`obj_applyscaling scale = 0.25`:
    /// Beach's gulls).
    scale: f32,
    /// Bobbing up and down (`Obj_Hover Amp = units Freq = hertz`), shown
    /// only: where it is stays put.
    hover: Option<(f32, f32)>,
    /// Its flags (`Obj_SetFlag`, `Obj_ClearFlag`; `Obj_FlagSet` asks).
    flags: Vec<u32>,
    /// Turning to face something: from, to, seconds in and how long.
    turn: Option<(Quat, Quat, f32, f32)>,
    /// Moving to a spot without turning to face the way (`Obj_MoveToPos`:
    /// a door sliding open).
    slide: bool,
    /// Hidden by its scripts (`Obj_Invisible`, till `Obj_Visible`), there
    /// all the same: a race's pro waiting, Zurg's missile in his hand.
    invisible: bool,
    /// A spot it's remembered (`Obj_StorePos`), to look at and go to
    /// (`Obj_LookAtPosStored`, `Obj_MoveToPosStored`).
    stored: Option<Vec3>,
    /// Carried along by another (`Obj_LockToObject ObjectName = skater
    /// (0, 36, 0)`): the skater (`None`) or an object, and where, in its
    /// frame.
    locked: Option<(Option<usize>, Vec3)>,
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
    /// What the skater's doing, for the scripts' `skater:OnGround` and
    /// the like (set by whoever skates it).
    pub skater_state: SkaterState,
    /// Voice lines scripts asked for (`midgoalvoiceover stream = name`).
    pub voice_lines: Vec<u32>,
    /// Text scripts put on screen (`Create_Panel_Message`,
    /// `create_panel_block`: a goal's messages), for the viewer to show.
    pub messages: Vec<String>,
    /// The scripts' screen-element commands (a goal's panel messages, its
    /// text and counters), for the viewer to pass to the game's screen:
    /// (command, its arguments).
    pub screen_calls: Vec<(u32, Value)>,
    /// The screen elements the level's scripts have made and not
    /// destroyed (their ids), for `ScreenElementExists`.
    screen_ids: std::collections::HashSet<u32>,
    /// The game's UI scripts the level's called (`create_speech_box`,
    /// `create_panel_block`...), for the viewer to run on the game's
    /// screen, where they can see what they've made.
    pub screen_scripts: Vec<(u32, Params)>,
    next_screen_id: u32,
    /// Camera paths scripts asked to play (`PlaySkaterCamAnim Name =
    /// path`), for the viewer.
    pub cameras: Vec<u32>,
    /// The camera path playing stopped by the scripts (`KillSkaterCamAnim
    /// current`, `PlaySkaterCamAnim ... STOP`), before any they play next.
    pub camera_stop: bool,
    /// The goals won (their ids), for `GoalManager_HasWonGoal`.
    pub won_goals: std::collections::HashSet<u32>,
    /// The level's own runner (an extra state past the objects).
    level_runner: usize,
    /// Where the level's moving pieces start among the objects (one each,
    /// in `LevelNodes::movers` order).
    movers: usize,
    /// Where the bouncy things are among them.
    bouncies: std::ops::Range<usize>,
    /// Which of the level's objects are pedestrians (in `objects` order).
    peds: Vec<bool>,
    /// Every named node's name as written (for `create prefix = "..."`).
    labels: Vec<(u32, String)>,
    /// The goal manager: the goal on (its id), the flags its scripts have
    /// set (`GoalManager_SetGoalFlag`), how many win it, and whether a
    /// script has won it (`GoalManager_WinGoal`).
    pub active_goal: Option<u32>,
    /// The goal just ended, whose end scripts still ask for its settings.
    pub ended_goal: Option<u32>,
    pub goal_flags: std::collections::HashSet<u32>,
    /// A counter goal's things got (`GoalManager_GotCounterObject`).
    pub goal_count: usize,
    /// The `number_collected` its scripts keep (`GoalManager_EditGoal`),
    /// and whether it counts towards winning (a collect or counter goal's:
    /// Hub's groceries, five dollars an item, and sodas).
    pub goal_collected: usize,
    pub goal_collects: bool,
    pub goal_needed: usize,
    pub goal_won: bool,
    /// The goal on's parameters, as `GoalManager_GetGoalParams` gives
    /// them (and `GoalManager_EditGoal` changes them).
    pub goal_params: Params,
    /// Lost by its scripts (`GoalManager_LoseGoal`, or deactivated).
    pub goal_lost: bool,
    /// Its clock held by its scripts (`GoalManager_PauseGoal`,
    /// `GoalManager_PauseAllGoals`: a cutscene's on).
    pub goal_paused: bool,
    paused_for: f32,
    /// Scripts to stop once this frame's have run: those spawned by name
    /// (`KillSpawnedScript Name = x`), and every one of an object's
    /// (`TerminateObjectsScripts id = obj`).
    killing: Vec<u32>,
    /// What the scripts have shattered (`Shatter Name = x`,
    /// `ShatterAndDie`), for the viewer to break.
    pub shatters: Vec<u32>,
    /// The skater knocked down (`MakeSkaterGoto PedKnockDown`: a
    /// heckler's swipe, Zurg's missile), for the viewer to bail.
    pub knock_down: bool,
    /// The restart the scripts sent the skater back to (`KillSkater`).
    pub skater_restart: Option<u32>,
    /// The skater hidden by the scripts (`skater:Hide`, till `Unhide`).
    pub skater_hidden: bool,
    /// Sectors whose vertex colors the scripts have stopped, started or
    /// started over (`wibbleVC Name = x On`, `off`, `reset`): each one's
    /// own time in its colors and whether that's running. The others
    /// play on the level's clock.
    pub color_clocks: HashMap<u32, (f32, bool)>,
    now: f32,
    terminating: Vec<usize>,
    /// Every goal the level's set-up added (`GoalManager_AddGoal`): its
    /// parameters, by its id; the ids made with the level's prefix
    /// (`GoalManager_CreateGoalName`).
    goal_defs: HashMap<u32, Params>,
    pub goal_prefix: String,
    /// Goals the scripts have started alongside the one on (Hub's
    /// lighthouse minigame, Zurg's height one), and the best each
    /// minigame's had (`GoalManager_CheckMinigameRecord`).
    side_goals: HashMap<u32, SideGoal>,
    records: HashMap<u32, i32>,
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
                spin_axis: Vec3::Y,
                scale: 1.0,
                locked: None,
                hover: None,
                stick: None,
                flags: Vec::new(),
                turn: None,
                slide: false,
                invisible: false,
                stored: None,
            })
            .collect();
        // Then the level's pieces that move (`LevelObject`s), objects too.
        let mut states: Vec<State> = states;
        let movers = states.len();
        for m in &nodes.movers {
            states.push(State {
                position: m.position,
                rotation: m.rotation(),
                alive: m.created_at_start,
                dirty: false,
                path: Path::default(),
                moving: None,
                inner: 0.0,
                outer: 0.0,
                was_inside: false,
                was_outside: true,
                exceptions: Vec::new(),
                spin: 0.0,
                spin_axis: Vec3::Y,
                scale: 1.0,
                locked: None,
                hover: None,
                stick: None,
                flags: Vec::new(),
                turn: None,
                slide: false,
                invisible: false,
                stored: None,
            });
        }
        // And the bouncy things knocked about (their `CollideScript`s set
        // their own exceptions: Hub's groceries), whose comings and goings
        // the viewer's told of too.
        let bouncies = states.len()..states.len() + nodes.bouncies.len();
        for bouncy in &nodes.bouncies {
            states.push(State {
                position: bouncy.position,
                rotation: Quat::IDENTITY,
                alive: bouncy.created_at_start,
                dirty: false,
                path: Path::default(),
                moving: None,
                inner: 0.0,
                outer: 0.0,
                was_inside: false,
                was_outside: true,
                exceptions: Vec::new(),
                spin: 0.0,
                spin_axis: Vec3::Y,
                scale: 1.0,
                locked: None,
                hover: None,
                stick: None,
                flags: Vec::new(),
                turn: None,
                slide: false,
                invisible: false,
                stored: None,
            });
        }
        // One more, the level's own: what runs scripts that are nobody's
        // (a gap's, a goal's).
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
            spin_axis: Vec3::Y,
            scale: 1.0,
            locked: None,
            hover: None,
            stick: None,
            flags: Vec::new(),
            turn: None,
            slide: false,
            invisible: false,
            stored: None,
        });
        let threads = nodes
            .objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.created_at_start)
            .filter_map(|(i, o)| Some((i, Thread::new(o.script?, Vec::new()))))
            .chain(
                nodes
                    .movers
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| m.created_at_start)
                    .filter_map(|(i, m)| Some((movers + i, Thread::new(m.script?, Vec::new())))),
            )
            .collect();
        Behaviour {
            program,
            threads,
            states,
            nodes: nodes.nodes.clone(),
            by_name: nodes
                .objects
                .iter()
                .map(|o| o.name)
                .chain(nodes.movers.iter().map(|m| m.name))
                .chain(nodes.bouncies.iter().map(|b| b.name))
                .enumerate()
                .map(|(i, name)| (name, i))
                .collect(),
            movers,
            bouncies,
            peds: nodes
                .objects
                .iter()
                .map(|o| o.kind == crate::nodes::ObjectKind::Pedestrian)
                .collect(),
            node_by_name: nodes
                .nodes
                .iter()
                .enumerate()
                .map(|(i, n)| (n.name, i))
                .collect(),
            object_node: nodes
                .objects
                .iter()
                .map(|o| o.node)
                .chain(nodes.movers.iter().map(|_| 0))
                .chain(nodes.bouncies.iter().map(|_| 0))
                .chain([0])
                .collect(),
            level_runner,
            labels: nodes
                .labels
                .iter()
                .map(|(n, l)| (*n, l.to_ascii_lowercase()))
                .collect(),
            active_goal: None,
            ended_goal: None,
            goal_flags: Default::default(),
            goal_count: 0,
            goal_collected: 0,
            goal_collects: false,
            goal_needed: 0,
            goal_won: false,
            goal_params: Vec::new(),
            goal_lost: false,
            goal_paused: false,
            paused_for: 0.0,
            killing: Vec::new(),
            shatters: Vec::new(),
            camera_stop: false,
            knock_down: false,
            skater_restart: None,
            skater_hidden: false,
            color_clocks: HashMap::new(),
            now: 0.0,
            terminating: Vec::new(),
            goal_defs: HashMap::new(),
            goal_prefix: String::new(),
            side_goals: HashMap::new(),
            records: HashMap::new(),
            spawning: Vec::new(),
            random: 0x2545_F491,
            unknown: HashMap::new(),
            scripts: nodes
                .objects
                .iter()
                .map(|o| o.script)
                .chain(nodes.movers.iter().map(|m| m.script))
                .chain(nodes.bouncies.iter().map(|_| None))
                .chain([None])
                .collect(),
            goal: nodes
                .objects
                .iter()
                .map(|o| !o.created_at_start)
                .chain(nodes.movers.iter().map(|_| false))
                .chain(nodes.bouncies.iter().map(|_| false))
                .chain([false])
                .collect(),
            goal_shown: None,
            skater: None,
            starting: Vec::new(),
            sounds: Vec::new(),
            other_creates: Vec::new(),
            skater_state: SkaterState::default(),
            voice_lines: Vec::new(),
            messages: Vec::new(),
            screen_calls: Vec::new(),
            screen_ids: Default::default(),
            screen_scripts: Vec::new(),
            next_screen_id: 0,
            cameras: Vec::new(),
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
        self.radius_trigger_objects()
            .into_iter()
            .map(|i| self.states[i].position)
            .collect()
    }

    /// The script object `object` runs when the skater comes near (its
    /// `SkaterInRadius` exception), if any.
    pub fn radius_script(&self, object: usize) -> Option<u32> {
        let key = checksum("SkaterInRadius");
        self.states[object]
            .exceptions
            .iter()
            .find(|(e, ..)| *e == key)
            .map(|(_, script, _)| *script)
    }

    /// The objects that do something when the skater comes near.
    pub fn radius_trigger_objects(&self) -> Vec<usize> {
        let key = checksum("SkaterInRadius");
        self.states
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.alive && s.inner > 0.0 && s.exceptions.iter().any(|(e, ..)| *e == key)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// How far the goal on's got, towards winning it: its flags set or
    /// things got (`GoalManager_GotCounterObject`, or for a collect or
    /// counter goal the `number_collected` its scripts keep), whichever's
    /// further; a goal its scripts have won is done.
    pub fn goal_progress(&self) -> usize {
        let mut got = self.goal_flags.len().max(self.goal_count);
        if self.goal_collects {
            got = got.max(self.goal_collected);
        }
        if self.goal_won {
            got.max(self.goal_needed)
        } else {
            got
        }
    }

    /// How far it's got to show: that, or the `number_collected` its
    /// scripts keep, if more (a gaps goal's isn't what wins it: Free Zazu
    /// keeps its tries there).
    pub fn goal_shown(&self) -> usize {
        self.goal_progress().max(self.goal_collected)
    }

    /// Runs `script` on the object named `name` (a bouncy's
    /// `CollideScript`, knocked: it sets the object's own exceptions),
    /// alongside its own; or, with no such object, on its own.
    pub fn run_object_script(&mut self, name: u32, script: u32) {
        let thread = Thread::new(script, Vec::new());
        match self.object_named(name) {
            Some(o) => self.spawning.push((o, thread)),
            None => self.starting.push((self.level_runner, thread)),
        }
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
            state.rotation = if state.spin_axis == Vec3::Y {
                Quat::from_rotation_y(state.spin * dt) * state.rotation
            } else {
                // (About its own axis.)
                state.rotation * Quat::from_axis_angle(state.spin_axis, state.spin * dt)
            };
            state.dirty = true;
        }
        let Some((to, speed, orient)) = state.moving else {
            return;
        };
        let along = to - state.position;
        let distance = along.length();
        let step = speed * dt;
        if distance > 1e-3 && !state.slide {
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

    /// The object standing for the level's moving piece `i` (in
    /// `LevelNodes::movers` order).
    pub fn mover(&self, i: usize) -> usize {
        self.movers + i
    }

    /// How an object's turned now.
    pub fn rotation(&self, object: usize) -> Quat {
        self.states[object].rotation
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
    /// Creates the object named `name` (as `create Name = name`), if it
    /// isn't there: a goal's pro, there to talk to and take things back.
    pub fn create_object(&mut self, name: u32) {
        let Some(o) = self.object_named(name) else {
            self.other_creates.push((name, true));
            return;
        };
        if self.states[o].alive {
            return;
        }
        if let Some(script) = self.scripts[o] {
            self.starting.push((o, Thread::new(script, Vec::new())));
        }
        self.states[o].alive = true;
        self.states[o].dirty = true;
        if self.bouncies.contains(&o) {
            self.other_creates.push((name, true));
        }
    }

    /// Whether the object named `name` is there (created, not killed).
    pub fn is_alive(&self, name: u32) -> bool {
        self.object_named(name)
            .is_some_and(|o| self.states[o].alive)
    }

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
        self.now = now;
        for (time, running) in self.color_clocks.values_mut() {
            if *running {
                *time += dt;
            }
        }
        // (A pause its scripts never lifted, stopped midway, lapses.)
        self.paused_for = if self.goal_paused {
            self.paused_for + dt
        } else {
            0.0
        };
        if self.paused_for > 30.0 {
            self.goal_paused = false;
        }
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
                program: &program,
            };
            thread.run(&program, &mut host, dt);
        }
        // Scripts started meanwhile (by `create`) join in, and those of
        // objects gone stop.
        threads.retain(|(o, _)| self.states[*o].alive);
        let (killing, terminating) = (
            std::mem::take(&mut self.killing),
            std::mem::take(&mut self.terminating),
        );
        if !killing.is_empty() || !terminating.is_empty() {
            let stopped = |o: &usize, t: &Thread| {
                terminating.contains(o) || t.script().is_some_and(|s| killing.contains(&s))
            };
            threads.retain(|(o, t)| !stopped(o, t));
            self.spawning.retain(|(o, t)| !stopped(o, t));
            self.starting.retain(|(o, t)| !stopped(o, t));
        }
        self.threads = threads;
        for (i, thread) in std::mem::take(&mut self.starting) {
            self.start(i, thread);
        }
        self.threads.append(&mut self.spawning);
        self.program = program;

        for i in 0..self.states.len() {
            self.follow_locked(i);
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
            let shown = (state.alive || (self.goal[i] && show_goal)) && !state.invisible;
            let bob = state.hover.map_or(0.0, |(amplitude, frequency)| {
                amplitude * (now * frequency * std::f32::consts::TAU).sin()
            });
            let placement = if shown {
                Mat4::from_scale_rotation_translation(
                    Vec3::splat(state.scale),
                    state.rotation,
                    state.position + Vec3::Y * bob,
                )
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

    /// Where something carried is: with what carries it, at its offset
    /// turned with it.
    fn follow_locked(&mut self, i: usize) {
        let Some((to, offset)) = self.states[i].locked else {
            return;
        };
        let (at, turn) = match to {
            None => match self.skater {
                Some(p) => (p, Quat::from_rotation_y(self.skater_state.heading)),
                None => return,
            },
            Some(o) => (self.states[o].position, self.states[o].rotation),
        };
        let state = &mut self.states[i];
        state.position = at + turn * offset;
        state.rotation = turn;
        state.dirty = true;
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
        let mut reached = None;
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
            reached = self.nodes[target].spawn_script;
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
        // The node reached's own script, run as what got there.
        if let Some(script) = reached.filter(|s| self.program.has_script(*s)) {
            self.spawning.push((i, Thread::new(script, Vec::new())));
        }
    }

    fn object_named(&self, name: u32) -> Option<usize> {
        self.by_name.get(&name).copied()
    }
}

/// A goal going alongside the one played (a minigame): its parameters,
/// the flags its scripts have set and the things it's got.
#[derive(Clone, Debug, Default)]
struct SideGoal {
    params: Params,
    flags: std::collections::HashSet<u32>,
    count: usize,
}

/// `GoalManager_SetGoalFlag Name = goal Got_1 1`'s flag and value (or
/// `GoalFlagSet`'s `flag = Got_1`).
fn goal_flag_args(args: &Value) -> (Option<u32>, i32) {
    let c = checksum;
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
    (flag, value)
}

impl Behaviour {
    /// The goal manager's commands that aren't about the goal being
    /// played: making goal ids and adding goals (the level's set-up),
    /// goals started alongside it and their flags, records, and other
    /// goals' settings. `None` leaves the command to the goal played.
    fn goal_command(&mut self, program: &Program, name: u32, args: &Value) -> Option<Outcome> {
        let c = checksum;
        let named = |key: &str| args.get(c(key)).and_then(Value::as_name);
        let goal = named("Name");
        let side = goal.filter(|g| self.side_goals.contains_key(g) && Some(*g) != self.active_goal);
        if name == c("GoalManager_CreateGoalName") {
            // `goal_type = "Collect"`: `<level>_goal_Collect`.
            let kind = match args.get(c("goal_type")) {
                Some(Value::String(s) | Value::LocalString(s)) => s.clone(),
                _ => return None,
            };
            if self.goal_prefix.is_empty() {
                return None;
            }
            let id = c(&format!("{}_goal_{kind}", self.goal_prefix));
            return Some(Outcome::Params(vec![(Some(c("goal_ID")), Value::Name(id))]));
        }
        if name == c("GoalManager_AddGoal") {
            // Its parameters: the generic ones its type gives (a global
            // struct), then those it's added with.
            let id = goal?;
            let mut params: Params = Vec::new();
            if let Some(Value::Struct(items)) = args.get(c("params")) {
                for (k, v) in items {
                    match (k, v) {
                        (None, Value::Name(n)) => {
                            if let Some(Value::Struct(base)) = program.value(*n) {
                                for (bk, bv) in base {
                                    params.retain(|(k, _)| bk.is_none() || k != bk);
                                    params.push((*bk, bv.clone()));
                                }
                            } else {
                                params.push((None, v.clone()));
                            }
                        }
                        (Some(_), _) => {
                            params.retain(|(pk, _)| pk != k);
                            params.push((*k, v.clone()));
                        }
                        (None, _) => params.push((None, v.clone())),
                    }
                }
            }
            params.push((Some(c("goal_ID")), Value::Name(id)));
            self.goal_defs.insert(id, params);
            return Some(Outcome::Done(true));
        }
        if name == c("GoalManager_ActivateGoal") {
            let id = goal?;
            if Some(id) == self.active_goal || self.side_goals.contains_key(&id) {
                return Some(Outcome::Done(true));
            }
            let params = self
                .goal_defs
                .get(&id)
                .cloned()
                .unwrap_or_else(|| vec![(Some(c("goal_ID")), Value::Name(id))]);
            // Its activate script, as the game's goal manager runs it.
            let activate = params
                .iter()
                .find(|(k, _)| *k == Some(c("activate")))
                .and_then(|(_, v)| v.as_name());
            if let Some(script) = activate.filter(|s| program.has_script(*s)) {
                self.starting
                    .push((self.level_runner, Thread::new(script, params.clone())));
            }
            self.side_goals.insert(
                id,
                SideGoal {
                    params,
                    ..SideGoal::default()
                },
            );
            return Some(Outcome::Done(true));
        }
        if name == c("GoalManager_DeactivateGoal") || name == c("GoalManager_LoseGoal") {
            let id = goal?;
            if Some(id) == self.active_goal {
                // The goal played: lost.
                self.goal_lost = true;
                return Some(Outcome::Done(true));
            }
            let gone = self.side_goals.remove(&id)?;
            let deactivate = gone
                .params
                .iter()
                .find(|(k, _)| *k == Some(c("deactivate")))
                .and_then(|(_, v)| v.as_name());
            if let Some(script) = deactivate.filter(|s| program.has_script(*s)) {
                self.starting
                    .push((self.level_runner, Thread::new(script, gone.params)));
            }
            return Some(Outcome::Done(true));
        }
        if name == c("GoalManager_PauseAllGoals") || name == c("GoalManager_UnPauseAllGoals") {
            self.goal_paused = name == c("GoalManager_PauseAllGoals");
            return Some(Outcome::Done(true));
        }
        if (name == c("GoalManager_PauseGoal") || name == c("GoalManager_UnPauseGoal"))
            && goal.is_some()
            && goal == self.active_goal
        {
            self.goal_paused = name == c("GoalManager_PauseGoal");
            return Some(Outcome::Done(true));
        }
        if name == c("GoalManager_HasActiveGoals") {
            return Some(Outcome::Done(
                self.active_goal.is_some() || !self.side_goals.is_empty(),
            ));
        }
        if name == c("GoalManager_CheckDistanceRecord")
            && goal.is_some()
            && goal == self.active_goal
        {
            // How far the skater is from the goal's start object
            // (`distance_record_object`), across, in feet and inches, kept
            // in its settings with the best; whether this beats it.
            let from = self
                .goal_params
                .iter()
                .find(|(k, _)| *k == Some(c("distance_record_object")))
                .and_then(|(_, v)| v.as_name())
                .and_then(|n| self.object_named(n))
                .map(|o| self.states[o].position);
            let (Some(from), Some(skater)) = (from, self.skater) else {
                return Some(Outcome::Done(false));
            };
            let inches = (skater - from).with_y(0.0).length().round() as i32;
            let best = self
                .goal_params
                .iter()
                .find(|(k, _)| *k == Some(c("record_distance")))
                .and_then(|(_, v)| v.as_int())
                .unwrap_or(0);
            let record = inches > best;
            let best = best.max(inches);
            for (key, value) in [
                ("current_distance_feet", inches / 12),
                ("current_distance_inches", inches % 12),
                ("record_distance", best),
                ("record_feet", best / 12),
                ("record_inches", best % 12),
            ] {
                self.goal_params.retain(|(k, _)| *k != Some(c(key)));
                self.goal_params.push((Some(c(key)), Value::Integer(value)));
            }
            return Some(Outcome::Done(record));
        }
        if name == c("GoalManager_CheckMinigameRecord") {
            // A new best (`... Name = goal <number_collected>`)?
            let id = goal?;
            let value = match args {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Integer(i)) => Some(*i),
                    _ => None,
                }),
                _ => None,
            }?;
            let best = self.records.entry(id).or_insert(0);
            let new = value > *best;
            if new {
                *best = value;
            }
            return Some(Outcome::Done(new));
        }
        // A goal alongside's own flags, things and settings.
        if let Some(id) = side {
            let sg = self.side_goals.get_mut(&id)?;
            if name == c("GoalManager_GoalIsActive") {
                return Some(Outcome::Done(true));
            }
            if name == c("GoalManager_SetGoalFlag") || name == c("GoalManager_GoalFlagSet") {
                let (flag, value) = goal_flag_args(args);
                let flag = flag?;
                if name == c("GoalManager_GoalFlagSet") {
                    return Some(Outcome::Done(sg.flags.contains(&flag)));
                }
                if value != 0 {
                    sg.flags.insert(flag);
                } else {
                    sg.flags.remove(&flag);
                }
                return Some(Outcome::Done(true));
            }
            if name == c("GoalManager_GotCounterObject") {
                sg.count += 1;
                return Some(Outcome::Done(true));
            }
            if name == c("GoalManager_GetGoalParams") || name == c("GoalManager_GetNumberCollected")
            {
                let mut params = sg.params.clone();
                params.retain(|(k, _)| {
                    *k != Some(c("num_flags_set")) && *k != Some(c("number_collected"))
                });
                let collected = sg
                    .params
                    .iter()
                    .find(|(k, _)| *k == Some(c("number_collected")))
                    .and_then(|(_, v)| v.as_int())
                    .unwrap_or(0)
                    .max(sg.count as i32);
                params.push((
                    Some(c("num_flags_set")),
                    Value::Integer(sg.flags.len() as i32),
                ));
                params.push((Some(c("number_collected")), Value::Integer(collected)));
                return Some(Outcome::Params(params));
            }
            if name == c("GoalManager_EditGoal") {
                if let Some(Value::Struct(items)) = args.get(c("params")) {
                    for (k, v) in items.iter().filter(|(k, _)| k.is_some()) {
                        sg.params.retain(|(o, _)| o != k);
                        sg.params.push((*k, v.clone()));
                    }
                }
                return Some(Outcome::Done(true));
            }
            if name == c("GoalManager_WinGoal") {
                // Done: its success script, and it's over.
                let gone = self.side_goals.remove(&id)?;
                let success = gone
                    .params
                    .iter()
                    .find(|(k, _)| *k == Some(c("success")))
                    .and_then(|(_, v)| v.as_name());
                if let Some(script) = success.filter(|s| program.has_script(*s)) {
                    self.starting
                        .push((self.level_runner, Thread::new(script, gone.params)));
                }
                return Some(Outcome::Done(true));
            }
            return None;
        }
        // Another goal's settings (not on), as the level added it.
        if (name == c("GoalManager_GetGoalParams") || name == c("GoalManager_GetNumberCollected"))
            && goal.is_some()
            && goal != self.active_goal
            && goal != self.ended_goal
        {
            let mut params = self.goal_defs.get(&goal?)?.clone();
            params.push((Some(c("num_flags_set")), Value::Integer(0)));
            params.push((Some(c("number_collected")), Value::Integer(0)));
            return Some(Outcome::Params(params));
        }
        None
    }
}

/// What the skater's doing, as the level's scripts ask it
/// (`skater:OnGround`, `skater:IsInBail`, `skater:SpeedLessThan 900`...).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkaterState {
    pub on_ground: bool,
    pub in_air: bool,
    pub on_rail: bool,
    pub bailing: bool,
    pub skitching: bool,
    /// In the middle of a trick (an air trick, a grind, a manual...).
    pub doing_trick: bool,
    /// Its speed (in the game's units, inches a second).
    pub speed: f32,
    /// Which way it faces (radians about Y, as the skater's heading).
    pub heading: f32,
}

impl SkaterState {
    /// The answer to `skater:<name> <args>`, if it's a question about the
    /// skater.
    fn answer(&self, name: u32, args: &Value) -> Option<bool> {
        let c = checksum;
        let number = || match args {
            Value::Struct(items) => items.iter().find_map(|(k, v)| match k {
                None => v.as_f32(),
                _ => None,
            }),
            v => v.as_f32(),
        };
        Some(if name == c("OnGround") {
            self.on_ground
        } else if name == c("InAir") {
            self.in_air
        } else if name == c("OnRail") {
            self.on_rail
        } else if name == c("IsInBail") {
            self.bailing
        } else if name == c("Skitching") {
            self.skitching
        } else if name == c("doingtrick") {
            self.doing_trick
        } else if name == c("SpeedLessThan") {
            self.speed < number()?
        } else if name == c("SpeedGreaterThan") {
            self.speed > number()?
        } else {
            return None;
        })
    }
}

/// The host commands for one object's script.
/// The game's UI scripts, run on its screen rather than by the level's
/// scripts (they look at what they've made: `GetScreenElementDims`).
const UI_SCRIPTS: &[&str] = &[
    "create_speech_box",
    "speech_box_exit",
    "create_panel_block",
    "Create_Panel_Message",
    "create_panel_sprite",
    "goal_create_counter",
    "goal_update_counter",
    "Goal_Destroy_Counter",
    "destroy_goal_panel_messages",
    "create_dialog_box",
    "Dialog_Box_Exit",
    "kill_panel_message_if_it_exists",
    "hide_panel_message",
];

struct Commands<'a> {
    behaviour: &'a mut Behaviour,
    objects: &'a mut LevelObjects,
    object: usize,
    now: f32,
    /// The scripts and globals (the localized texts scripts name).
    program: &'a Program,
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
    fn is_self(&self, target: u32) -> bool {
        self.behaviour.object_named(target) == Some(self.object)
    }

    /// The game's UI scripts go to its screen, whole.
    fn calling(&mut self, script: u32, args: &Value) -> bool {
        if !UI_SCRIPTS.iter().any(|s| checksum(s) == script) {
            return false;
        }
        let b = &mut *self.behaviour;
        if b.screen_scripts.len() < 64 {
            // (A struct passed whole gives its fields, as for any call.)
            let mut params = Vec::new();
            if let Value::Struct(items) = args {
                for (k, v) in items {
                    match (k, v) {
                        (None, Value::Struct(fields)) => params.extend(fields.iter().cloned()),
                        item => params.push((*item.0, item.1.clone())),
                    }
                }
            }
            b.screen_scripts.push((script, params));
        }
        true
    }

    fn command(&mut self, target: Option<u32>, name: u32, args: &Value) -> Outcome {
        // About the skater: `skater:OnGround`, or a plain `OnRail` in what
        // the level runs for it (a trigger's script runs as the skater
        // that touched it: Hub's lighthouse counts laps grinding).
        let about_skater = target == Some(checksum("skater"))
            || (target.is_none() && self.object == self.behaviour.level_runner);
        if about_skater {
            if let Some(answer) = self.behaviour.skater_state.answer(name, args) {
                return Outcome::Done(answer);
            }
        }
        // `skater:KillSkater node = restart`: back at that restart, for
        // the viewer to put it there (Free Zazu's tries).
        if target == Some(checksum("skater"))
            && (name == checksum("Hide") || name == checksum("Unhide"))
        {
            self.behaviour.skater_hidden = name == checksum("Hide");
            return Outcome::Done(true);
        }
        if about_skater && name == checksum("KillSkater") {
            let node = args
                .get(checksum("node"))
                .or_else(|| args.get(checksum("nodename")))
                .and_then(Value::as_name);
            self.behaviour.skater_restart = node;
            return Outcome::Done(node.is_some());
        }
        let object = match target {
            Some(t) => match self.behaviour.object_named(t) {
                Some(o) => o,
                None => return Outcome::Done(false),
            },
            None => self.object,
        };
        let named = |key: &str| args.get(checksum(key)).and_then(Value::as_name);
        let c = |s: &str| checksum(s);
        let program = self.program;
        let b = &mut *self.behaviour;
        if target.is_none() {
            if let Some(outcome) = b.goal_command(program, name, args) {
                return outcome;
            }
        }

        // `object:script args`: the script run as that object, alongside
        // what it's doing (the wildebeest's `HerdWildebeast_Waiting`).
        if target.is_some() && self.program.has_script(name) {
            let params = match args {
                Value::Struct(items) => items.clone(),
                _ => Vec::new(),
            };
            b.spawning.push((object, Thread::new(name, params)));
            return Outcome::Done(true);
        }

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
                        state.slide = false;
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
                state.slide = false;
            }
        } else if name == c("Obj_MoveToPos") {
            // `Obj_MoveToPos (x, y, z) time = t seconds`: to a spot, over
            // that long, facing as it was. (These come from the levels'
            // animation exports, already in mesh space: Pizza's alien
            // heads move to where their nodes put them with Z unmirrored.)
            let to = match args {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Vector(v)) => Some(Vec3::from(*v)),
                    _ => None,
                }),
                _ => None,
            };
            if let Some(to) = to {
                let time = args.get(c("time")).and_then(Value::as_f32).unwrap_or(0.0);
                let state = &mut b.states[object];
                let distance = state.position.distance(to);
                if time <= 0.0 || distance < 1e-3 {
                    state.position = to;
                    state.moving = None;
                    state.dirty = true;
                } else {
                    state.moving = Some((to, distance / time, false));
                    state.slide = true;
                }
            }
        } else if name == c("Obj_Rotate") {
            // `Obj_Rotate absolute = (axis) axis_angle = degrees time = t`:
            // to that orientation in the world, over that long. The axis
            // is the animation export's, Z up: Pizza's alien heads, placed
            // turned half round, start at about 180 degrees round (0, 0,
            // 1). In mesh space that's about Y, the other way round, as
            // the nodes' headings are.
            let axis = match args.get(c("absolute")) {
                Some(Value::Vector([x, y, z])) => Vec3::new(*x, *z, *y).normalize_or_zero(),
                _ => Vec3::ZERO,
            };
            if axis != Vec3::ZERO {
                let angle = args
                    .get(c("axis_angle"))
                    .and_then(Value::as_f32)
                    .unwrap_or(0.0)
                    .to_radians();
                let time = args.get(c("time")).and_then(Value::as_f32).unwrap_or(0.0);
                let state = &mut b.states[object];
                let to = Quat::from_axis_angle(axis, -angle);
                if time > 0.0 {
                    state.turn = Some((state.rotation, to, 0.0, time));
                } else {
                    state.rotation = to;
                    state.dirty = true;
                }
            }
        } else if name == c("Obj_Invisible") || name == c("Obj_Visible") {
            let state = &mut b.states[object];
            state.invisible = name == c("Obj_Invisible");
            state.dirty = true;
        } else if name == c("Obj_StorePos") {
            let state = &mut b.states[object];
            state.stored = Some(state.position);
        } else if name == c("Obj_LookAtPosStored") {
            // Turned to face the spot (all the way: a missile aims up or
            // down at it too), over `time` seconds.
            let state = &mut b.states[object];
            if let Some(at) = state.stored {
                let along = (at - state.position).normalize_or_zero();
                if along != Vec3::ZERO {
                    let to = Quat::from_rotation_arc(Vec3::Z, along);
                    let time = args.get(c("time")).and_then(Value::as_f32).unwrap_or(0.0);
                    if time > 0.0 {
                        state.turn = Some((state.rotation, to, 0.0, time));
                    } else {
                        state.rotation = to;
                        state.dirty = true;
                    }
                }
            }
        } else if name == c("Obj_MoveToPosStored") {
            // Off to the spot at `speed` (mph), facing as it is.
            let speed = args.get(c("speed")).and_then(Value::as_f32).unwrap_or(0.0);
            let state = &mut b.states[object];
            if let Some(to) = state.stored {
                if speed > 0.0 {
                    state.moving = Some((to, speed * MPH, false));
                    state.slide = true;
                } else {
                    state.position = to;
                    state.moving = None;
                    state.dirty = true;
                }
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
        } else if name == c("SendException") {
            // `SendException 1 Name = obj`: that object's exception of
            // that number (or name) run, as an event would.
            let ex = match args {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Integer(i)) => Some(*i as u32),
                    (None, Value::Name(n)) => Some(*n),
                    _ => None,
                }),
                _ => None,
            };
            let on = named("Name").and_then(|n| b.object_named(n));
            if let (Some(ex), Some(on)) = (ex, on) {
                let found = b.states[on]
                    .exceptions
                    .iter()
                    .find(|(e, ..)| *e == ex)
                    .map(|(_, script, params)| (*script, params.clone()));
                if let Some((script, params)) = found {
                    b.starting.push((on, Thread::new(script, params)));
                }
            }
        } else if name == c("Obj_SetException") || name == c("SetException") {
            // (Its event by name, or a number that `SendException` sends.)
            let ex =
                named("ex").or_else(|| args.get(c("ex")).and_then(Value::as_int).map(|i| i as u32));
            if let (Some(ex), Some(scr)) = (ex, named("scr")) {
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
        } else if name == c("MakeSkaterGoto") {
            // The skater's own scripts aren't run; a knock down is a bail.
            let to = match args {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Name(n)) => Some(*n),
                    _ => None,
                }),
                _ => None,
            };
            if to == Some(c("PedKnockDown")) {
                b.knock_down = true;
            } else {
                *b.unknown.entry(name).or_default() += 1;
                return Outcome::Done(false);
            }
        } else if name == c("Obj_StopSound") {
            // (Objects' sounds play once here, loops too: nothing to stop.)
        } else if name == c("Shatter") || name == c("ShatterAndDie") {
            if let Some(n) = named("Name") {
                if b.shatters.len() < 64 {
                    b.shatters.push(n);
                }
            }
        } else if name == c("wibbleVC") {
            if let Some(sector) = named("Name") {
                let now = b.now;
                let clock = b.color_clocks.entry(sector).or_insert((now, true));
                if args.has_flag(c("reset")) {
                    clock.0 = 0.0;
                }
                if args.has_flag(c("On")) {
                    clock.1 = true;
                } else if args.has_flag(c("off")) {
                    clock.1 = false;
                }
            }
        } else if name == c("KillSpawnedScript") {
            if let Some(script) = named("Name") {
                b.killing.push(script);
            }
        } else if name == c("TerminateObjectsScripts") {
            if let Some(o) = named("id").and_then(|n| b.object_named(n)) {
                b.terminating.push(o);
            }
        } else if name == c("Obj_GetID") {
            // Its own name, as `objId`.
            let id = b
                .by_name
                .iter()
                .find(|(_, o)| **o == object)
                .map(|(n, _)| *n);
            return match id {
                Some(id) => Outcome::Params(vec![(Some(c("objId")), Value::Name(id))]),
                None => Outcome::Done(false),
            };
        } else if name == c("GetSkaterId") {
            return Outcome::Params(vec![(Some(c("objId")), Value::Name(c("skater")))]);
        } else if name == c("Obj_LockToObject") {
            // Carried along by the skater or another object, at an offset
            // (or let go: `off`).
            let state_off = args.has_flag(c("off"));
            let to = match named("ObjectName") {
                Some(n) if n == c("skater") => Some(None),
                Some(n) => b.object_named(n).map(Some),
                None => None,
            };
            let offset = match args {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Vector([x, y, z])) => Some(Vec3::new(*x, *y, *z)),
                    _ => None,
                }),
                _ => None,
            }
            .unwrap_or(Vec3::ZERO);
            b.states[object].locked = match (state_off, to) {
                (false, Some(to)) => Some((to, offset)),
                _ => None,
            };
        } else if name == c("obj_applyscaling") {
            if let Some(k) = args.get(c("scale")).and_then(Value::as_f32) {
                b.states[object].scale = k;
                b.states[object].dirty = true;
            }
        } else if name == c("Obj_RotY") || name == c("Obj_RotX") || name == c("Obj_RotZ") {
            // `Obj_RotY speed = 250`: spinning (degrees a second); with
            // `angle = -95`, turning that far at that speed and stopping
            // (Hub's doors, Camp's tent flaps).
            let axis = if name == c("Obj_RotX") {
                Vec3::X
            } else if name == c("Obj_RotZ") {
                Vec3::Z
            } else {
                Vec3::Y
            };
            let speed = args.get(c("speed")).and_then(Value::as_f32).unwrap_or(0.0);
            let state = &mut b.states[object];
            match args.get(c("angle")).and_then(Value::as_f32) {
                Some(angle) => {
                    let to = state.rotation * Quat::from_axis_angle(axis, angle.to_radians());
                    let time = if speed.abs() > 0.0 {
                        (angle / speed).abs()
                    } else {
                        0.0
                    };
                    state.spin = 0.0;
                    if time > 0.0 {
                        state.turn = Some((state.rotation, to, 0.0, time));
                    } else {
                        state.rotation = to;
                        state.dirty = true;
                    }
                }
                None => {
                    state.spin = speed.to_radians();
                    state.spin_axis = axis;
                }
            }
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
        } else if name == c("midgoalvoiceover")
            || name == c("obj_playstream")
            || name == c("PlayStream")
        {
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
            // (`Type = ped`: any pedestrian that near: Beach's cage
            // dropped on Clayton.)
            let at = b.states[object].position;
            let near = if named("Type") == Some(c("ped")) {
                (0..b.peds.len()).any(|o| {
                    o != object
                        && b.peds[o]
                        && b.states[o].alive
                        && b.states[o].position.distance(at) < radius
                })
            } else {
                b.skater.is_some_and(|s| s.distance(at) < radius)
            };
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
        } else if name == c("GoalManager_GetProsetNotPrefix") {
            // The other pro sets' pieces, hidden while a goal's are shown
            // (`goal_start`: `kill prefix = <proset_not_prefix>`, and
            // "TRG_" + it). The levels have none: a prefix nothing has.
            // (Unanswered, "TRG_" + nothing killed every trigger.)
            return Outcome::Params(vec![(
                Some(c("proset_not_prefix")),
                Value::String("proset_not_none_".into()),
            )]);
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
        } else if name == c("PlaySkaterCamAnim") {
            // A camera path played (not `STOP`ped), unless it's the goal's
            // own (`virtual_cam`, made up round the pro).
            let path = named("Name");
            // (Only a goal's: the one on, or just over.)
            let goal = b.active_goal.is_some() || b.ended_goal.is_some();
            if args.has_flag(c("STOP")) {
                b.cameras.clear();
                b.camera_stop = true;
            }
            if let (Some(path), false, false, true) = (
                path,
                args.has_flag(c("STOP")),
                args.has_flag(c("virtual_cam")),
                goal,
            ) {
                if b.cameras.len() < 8 {
                    b.cameras.push(path);
                }
            }
        } else if name == c("KillSkaterCamAnim") {
            // (Any asked for before it this frame go too.)
            b.cameras.clear();
            b.camera_stop = true;
        } else if name == c("SkaterCamAnimFinished") {
            // (The viewer plays them as cutscenes; scripts needn't wait.)
            return Outcome::Done(true);
        } else if name == c("ScreenElementExists") || name == c("ObjectExists") {
            // One the scripts made (a 3D arrow, a panel message), or an
            // object that's there.
            let id = named("id");
            return Outcome::Done(id.is_some_and(|i| {
                b.screen_ids.contains(&i) || b.object_named(i).is_some_and(|o| b.states[o].alive)
            }));
        } else if name == c("Create3dArrowPointer") {
            // (A 3D model on the HUD: not drawn, but there to ask about:
            // Tarzan's fruit go back by theirs.)
            if let Some(id) = named("id") {
                b.screen_ids.insert(id);
            }
        } else if [
            "SetScreenElementProps",
            "DoScreenElementMorph",
            "DestroyScreenElement",
            "RunScriptOnScreenElement",
            "AssignAlias",
        ]
        .iter()
        .any(|n| name == c(n))
        {
            // For the game's screen.
            if name == c("DestroyScreenElement") {
                if let Some(id) = named("id") {
                    b.screen_ids.remove(&id);
                }
            }
            if b.screen_calls.len() < 256 {
                b.screen_calls.push((name, args.clone()));
            }
        } else if name == c("CreateScreenElement") {
            // Passed on with an id (one made up if it has none), handed
            // back as the game does, for the script's next steps (its
            // style run on it).
            let mut forwarded = args.clone();
            let given = args
                .get(c("id"))
                .or_else(|| match args {
                    Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                        (None, inner @ Value::Struct(_)) => inner.get(c("id")),
                        _ => None,
                    }),
                    _ => None,
                })
                .and_then(Value::as_name)
                .filter(|id| *id != 0);
            let id = given.unwrap_or_else(|| {
                b.next_screen_id += 1;
                checksum(&format!("level_screen_element_{}", b.next_screen_id))
            });
            if given.is_none() {
                if let Value::Struct(items) = &mut forwarded {
                    items.push((Some(c("id")), Value::Name(id)));
                }
            }
            if b.screen_calls.len() < 256 {
                b.screen_calls.push((name, forwarded));
            }
            b.screen_ids.insert(id);
            let made = id;
            // Text put on screen (a panel message): its words, the goal's
            // own text block aside (the viewer shows that already). (Its
            // settings come in a struct of their own.)
            let args = match args {
                Value::Struct(items) => items
                    .iter()
                    .find_map(|(k, v)| match (k, v) {
                        (None, inner @ Value::Struct(_)) => Some(inner),
                        _ => None,
                    })
                    .unwrap_or(args),
                _ => args,
            };
            let named = |key: &str| args.get(checksum(key)).and_then(Value::as_name);
            let kind = args.get(c("Type")).and_then(Value::as_name);
            let text_kind = kind == Some(c("TextElement")) || kind == Some(c("TextBlockElement"));
            let goal_block = named("id") == Some(c("current_goal"));
            let text = match args.get(c("text")) {
                Some(Value::String(s) | Value::LocalString(s)) => Some(s.clone()),
                Some(Value::Name(n)) => match self.program.value(*n) {
                    Some(Value::String(s) | Value::LocalString(s)) => Some(s.clone()),
                    _ => None,
                },
                _ => None,
            };
            if let (true, false, Some(text)) = (text_kind, goal_block, text) {
                if !text.trim().is_empty() {
                    // (Kept few, for when nobody's taking them.)
                    if b.messages.len() >= 8 {
                        b.messages.remove(0);
                    }
                    b.messages.push(text);
                }
            }
            return Outcome::Params(vec![(Some(c("id")), Value::Name(made))]);
        } else if name == c("SendFlag") || name == c("QueryFlag") || name == c("ClearFlag") {
            // An object's flags, by `Name = object`: `SendFlag Name = obj
            // flag` sets one, `QueryFlag flag Name = obj` asks. (Or every
            // object whose name starts so: `prefix = "TRG_Zurg_SpineChecker"`.)
            let mut on = vec![
                named("Name")
                    .and_then(|n| b.object_named(n))
                    .unwrap_or(object),
            ];
            if let Some(Value::String(p) | Value::LocalString(p)) = args.get(c("prefix")) {
                let p = p.to_ascii_lowercase();
                on = b
                    .labels
                    .iter()
                    .filter(|(_, l)| l.starts_with(&p))
                    .filter_map(|(n, _)| b.object_named(*n))
                    .collect();
            }
            let flag = match args {
                Value::Struct(items) => items.iter().find_map(|(k, v)| match (k, v) {
                    (None, Value::Name(f)) => Some(*f),
                    _ => None,
                }),
                _ => None,
            };
            let Some(flag) = flag else {
                return Outcome::Done(false);
            };
            if name == c("QueryFlag") {
                return Outcome::Done(on.iter().any(|&o| b.states[o].flags.contains(&flag)));
            }
            for o in on {
                let flags = &mut b.states[o].flags;
                if name == c("SendFlag") {
                    if !flags.contains(&flag) {
                        flags.push(flag);
                    }
                } else {
                    flags.retain(|f| *f != flag);
                }
            }
        } else if name == c("GoalManager_CanStartGoal") {
            // The goal on starts (`goal_start` then makes its things); with
            // no goal named, whether a goal's cutscene can play now (yes:
            // Graveyard's happy skulls wait on it).
            return Outcome::Done(match named("Name") {
                Some(goal) => Some(goal) == b.active_goal,
                None => true,
            });
        } else if name == c("GoalManager_GetGoalParams")
            || name == c("GoalManager_GetNumberCollected")
        {
            // The goal on's settings, with how far it's got.
            let ours = named("Name").is_some()
                && (named("Name") == b.active_goal || named("Name") == b.ended_goal);
            if !ours {
                return Outcome::Done(false);
            }
            let mut params = b.goal_params.clone();
            params.retain(|(k, _)| {
                *k != Some(c("num_flags_set")) && *k != Some(c("number_collected"))
            });
            params.push((
                Some(c("num_flags_set")),
                Value::Integer(b.goal_flags.len() as i32),
            ));
            params.push((
                Some(c("number_collected")),
                Value::Integer(b.goal_count.max(b.goal_collected) as i32),
            ));
            return Outcome::Params(params);
        } else if name == c("GoalManager_EditGoal") {
            let ours = named("Name").is_some()
                && (named("Name") == b.active_goal || named("Name") == b.ended_goal);
            if let (true, Some(Value::Struct(items))) = (ours, args.get(c("params"))) {
                for (k, v) in items.iter().filter(|(k, _)| k.is_some()) {
                    // How many it's got, kept as its count; and how many
                    // it needs.
                    if *k == Some(c("number_collected")) {
                        b.goal_collected = v.as_int().unwrap_or(0).max(0) as usize;
                    }
                    if *k == Some(c("num_flags_to_win")) && b.active_goal.is_some() {
                        b.goal_needed = v.as_int().unwrap_or(0).max(0) as usize;
                    }
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
            if let (None, Some((_, _, t, length))) = (state.moving, state.turn) {
                return Outcome::Wait((length - t).max(0.0));
            }
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
                if b.bouncies.contains(&o) {
                    b.other_creates.push((target, create));
                }
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
