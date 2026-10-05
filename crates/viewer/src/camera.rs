use glam::{Mat4, Quat, Vec3};

/// A camera placed by a script or camera path: any orientation (it looks
/// down its local -Z), and its own field of view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScriptedCamera {
    pub rotation: Quat,
    pub position: Vec3,
    /// Vertical field of view in radians.
    pub fov_y: f32,
}

impl ScriptedCamera {
    /// A fly camera at the same spot, looking the same way (minus roll).
    pub fn to_fly(&self) -> FlyCamera {
        FlyCamera::looking_at(self.position, self.position + self.rotation * Vec3::NEG_Z)
    }
}

/// The vertical field of view that shows `horizontal` across a 4:3 screen,
/// as the game did on a television. Wider windows then see more at the sides.
pub fn vertical_fov(horizontal: f32) -> f32 {
    2.0 * ((horizontal / 2.0).tan() * 0.75).atan()
}

/// A free-flying camera in the game's Y-up, right-handed space.
/// Yaw 0 looks down -Z; positive yaw turns right, positive pitch looks up.
#[derive(Clone, Copy, Debug)]
pub struct FlyCamera {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
}

const MAX_PITCH: f32 = 1.55;

impl FlyCamera {
    pub fn looking_at(position: Vec3, target: Vec3) -> Self {
        let dir = (target - position).normalize_or(Vec3::NEG_Z);
        Self {
            position,
            yaw: dir.x.atan2(-dir.z),
            pitch: dir.y.clamp(-1.0, 1.0).asin().clamp(-MAX_PITCH, MAX_PITCH),
        }
    }

    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(sy * cp, sp, -cy * cp)
    }

    pub fn right(&self) -> Vec3 {
        self.forward().cross(Vec3::Y).normalize_or(Vec3::X)
    }

    pub fn view(&self) -> Mat4 {
        glam::camera::rh::view::look_to_mat4(self.position, self.forward(), Vec3::Y)
    }

    /// The view without translation, for the sky, which stays centered on the camera.
    pub fn rotation_view(&self) -> Mat4 {
        glam::camera::rh::view::look_to_mat4(Vec3::ZERO, self.forward(), Vec3::Y)
    }

    pub fn turn(&mut self, yaw: f32, pitch: f32) {
        self.yaw += yaw;
        self.pitch = (self.pitch + pitch).clamp(-MAX_PITCH, MAX_PITCH);
    }

    /// Prints the camera in the form `--camera` accepts.
    pub fn describe(&self) -> String {
        format!(
            "{:.0},{:.0},{:.0},{:.1},{:.1}",
            self.position.x,
            self.position.y,
            self.position.z,
            self.yaw.to_degrees(),
            self.pitch.to_degrees()
        )
    }

    /// Parses `x,y,z,yaw,pitch` with angles in degrees.
    pub fn parse(text: &str) -> Result<Self, String> {
        let values: Vec<f32> = text
            .split(',')
            .map(|v| v.trim().parse::<f32>().map_err(|e| format!("{v:?}: {e}")))
            .collect::<Result<_, _>>()?;
        let [x, y, z, yaw, pitch] = values[..] else {
            return Err("expected x,y,z,yaw,pitch".into());
        };
        Ok(Self {
            position: Vec3::new(x, y, z),
            yaw: yaw.to_radians(),
            pitch: pitch.to_radians().clamp(-MAX_PITCH, MAX_PITCH),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looking_at_points_the_camera_at_the_target() {
        let cam = FlyCamera::looking_at(Vec3::new(10.0, 5.0, 3.0), Vec3::new(-2.0, 1.0, 8.0));
        let expected = (Vec3::new(-2.0, 1.0, 8.0) - cam.position).normalize();
        assert!(cam.forward().abs_diff_eq(expected, 1e-5));
    }

    #[test]
    fn yaw_zero_looks_down_negative_z_with_x_to_the_right() {
        let cam = FlyCamera::parse("0,0,0,0,0").unwrap();
        assert!(cam.forward().abs_diff_eq(Vec3::NEG_Z, 1e-6));
        assert!(cam.right().abs_diff_eq(Vec3::X, 1e-6));
    }

    #[test]
    fn describe_round_trips_through_parse() {
        let cam = FlyCamera::parse("100,-20,300,45,-10").unwrap();
        let again = FlyCamera::parse(&cam.describe()).unwrap();
        assert!(again.position.abs_diff_eq(cam.position, 0.5));
        assert!((again.yaw - cam.yaw).abs() < 1e-3 && (again.pitch - cam.pitch).abs() < 1e-3);
        assert!(FlyCamera::parse("1,2,3").is_err());
    }
}
