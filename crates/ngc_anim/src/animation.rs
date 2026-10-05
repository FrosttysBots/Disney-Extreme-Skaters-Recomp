//! `.ska.ngc` bone animations.
//!
//! The header and tables are big-endian; the key streams are little-endian.
//!
//! ```text
//! u32  version (1)
//! u32  flags (0x06800000 for bone animations; camera paths use others)
//! f32  duration in seconds
//! u32  bone count
//! u32  rotation key count, u32 translation key count (totals)
//! u32  unknown (0)
//! u32  rotation stream bytes, u32 translation stream bytes (totals)
//! u16 × bones  each bone's rotation stream size
//! u16 × bones  each bone's translation stream size
//! rotation streams, then translation streams, one per bone
//! ```
//!
//! **Rotation key**: a u16 header, then data. The low 11 bits are the frame
//! (60 per second); bit 15 makes w negative.
//! - bits 11-13 (`0x3800`) clear and bit 14 (`0x4000`) set: one byte
//!   indexing the rotation table.
//! - all clear: x, y, z as i16.
//! - otherwise x, y, z in order, each a u8 if its bit is set (`0x2000` x,
//!   `0x1000` y, `0x0800` z) and an i16 if not.
//!
//! Components are quaternion x, y, z scaled by 1/16384, and
//! w = ±sqrt(1 - x² - y² - z²). The stored quaternion is the inverse of
//! the bone's rotation in the usual (column-vector) convention, so it's
//! conjugated on decode: posing Jessie that way stretches nearby vertices
//! apart by 0.25 units on average, against 3.2 or more for the alternatives.
//!
//! **Translation key**: a flag byte. With bit 6 (`0x40`) set, its low 6
//! bits are the frame; otherwise an i16 frame follows. With bit 7 (`0x80`)
//! set, one byte indexes the translation table; otherwise x, y, z follow as
//! i16. Translations are divided by 32.

use glam::{Quat, Vec3};

use crate::{Error, Result};

pub const BONE_ANIMATION_FLAGS: u32 = 0x0680_0000;
pub const FRAMES_PER_SECOND: f32 = 60.0;
const ROTATION_SCALE: f32 = 1.0 / 16384.0;
const TRANSLATION_SCALE: f32 = 1.0 / 32.0;
const HEADER_SIZE: usize = 0x24;

/// The 256 common rotations and translations that one-byte keys index.
#[derive(Debug, Clone)]
pub struct KeyTables {
    rotations: Vec<[i16; 3]>,
    translations: Vec<[i16; 3]>,
}

impl KeyTables {
    /// Parses `standardkeyq.bin` and `standardkeyt.bin`: 256 entries of
    /// four little-endian i16 each (the fourth is unused).
    pub fn parse(rotations: &[u8], translations: &[u8]) -> Result<Self> {
        let table = |data: &[u8], what: &str| -> Result<Vec<[i16; 3]>> {
            if data.len() != 256 * 8 {
                return Err(Error::Invalid(format!(
                    "{what} table is {} bytes, expected 2048",
                    data.len()
                )));
            }
            Ok(data
                .chunks_exact(8)
                .map(|e| [0, 2, 4].map(|i| i16::from_le_bytes([e[i], e[i + 1]])))
                .collect())
        };
        Ok(Self {
            rotations: table(rotations, "rotation")?,
            translations: table(translations, "translation")?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RotationKey {
    pub frame: u16,
    pub rotation: Quat,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TranslationKey {
    pub frame: i16,
    pub translation: Vec3,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BoneTrack {
    pub rotations: Vec<RotationKey>,
    pub translations: Vec<TranslationKey>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Animation {
    pub duration: f32,
    pub tracks: Vec<BoneTrack>,
}

impl Animation {
    pub fn parse(data: &[u8], tables: &KeyTables) -> Result<Self> {
        let be32 = |at: usize| {
            data.get(at..at + 4)
                .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
                .ok_or_else(|| Error::Truncated(format!("header at {at:#x}")))
        };
        if be32(0)? != 1 {
            return Err(Error::Unsupported(format!(
                "animation version {}",
                be32(0)?
            )));
        }
        let flags = be32(4)?;
        if flags != BONE_ANIMATION_FLAGS {
            return Err(Error::Unsupported(format!(
                "animation flags {flags:#010x} (camera paths aren't supported yet)"
            )));
        }
        let duration = f32::from_bits(be32(8)?);
        let bones = be32(12)? as usize;
        let (rotation_keys, translation_keys) = (be32(16)? as usize, be32(20)? as usize);
        if bones > 1024 {
            return Err(Error::Invalid(format!("{bones} bones")));
        }

        let sizes_end = HEADER_SIZE + bones * 4;
        let sizes = data
            .get(HEADER_SIZE..sizes_end)
            .ok_or_else(|| Error::Truncated("stream size tables".into()))?;
        let size = |i: usize| usize::from(u16::from_be_bytes([sizes[i * 2], sizes[i * 2 + 1]]));
        let total: usize = (0..bones * 2).map(size).sum();
        if sizes_end + total != data.len() {
            return Err(Error::Invalid(format!(
                "streams need {} bytes, the file has {}",
                sizes_end + total,
                data.len()
            )));
        }

        let mut at = sizes_end;
        let mut tracks = vec![BoneTrack::default(); bones];
        for (bone, track) in tracks.iter_mut().enumerate() {
            let len = size(bone);
            track.rotations = decode_rotations(&data[at..at + len], tables)
                .map_err(|e| Error::Invalid(format!("bone {bone} rotations: {e}")))?;
            at += len;
        }
        for (bone, track) in tracks.iter_mut().enumerate() {
            let len = size(bones + bone);
            track.translations = decode_translations(&data[at..at + len], tables)
                .map_err(|e| Error::Invalid(format!("bone {bone} translations: {e}")))?;
            at += len;
        }

        let decoded_r: usize = tracks.iter().map(|t| t.rotations.len()).sum();
        let decoded_t: usize = tracks.iter().map(|t| t.translations.len()).sum();
        if (decoded_r, decoded_t) != (rotation_keys, translation_keys) {
            return Err(Error::Invalid(format!(
                "decoded {decoded_r}/{decoded_t} keys, header says {rotation_keys}/{translation_keys}"
            )));
        }
        Ok(Self { duration, tracks })
    }

    /// Each bone's rotation and translation at `seconds` (clamped to the
    /// animation), relative to its parent.
    pub fn sample(&self, seconds: f32) -> Vec<(Quat, Vec3)> {
        let frame = seconds.clamp(0.0, self.duration) * FRAMES_PER_SECOND;
        self.tracks
            .iter()
            .map(|track| {
                (
                    sample_rotation(&track.rotations, frame),
                    sample_translation(&track.translations, frame),
                )
            })
            .collect()
    }
}

fn decode_rotations(
    stream: &[u8],
    tables: &KeyTables,
) -> std::result::Result<Vec<RotationKey>, String> {
    let mut keys = Vec::new();
    let mut at = 0;
    let byte = |at: usize| stream.get(at).copied().ok_or("stream ends inside a key");
    let i16_at = |at: usize| Ok::<i16, &str>(i16::from_le_bytes([byte(at)?, byte(at + 1)?]));
    while at < stream.len() {
        let header = i16_at(at)? as u16;
        at += 2;
        let components: [i16; 3] = if header & 0x3800 == 0 {
            if header & 0x4000 != 0 {
                let entry = tables.rotations[usize::from(byte(at)?)];
                at += 1;
                entry
            } else {
                let v = [i16_at(at)?, i16_at(at + 2)?, i16_at(at + 4)?];
                at += 6;
                v
            }
        } else {
            let mut v = [0i16; 3];
            for (c, bit) in [0x2000u16, 0x1000, 0x0800].into_iter().enumerate() {
                if header & bit != 0 {
                    v[c] = i16::from(byte(at)?);
                    at += 1;
                } else {
                    v[c] = i16_at(at)?;
                    at += 2;
                }
            }
            v
        };
        let [x, y, z] = components.map(|c| f32::from(c) * ROTATION_SCALE);
        let mut w = (1.0 - x * x - y * y - z * z).max(0.0).sqrt();
        if header & 0x8000 != 0 {
            w = -w;
        }
        // Conjugate: see the module docs.
        keys.push(RotationKey {
            frame: header & 0x7FF,
            rotation: Quat::from_xyzw(-x, -y, -z, w),
        });
    }
    Ok(keys)
}

fn decode_translations(
    stream: &[u8],
    tables: &KeyTables,
) -> std::result::Result<Vec<TranslationKey>, String> {
    let mut keys = Vec::new();
    let mut at = 0;
    let byte = |at: usize| stream.get(at).copied().ok_or("stream ends inside a key");
    let i16_at = |at: usize| Ok::<i16, &str>(i16::from_le_bytes([byte(at)?, byte(at + 1)?]));
    while at < stream.len() {
        let flags = byte(at)?;
        let frame = if flags & 0x40 != 0 {
            at += 1;
            i16::from(flags & 0x3F)
        } else {
            let f = i16_at(at + 1)?;
            at += 3;
            f
        };
        let v = if flags & 0x80 != 0 {
            let entry = tables.translations[usize::from(byte(at)?)];
            at += 1;
            entry
        } else {
            let v = [i16_at(at)?, i16_at(at + 2)?, i16_at(at + 4)?];
            at += 6;
            v
        };
        let [x, y, z] = v.map(|c| f32::from(c) * TRANSLATION_SCALE);
        keys.push(TranslationKey {
            frame,
            translation: Vec3::new(x, y, z),
        });
    }
    Ok(keys)
}

/// The two keys around `frame` and how far between them it is.
fn bracket<K>(keys: &[K], frame: f32, frame_of: impl Fn(&K) -> f32) -> Option<(&K, &K, f32)> {
    let first = keys.first()?;
    if keys.len() == 1 || frame <= frame_of(first) {
        return Some((first, first, 0.0));
    }
    let next = keys
        .iter()
        .position(|k| frame_of(k) > frame)
        .unwrap_or(keys.len());
    if next == keys.len() {
        let last = keys.last().unwrap();
        return Some((last, last, 0.0));
    }
    let (a, b) = (&keys[next - 1], &keys[next]);
    let span = frame_of(b) - frame_of(a);
    Some((
        a,
        b,
        if span > 0.0 {
            (frame - frame_of(a)) / span
        } else {
            0.0
        },
    ))
}

fn sample_rotation(keys: &[RotationKey], frame: f32) -> Quat {
    match bracket(keys, frame, |k| f32::from(k.frame)) {
        Some((a, b, t)) => {
            // Blend along the shorter path, as the game does.
            let to = if a.rotation.dot(b.rotation) < 0.0 {
                -b.rotation
            } else {
                b.rotation
            };
            a.rotation.slerp(to, t).normalize()
        }
        None => Quat::IDENTITY,
    }
}

fn sample_translation(keys: &[TranslationKey], frame: f32) -> Vec3 {
    match bracket(keys, frame, |k| f32::from(k.frame)) {
        Some((a, b, t)) => a.translation.lerp(b.translation, t),
        None => Vec3::ZERO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables() -> KeyTables {
        let mut q = vec![0u8; 2048];
        let mut t = vec![0u8; 2048];
        // Rotation entry 2: 90 degrees about X. Translation entry 1: y = 64.
        q[16..18].copy_from_slice(&11585i16.to_le_bytes());
        t[8 + 2..8 + 4].copy_from_slice(&64i16.to_le_bytes());
        KeyTables::parse(&q, &t).unwrap()
    }

    #[test]
    fn decodes_every_rotation_key_form() {
        let mut s = Vec::new();
        s.extend_from_slice(&0x4000u16.to_le_bytes()); // frame 0, table entry
        s.push(2);
        s.extend_from_slice(&6u16.to_le_bytes()); // frame 6, full components
        for v in [100i16, -200, 300] {
            s.extend_from_slice(&v.to_le_bytes());
        }
        s.extend_from_slice(&(0x8000u16 | 0x2800 | 12).to_le_bytes()); // frame 12, x and z bytes, w negative
        s.push(200);
        s.extend_from_slice(&(-50i16).to_le_bytes());
        s.push(7);
        let keys = decode_rotations(&s, &tables()).unwrap();
        assert_eq!(keys.iter().map(|k| k.frame).collect::<Vec<_>>(), [0, 6, 12]);
        // Decoded quaternions are conjugated, so x, y and z flip sign.
        assert!((keys[0].rotation.x + 0.70709).abs() < 1e-4 && keys[0].rotation.w > 0.7);
        assert!((keys[1].rotation.y - 200.0 / 16384.0).abs() < 1e-6);
        let k = keys[2].rotation;
        assert!((k.x + 200.0 / 16384.0).abs() < 1e-6 && (k.y - 50.0 / 16384.0).abs() < 1e-6);
        assert!((k.z + 7.0 / 16384.0).abs() < 1e-6 && k.w < -0.99);
    }

    #[test]
    fn decodes_every_translation_key_form() {
        let mut s = vec![0x40 | 0x80 | 3, 1]; // short frame 3, table entry 1
        s.push(0); // long frame 300, full components
        s.extend_from_slice(&300i16.to_le_bytes());
        for v in [32i16, -64, 96] {
            s.extend_from_slice(&v.to_le_bytes());
        }
        let keys = decode_translations(&s, &tables()).unwrap();
        assert_eq!(keys[0].frame, 3);
        assert_eq!(keys[0].translation, Vec3::new(0.0, 2.0, 0.0));
        assert_eq!(keys[1].frame, 300);
        assert_eq!(keys[1].translation, Vec3::new(1.0, -2.0, 3.0));
    }

    #[test]
    fn truncated_streams_are_errors() {
        assert!(decode_rotations(&[0, 0, 1], &tables()).is_err());
        assert!(decode_translations(&[0x40, 1, 2], &tables()).is_err());
    }

    #[test]
    fn samples_between_and_beyond_keys() {
        let track = BoneTrack {
            rotations: vec![
                RotationKey {
                    frame: 0,
                    rotation: Quat::IDENTITY,
                },
                RotationKey {
                    frame: 60,
                    rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
                },
            ],
            translations: vec![
                TranslationKey {
                    frame: 0,
                    translation: Vec3::ZERO,
                },
                TranslationKey {
                    frame: 60,
                    translation: Vec3::new(10.0, 0.0, 0.0),
                },
            ],
        };
        let anim = Animation {
            duration: 1.0,
            tracks: vec![track, BoneTrack::default()],
        };
        let [(r, t), (r2, t2)] = anim.sample(0.5)[..] else {
            panic!()
        };
        assert!(r.abs_diff_eq(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4), 1e-5));
        assert!(t.abs_diff_eq(Vec3::new(5.0, 0.0, 0.0), 1e-5));
        assert_eq!((r2, t2), (Quat::IDENTITY, Vec3::ZERO));
        assert!(
            anim.sample(5.0)[0]
                .1
                .abs_diff_eq(Vec3::new(10.0, 0.0, 0.0), 1e-5)
        );
    }
}
