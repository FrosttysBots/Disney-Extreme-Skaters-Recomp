//! A skater rolling over a level's collision.
//!
//! This is a first version driven by the game's constants (see
//! `constants`), not yet a frame-exact copy of the game's physics: that
//! needs the update code in `main.dol` compared against the running game.
//! What's been read from that code so far is in `docs/physics-notes.md`
//! and matches it here: pushing along the current velocity up to the kick
//! speed, drag while pushing, gravity along the whole ground plane, the
//! velocity kept along the board (it can roll backwards), the speed
//! limits, steering (sharp while braking, building up when nearly
//! stopped), air spins at the air rotation stat, and air gravity divided
//! by the hang-time stat, braking, and the ollie's strength from how long
//! the skater crouched. Ground snapping, walls and the rest
//! of the air update are still this crate's own.
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
    /// How long a turn has been held, in seconds.
    turn_time: f32,
    /// How long the skater has been crouched (tensing for an ollie).
    crouch_time: f32,
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
            turn_time: 0.0,
            crouch_time: 0.0,
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
        self.crouch_time = if input.crouch {
            self.crouch_time + STEP
        } else if self.on_ground {
            self.crouch_time
        } else {
            0.0
        };
        self.turn_time = if input.turn == 0.0 {
            0.0
        } else {
            self.turn_time + STEP
        };
        if self.on_ground {
            self.ground_step(input, p, world);
        } else {
            self.air_step(input, p, world);
        }
    }

    fn ground_step(&mut self, input: Input, p: &Physics, world: &World) {
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
            // The ollie (0x800F62C4): stronger the longer the crouch, up
            // to the max tense time; along the ground's normal when moving
            // down, else straight up.
            let tense = (self.crouch_time / p.max_tense_time).min(1.0);
            let jump = p.jump_speed_min + (p.jump_speed - p.jump_speed_min) * tense;
            let mut velocity = forward * speed;
            if velocity.y < 0.0 {
                velocity += up * jump;
            } else {
                velocity.y += jump;
            }
            self.velocity = velocity;
            self.on_ground = false;
            self.up = Vec3::Y;
            self.set_action(Action::Air);
            return;
        }
        self.crouched = input.crouch;

        // Walls ahead at knee height.
        let knee = self.position + up * p.forward_collision_height;
        let reach = speed.abs() * STEP + p.min_distance_to_wall;
        let ahead = forward * speed.signum();
        if let Some(hit) = world.ray(knee, knee + ahead * reach) {
            if hit.normal.y.abs() < WALL_COSINE {
                speed = 0.0;
            }
        }

        let target = self.position + forward * speed * STEP;
        // Stand on whatever is under the new spot.
        let from = target + up * p.ground_snap_up;
        let to = target - up * p.ground_snap_down;
        // Ground falling away more sharply than the stick angle isn't
        // followed: the skater flies off, like off the top of a kicker
        // (main.dol 0x800F52AC).
        let sticks = |normal: Vec3| {
            let turning_down =
                normal.dot(forward * speed.signum()) > up.dot(forward * speed.signum());
            !turning_down || normal.dot(up) >= p.ground_stick_angle.to_radians().cos()
        };
        match world.ray(from, to) {
            Some(hit) if hit.normal.y > WALL_COSINE && sticks(hit.normal) => {
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
        } else if speed.abs() > 5.0 {
            Action::Rolling
        } else {
            Action::Standing
        };
        self.set_action(action);
    }

    fn air_step(&mut self, input: Input, p: &Physics, world: &World) {
        // Spinning at the air rotation stat (0x800EE4CC).
        self.heading -= input.turn * p.air_rotation * STEP;
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
}
