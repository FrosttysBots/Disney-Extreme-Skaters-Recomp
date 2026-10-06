//! A chase camera behind the skater, on the game's camera settings
//! (`Skater_Camera_Standard_Medium` in `PHYSICS.q`): `behind` and `above`
//! (feet), turning towards the way the skater faces on the ground (and
//! travels in the air) at `slerp` a frame, and following at `lerp_xz`
//! across and `lerp_y` up and down. In vert air and on lips it stays
//! square to the ramp. It never ends up behind a wall: a line from the
//! skater to where it wants to be pulls it in at once, and it eases back
//! out, so nothing at the edge of the view makes it shake.
//!
//! The game's own camera code isn't ported; this uses its settings.

use glam::Vec3;

use crate::constants::Physics;
use crate::skater::Skater;
use crate::world::World;

/// Feet to the game's units (inches).
const FOOT: f32 = 12.0;
/// How close to a wall the camera may come.
const WALL_MARGIN: f32 = 10.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChaseCamera {
    pub eye: Vec3,
    /// Where it looks.
    pub target: Vec3,
    /// The flat direction it looks along.
    dir: Vec3,
    /// Where it would be with no walls about (followed smoothly; `eye` is
    /// this pulled in).
    free_eye: Vec3,
    /// How far from the target walls let it be: in at once, back out
    /// slowly, so a wall at the edge of the view doesn't make it jump.
    reach: f32,
    /// Following a skater going backwards (fakie), which flips only once
    /// it's clearly so.
    backwards: bool,
}

impl ChaseCamera {
    /// Behind a skater, settled.
    pub fn behind(skater: &Skater, p: &Physics) -> Self {
        let dir = skater.forward();
        let target = look_at(skater, p);
        let eye = target - dir * p.camera_behind * FOOT + Vec3::Y * p.camera_above * FOOT;
        ChaseCamera {
            eye,
            target,
            dir,
            free_eye: eye,
            reach: eye.distance(target),
            backwards: false,
        }
    }

    /// Follows the skater for `dt` seconds.
    pub fn update(&mut self, skater: &Skater, p: &Physics, world: &World, dt: f32) {
        // Per-frame rates (at 60 a second) for any frame length.
        let rate = |per_frame: f32| 1.0 - (1.0 - per_frame.clamp(0.0, 1.0)).powf(dt * 60.0);
        // The way to look: square to the ramp in vert air and on lips; on
        // the ground and rails the skater's facing (steady, where its
        // velocity shakes with every bump and wall), turned round when
        // it's clearly going backwards; in the air the way it's going
        // (its facing when slow), so spins don't swing the camera.
        let ramp = skater
            .vert
            .map(|v| v.out)
            .or(skater.lip.as_ref().map(|l| l.out));
        let flat = Vec3::new(skater.velocity.x, 0.0, skater.velocity.z);
        let facing = skater.forward();
        let along = flat.dot(facing);
        if along < -60.0 {
            self.backwards = true;
        } else if along > 60.0 {
            self.backwards = false;
        }
        let facing = if self.backwards { -facing } else { facing };
        let wanted_dir = match ramp {
            Some(out) => -out,
            None if skater.on_ground || skater.grind.is_some() => facing,
            None if flat.length() > 60.0 => flat.normalize(),
            None => facing,
        };
        let turn = if ramp.is_some() {
            p.camera_vert_air_slerp
        } else {
            p.camera_slerp
        };
        // Turning much faster when it has a long way to go (a skater who
        // turned round), as a fixed fraction alone would lag far behind.
        let behind = (1.0 - self.dir.dot(wanted_dir)).clamp(0.0, 2.0);
        self.dir = self
            .dir
            .lerp(wanted_dir, rate(turn * (1.0 + 4.0 * behind)))
            .normalize_or(wanted_dir);

        // The target follows the skater closely, but smooths out the small
        // jolts up and down of riding over bumps.
        let at = look_at(skater, p);
        let target = Vec3::new(
            at.x,
            self.target.y + (at.y - self.target.y) * rate(0.4),
            at.z,
        );
        let wanted = target - self.dir * p.camera_behind * FOOT + Vec3::Y * p.camera_above * FOOT;
        let (xz, y) = if ramp.is_some() {
            (p.camera_vert_air_lerp_xz, p.camera_vert_air_lerp_y)
        } else {
            (p.camera_lerp_xz, p.camera_lerp_y)
        };
        let mut eye = self.free_eye;
        let across = rate(xz);
        eye.x += (wanted.x - eye.x) * across;
        eye.z += (wanted.z - eye.z) * across;
        eye.y += (wanted.y - eye.y) * rate(y);
        self.free_eye = eye;
        // Not behind a wall: pulled in along the line from the skater,
        // short of the wall by the margin (never past the skater).
        let full = eye.distance(target);
        let allowed = match world.ray(target, eye) {
            Some(hit) => (hit.point.distance(target) - WALL_MARGIN).max(0.0),
            None => full,
        };
        self.reach = if allowed < self.reach {
            allowed
        } else {
            self.reach + (allowed - self.reach) * rate(0.08)
        };
        let reach = self.reach.min(full);
        self.eye = target + (eye - target).normalize_or_zero() * reach;
        self.target = target;
    }
}

/// A little above the skater's middle.
fn look_at(skater: &Skater, p: &Physics) -> Vec3 {
    skater.position + Vec3::Y * p.head_height * 0.6
}
