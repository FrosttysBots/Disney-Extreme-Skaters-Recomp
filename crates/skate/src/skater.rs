//! A skater rolling over a level's collision.
//!
//! This is a first version driven by the game's constants (see
//! `constants`), not yet a frame-exact copy of the game's physics: that
//! needs the update code in `main.dol` compared against the running game.
//! What's been read from that code so far is in `docs/physics-notes.md`;
//! pushing (along the current velocity, up to the kick speed) and air
//! gravity (divided by the hang-time stat) already match it.
//!
//! - **On the ground** the skater follows the surface: each step it moves
//!   along its heading, then looks for ground from `ground_snap_up` above
//!   to `ground_snap_down` below and stands on it, tilting to its normal.
//!   Pushing accelerates up to the kick speed (higher when crouched),
//!   braking slows, slopes pull with `ground_gravity`, and steering turns
//!   at `ground_rotation`. No ground means it rolled off an edge.
//! - **Jumping**: crouch, then let go to ollie at `jump_speed` off the
//!   ground's normal.
//! - **In the air** gravity is `air_gravity` and steering spins at
//!   `air_rotation`; landing on ground facing up enough puts it back on
//!   the ground, keeping the speed along the surface.
//! - **Walls** (steep faces at knee height ahead) stop it, keeping
//!   `min_distance_to_wall`.

use glam::{Mat4, Quat, Vec3};

use crate::constants::Physics;
use crate::world::World;

/// The controls held this step.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Input {
    pub push: bool,
    pub brake: bool,
    /// -1 (left) to 1 (right).
    pub turn: f32,
    /// Crouching; letting go jumps.
    pub crouch: bool,
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
}

/// Steeper than this (cosine of the angle from vertical) is a wall.
const WALL_COSINE: f32 = 0.5;
/// Fixed physics step.
const STEP: f32 = 1.0 / 60.0;

pub struct Skater {
    pub position: Vec3,
    pub velocity: Vec3,
    /// Heading around the vertical, in radians; 0 faces +Z.
    pub heading: f32,
    /// The surface it stands on (up when in the air).
    pub up: Vec3,
    pub on_ground: bool,
    pub action: Action,
    /// Seconds in the current action.
    pub action_time: f32,
    crouched: bool,
    leftover: f32,
}

impl Skater {
    pub fn new(position: Vec3, heading: f32) -> Self {
        Skater {
            position,
            velocity: Vec3::ZERO,
            heading,
            up: Vec3::Y,
            on_ground: true,
            action: Action::Standing,
            action_time: 0.0,
            crouched: false,
            leftover: 0.0,
        }
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
        let tilt = Quat::from_rotation_arc(Vec3::Y, self.up.normalize_or(Vec3::Y));
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

    fn set_action(&mut self, action: Action) {
        if self.action != action {
            self.action = action;
            self.action_time = 0.0;
        }
    }

    fn step(&mut self, input: Input, p: &Physics, world: &World) {
        self.action_time += STEP;
        if self.on_ground {
            self.ground_step(input, p, world);
        } else {
            self.air_step(input, p, world);
        }
    }

    fn ground_step(&mut self, input: Input, p: &Physics, world: &World) {
        self.heading -= input.turn * p.ground_rotation * STEP;
        let up = self.up;
        // Forward along the surface.
        let flat = self.forward();
        let forward = (flat - up * flat.dot(up)).normalize_or(flat);
        let mut speed = self.velocity.dot(forward).max(0.0);

        if input.push {
            let (top, accel) = if input.crouch {
                (p.max_crouched_kick_speed, p.crouched_acceleration)
            } else {
                (p.max_standing_kick_speed, p.standing_acceleration)
            };
            if speed < top {
                speed = (speed + accel * STEP).min(top);
            }
        }
        if input.brake {
            speed = (speed - p.brake_acceleration * STEP).max(0.0);
        }
        // Slopes: gravity along the surface.
        speed += Vec3::Y.dot(forward) * p.ground_gravity * STEP;
        speed -= p.rolling_friction * speed * speed * STEP * 60.0;
        speed = speed.clamp(0.0, p.max_speed);

        // Jump when letting go of a crouch.
        if self.crouched && !input.crouch {
            self.crouched = false;
            self.velocity = forward * speed + up * p.jump_speed;
            self.on_ground = false;
            self.up = Vec3::Y;
            self.set_action(Action::Air);
            return;
        }
        self.crouched = input.crouch;

        // Walls ahead at knee height.
        let knee = self.position + up * p.forward_collision_height;
        let reach = speed * STEP + p.min_distance_to_wall;
        if let Some(hit) = world.ray(knee, knee + forward * reach) {
            if hit.normal.y.abs() < WALL_COSINE {
                speed = 0.0;
            }
        }

        let target = self.position + forward * speed * STEP;
        // Stand on whatever is under the new spot.
        let from = target + up * p.ground_snap_up;
        let to = target - up * p.ground_snap_down;
        match world.ray(from, to) {
            Some(hit) if hit.normal.y > WALL_COSINE => {
                self.position = hit.point;
                self.up = self.up.lerp(hit.normal, 0.25).normalize_or(Vec3::Y);
                self.velocity = forward * speed;
            }
            _ => {
                // Rolled off an edge.
                self.position = target;
                self.velocity = forward * speed;
                self.on_ground = false;
                self.up = Vec3::Y;
                self.set_action(Action::Air);
                return;
            }
        }

        let action = if self.action == Action::Landing && self.action_time < 0.3 {
            Action::Landing
        } else if input.crouch {
            Action::Crouching
        } else if input.push && speed < p.max_crouched_kick_speed {
            Action::Pushing
        } else if speed > 5.0 {
            Action::Rolling
        } else {
            Action::Standing
        };
        self.set_action(action);
    }

    fn air_step(&mut self, input: Input, p: &Physics, world: &World) {
        self.heading -= input.turn * p.air_rotation * STEP * 0.5;
        // As the game does: gravity divided by the hang-time stat.
        self.velocity.y += p.air_gravity / p.air_hang.max(0.01) * STEP;
        let target = self.position + self.velocity * STEP;
        // Hit something on the way?
        let from = self.position + Vec3::Y * p.forward_collision_height;
        if let Some(hit) = world.ray(from, target + Vec3::Y * p.forward_collision_height) {
            if hit.normal.y.abs() < WALL_COSINE {
                // A wall: lose the speed into it.
                let into = self.velocity.dot(hit.normal);
                if into < 0.0 {
                    self.velocity -= hit.normal * into;
                }
            }
        }
        let target = self.position + self.velocity * STEP;
        if self.velocity.y <= 0.0 {
            if let Some(hit) = world.ray(self.position + Vec3::Y * p.ground_snap_up, target) {
                if hit.normal.y > WALL_COSINE {
                    // Land: keep the speed along the surface.
                    self.position = hit.point;
                    self.up = hit.normal;
                    let along = self.velocity - hit.normal * self.velocity.dot(hit.normal);
                    self.velocity = along;
                    if along.length() > 1.0 {
                        let flat = Vec3::new(along.x, 0.0, along.z);
                        if flat.length() > 1.0 {
                            self.heading = flat.x.atan2(flat.z);
                        }
                    }
                    self.on_ground = true;
                    self.crouched = input.crouch;
                    self.set_action(Action::Landing);
                    return;
                }
            }
        }
        self.position = target;
        // Fell out of the level.
        if self.position.y < -100_000.0 {
            self.velocity = Vec3::ZERO;
        }
    }
}
