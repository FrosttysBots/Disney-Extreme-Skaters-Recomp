//! A skater rolling over a level's collision.
//!
//! This follows the game's own update code in `main.dol`, as far as it's
//! been read (see `docs/physics-notes.md`), with the game's constants
//! (see `constants`): pushing, drag, gravity along the ground, the velocity
//! kept along the board (it can roll backwards), the speed limits,
//! steering, braking, the ollie, following the ground, walls on the ground
//! and in the air, the air step, landing and the ledge pop, grinding, vert
//! air, and manuals and grinds balanced on the game's meter (see
//! `balance`). Not yet checked frame by frame against the running game,
//! and without lips, transfers, tricks, moving objects and the game's
//! events (wall rides go unused in this game).
//!
//! - **On the ground** the skater follows the surface as the game does
//!   (0x800F52AC): each step it moves along its heading, then looks down
//!   from `ground_snap_up` above, and stands on the ground there if it
//!   isn't falling away more than `ground_stick_angle` ahead and is within
//!   `ground_snap_down` (more at a sharp change of slope).
//!   Pushing accelerates up to the kick speed (higher when crouched),
//!   braking slows, slopes pull with `ground_gravity`, and steering turns
//!   at `ground_rotation`. No ground means it rolled off an edge.
//! - **Jumping**: crouch, then let go to ollie at `jump_speed` off the
//!   ground's normal.
//! - **In the air** gravity is `air_gravity` over the hang-time stat, the
//!   exact step of a thrown body as in the game, and steering spins at
//!   `air_rotation`; walls work as the game's (0x800F847C): the speed into
//!   them is lost, a tenth of the rest pushes away, and the skater turns
//!   along the wall and is put `min_distance_to_wall` out. Landing is the
//!   game's too: a line along the move; ground (by the face's flags and
//!   slope, so vert ramps count) keeps the speed along it and the skater's
//!   facing, walls slide it along them at full speed, and ledges it nearly
//!   cleared pop it up onto them (`air_snap_up`). Off the lip of a vert
//!   ramp it flies in the ramp's plane and comes back down onto it; landing on ground facing up enough (or on a ramp's
//!   vert face) puts it back on the ground, keeping the speed along the
//!   surface. Anything else it would pass through stops it instead.
//! - **Walls** on the ground are the game's (0x800F7D38): a line at knee
//!   height along the move; a wall turns the skater away by 1.1 times the
//!   angle it hit at, slows it the more head-on it was, and puts it 6 units
//!   out; skatable ground ahead (a ramp's curve) is stepped onto.

use std::f32::consts::{FRAC_PI_2, PI};

use glam::{Mat4, Quat, Vec3};

use crate::balance::{Balance, Lean, METER};
use crate::constants::Physics;
use crate::rails::RailHit;
use crate::score::Combo;
use crate::tricks::{Button, Dir, Kind, LipTrick, TrickBook};
use crate::world::{Hit, World};

/// The controls held this step.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Input {
    pub push: bool,
    pub brake: bool,
    /// -1 (left) to 1 (right).
    pub turn: f32,
    /// Crouching; letting go jumps.
    pub crouch: bool,
    /// Grind: get onto a rail within reach.
    pub grind: bool,
    /// The trick buttons: Square (flip tricks) and Circle (grabs).
    pub flip: bool,
    pub grab: bool,
}

/// What the skater is doing, for picking animations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Standing,
    Rolling,
    Pushing,
    Crouching,
    /// In the air, after an ollie or off an edge.
    Air,
    /// Just landed.
    Landing,
    /// Bounced off a wall fast enough to flail (turning left or right).
    FlailLeft,
    FlailRight,
    /// On a rail.
    Grinding,
    /// Balancing on two wheels.
    Manual,
    /// Fallen off a manual (the game's `BailManual`) or a rail
    /// (`BailGrind`).
    BailManual,
    BailGrind,
    /// Landed in the middle of a trick.
    Bail,
    /// Stalled on the coping of a quarter pipe.
    Lip,
}

/// A lip trick being held: the trick, the ramp's way out, and how long.
#[derive(Clone, Debug, PartialEq)]
pub struct Lip {
    pub trick: LipTrick,
    /// The ramp's normal, flattened: out of the ramp.
    pub out: Vec3,
    pub time: f32,
}

/// A trick being played: which, how far into its animation, and where.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Playing {
    pub trick: usize,
    /// Seconds into the animation.
    pub time: f32,
    pub phase: Phase,
    /// Real seconds since the trick began, and whether it has scored.
    age: f32,
    credited: bool,
}

/// A grab's way in, hold and way out (flips only go in).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    In,
    Hold,
    Out,
}

/// A press that can be part of a special's combination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Press {
    Dir(Dir),
    Flip,
    Grind,
}

/// The special meter: full at 3000 (`0xBB8` in the score object), draining
/// 50 a second, or 200 while full (0x800AFCF0).
const SPECIAL_FULL: f32 = 3000.0;
const SPECIAL_DRAIN: f32 = 50.0;
const SPECIAL_DRAIN_FULL: f32 = 200.0;
/// The window for a special's three presses (`TripleInOrder ... 400`).
const SPECIAL_WINDOW: f32 = 0.4;

/// A finished combo: its tricks and what it scored (nothing if it ended in
/// a bail).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Landed {
    pub combo: Combo,
    pub total: u32,
    pub bailed: bool,
}

/// Vert air: the ramp's normal, flattened (out of the ramp), and where
/// along it the skater flies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VertAir {
    pub out: Vec3,
    pub offset: f32,
}

/// Which rail segment the skater is grinding, which way along it, and
/// how fast.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grind {
    pub segment: usize,
    /// Towards the segment's end.
    pub forwards: bool,
    pub speed: f32,
}

/// Fixed physics step.
const STEP: f32 = 1.0 / 60.0;

/// How long a bail lasts before the skater can go again (this crate's
/// stand-in for the game's bail and get-up animations).
const BAIL_TIME: f32 = 1.5;

/// Points a frame for grinding (`Grind GrindTweak = 7` in
/// `grindscripts.q`, 36 for specials) and balancing a manual
/// (`DoBalanceTrick ... Tweak = 1`, 5 for specials).
const GRIND_TWEAK: u32 = 7;

/// How long a press of the grind button keeps looking for a rail.
const GRIND_WINDOW: f32 = 0.5;
const MANUAL_TWEAK: u32 = 1;
const SPECIAL_MANUAL_TWEAK: u32 = 5;
/// Points a frame on a lip (`LipMacro2`'s `TweakTrick 10`).
const LIP_TWEAK: u32 = 10;

/// The window for the manual's up-down press (`{ inorder, Up, Down, 400 }`
/// in the game's `manualtricks.q`).
const MANUAL_WINDOW: f32 = 0.4;

pub struct Skater {
    pub position: Vec3,
    pub velocity: Vec3,
    /// Heading around the vertical, in radians; 0 faces +Z.
    pub heading: f32,
    /// The surface it stands on (up when in the air).
    pub up: Vec3,
    /// `up` eased for drawing: the game tilts the skater's model over to a
    /// new slope gradually (`Normal_Lerp_Speed`) while its physics uses
    /// the new normal at once. The easing here is this crate's.
    pub shown_up: Vec3,
    pub on_ground: bool,
    pub action: Action,
    /// Seconds in the current action.
    pub action_time: f32,
    crouched: bool,
    /// How long a turn has been held, in seconds.
    turn_time: f32,
    /// How long the skater has been crouched (tensing for an ollie).
    crouch_time: f32,
    pub grind: Option<Grind>,
    /// Seconds since it last left a rail.
    since_rail: f32,
    /// Where it last stood on the ground: the nearest spawn to it is where
    /// it goes back to if it falls out of the level.
    last_ground: Vec3,
    /// The level's spawn points (position and heading), for that.
    pub spawns: Vec<(Vec3, f32)>,
    /// Print why the ground was lost, for debugging (off by default).
    pub trace: bool,
    /// The flags of the face it stands on.
    ground_flags: u16,
    /// In vert air: launched off a vert ramp, held in the ramp's vertical
    /// plane so it comes back down onto it.
    pub vert: Option<VertAir>,
    /// The balance meter, for manuals and grinds.
    pub balance: Balance,
    /// In a manual.
    pub manual: bool,
    /// In a combo: balance tricks carry their lean over (from one
    /// balance trick until landing without one).
    combo: bool,
    /// Seconds since up and down were last pressed, for the manual's
    /// up-down (or down-up) press.
    since_up: f32,
    since_down: f32,
    /// Seconds since the grind button was pressed.
    since_grind: f32,
    last_input: Input,
    /// Falling off a rail: bail on landing.
    bail_on_landing: bool,
    /// The character's tricks (empty until given).
    pub tricks: TrickBook,
    /// The air trick being played.
    pub trick: Option<Playing>,
    /// The trick buttons pressed this step.
    pressed: (bool, bool),
    /// The combo so far, the last one finished, and the points banked.
    pub combo_tricks: Combo,
    pub last_combo: Option<Landed>,
    /// Degrees turned in the air since leaving the ground or a rail.
    air_spin: f32,
    /// The special meter (up to 3000) and whether it's full (specials
    /// allowed until it drains).
    pub special_meter: f32,
    pub special: bool,
    /// The combo total last frame, to fill the meter by what it gained.
    last_total: u32,
    /// In the special manual.
    pub special_manual: bool,
    /// On a lip.
    pub lip: Option<Lip>,
    /// The lip trick's way out, playing in the air after it.
    pub lip_out: Option<(u32, f32)>,
    /// Recent presses, for the specials' three-press combinations, with
    /// the clock they're timed by.
    presses: Vec<(Press, f32)>,
    clock: f32,
    pub score: u32,
    leftover: f32,
}

impl Skater {
    pub fn new(position: Vec3, heading: f32) -> Self {
        Skater {
            position,
            velocity: Vec3::ZERO,
            heading,
            up: Vec3::Y,
            shown_up: Vec3::Y,
            on_ground: true,
            action: Action::Standing,
            action_time: 0.0,
            crouched: false,
            turn_time: 0.0,
            crouch_time: 0.0,
            grind: None,
            since_rail: f32::MAX,
            last_ground: position,
            spawns: Vec::new(),
            trace: false,
            ground_flags: 0,
            vert: None,
            balance: Balance::default(),
            manual: false,
            combo: false,
            since_up: f32::MAX,
            since_down: f32::MAX,
            since_grind: f32::MAX,
            last_input: Input::default(),
            bail_on_landing: false,
            tricks: TrickBook::default(),
            trick: None,
            pressed: (false, false),
            combo_tricks: Combo::default(),
            last_combo: None,
            air_spin: 0.0,
            special_meter: 0.0,
            special: false,
            last_total: 0,
            special_manual: false,
            lip: None,
            lip_out: None,
            presses: Vec::new(),
            clock: 0.0,
            score: 0,
            leftover: 0.0,
        }
    }

    /// The trick animation to show: its name's checksum, the time into it,
    /// and whether it loops (a grab's hold).
    pub fn trick_pose(&self) -> Option<(u32, f32, bool)> {
        let playing = self.trick?;
        let trick = self.tricks.tricks.get(playing.trick)?;
        Some(match playing.phase {
            Phase::Hold => (trick.idle.unwrap_or(trick.anim), playing.time, true),
            _ => (trick.anim, playing.time, false),
        })
    }

    /// Whether landing now would be a bail: in a trick that isn't nearly
    /// over (the scripts' `BailOn` until `trickslack` frames from the end).
    pub fn bail_on(&self) -> bool {
        let Some(playing) = self.trick else {
            return false;
        };
        let Some(trick) = self.tricks.tricks.get(playing.trick) else {
            return false;
        };
        let slack = trick.trickslack / 60.0;
        match playing.phase {
            Phase::In if trick.kind == Kind::Flip => {
                playing.time < self.tricks.duration(trick.anim) - slack
            }
            Phase::In | Phase::Hold => true,
            Phase::Out => playing.time > slack,
        }
    }

    /// A trick into the combo; grinds and manuals block spins.
    fn credit(&mut self, trick: Option<(String, u32)>, block_spin: bool) {
        if let Some((name, score)) = trick {
            self.combo_tricks.add(&name, score, block_spin);
        }
        if block_spin {
            self.air_spin = 0.0;
        }
    }

    /// The combo's over: banked if landed (the game's scoring, see
    /// `score`), lost in a bail.
    fn end_combo(&mut self, landed: bool) {
        self.combo = false;
        self.air_spin = 0.0;
        self.last_total = 0;
        // A bail empties the special meter.
        if !landed {
            self.special_meter = 0.0;
            self.special = false;
        }
        if self.combo_tricks.is_empty() {
            return;
        }
        let combo = std::mem::take(&mut self.combo_tricks);
        let total = if landed { combo.total() } else { 0 };
        self.score += total;
        self.last_combo = Some(Landed {
            combo,
            total,
            bailed: !landed,
        });
    }

    /// Air tricks: start one on a button press (when none is playing, or
    /// the last is past its bail), and play it on.
    fn air_tricks(&mut self, input: Input) {
        let (flip, grab) = self.pressed;
        let free = self.trick.is_none() || !self.bail_on();
        if (flip || grab) && free {
            let button = if flip { Button::Flip } else { Button::Grab };
            let dir = Dir::from_held(input.push, input.brake, input.turn < 0.0, input.turn > 0.0);
            // With the meter full, the special's combination does it.
            let special = self
                .tricks
                .special_air
                .filter(|&(a, b, _)| {
                    self.special && flip && self.pressed_in_order(a, b, Press::Flip)
                })
                .map(|(_, _, i)| i);
            if let Some(index) = special.or_else(|| self.tricks.air_trick(button, dir)) {
                self.trick = Some(Playing {
                    trick: index,
                    time: 0.0,
                    phase: Phase::In,
                    age: 0.0,
                    credited: false,
                });
            }
        }
        let Some(mut playing) = self.trick else {
            return;
        };
        let trick = self.tricks.tricks[playing.trick].clone();
        let length = self.tricks.duration(trick.anim);
        let step = STEP * trick.speed;
        playing.age += STEP;
        let done = match (trick.kind, playing.phase) {
            (Kind::Flip, _) => {
                playing.time += step;
                // `FlipTrick` names it after 15 frames.
                if !playing.credited && playing.age >= 15.0 / 60.0 {
                    playing.credited = true;
                    self.credit(Some((trick.name.clone(), trick.score)), false);
                }
                playing.time >= length
            }
            (Kind::Grab, Phase::In) => {
                playing.time += step;
                // `GrabTrick`: named halfway in; let go after 60% to come
                // out; held to the end, it holds.
                if !playing.credited && playing.time >= length * 0.5 {
                    playing.credited = true;
                    self.credit(Some((trick.name.clone(), trick.score)), false);
                }
                if playing.time >= length * 0.6 && !input.grab {
                    playing.phase = Phase::Out;
                } else if playing.time >= length {
                    playing.phase = Phase::Hold;
                    playing.time = 0.0;
                }
                false
            }
            (Kind::Grab, Phase::Hold) => {
                playing.time += step;
                // Held: `TweakTrick <GrabTweak>` every frame.
                self.combo_tricks.tweak(trick.tweak);
                if !input.grab {
                    playing.phase = Phase::Out;
                    playing.time = length;
                }
                false
            }
            (Kind::Grab, Phase::Out) => {
                // Back out: the way in, backwards.
                playing.time -= step;
                playing.time <= 0.0
            }
        };
        self.trick = (!done).then_some(playing);
    }

    /// The balance meter from -1 to 1 while balancing a manual or a grind.
    pub fn balance_meter(&self) -> Option<f32> {
        (self.manual || self.grind.is_some() || self.lip.is_some())
            .then(|| (self.balance.angle / METER).clamp(-1.0, 1.0))
    }

    pub fn forward(&self) -> Vec3 {
        Vec3::new(self.heading.sin(), 0.0, self.heading.cos())
    }

    /// Speed along the ground (or horizontally in the air).
    pub fn speed(&self) -> f32 {
        if self.on_ground {
            self.velocity.length()
        } else {
            Vec3::new(self.velocity.x, 0.0, self.velocity.z).length()
        }
    }

    /// Where to draw the skater: standing on `up`, facing its heading.
    pub fn placement(&self) -> Mat4 {
        let yaw = Quat::from_rotation_y(self.heading);
        let tilt = Quat::from_rotation_arc(Vec3::Y, self.shown_up.normalize_or(Vec3::Y));
        Mat4::from_rotation_translation(tilt * yaw, self.position)
    }

    /// Advances `dt` seconds in fixed steps.
    pub fn update(&mut self, input: Input, physics: &Physics, world: &World, dt: f32) {
        self.leftover += dt.min(0.25);
        while self.leftover >= STEP {
            self.leftover -= STEP;
            self.step(input, physics, world);
        }
    }

    /// Off a wall in the air (0x800F847C): the speed into the wall is lost
    /// (the vertical speed kept unless the wall faces down), a tenth of
    /// the speed left pushes away from it, and the skater turns to face
    /// along it, slightly away.
    fn air_bounce(&mut self, normal: Vec3) {
        let keep_rise = normal.y > -0.1;
        let rise = self.velocity.y;
        if keep_rise {
            self.velocity.y = 0.0;
        }
        self.velocity -= normal * self.velocity.dot(normal);
        self.velocity += normal * (self.velocity.length() / 10.0);
        if keep_rise {
            self.velocity.y = rise;
        }
        let flat = self.forward();
        let facing = flat - normal * flat.dot(normal) + normal * 0.05;
        if facing.x != 0.0 || facing.z != 0.0 {
            self.heading = facing.x.atan2(facing.z);
        }
    }

    fn set_action(&mut self, action: Action) {
        if self.action != action {
            self.action = action;
            self.action_time = 0.0;
        }
    }

    fn step(&mut self, input: Input, p: &Physics, world: &World) {
        self.shown_up = self.shown_up.lerp(self.up, 0.25).normalize_or(Vec3::Y);
        self.action_time += STEP;
        self.crouch_time = if input.crouch {
            self.crouch_time + STEP
        } else if self.on_ground || self.grind.is_some() {
            self.crouch_time
        } else {
            0.0
        };
        self.turn_time = if input.turn == 0.0 {
            0.0
        } else {
            self.turn_time + STEP
        };
        self.since_up = if input.push && !self.last_input.push {
            0.0
        } else {
            self.since_up + STEP
        };
        self.since_down = if input.brake && !self.last_input.brake {
            0.0
        } else {
            self.since_down + STEP
        };
        self.clock += STEP;
        if !self.manual {
            self.special_manual = false;
        }
        let last = self.last_input;
        let edges = [
            (input.push && !last.push, Press::Dir(Dir::Up)),
            (input.brake && !last.brake, Press::Dir(Dir::Down)),
            (input.turn < 0.0 && last.turn >= 0.0, Press::Dir(Dir::Left)),
            (input.turn > 0.0 && last.turn <= 0.0, Press::Dir(Dir::Right)),
            (input.flip && !last.flip, Press::Flip),
            (input.grind && !last.grind, Press::Grind),
        ];
        for (pressed, press) in edges {
            if pressed {
                self.presses.push((press, self.clock));
            }
        }
        let clock = self.clock;
        self.presses.retain(|&(_, t)| clock - t <= 1.0);
        self.since_grind = if input.grind && !self.last_input.grind {
            0.0
        } else {
            self.since_grind + STEP
        };
        self.pressed = (
            input.flip && !self.last_input.flip,
            input.grab && !self.last_input.grab,
        );
        self.last_input = input;
        // Bailing: no control until it's over.
        let input = if matches!(
            self.action,
            Action::BailManual | Action::BailGrind | Action::Bail
        ) {
            if self.action_time < BAIL_TIME {
                Input {
                    brake: true,
                    ..Input::default()
                }
            } else {
                self.set_action(Action::Standing);
                input
            }
        } else {
            input
        };
        // The speed limits run every frame whatever the skater is doing
        // (main.dol 0x8010B120 calls 0x800F4834 before the ground, air
        // and rail updates), so chaining grinds and jumps can't build
        // speed past `max_max_speed`. The ground step applies them itself.
        if let Some(grind) = self.grind.as_mut() {
            let velocity = self.velocity.normalize_or_zero() * grind.speed;
            let limited = limit_speed(velocity, p);
            if velocity.length() > 0.0 {
                grind.speed *= limited.length() / velocity.length();
            }
        } else if !self.on_ground {
            self.velocity = limit_speed(self.velocity, p);
        }
        let before = self.position;
        if self.lip.is_some() {
            self.lip_step(input, p);
        } else if self.grind.is_some() {
            self.grind_step(input, p, world);
        } else if self.on_ground {
            self.ground_step(input, p, world);
        } else {
            self.air_step(input, p, world);
        }
        if self.grind.is_none() && self.lip.is_none() {
            self.since_rail = (self.since_rail + STEP).min(f32::MAX);
            // Onto a rail met on the way (main.dol 0x801078A8), only from
            // the air (the main update asks with 0, which skips it on the
            // ground), and not too soon after leaving one.
            // The button held, or pressed within the last half second
            // (`{ Press, Triangle, 500 }` in the game's `GrindTricks`).
            let armed = input.grind || self.since_grind <= GRIND_WINDOW;
            if armed && !self.on_ground && self.since_rail >= p.regrind_time {
                if let Some(hit) = world.rails.nearest(before, self.position, p.rail_max_snap) {
                    // Rising in vert air at the coping: a lip trick (the
                    // game's grind start, 0x80108470, checks the skater is
                    // flat against a steep ramp, facing up and rising).
                    let level = world.rails.segments[hit.segment].direction().y.abs() < 0.3;
                    match self.vert {
                        Some(vert) if self.velocity.y > 0.0 && level => {
                            self.start_lip(hit.point, vert.out, p)
                        }
                        _ => self.start_grind(hit, input, p, world),
                    }
                }
            }
        }
        self.update_special();
    }

    /// The special meter, after each step (the score object, with
    /// `NewSpecial = 1`): it gains whatever the combo's total gained, fills
    /// at 3000, and drains.
    fn update_special(&mut self) {
        let total = self.combo_tricks.total();
        if total > self.last_total {
            self.special_meter += (total - self.last_total) as f32;
        }
        self.last_total = total;
        if self.special_meter >= SPECIAL_FULL {
            self.special_meter = SPECIAL_FULL;
            self.special = true;
        }
        let drain = if self.special {
            SPECIAL_DRAIN_FULL
        } else {
            SPECIAL_DRAIN
        };
        self.special_meter -= drain * STEP;
        if self.special_meter <= 0.0 {
            self.special_meter = 0.0;
            self.special = false;
        }
    }

    /// Whether the presses ended with `first`, `second` and then `button`
    /// (just now), within the special window (`TripleInOrder`).
    fn pressed_in_order(&self, first: Dir, second: Dir, button: Press) -> bool {
        let n = self.presses.len();
        if n < 3 || self.presses[n - 1].0 != button || self.presses[n - 1].1 != self.clock {
            return false;
        }
        let (a, b) = (self.presses[n - 3], self.presses[n - 2]);
        a.0 == Press::Dir(first) && b.0 == Press::Dir(second) && self.clock - a.1 <= SPECIAL_WINDOW
    }

    /// The direction pressed last, if within `window` seconds.
    fn last_dir_within(&self, window: f32) -> Option<Dir> {
        self.presses
            .iter()
            .rev()
            .find_map(|&(press, t)| match press {
                Press::Dir(d) if self.clock - t <= window => Some(d),
                _ => None,
            })
    }

    /// Whether `first`, `second` and then `button` were pressed in a row,
    /// all within the last `window` seconds.
    fn recently_in_order(&self, first: Dir, second: Dir, button: Press, window: f32) -> bool {
        let recent: Vec<Press> = self
            .presses
            .iter()
            .filter(|&&(_, t)| self.clock - t <= window)
            .map(|&(p, _)| p)
            .collect();
        recent
            .windows(3)
            .any(|w| w == [Press::Dir(first), Press::Dir(second), button])
    }

    /// Onto the coping (the `LipTrick` script): the special lip if its
    /// combination came within a second and the meter's full, else the one
    /// for the direction pressed in the last half second, else the plain
    /// one; held still on the coping, standing on the ramp's face.
    fn start_lip(&mut self, at: Vec3, out: Vec3, p: &Physics) {
        let special = self
            .tricks
            .special_lip
            .clone()
            .filter(|(a, b, _)| self.special && self.recently_in_order(*a, *b, Press::Grind, 1.0))
            .map(|(_, _, lip)| lip);
        let dir = self.last_dir_within(0.5);
        let trick = special.or_else(|| {
            let lips = &self.tricks.lips;
            lips.iter()
                .find(|(d, _)| dir.is_some() && *d == dir)
                .or_else(|| lips.iter().find(|(d, _)| d.is_none()))
                .map(|(_, lip)| lip.clone())
        });
        let Some(trick) = trick else {
            return;
        };
        self.vert = None;
        self.trick = None;
        self.manual = false;
        self.position = at;
        self.velocity = Vec3::ZERO;
        self.on_ground = false;
        // Standing on the ramp's face, head up the ramp.
        self.up = out;
        self.heading = (-out.x).atan2(-out.z);
        self.balance.start(&p.lip_balance, !self.combo);
        self.combo = true;
        self.credit(Some((trick.name.clone(), trick.score)), true);
        self.lip = Some(Lip {
            trick,
            out,
            time: 0.0,
        });
        self.set_action(Action::Lip);
    }

    /// On the coping (`LipMacro2`): balanced with right and left
    /// (`LipParams`), 10 points a frame; ollying drops back in (or ollies
    /// out), and off the meter it drops back in one way and bails the
    /// other.
    fn lip_step(&mut self, input: Input, p: &Physics) {
        let Some(mut lip) = self.lip.clone() else {
            return;
        };
        lip.time += STEP;
        self.combo_tricks.tweak(LIP_TWEAK);
        let lean = self
            .balance
            .update(input.turn > 0.0, input.turn < 0.0, &p.lip_balance, STEP);
        let ollie = self.crouched && !input.crouch;
        self.crouched = input.crouch;
        match lean {
            // `LipOut`: back into the ramp.
            Lean::OffBottom => self.leave_lip(&lip, 0.0),
            // `LipBail` (no skating in onto the deck yet).
            Lean::OffTop => {
                self.bail_on_landing = true;
                self.leave_lip(&lip, 0.0);
            }
            Lean::Balanced if ollie => {
                // `OllieLipOut` ollies out; `NoOllie` lips just drop in.
                let pop = if lip.trick.no_ollie {
                    0.0
                } else {
                    self.jump_speed(p)
                };
                self.leave_lip(&lip, pop);
            }
            Lean::Balanced => self.lip = Some(lip),
        }
    }

    /// Off the coping, back into the ramp (`LipOut`: a unit up and out,
    /// turned round to come down facing down the ramp), in vert air.
    fn leave_lip(&mut self, lip: &Lip, rise: f32) {
        self.lip = None;
        self.since_rail = 0.0;
        self.position += lip.out + Vec3::Y;
        self.velocity = Vec3::Y * rise;
        self.heading = lip.out.x.atan2(lip.out.z);
        self.up = Vec3::Y;
        self.vert = Some(VertAir {
            out: lip.out,
            offset: self.position.dot(lip.out),
        });
        self.lip_out = lip.trick.out.map(|anim| (anim, 0.0));
        self.set_action(Action::Air);
    }

    /// The lip's animation: its way in, then its range held along the
    /// balance meter (backwards, `PlayRangeAnimBackwards`), or its way out
    /// after it.
    pub fn lip_pose(&self, length: impl Fn(u32) -> f32) -> Option<(u32, f32)> {
        if let Some(lip) = &self.lip {
            if let Some(init) = lip.trick.init {
                if lip.time < length(init) {
                    return Some((init, lip.time));
                }
            }
            let meter = (self.balance.angle / METER).clamp(-1.0, 1.0);
            return Some((
                lip.trick.range,
                (1.0 - meter) / 2.0 * length(lip.trick.range),
            ));
        }
        let (anim, time) = self.lip_out?;
        (time < length(anim)).then_some((anim, time))
    }

    /// The ollie's speed (0x800F62C4): stronger the longer the crouch, up
    /// to the max tense time.
    fn jump_speed(&self, p: &Physics) -> f32 {
        let tense = (self.crouch_time / p.max_tense_time).min(1.0);
        p.jump_speed_min + (p.jump_speed - p.jump_speed_min) * tense
    }

    /// Onto a rail (main.dol 0x80108470): at the point found, unless
    /// something's in the way more than 6 short of it; the speed made
    /// horizontal, then along the rail, plus `rail_speed_boost`.
    fn start_grind(&mut self, hit: RailHit, input: Input, p: &Physics, world: &World) {
        if let Some(block) = world.ray(self.position, hit.point) {
            if block.point.distance(hit.point) > 6.0 {
                return;
            }
        }
        let rail = world.rails.segments[hit.segment];
        let direction = rail.direction();
        let mut velocity = self.velocity;
        if velocity.x == 0.0 && velocity.z == 0.0 {
            let facing = self.forward();
            velocity.x = facing.x;
            velocity.z = facing.z;
        }
        if velocity.length() > 10.0 {
            velocity = along_keeping_length(velocity, Vec3::Y);
        }
        let along = velocity.dot(direction);
        let forwards = along >= 0.0;
        self.vert = None;
        self.manual = false;
        self.balance.start(&p.grind_balance, !self.combo);
        self.combo = true;
        self.trick = None;
        self.credit(self.tricks.grind.clone(), true);
        self.grind = Some(Grind {
            segment: hit.segment,
            forwards,
            speed: along.abs() + p.rail_speed_boost,
        });
        self.position = hit.point;
        self.on_ground = false;
        self.up = Vec3::Y;
        self.crouched = input.crouch;
        let travel = if forwards { direction } else { -direction };
        self.velocity = travel * (along.abs() + p.rail_speed_boost);
        self.heading = travel.x.atan2(travel.z);
        self.set_action(Action::Grinding);
    }

    /// Along the rail (main.dol 0x80100A70): rail gravity along it, on
    /// round corners up to `rail_corner_leave_angle`, off the end or a
    /// sharper corner, and an ollie off it when letting go of a crouch,
    /// turned `rail_jump_angle` by the steering.
    fn grind_step(&mut self, input: Input, p: &Physics, world: &World) {
        let Some(mut grind) = self.grind else {
            return;
        };
        let rails = &world.rails;
        let mut rail = rails.segments[grind.segment];
        let mut travel = rail.direction() * if grind.forwards { 1.0 } else { -1.0 };
        grind.speed += p.rail_gravity * travel.y * STEP;
        if grind.speed < 0.0 {
            // Rolled back: the other way along the rail.
            grind.speed = -grind.speed;
            grind.forwards = !grind.forwards;
            travel = -travel;
        }

        if self.crouched && !input.crouch {
            self.crouched = false;
            let turn = if input.turn < 0.0 {
                p.rail_jump_angle.to_radians()
            } else if input.turn > 0.0 {
                -p.rail_jump_angle.to_radians()
            } else {
                0.0
            };
            let (sin, cos) = turn.sin_cos();
            let v = travel * grind.speed;
            let mut velocity = Vec3::new(cos * v.x + sin * v.z, v.y, -sin * v.x + cos * v.z);
            velocity.y += self.jump_speed(p);
            self.leave_rail(velocity, p, world);
            return;
        }
        self.crouched = input.crouch;

        // Balance (`DoBalanceTrick ButtonA = Right ButtonB = Left`): off
        // the top of the meter it falls to the left, off the bottom to the
        // right (the game's `SkateInOrBail`).
        let lean = self
            .balance
            .update(input.turn > 0.0, input.turn < 0.0, &p.grind_balance, STEP);
        // Each grinding frame adds the grind's tweak (0x801033B0).
        self.combo_tricks.tweak(GRIND_TWEAK);
        if lean != Lean::Balanced {
            let right = Vec3::new(-travel.z, 0.0, travel.x);
            let (side, turn) = if lean == Lean::OffTop {
                (-right, 30f32.to_radians())
            } else {
                (right, -30f32.to_radians())
            };
            // Ground just beside the rail (`SkateInAble`, approximated):
            // skate in onto it, turned 30 degrees that way, no bail.
            let beside = self.position + side * 20.0;
            let lift = Vec3::Y * 10.0;
            let ground = world
                .ray(beside + lift, beside - Vec3::Y * 30.0)
                .filter(|hit| !is_wall(hit, p))
                .filter(|hit| world.ray(self.position + lift, hit.point + lift).is_none());
            if let Some(hit) = ground {
                self.grind = None;
                self.since_rail = 0.0;
                self.position = hit.point;
                self.up = hit.normal;
                self.heading += turn;
                self.velocity = self.forward() * grind.speed;
                self.on_ground = true;
                self.end_combo(true);
                self.set_action(Action::Landing);
                return;
            }
            // Otherwise a nudge that way and down, and a bail on landing
            // (`FiftyFiftyFall`).
            self.position += side - Vec3::Y * 5.0;
            self.bail_on_landing = true;
            self.leave_rail(travel * grind.speed, p, world);
            return;
        }

        let mut position = self.position + travel * grind.speed * STEP;
        loop {
            let joint = if grind.forwards { rail.end } else { rail.start };
            let past = (position - joint).dot(travel);
            if past <= 0.0 {
                break;
            }
            let next = rails
                .next(grind.segment, grind.forwards)
                .filter(|&(n, same)| {
                    let d = rails.segments[n].direction() * if same { 1.0 } else { -1.0 };
                    d.dot(travel) >= p.rail_corner_leave_angle.to_radians().cos()
                });
            let Some((n, same)) = next else {
                // Off the end, or a corner too sharp to follow.
                if !self.grind_blocked(position, travel * grind.speed, p, world) {
                    self.position = position;
                    self.leave_rail(travel * grind.speed, p, world);
                }
                return;
            };
            grind.segment = n;
            grind.forwards = same;
            rail = rails.segments[n];
            travel = rail.direction() * if same { 1.0 } else { -1.0 };
            position = joint + travel * past;
        }
        if self.grind_blocked(position, travel * grind.speed, p, world) {
            return;
        }
        self.position = position;
        self.velocity = travel * grind.speed;
        self.heading = travel.x.atan2(travel.z);
        self.grind = Some(grind);
        self.set_action(Action::Grinding);
    }

    /// Anything in the way along the rail (main.dol 0x801032E4): a line 1
    /// up from where the skater is to 1 up from `to` and 6 on. Hitting
    /// anything, like the ground where a rail dips into it, knocks it off
    /// the rail where it was, 1 higher, its speed along what it hit.
    fn grind_blocked(&mut self, to: Vec3, velocity: Vec3, p: &Physics, world: &World) -> bool {
        let Some(direction) = (to - self.position).try_normalize() else {
            return false;
        };
        let from = self.position + Vec3::Y;
        let Some(hit) = world.ray(from, to + Vec3::Y + direction * 6.0) else {
            return false;
        };
        self.position.y += 1.0;
        self.leave_rail(velocity - hit.normal * velocity.dot(hit.normal), p, world);
        true
    }

    fn leave_rail(&mut self, velocity: Vec3, p: &Physics, world: &World) {
        // Rails often run along the edge of a ledge a little below its top:
        // up onto whatever surface is just above the feet, so the air step
        // doesn't start inside the ledge and fall through it. (This
        // crate's own safeguard; the game lifts the skater 1 when knocked
        // off.)
        let above = self.position + Vec3::Y * p.ground_snap_up;
        if let Some(hit) = world.ray(above, self.position) {
            if hit.normal.y > 0.0 && !is_wall(&hit, p) {
                self.position = hit.point + Vec3::Y * 0.1;
            }
        }
        self.grind = None;
        self.since_rail = 0.0;
        self.velocity = velocity;
        self.on_ground = false;
        self.up = Vec3::Y;
        self.set_action(Action::Air);
    }

    fn ground_step(&mut self, input: Input, p: &Physics, world: &World) {
        // A manual: up then down (or down then up) within the window
        // (the game's `ManualTricks`), balanced with up and down
        // (`DoBalanceTrick ButtonA = Up ButtonB = Down`).
        let pressed_pair = (input.push && self.since_up == 0.0 && self.since_down <= MANUAL_WINDOW)
            || (input.brake && self.since_down == 0.0 && self.since_up <= MANUAL_WINDOW);
        if !self.manual && pressed_pair && !input.crouch && self.action != Action::BailManual {
            self.manual = true;
            self.balance.start(&p.manual_balance, !self.combo);
            self.combo = true;
            self.credit(self.tricks.manual.clone(), true);
            self.set_action(Action::Manual);
        }
        // The special manual: its combination while in a manual, with the
        // meter full (the `Manual` script's `SpecialManualTricks`).
        if self.manual && !self.special_manual && self.special {
            if let Some((a, b, trick)) = self.tricks.special_manual.clone() {
                if self.pressed_in_order(a, b, Press::Grind) {
                    self.special_manual = true;
                    self.balance.start(&p.manual_balance, false);
                    self.credit(Some(trick), true);
                }
            }
        }
        if self.manual {
            match self
                .balance
                .update(input.push, input.brake, &p.manual_balance, STEP)
            {
                // Balanced: `DoBalanceTrick Tweak = 1` adds a point a
                // frame (0x800CAD30 calls `TweakTrick`).
                Lean::Balanced => self.combo_tricks.tweak(if self.special_manual {
                    SPECIAL_MANUAL_TWEAK
                } else {
                    MANUAL_TWEAK
                }),
                // Off the top: the game's `BailManual`.
                Lean::OffTop => {
                    self.manual = false;
                    self.end_combo(false);
                    self.set_action(Action::BailManual);
                }
                // Off the bottom: `ManualLand`, back on four wheels.
                Lean::OffBottom => {
                    self.manual = false;
                    self.end_combo(true);
                    self.set_action(Action::Rolling);
                }
            }
        }
        // Up and down balance a manual rather than push and brake.
        let input = if self.manual {
            Input {
                push: false,
                brake: false,
                ..input
            }
        } else {
            input
        };
        // Steering (main.dol 0x800ED848): the sharp rate while braking,
        // and when nearly stopped it builds up over the first 600 ms held.
        let mut rate = if input.brake {
            p.ground_sharp_rotation
        } else {
            p.ground_rotation
        };
        if self.velocity.length() < 10.0 && self.turn_time < 0.6 {
            rate *= self.turn_time / 0.6;
        }
        self.heading -= input.turn * rate * STEP;
        let up = self.up;
        // Forward along the surface.
        let flat = self.forward();
        let forward = (flat - up * flat.dot(up)).normalize_or(flat);
        let mut velocity = self.velocity;

        // Gravity along the ground plane, sideways too (main.dol 0x800FB3E4).
        let gravity = Vec3::Y * p.ground_gravity;
        velocity += (gravity - up * gravity.dot(up)) * STEP;

        if input.push {
            let (top, accel, friction) = if input.crouch {
                (
                    p.max_crouched_kick_speed,
                    p.crouched_acceleration,
                    p.crouched_air_friction,
                )
            } else {
                (
                    p.max_standing_kick_speed,
                    p.standing_acceleration,
                    p.standing_air_friction,
                )
            };
            // Push along the current velocity while under the kick speed
            // (0x800F43F0, 0x800F44CC), with drag (0x800F4CF0).
            if velocity.length() <= top {
                let direction = velocity.try_normalize().unwrap_or(forward);
                velocity += direction * accel * STEP;
            }
            velocity = drag(velocity, friction);
        }
        if input.brake {
            // Braking (0x800F418C): slow along the velocity, stopping
            // outright below two steps' worth or if it would reverse.
            let length = velocity.length();
            let step = p.brake_acceleration * STEP;
            if length < 2.0 * step {
                velocity = Vec3::ZERO;
            } else {
                let before = velocity;
                velocity -= velocity / length * step;
                if velocity.dot(before) < 0.0 {
                    velocity = Vec3::ZERO;
                }
            }
        }
        // Along the board, keeping the sign: it can roll backwards
        // (0x800F4D5C).
        let sign = if velocity.dot(forward) < 0.0 {
            -1.0
        } else {
            1.0
        };
        velocity = forward * velocity.length() * sign;
        // Speed limits, on the horizontal speed (0x800F4834).
        velocity = limit_speed(velocity, p);
        let mut speed = velocity.dot(forward);

        // Jump when letting go of a crouch.
        if self.crouched && !input.crouch {
            self.crouched = false;
            self.manual = false;
            // The ollie (0x800F62C4): along the ground's normal when moving
            // down, else straight up.
            let jump = self.jump_speed(p);
            let mut velocity = forward * speed;
            if velocity.y < 0.0 {
                velocity += up * jump;
            } else {
                velocity.y += jump;
            }
            self.velocity = velocity;
            self.on_ground = false;
            // Off the ground by a unit, as the game lands the skater a unit
            // above it: standing exactly on the surface, the first line of
            // the jump can catch the floor from below (rounding puts the
            // feet a hair under it) and push the skater through it.
            self.position += up;
            self.up = Vec3::Y;
            self.set_action(Action::Air);
            return;
        }
        self.crouched = input.crouch;

        // Walls (main.dol 0x800F7D38): a line at knee height from here to
        // where the skater is going, and the collision length on.
        let mut forward = forward;
        let mut target = self.position + forward * speed * STEP;
        let mut flailed = None;
        let lift = up * p.forward_collision_height;
        if let Some(direction) = (target - self.position).try_normalize() {
            let from = self.position + lift;
            let to = target + lift + direction * p.forward_collision_length;
            if let Some(hit) = world.ray(from, to) {
                if !is_wall(&hit, p) && hit.normal.dot(up).abs() >= 0.01 {
                    // Skatable ground ahead, like the curve of a ramp:
                    // step onto it.
                    target = hit.point + hit.normal * 0.1;
                } else {
                    let (turn, angle) = wall_bounce(forward, up, hit.normal, p);
                    self.heading += turn;
                    let flat = self.forward();
                    forward = (flat - up * flat.dot(up)).normalize_or(flat);
                    let before = speed.abs();
                    let dont_slow = p.wall_bounce_dont_slow_angle.to_radians();
                    if angle.abs() > dont_slow {
                        speed *= 1.0 - (angle.abs() - dont_slow) / (FRAC_PI_2 - dont_slow);
                    }
                    if before > p.wall_bounce_dont_flail_speed {
                        flailed = Some(if turn > 0.0 {
                            Action::FlailLeft
                        } else {
                            Action::FlailRight
                        });
                    }
                    // Out of the wall a little, at the skater's feet.
                    target = hit.point - lift + hit.normal * 6.0;
                }
            }
        }

        // Ground following (main.dol 0x800F52AC): down the skater's up from
        // `ground_snap_up` above the new spot.
        let from = target + up * p.ground_snap_up;
        let to = target - up * 200.0;
        // A wall above the new spot (an overhang the skater has ducked
        // under) is looked past for the ground beneath it.
        let ground = world
            .ray_past(from, to, |hit| {
                !is_wall(hit, p) || up.dot(hit.point - target) <= 0.0
            })
            .filter(|hit| !is_wall(hit, p))
            .filter(|hit| sticks(hit, self.position, target, forward, up, p))
            // This crate's safeguard: a wall bounce can leave the new spot
            // just inside a block whose top the tilted line misses (it
            // goes in through the side); ground straight above the feet,
            // within the snap distances, holds the skater.
            .or_else(|| {
                world
                    .ray_past(
                        target + Vec3::Y * p.ground_snap_up,
                        target - Vec3::Y * p.ground_snap_down,
                        |hit| !is_wall(hit, p),
                    )
                    .filter(|hit| hit.point.y >= target.y)
            });
        match ground {
            Some(hit) => {
                self.position = hit.point;
                self.last_ground = hit.point;
                self.ground_flags = hit.flags;
                self.up = hit.normal;
                self.velocity = forward * speed;
            }
            None => {
                if self.trace {
                    let first = world.ray(from, to);
                    let past = world.ray_past(from, to, |hit| {
                        !is_wall(hit, p) || up.dot(hit.point - target) <= 0.0
                    });
                    eprintln!(
                        "ground gone: from {:?} at {:?} to {target:?}, up {up:?}; first {:?}; past walls {:?}; stick {:?}",
                        self.position,
                        from,
                        first.map(|h| (h.point, h.normal, is_wall(&h, p))),
                        past.map(|h| (h.point, h.normal, is_wall(&h, p))),
                        past.map(|h| sticks(&h, self.position, target, forward, up, p)),
                    );
                }
                // The ground is gone: off an edge, or over the lip of a
                // kicker (the game's `GroundGone`).
                self.position = target;
                self.velocity = forward * speed;
                self.on_ground = false;
                self.manual = false;
                self.take_off_vert(p);
                self.up = Vec3::Y;
                self.set_action(Action::Air);
                return;
            }
        }

        let action = if matches!(
            self.action,
            Action::BailManual | Action::BailGrind | Action::Bail
        ) {
            self.action
        } else if self.manual {
            Action::Manual
        } else if let Some(flail) = flailed {
            flail
        } else if matches!(self.action, Action::FlailLeft | Action::FlailRight)
            && self.action_time < 0.5
        {
            self.action
        } else if self.action == Action::Landing && self.action_time < 0.3 {
            Action::Landing
        } else if input.crouch {
            Action::Crouching
        } else if input.push && speed < p.max_crouched_kick_speed {
            Action::Pushing
        } else if speed.abs() > 5.0 {
            Action::Rolling
        } else {
            Action::Standing
        };
        self.set_action(action);
    }

    /// Off the lip of a vert ramp going up (the game's 0x800F140C, when
    /// the ground it left was a vert face): the speed away from the ramp
    /// is turned up or along it, keeping its length, and the skater is put
    /// `vert_push_out` out from the lip, to come back down onto the ramp.
    /// The game also aims transfers to other ramps here; not ported.
    fn take_off_vert(&mut self, p: &Physics) {
        use ngc_collision::face_flags::VERT;
        let out = Vec3::new(self.up.x, 0.0, self.up.z);
        if self.ground_flags & VERT == 0 || self.velocity.y <= 0.0 || out.length() < 0.5 {
            return;
        }
        let out = out.normalize();
        let speed = self.velocity.length();
        let kept = self.velocity - out * self.velocity.dot(out);
        self.velocity = kept.normalize_or(Vec3::Y) * speed;
        self.position += out * p.vert_push_out;
        self.vert = Some(VertAir {
            out,
            offset: self.position.dot(out),
        });
    }

    fn air_step(&mut self, input: Input, p: &Physics, world: &World) {
        if let Some((_, time)) = self.lip_out.as_mut() {
            *time += STEP;
        }
        self.air_tricks(input);
        // Spinning at the air rotation stat (0x800EE4CC); the turn counts
        // towards the latest trick's spin.
        let turn = input.turn * p.air_rotation * STEP;
        self.heading -= turn;
        self.air_spin += turn.to_degrees();
        self.combo_tricks.spin(self.air_spin);
        // As the game does (0x800FC7F8): gravity, divided by the hang-time
        // stat, and the exact step of a thrown body.
        let gravity = p.air_gravity / p.air_hang.max(0.01);
        let from = self.position;
        let mut target = from + self.velocity * STEP + Vec3::Y * (0.5 * gravity * STEP * STEP);
        self.velocity.y += gravity * STEP;
        // Vert air stays in the ramp's plane.
        if let Some(vert) = self.vert {
            self.velocity -= vert.out * self.velocity.dot(vert.out);
            target += vert.out * (vert.offset - target.dot(vert.out));
        }

        // Walls (0x800F847C): the ground check's line at knee height;
        // what's ground-like is left for landing.
        let lift = self.up * p.forward_collision_height;
        if let Some(direction) = (target - from).try_normalize() {
            let to = target + lift + direction * p.forward_collision_length;
            if let Some(hit) = world.ray(from + lift, to) {
                let ground =
                    !is_wall(&hit, p) && (hit.normal.dot(self.up) >= 0.8 || hit.normal.y >= 0.5);
                // In vert air the ramp is for landing on.
                let ramp = self.vert.is_some() && hit.flags & ngc_collision::face_flags::VERT != 0;
                if !ground && !ramp {
                    self.air_bounce(hit.normal);
                    target = hit.point - lift + hit.normal * p.min_distance_to_wall;
                }
            }
        }

        // Landing (0x800FC7F8): a line from where the skater was to where
        // it's going.
        if let Some(hit) = world.ray(from, target) {
            // A ledge it nearly cleared: up onto it (0x800F81A8), unless
            // it's falling onto a face it can land on anyway.
            if self.velocity.y > 10.0 || hit.normal.y < 0.1 {
                if let Some(ledge) = air_snap_up(from, target, p, world) {
                    self.position = ledge;
                    return;
                }
            }
            if !is_wall(&hit, p) {
                // Land, a unit off the ground, keeping the speed along it
                // (none below 10) and the way the skater faces: the ground
                // step then turns the speed along the board.
                self.position = hit.point + hit.normal;
                self.up = hit.normal;
                self.velocity -= hit.normal * self.velocity.dot(hit.normal);
                if self.velocity.length() < 10.0 {
                    self.velocity = Vec3::ZERO;
                }
                self.on_ground = true;
                self.vert = None;
                self.lip_out = None;
                self.crouched = input.crouch;
                if self.bail_on_landing {
                    self.bail_on_landing = false;
                    self.trick = None;
                    self.end_combo(false);
                    self.set_action(Action::BailGrind);
                    return;
                }
                // Landing mid-trick (`BailOn`): a bail.
                if self.bail_on() {
                    self.trick = None;
                    self.end_combo(false);
                    self.set_action(Action::Bail);
                    return;
                }
                self.trick = None;
                // Landing in a manual (up-down pressed just before) keeps
                // the combo going; otherwise it ends.
                if self.since_up.min(self.since_down) <= MANUAL_WINDOW
                    && (self.since_up - self.since_down).abs() <= MANUAL_WINDOW
                    && self.since_up.max(self.since_down) < 1.0
                {
                    self.manual = true;
                    self.balance.start(&p.manual_balance, !self.combo);
                    self.combo = true;
                    self.credit(self.tricks.manual.clone(), true);
                    self.set_action(Action::Manual);
                    return;
                }
                self.end_combo(true);
                self.set_action(Action::Landing);
                return;
            }
            // A wall: slide along it at the same speed, facing along it,
            // `min_distance_to_wall` out.
            self.velocity = along_keeping_length(self.velocity, hit.normal);
            let facing = along_keeping_length(self.forward(), hit.normal);
            if facing.x != 0.0 || facing.z != 0.0 {
                self.heading = facing.x.atan2(facing.z);
            }
            self.position = hit.point + hit.normal * (1.0 + p.min_distance_to_wall);
            return;
        }
        self.position = target;
        // Fell out of the level, off its edge or through a gap: well below
        // where it last stood with nothing at all underneath, or below the
        // level altogether. (The game uses its own out-of-bounds triggers,
        // not ported; this is the crate's safety net.)
        let lost = self.position.y < self.last_ground.y - 1000.0
            && world
                .ray(self.position, self.position - Vec3::Y * 100_000.0)
                .is_none();
        if lost || self.position.y < world.floor() - 500.0 {
            self.respawn(p, world);
        }
    }

    /// Back on the level at the spawn point nearest to where the skater
    /// last stood (or there, without spawns), on the ground under it,
    /// facing the spawn's way and at rest.
    pub fn respawn(&mut self, p: &Physics, world: &World) {
        let (position, heading) = self
            .spawns
            .iter()
            .copied()
            .min_by(|a, b| {
                a.0.distance_squared(self.last_ground)
                    .total_cmp(&b.0.distance_squared(self.last_ground))
            })
            .unwrap_or((self.last_ground, self.heading));
        let ground = world
            .ray(position + Vec3::Y * 100.0, position - Vec3::Y * 1000.0)
            .filter(|hit| !is_wall(hit, p));
        self.position = ground.map_or(position, |hit| hit.point);
        self.heading = heading;
        self.vert = None;
        self.grind = None;
        self.lip = None;
        self.lip_out = None;
        self.manual = false;
        self.trick = None;
        self.end_combo(false);
        self.velocity = Vec3::ZERO;
        self.up = ground.map_or(Vec3::Y, |hit| hit.normal);
        self.on_ground = true;
        self.set_action(Action::Standing);
    }
}

/// Whether the skater stays on the ground it found under its new spot
/// (main.dol 0x800F52AC). Going by its facing, not its speed: ground ahead
/// falling away more than `ground_stick_angle` from the ground it's on
/// isn't followed; and it only snaps down as far as `ground_snap_down`, or
/// further at a sharp change of slope (as far as moving on along the old
/// slope would leave it above the new one).
fn sticks(hit: &Hit, from: Vec3, to: Vec3, facing: Vec3, up: Vec3, p: &Physics) -> bool {
    // Ground at or above where the skater is going isn't falling away,
    // whichever way it faces: at the foot of a slope the move along it
    // ends a little under the flat. (The game's test goes by the facing
    // alone, which rolling backwards into the foot of a slope reads as the
    // ground falling away; flying off from under the flat, nothing would
    // catch the skater.)
    let height = up.dot(to - hit.point);
    if height < 0.0 {
        return true;
    }
    // Facing the same way as the new normal: the ground falls away ahead.
    let falling_away = facing.dot(hit.normal) > 0.0;
    let on_new = along_keeping_length(facing, hit.normal);
    let on_old = along_keeping_length(facing, up);
    let cos = on_new.dot(on_old);
    if falling_away && cos > 0.0 && cos < p.ground_stick_angle.to_radians().cos() {
        return false;
    }
    {
        let angle = cos.clamp(-1.0, 1.0).acos();
        let reach = (from.distance(to) * angle.tan()).max(p.ground_snap_down);
        if to.distance(hit.point) > reach {
            return false;
        }
    }
    true
}

/// Up onto a ledge (main.dol 0x800F81A8): straight down from
/// `air_snap_up` above the higher of where the skater was and where it's
/// going, to the lower; ground facing up there that the skater can reach
/// (a clear line from `air_snap_up` above where it was) is where it goes,
/// a unit off it and 0.1 higher.
fn air_snap_up(from: Vec3, to: Vec3, p: &Physics, world: &World) -> Option<Vec3> {
    let up = Vec3::Y * p.air_snap_up;
    let top = Vec3::new(to.x, to.y.max(from.y) + p.air_snap_up, to.z);
    let bottom = Vec3::new(to.x, to.y.min(from.y), to.z);
    let ground = world.ray(top, bottom)?;
    if ground.normal.y <= 0.5 {
        return None;
    }
    let spot = ground.point + ground.normal;
    if world.ray(from + up, spot + up).is_some() {
        return None;
    }
    Some(spot + Vec3::Y * 0.1)
}

/// `v` along the plane with this normal, at the same length (main.dol
/// 0x80009B3C); straight across the normal if it pointed along it.
fn along_keeping_length(v: Vec3, normal: Vec3) -> Vec3 {
    let length = v.length();
    let along = v - normal * v.dot(normal);
    match along.try_normalize() {
        Some(direction) => direction * length,
        None => Vec3::new(-normal.z, 0.0, normal.x).normalize_or_zero() * length,
    }
}

/// Whether a face the skater ran into is a wall (main.dol 0x800F66F0):
/// skatable and vert faces never are, not-skatable and wall-ridable ones
/// always are, and the rest by their slope.
fn is_wall(hit: &Hit, p: &Physics) -> bool {
    use ngc_collision::face_flags as f;
    if hit.flags & (f::SKATABLE | f::VERT) != 0 {
        false
    } else if hit.flags & (f::NOT_SKATABLE | f::WALL_RIDABLE) != 0 {
        true
    } else {
        hit.normal.y < p.wall_non_skatable_angle.to_radians().sin()
    }
}

/// How much to turn off a wall, and the angle it was hit at (main.dol
/// 0x800F6860): the angle between the skater's side and the wall's
/// normal, folded to within a right angle, so 90 degrees is head-on and 0
/// is skimming along it. The turn is that times
/// `wall_bounce_angle_multiplier`, away from the wall.
fn wall_bounce(forward: Vec3, up: Vec3, normal: Vec3, p: &Physics) -> (f32, f32) {
    let side = up.cross(forward).normalize_or(Vec3::X);
    let mut angle = side.dot(normal).clamp(-1.0, 1.0).acos();
    if angle > FRAC_PI_2 {
        angle -= PI;
    }
    let turn = angle.abs() * p.wall_bounce_angle_multiplier;
    // The game's sign comes from its matrix; here, whichever way turns the
    // skater away from the wall. Turning the heading by t turns the
    // forward vector about Y.
    let away = |t: f32| {
        let (s, c) = t.sin_cos();
        Vec3::new(
            c * forward.x + s * forward.z,
            0.0,
            -s * forward.x + c * forward.z,
        )
        .dot(normal)
    };
    let turn = if away(turn) >= away(-turn) {
        turn
    } else {
        -turn
    };
    (turn, angle)
}

/// Quadratic drag: `v -= v * |v| * k * 60 * step` (0x800F46C4).
fn drag(v: Vec3, k: f32) -> Vec3 {
    if v.length_squared() <= 1e-5 {
        return v;
    }
    v - v * v.length() * k * 60.0 * STEP
}

/// The game's speed limits on horizontal speed: a hard cap at
/// `max_max_speed`, and heavy drag above `max_speed` (0x800F4834).
fn limit_speed(v: Vec3, p: &Physics) -> Vec3 {
    let mut flat = Vec3::new(v.x, 0.0, v.z);
    let speed = flat.length();
    if speed > p.max_max_speed {
        flat *= p.max_max_speed / speed;
    }
    if flat.length() > p.max_speed {
        flat = drag(flat, p.heavy_air_friction);
    }
    Vec3::new(flat.x, v.y, flat.z)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn physics() -> Physics {
        Physics::new(&qb::vm::Program::new(), &crate::Stats::default())
    }

    #[test]
    fn drag_grows_with_the_square_of_speed() {
        let slow = drag(Vec3::new(100.0, 0.0, 0.0), 1e-5);
        let fast = drag(Vec3::new(500.0, 0.0, 0.0), 1e-5);
        // 100 * 100 * 1e-5 * 60 / 60 = 0.1; 500 * 500 * ... = 2.5.
        assert!((100.0 - slow.x - 0.1).abs() < 1e-4);
        assert!((500.0 - fast.x - 2.5).abs() < 1e-3);
    }

    #[test]
    fn speed_is_capped_horizontally_only() {
        let p = physics();
        let v = limit_speed(Vec3::new(3000.0, -500.0, 0.0), &p);
        assert!(v.x <= p.max_max_speed);
        assert_eq!(v.y, -500.0);
        let v = limit_speed(Vec3::new(p.max_speed - 1.0, 0.0, 0.0), &p);
        assert_eq!(v.x, p.max_speed - 1.0, "below top speed nothing changes");
    }

    #[test]
    fn grinds_down_a_rail_round_a_bend_and_off_a_sharp_corner() {
        use crate::rails::{Rails, Segment};
        let p = physics();
        let empty = ngc_collision::Collision {
            objects: Vec::new(),
            repaired_fields: 0,
            repaired_bsp_fields: 0,
        };
        let a = Vec3::new(0.0, 100.0, 0.0);
        let b = Vec3::new(200.0, 80.0, 0.0);
        // A 30 degree bend, then a right angle.
        let c = b + Vec3::new(30f32.to_radians().cos(), 0.0, 30f32.to_radians().sin()) * 200.0;
        let d = c + Vec3::new(-0.5, 0.0, 0.866) * 200.0;
        let world = World::new(empty).with_rails(Rails::new(vec![
            Segment { start: a, end: b },
            Segment { start: b, end: c },
            Segment { start: c, end: d },
        ]));
        let mut skater = Skater::new(Vec3::new(40.0, 110.0, 3.0), FRAC_PI_2);
        skater.on_ground = false;
        skater.velocity = Vec3::new(300.0, -100.0, 0.0);
        let grind = Input {
            grind: true,
            ..Input::default()
        };
        skater.update(grind, &p, &world, STEP * 2.0);
        let on = skater.grind.expect("onto the rail");
        assert_eq!(on.segment, 0);
        assert!(on.forwards);
        // 300 along it, plus the boost.
        assert!((on.speed - 450.0).abs() < 30.0, "{}", on.speed);
        let mut reached_bend = false;
        for _ in 0..120 {
            skater.update(grind, &p, &world, STEP);
            match skater.grind {
                Some(g) => reached_bend |= g.segment == 1,
                None => break,
            }
        }
        assert!(reached_bend, "round the 30 degree bend");
        assert!(skater.grind.is_none(), "off at the right angle");
        assert_eq!(skater.action, Action::Air);
        assert!((skater.position.distance(c)) < 20.0, "{}", skater.position);
    }

    #[test]
    fn speed_is_capped_in_the_air_and_on_rails_too() {
        use crate::rails::{Rails, Segment};
        let p = physics();
        let empty = ngc_collision::Collision {
            objects: Vec::new(),
            repaired_fields: 0,
            repaired_bsp_fields: 0,
        };
        let world = World::new(empty).with_rails(Rails::new(vec![Segment {
            start: Vec3::new(0.0, 0.0, 0.0),
            end: Vec3::new(100_000.0, 0.0, 0.0),
        }]));
        // Far too fast in the air: back under the hard cap at once.
        let mut skater = Skater::new(Vec3::new(0.0, 5000.0, 0.0), 0.0);
        skater.on_ground = false;
        skater.velocity = Vec3::new(5000.0, 0.0, 0.0);
        skater.update(Input::default(), &p, &world, STEP);
        assert!(skater.velocity.x <= p.max_max_speed + 1.0);
        // Grinding: chained boosts can't build speed past it either.
        let mut skater = Skater::new(Vec3::new(10.0, 0.0, 0.0), FRAC_PI_2);
        skater.on_ground = false;
        skater.grind = Some(Grind {
            segment: 0,
            forwards: true,
            speed: 5000.0,
        });
        skater.velocity = Vec3::X * 5000.0;
        for _ in 0..10 {
            skater.update(Input::default(), &p, &world, STEP);
        }
        assert!(skater.grind.unwrap().speed <= p.max_max_speed + 1.0);
    }

    #[test]
    fn sticks_to_gentle_slopes_and_not_over_a_lip() {
        let p = physics();
        let hit = |normal: Vec3, point: Vec3| Hit {
            point,
            normal: normal.normalize(),
            fraction: 0.5,
            flags: 0,
        };
        let (from, to) = (Vec3::ZERO, Vec3::new(0.0, 0.0, 10.0));
        let slope = |degrees: f32| {
            let (s, c) = degrees.to_radians().sin_cos();
            // Falling away ahead (+Z) by this many degrees.
            Vec3::new(0.0, c, s)
        };
        // Flat ground just below: stick.
        assert!(sticks(
            &hit(Vec3::Y, Vec3::new(0.0, -1.0, 10.0)),
            from,
            to,
            Vec3::Z,
            Vec3::Y,
            &p
        ));
        // Falling away 10 degrees: stick; 45: fly off the lip.
        assert!(sticks(
            &hit(slope(10.0), to),
            from,
            to,
            Vec3::Z,
            Vec3::Y,
            &p
        ));
        assert!(!sticks(
            &hit(slope(45.0), to),
            from,
            to,
            Vec3::Z,
            Vec3::Y,
            &p
        ));
        // Rising 45 degrees ahead (a ramp): stick.
        assert!(sticks(
            &hit(slope(-45.0), to),
            from,
            to,
            Vec3::Z,
            Vec3::Y,
            &p
        ));
        // Flat ground 20 below: too far to snap down to.
        assert!(!sticks(
            &hit(Vec3::Y, Vec3::new(0.0, -20.0, 10.0)),
            from,
            to,
            Vec3::Z,
            Vec3::Y,
            &p
        ));
    }

    #[test]
    fn sliding_along_a_wall_keeps_the_speed() {
        let v = along_keeping_length(Vec3::new(300.0, 0.0, -400.0), Vec3::Z);
        assert!((v - Vec3::new(500.0, 0.0, 0.0)).length() < 1e-3);
        // Straight into it: across it, still at full speed.
        let v = along_keeping_length(Vec3::new(0.0, 0.0, -400.0), Vec3::Z);
        assert!((v.length() - 400.0).abs() < 1e-3 && v.z == 0.0);
    }

    #[test]
    fn air_walls_keep_the_rise_and_push_away() {
        // Flying up and into a wall facing +Z, at an angle, facing the way
        // it flies.
        let mut skater = Skater::new(Vec3::ZERO, 300f32.atan2(-400.0));
        skater.on_ground = false;
        skater.velocity = Vec3::new(300.0, 200.0, -400.0);
        skater.air_bounce(Vec3::Z);
        assert_eq!(skater.velocity, Vec3::new(300.0, 200.0, 30.0));
        // Now facing along the wall, slightly out from it.
        let forward = skater.forward();
        assert!(forward.x > 0.99 && forward.z > 0.0, "{forward}");
    }

    #[test]
    fn walls_turn_the_skater_away_by_the_angle_it_hit_at() {
        let p = physics();
        // Heading along +Z into a wall facing -Z: head-on.
        let (_, angle) = wall_bounce(Vec3::Z, Vec3::Y, -Vec3::Z, &p);
        assert!((angle.abs() - FRAC_PI_2).abs() < 1e-4);
        // At 30 degrees to a wall on the right (facing -X).
        let forward = Vec3::new(30f32.to_radians().sin(), 0.0, 30f32.to_radians().cos());
        let (turn, angle) = wall_bounce(forward, Vec3::Y, -Vec3::X, &p);
        assert!((angle.abs() - 30f32.to_radians()).abs() < 1e-4);
        assert!((turn.abs() - 33f32.to_radians()).abs() < 1e-4);
        // Turned away: now heading slightly away from the wall.
        let (s, c) = turn.sin_cos();
        let after = Vec3::new(
            c * forward.x + s * forward.z,
            0.0,
            -s * forward.x + c * forward.z,
        );
        assert!(after.x < 0.0 && after.z > 0.9);
    }
}
