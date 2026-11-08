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

/// Where the picture goes in a `width` by `height` window: all of it, or,
/// at a shape of its own (`aspect`, width over height), as large as fits,
/// in the middle (black bars at the sides or above and below). As `(x, y,
/// width, height)`.
pub fn picture_box(width: f32, height: f32, aspect: Option<f32>) -> (f32, f32, f32, f32) {
    let Some(aspect) = aspect.filter(|a| *a > 0.0 && width > 0.0 && height > 0.0) else {
        return (0.0, 0.0, width, height);
    };
    let (w, h) = if width / height > aspect {
        ((height * aspect).round(), height)
    } else {
        (width, (width / aspect).round())
    };
    (
        ((width - w) / 2.0).floor(),
        ((height - h) / 2.0).floor(),
        w,
        h,
    )
}

/// A camera path's vertical field of view for a window `aspect` wide: as
/// it is up to 21:9 (wider than the game's 4:3 screen, seeing more at the
/// sides: an ultrawide screen sees the skater select's stage whole, not
/// cut off above and below), and no wider than that beyond, narrowed to
/// keep 21:9's view across, so a wider one (32:9) doesn't see past the
/// sides of a set made for the game's screen (the Skate Shop's stage).
pub fn fit_fov(fov_y: f32, aspect: f32) -> f32 {
    // (21:9 as the screens sold so are: 2560x1080, 3440x1440.)
    const WIDEST: f32 = 2.4;
    if aspect <= WIDEST {
        return fov_y;
    }
    2.0 * ((fov_y / 2.0).tan() * WIDEST / aspect).atan()
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

    #[test]
    fn the_picture_box_fits_its_shape_in_the_middle() {
        // As the window is: all of it.
        assert_eq!(
            picture_box(2560.0, 1080.0, None),
            (0.0, 0.0, 2560.0, 1080.0)
        );
        // 16:9 on a 21:9 screen: bars at the sides.
        assert_eq!(
            picture_box(2560.0, 1080.0, Some(16.0 / 9.0)),
            (320.0, 0.0, 1920.0, 1080.0)
        );
        // 21:9 on a 16:9 screen: bars above and below.
        let (x, y, w, h) = picture_box(1920.0, 1080.0, Some(21.0 / 9.0));
        assert_eq!((x, w), (0.0, 1920.0));
        assert_eq!(h, 823.0);
        assert_eq!(y, 128.0);
    }

    #[test]
    fn a_very_wide_window_sees_no_more_across_than_21_by_9() {
        let fov_y = vertical_fov(72f32.to_radians());
        let across = |aspect: f32| 2.0 * ((fit_fov(fov_y, aspect) / 2.0).tan() * aspect).atan();
        // Up to 21:9: as it is.
        assert_eq!(fit_fov(fov_y, 4.0 / 3.0), fov_y);
        assert_eq!(fit_fov(fov_y, 16.0 / 9.0), fov_y);
        assert_eq!(fit_fov(fov_y, 2000.0 / 844.0), fov_y);
        assert_eq!(fit_fov(fov_y, 3440.0 / 1440.0), fov_y);
        // Wider: no more across than 21:9 sees.
        assert!((across(32.0 / 9.0) - across(2.4)).abs() < 1e-5);
    }
}
