//! A chase camera behind the skater, on the game's camera settings
//! (`Skater_Camera_Standard_Medium` in `PHYSICS.q`) and the way its
//! camera update (main.dol 0x80120070) uses them:
//!
//! - It follows a focus point that lerps towards the skater, `lerp_xz` a
//!   frame across and `lerp_y` up and down (`vert_air_lerp_...` in vert
//!   air; up and down at once on a rail), and hangs `behind` it and
//!   `above` it (feet) along the way it looks (0x80122BB8).
//! - The way it looks turns at `slerp` a frame towards the way the skater
//!   faces on the ground (travels in the air), square to the ramp in vert
//!   air and on lips (`vert_air_slerp`), and for a sixth of a second after
//!   landing from vert at `vert_air_landed_slerp`, so it swings round
//!   quickly (0x801223F8).
//! - It zooms, at `zoom_lerp` a frame: `big_air_trick_zoom` during a trick
//!   in vert air, `grind_zoom` on a rail and `lip_trick_zoom` on a lip,
//!   scaling `behind`, and `above` towards 3 when closer (0x80122DB0).
//! - Rates scale with the frame time as the game's 0x8011FDFC does:
//!   `n r / (1 - r + n r)` for `n` frames.
//!
//! Our own: it never ends up behind a wall (it rises up to 120 to see over
//! what's in the way, or else a line from the focus pulls it in at once,
//! and it eases back), and it turns faster the further it has to go. The game's own collision and look-at tilt aren't ported.

use glam::Vec3;

use crate::constants::Physics;
use crate::skater::Skater;
use crate::world::World;

/// Feet to the game's units (inches).
const FOOT: f32 = 12.0;
/// The faces the camera's line meets (the game's camera feeler requires
/// flag 0x80: camera-collidable; poles and the like let it through).
const CAMERA: u16 = ngc_collision::face_flags::CAMERA_COLLIDABLE;
/// How close to a wall the camera may come.
const WALL_MARGIN: f32 = 10.0;
/// How long after vert air it turns at `vert_air_landed_slerp`.
const LANDED_TIME: f32 = 1.0 / 6.0;
/// How long after vert air it holds back from turning into a blocked view.
const HOLD_AFTER_VERT: f32 = 1.0;
/// How much higher it tries, to see over something in the way.
const LIFTS: [f32; 5] = [0.0, 30.0, 60.0, 90.0, 120.0];
/// The height `above` closes in on when zoomed in (feet).
const ZOOM_ABOVE: f32 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChaseCamera {
    pub eye: Vec3,
    /// Where it looks: the focus point.
    pub target: Vec3,
    /// The flat direction it looks along.
    dir: Vec3,
    /// How far from the target walls let it be: in at once, back out
    /// slowly, so a wall at the edge of the view doesn't make it jump.
    reach: f32,
    /// Following a skater going backwards (fakie), which flips only once
    /// it's clearly so.
    backwards: bool,
    /// The zoom (1 normally), and seconds left of the quick turn after
    /// vert air.
    zoom: f32,
    landed: f32,
    /// Raised to see over something in the way.
    lift: f32,
    /// Holding its turn while the view it's turning to is blocked, and
    /// seconds since vert air (when it does).
    holding: bool,
    since_vert: f32,
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
            reach: eye.distance(target),
            backwards: false,
            zoom: 1.0,
            landed: 0.0,
            lift: 0.0,
            holding: false,
            since_vert: f32::MAX,
        }
    }

    /// Follows the skater for `dt` seconds.
    pub fn update(&mut self, skater: &Skater, p: &Physics, world: &World, dt: f32) {
        // A per-frame rate for any frame length (0x8011FDFC).
        let frames = dt * 60.0;
        let rate = |per_frame: f32| {
            let r = per_frame.clamp(0.0, 1.0);
            (frames * r / (1.0 - r + frames * r)).clamp(0.0, 1.0)
        };
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
        self.since_vert = if ramp.is_some() {
            0.0
        } else {
            self.since_vert + dt
        };
        let turn = if ramp.is_some() {
            self.landed = LANDED_TIME;
            p.camera_vert_air_slerp
        } else if self.landed > 0.0 && skater.on_ground {
            self.landed -= dt;
            p.camera_vert_air_landed_slerp
        } else {
            self.landed = 0.0;
            p.camera_slerp
        };
        // Zooming in for a trick in vert air, on rails and lips.
        let zoom = if skater.lip.is_some() {
            p.camera_lip_trick_zoom
        } else if skater.grind.is_some() {
            p.camera_grind_zoom
        } else if skater.vert.is_some() && skater.trick.is_some() {
            p.camera_big_air_trick_zoom
        } else {
            1.0
        };
        self.zoom += (zoom - self.zoom) * rate(p.camera_zoom_lerp);

        // The focus follows the skater: across at `lerp_xz`, up and down at
        // `lerp_y` (at once on a rail).
        let at = look_at(skater, p);
        let (xz, y) = if ramp.is_some() {
            (p.camera_vert_air_lerp_xz, p.camera_vert_air_lerp_y)
        } else if skater.grind.is_some() {
            (p.camera_lerp_xz, 1.0)
        } else {
            (p.camera_lerp_xz, p.camera_lerp_y)
        };
        let across = rate(xz);
        let mut target = self.target;
        target.x += (at.x - target.x) * across;
        target.z += (at.z - target.z) * across;
        target.y += (at.y - target.y) * rate(y);

        // Behind and above the focus, as zoomed.
        let back = p.camera_behind * self.zoom;
        let above = if self.zoom < 1.0 {
            ZOOM_ABOVE + (p.camera_above - ZOOM_ABOVE) * self.zoom
        } else {
            p.camera_above
        };
        // Turning round the shorter way by angle (a straight blend of two
        // opposite directions would sit still, then flip), much faster
        // when it has a long way to go (a skater who turned round), as a
        // fixed fraction alone would lag far behind.
        let yaw = self.dir.x.atan2(self.dir.z);
        let wanted_yaw = wanted_dir.x.atan2(wanted_dir.z);
        let mut gap = (wanted_yaw - yaw).rem_euclid(std::f32::consts::TAU);
        if gap > std::f32::consts::PI {
            gap -= std::f32::consts::TAU;
        }
        let behind = 1.0 - gap.cos();
        let yaw = yaw + gap * rate(turn * (1.0 + 4.0 * behind));
        let turned = Vec3::new(yaw.sin(), 0.0, yaw.cos());
        // Not into a view something blocks (swinging round behind a skater
        // landing from a quarter pipe, into the ramp): it holds while the
        // view it has is clearer, and turns on as the skater gets clear.
        let clear = |dir: Vec3| {
            let eye = target - dir * back * FOOT + Vec3::Y * above * FOOT;
            world
                .ray_requiring(target, eye, CAMERA)
                .map_or(1.0, |hit| hit.fraction)
        };
        let (now, then) = (clear(self.dir), clear(turned));
        // (Only just after vert air: anywhere else holding back makes it
        // stutter along walls.)
        let blocked = self.since_vert < HOLD_AFTER_VERT
            && if self.holding {
                then < 0.7 && then < now
            } else {
                then < 0.5 && then < now - 0.15
            };
        self.holding = blocked;
        if !blocked {
            self.dir = turned;
        }

        let eye = target - self.dir * back * FOOT + Vec3::Y * above * FOOT;
        // Something between it and the skater (a quarter pipe it has
        // swung round behind, a low wall): up over it if a little higher
        // clears it, rising quickly and settling back slowly (the pull-in
        // below covers what's still in the way meanwhile).
        let lift = LIFTS
            .iter()
            .copied()
            .find(|&h| {
                world
                    .ray_requiring(target, eye + Vec3::Y * h, CAMERA)
                    .is_none()
            })
            .unwrap_or(0.0);
        self.lift = if lift > self.lift {
            self.lift + (lift - self.lift) * rate(0.25)
        } else {
            self.lift + (lift - self.lift) * rate(0.05)
        };
        let eye = eye + Vec3::Y * self.lift;
        // Still behind a wall: pulled in along the line from the focus,
        // short of the wall by the margin (never past the focus).
        let full = eye.distance(target);
        let allowed = match world.ray_requiring(target, eye, CAMERA) {
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

/// A little above the skater's middle, along the way it stands (on a
/// ramp's face, straight up would be inside the ramp).
fn look_at(skater: &Skater, p: &Physics) -> Vec3 {
    skater.position + skater.shown_up.normalize_or(Vec3::Y) * p.head_height * 0.6
}
