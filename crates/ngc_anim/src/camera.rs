//! Camera paths: `.ska.ngc` files for cutscenes, goal intros and level
//! fly-throughs, all big-endian.
//!
//! ```text
//! u32  version (1)
//! u32  flags: 0x1E000000, or 0x1E400000 when key counts are u16
//! f32  duration in seconds
//! u32  track count (always 1)
//! u32  rotation key count, u32 translation key count
//! u32  custom key count
//! u8 rotation count, u8 translation count   (u16 each with 0x00400000)
//! rotation keys:    u16 frame, u16 0, f32 x, y, z
//! translation keys: u16 frame, u16 0, f32 x, y, z
//! custom keys:      u32 frame, u32 type (1), u32 size (16), f32 value
//! ```
//!
//! Frames are at 60 per second. A rotation key's bit 15 makes w negative,
//! with w rebuilt from x, y and z as in bone animations, and the stored
//! quaternion is again the inverse of the camera's orientation. The camera
//! looks down its local -Z: the view rays of the 93 moving paths meet at
//! their subject with a median error of 0.4 degrees that way, against 50 or
//! more for every other axis and convention. Positions are in the level
//! meshes' space (not mirrored like node positions): 752 of the hub's 792
//! sampled positions are within 600 units of its collision, against none
//! mirrored.
//!
//! Every custom key on the disc is type 1, a field of view in radians
//! (usually 72 degrees, also 45, 50, 54, 60, 66, 70 and 100). It's taken
//! to be horizontal, as elsewhere in the engine family.
//!
//! **Durations** often run past the last key: the hub's warp paths say
//! 55.9 seconds but their keys end at 11, and 1-key paths are fixed shots
//! held for a while. The game's scripts decide when to cut away;
//! [`CameraPath::last_key_time`] gives the end of the movement.
//!
//! **Frame numbers** suffer the export tool's `0x20` -> `0x00` corruption
//! (frame 32 is stored as 0, 288 as 256). A frame that doesn't come after
//! the one before gets its low byte restored to `0x20`. The floats are
//! unaffected: no key strays from the line between its neighbors.

use glam::{Quat, Vec3};

use crate::animation::{FRAMES_PER_SECOND, sample_rotation, sample_translation};
use crate::{Error, Result, RotationKey, TranslationKey};

pub const CAMERA_FLAGS: u32 = 0x1E00_0000;
/// Key counts are u16 instead of u8.
const WIDE_COUNTS: u32 = 0x0040_0000;
const HEADER_SIZE: usize = 0x1C;
const KEY_SIZE: usize = 16;
/// The only custom key type on the disc.
const FOV_KEY: u32 = 1;

/// Whether `data` is a camera path rather than a bone animation.
pub fn is_camera_path(data: &[u8]) -> bool {
    data.get(4..8)
        .is_some_and(|f| u32::from_be_bytes(f.try_into().unwrap()) & !WIDE_COUNTS == CAMERA_FLAGS)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FovKey {
    pub frame: u32,
    /// Horizontal field of view in radians.
    pub fov: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CameraPath {
    pub duration: f32,
    pub rotations: Vec<RotationKey>,
    pub translations: Vec<TranslationKey>,
    pub fov: Vec<FovKey>,
    /// Frame numbers that were repaired (see the module docs).
    pub repaired_frames: usize,
}

/// Where the camera is at some moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraSample {
    /// The camera's orientation; it looks down its local -Z.
    pub rotation: Quat,
    pub position: Vec3,
    /// Horizontal field of view in radians, if the path sets one.
    pub fov: Option<f32>,
}

impl CameraPath {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let be32 = |at: usize| {
            data.get(at..at + 4)
                .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
                .ok_or_else(|| Error::Truncated(format!("camera path at {at:#x}")))
        };
        if be32(0)? != 1 {
            return Err(Error::Unsupported(format!(
                "camera path version {}",
                be32(0)?
            )));
        }
        let flags = be32(4)?;
        if flags & !WIDE_COUNTS != CAMERA_FLAGS {
            return Err(Error::Unsupported(format!(
                "not a camera path (flags {flags:#010x})"
            )));
        }
        let duration = f32::from_bits(be32(8)?);
        if be32(12)? != 1 {
            return Err(Error::Unsupported(format!("{} camera tracks", be32(12)?)));
        }
        let (rotation_count, translation_count, custom_count) =
            (be32(16)? as usize, be32(20)? as usize, be32(24)? as usize);
        let keys_at = HEADER_SIZE + if flags & WIDE_COUNTS != 0 { 4 } else { 2 };
        let expected = keys_at + KEY_SIZE * (rotation_count + translation_count + custom_count);
        if data.len() != expected {
            return Err(Error::Invalid(format!(
                "{rotation_count}/{translation_count}/{custom_count} keys need {expected} bytes, the file has {}",
                data.len()
            )));
        }

        let key = |i: usize| &data[keys_at + KEY_SIZE * i..keys_at + KEY_SIZE * (i + 1)];
        let vec3 = |k: &[u8]| {
            let f = |at: usize| f32::from_be_bytes(k[at..at + 4].try_into().unwrap());
            Vec3::new(f(4), f(8), f(12))
        };
        let mut repaired_frames = 0;
        let mut frames = Frames::default();

        let mut rotations = Vec::with_capacity(rotation_count);
        for i in 0..rotation_count {
            let k = key(i);
            let raw = u16::from_be_bytes([k[0], k[1]]);
            let frame = frames.next(raw & 0x7FFF, &mut repaired_frames);
            let v = vec3(k);
            let mut w = (1.0 - v.length_squared()).max(0.0).sqrt();
            if raw & 0x8000 != 0 {
                w = -w;
            }
            // Conjugate, as with bone rotations.
            rotations.push(RotationKey {
                frame,
                rotation: Quat::from_xyzw(-v.x, -v.y, -v.z, w),
            });
        }

        frames = Frames::default();
        let mut translations = Vec::with_capacity(translation_count);
        for i in 0..translation_count {
            let k = key(rotation_count + i);
            let frame = frames.next(u16::from_be_bytes([k[0], k[1]]), &mut repaired_frames);
            let frame = i16::try_from(frame)
                .map_err(|_| Error::Invalid(format!("translation key at frame {frame}")))?;
            translations.push(TranslationKey {
                frame,
                translation: vec3(k),
            });
        }

        frames = Frames::default();
        let mut fov = Vec::with_capacity(custom_count);
        for i in 0..custom_count {
            let k = key(rotation_count + translation_count + i);
            let word = |at: usize| u32::from_be_bytes(k[at..at + 4].try_into().unwrap());
            let (kind, size) = (word(4), word(8));
            if kind != FOV_KEY || size != KEY_SIZE as u32 {
                return Err(Error::Unsupported(format!(
                    "custom key type {kind} of {size} bytes"
                )));
            }
            let frame = u16::try_from(word(0))
                .map_err(|_| Error::Invalid(format!("custom key at frame {}", word(0))))?;
            fov.push(FovKey {
                frame: u32::from(frames.next(frame, &mut repaired_frames)),
                fov: f32::from_bits(word(12)),
            });
        }

        Ok(Self {
            duration,
            rotations,
            translations,
            fov,
            repaired_frames,
        })
    }

    /// When the last key (of any kind) happens, in seconds.
    pub fn last_key_time(&self) -> f32 {
        let last = self
            .rotations
            .iter()
            .map(|k| f32::from(k.frame))
            .chain(self.translations.iter().map(|k| f32::from(k.frame)))
            .chain(self.fov.iter().map(|k| k.frame as f32))
            .fold(0.0, f32::max);
        (last / FRAMES_PER_SECOND).min(self.duration)
    }

    /// The camera at `seconds` (clamped to the path).
    pub fn sample(&self, seconds: f32) -> CameraSample {
        let frame = seconds.clamp(0.0, self.duration) * FRAMES_PER_SECOND;
        CameraSample {
            rotation: sample_rotation(&self.rotations, frame),
            position: sample_translation(&self.translations, frame),
            // Field of view changes are cuts, not blends.
            fov: self
                .fov
                .iter()
                .take_while(|k| k.frame as f32 <= frame)
                .last()
                .or(self.fov.first())
                .map(|k| k.fov),
        }
    }
}

/// Repairs frame numbers in one key list (see the module docs).
#[derive(Default)]
struct Frames {
    previous: Option<u16>,
}

impl Frames {
    fn next(&mut self, stored: u16, repaired: &mut usize) -> u16 {
        let mut frame = stored;
        if let Some(previous) = self.previous {
            if frame <= previous && frame & 0xFF == 0 && (frame | 0x20) > previous {
                frame |= 0x20;
                *repaired += 1;
            }
        }
        self.previous = Some(frame);
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(
        wide: bool,
        rotations: &[(u16, [f32; 3])],
        translations: &[(u16, [f32; 3])],
        fov: &[(u32, f32)],
    ) -> Vec<u8> {
        let mut d = Vec::new();
        let flags = if wide {
            CAMERA_FLAGS | WIDE_COUNTS
        } else {
            CAMERA_FLAGS
        };
        for w in [
            1,
            flags,
            2.0f32.to_bits(),
            1,
            rotations.len() as u32,
            translations.len() as u32,
            fov.len() as u32,
        ] {
            d.extend_from_slice(&w.to_be_bytes());
        }
        if wide {
            d.extend_from_slice(&(rotations.len() as u16).to_be_bytes());
            d.extend_from_slice(&(translations.len() as u16).to_be_bytes());
        } else {
            d.extend_from_slice(&[rotations.len() as u8, translations.len() as u8]);
        }
        for &(frame, v) in rotations.iter().chain(translations) {
            d.extend_from_slice(&frame.to_be_bytes());
            d.extend_from_slice(&[0, 0]);
            for c in v {
                d.extend_from_slice(&c.to_be_bytes());
            }
        }
        for &(frame, value) in fov {
            for w in [frame, FOV_KEY, KEY_SIZE as u32, value.to_bits()] {
                d.extend_from_slice(&w.to_be_bytes());
            }
        }
        d
    }

    #[test]
    fn parses_and_samples_a_path() {
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let data = path(
            false,
            &[(0, [0.0, 0.0, 0.0]), (60 | 0x8000, [0.0, s, 0.0])],
            &[(0, [0.0, 10.0, 0.0]), (60, [100.0, 10.0, 0.0])],
            &[(0, 1.25), (30, 0.8)],
        );
        assert!(is_camera_path(&data));
        let p = CameraPath::parse(&data).unwrap();
        assert_eq!(p.duration, 2.0);
        // Bit 15 makes w negative, and the result is conjugated.
        assert!(
            p.rotations[1]
                .rotation
                .abs_diff_eq(Quat::from_xyzw(0.0, -s, 0.0, -s), 1e-6)
        );
        let half = p.sample(0.5);
        assert!(half.position.abs_diff_eq(Vec3::new(50.0, 10.0, 0.0), 1e-4));
        assert_eq!(half.fov, Some(0.8));
        assert_eq!(p.last_key_time(), 1.0);
        assert_eq!(p.sample(0.1).fov, Some(1.25));
    }

    #[test]
    fn repairs_frames_and_reads_wide_counts() {
        // Frames 0, 16, 32 (stored as 0), 48.
        let keys = [
            (0, [0.0; 3]),
            (16, [1.0, 0.0, 0.0]),
            (0, [2.0, 0.0, 0.0]),
            (48, [3.0, 0.0, 0.0]),
        ];
        let p = CameraPath::parse(&path(true, &keys[..1], &keys, &[])).unwrap();
        let frames: Vec<i16> = p.translations.iter().map(|k| k.frame).collect();
        assert_eq!(frames, [0, 16, 32, 48]);
        assert_eq!(p.repaired_frames, 1);
        assert_eq!(p.sample(0.0).fov, None);
    }

    #[test]
    fn rejects_bone_animations_and_bad_sizes() {
        let mut data = path(false, &[(0, [0.0; 3])], &[(0, [0.0; 3])], &[]);
        data.push(0);
        assert!(matches!(CameraPath::parse(&data), Err(Error::Invalid(_))));
        data[4..8].copy_from_slice(&0x0680_0000u32.to_be_bytes());
        assert!(!is_camera_path(&data));
        assert!(matches!(
            CameraPath::parse(&data),
            Err(Error::Unsupported(_))
        ));
    }
}
