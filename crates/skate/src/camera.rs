//! A chase camera behind the skater, on the game's camera settings
//! (`Skater_Camera_Standard_Medium` in `PHYSICS.q`): `behind` and `above`
//! (feet), turning towards the way the skater travels at `slerp` a frame,
//! and following at `lerp_xz` across and `lerp_y` up and down. In vert air
//! and on lips it stays square to the ramp. It never ends up behind a
//! wall: a line from the skater to where it wants to be pulls it in.
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
}

impl ChaseCamera {
    /// Behind a skater, settled.
    pub fn behind(skater: &Skater, p: &Physics) -> Self {
        let dir = skater.forward();
        let target = look_at(skater, p);
        ChaseCamera {
            eye: target - dir * p.camera_behind * FOOT + Vec3::Y * p.camera_above * FOOT,
            target,
            dir,
        }
    }

    /// Follows the skater for `dt` seconds.
    pub fn update(&mut self, skater: &Skater, p: &Physics, world: &World, dt: f32) {
        // Per-frame rates (at 60 a second) for any frame length.
        let rate = |per_frame: f32| 1.0 - (1.0 - per_frame.clamp(0.0, 1.0)).powf(dt * 60.0);
        // The way to look: square to the ramp in vert air and on lips,
        // else the way the skater's going (its facing when slow).
        let ramp = skater
            .vert
            .map(|v| v.out)
            .or(skater.lip.as_ref().map(|l| l.out));
        let flat = Vec3::new(skater.velocity.x, 0.0, skater.velocity.z);
        let wanted_dir = match ramp {
            Some(out) => -out,
            None if flat.length() > 60.0 => flat.normalize(),
            None => skater.forward(),
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

        let target = look_at(skater, p);
        let wanted = target - self.dir * p.camera_behind * FOOT + Vec3::Y * p.camera_above * FOOT;
        let (xz, y) = if ramp.is_some() {
            (p.camera_vert_air_lerp_xz, p.camera_vert_air_lerp_y)
        } else {
            (p.camera_lerp_xz, p.camera_lerp_y)
        };
        let mut eye = self.eye;
        let across = rate(xz);
        eye.x += (wanted.x - eye.x) * across;
        eye.z += (wanted.z - eye.z) * across;
        eye.y += (wanted.y - eye.y) * rate(y);
        // Not behind a wall: pulled in along the line from the skater.
        if let Some(hit) = world.ray(target, eye) {
            // Short of the wall by the margin, but never past the skater.
            let reach = hit.point.distance(target);
            let keep = ((reach - WALL_MARGIN) / reach.max(0.001)).max(0.0);
            eye = target + (hit.point - target) * keep;
        }
        self.eye = eye;
        self.target = target;
    }
}

/// A little above the skater's middle.
fn look_at(skater: &Skater, p: &Physics) -> Vec3 {
    skater.position + Vec3::Y * p.head_height * 0.6
}
