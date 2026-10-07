//! Playable characters: a skinned model and its board, posed by an
//! animation on the CPU every frame.
//!
//! Posing follows `ngc_anim::pose`: bone matrices for the animation at
//! some time, times the inverse of the rest pose (the `default` animation
//! at time 0), blending two bones per vertex. The board is skinned to the
//! character's own skeleton (bone 1, with the trucks on bones 2 and 3), so
//! one set of matrices moves both. A character has about 2,500 vertices,
//! which takes well under a millisecond.
//!
//! **Blinking.** Most characters' eyes are a small mesh textured with
//! `eyes.png`, and every such model also carries three frames on tiny
//! quads hidden inside the body: `eyes00` (open), `eyes01` (half shut)
//! and `eyes03` (shut). The names come from the created-skater script
//! (`disneytricks.qb`), which replaces all four together; the texture
//! checksums are the same in every character. The GameCube executable
//! never refers to them, though (no checksum, name or format string, and
//! `CSkinComponent::replace_texture` is a stub there), so the original
//! may not blink. [`Blink`] plays them anyway, on a timing of our own.

use anyhow::{Context, Result, bail};
use glam::{Mat4, Quat, Vec3};
use ngc_anim::{Animation, KeyTables, Skeleton, pose};
use ngc_model::Scene;

use crate::level::{Level, Vertex};
use crate::source::CharacterFiles;

pub struct Character {
    /// Materials, textures and triangles, with the vertices in the rest pose.
    pub mesh: Level,
    /// For each vertex of `mesh`: its bones and their weights.
    influences: Vec<[(u8, f32); 3]>,
    skeleton: Skeleton,
    /// Model-space bone matrices of the rest pose.
    rest: Vec<Mat4>,
    /// Sorted by name. Only animations that fit the skeleton are kept: a
    /// few files were made for another character's skeleton (Jane's
    /// `Crouch` has Jessie's 39 bones, for example).
    pub animations: Vec<(String, Animation)>,
    /// `None` for characters without blink frames (Tantor, Tarzan, Woody
    /// and Zurg).
    pub blink: Option<Blink>,
}

impl Character {
    pub fn from_files(files: &CharacterFiles) -> Result<Self> {
        let tables = KeyTables::parse(&files.key_tables.0, &files.key_tables.1)
            .context("could not read the animation key tables")?;
        let skeleton = Skeleton::parse(&files.skeleton).context("could not read the skeleton")?;
        let animations: Vec<(String, Animation)> = files
            .animations
            .iter()
            .filter_map(|(name, data)| {
                let animation = Animation::parse(data, &tables).ok()?;
                (animation.tracks.len() == skeleton.bones.len()).then(|| (name.clone(), animation))
            })
            .collect();
        let rest = animations
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("default"))
            .map(|(_, a)| pose::model_space(&skeleton, &a.sample(0.0)))
            .context("the character has no default animation (its rest pose)")?;

        let mut mesh = Level::from_bytes(&files.skin, files.textures.as_deref())
            .context("could not load the model")?;
        let mut influences = influences(&files.skin)?;
        if let Some((board, textures)) = &files.board {
            mesh.append(
                Level::from_bytes(board, textures.as_deref())
                    .context("could not load the board")?,
            );
            influences.extend(self::influences(board)?);
        }
        if influences.len() != mesh.vertices.len() {
            bail!(
                "{} vertices but {} bone weights",
                mesh.vertices.len(),
                influences.len()
            );
        }
        Ok(Self {
            blink: Blink::find(&mesh),
            mesh,
            influences,
            skeleton,
            rest,
            animations,
        })
    }

    pub fn animation(&self, name: &str) -> Option<usize> {
        self.animations
            .iter()
            .position(|(n, _)| n.eq_ignore_ascii_case(name))
    }

    /// The mesh's vertices posed by animation `index` at `seconds`, then
    /// moved into the world by `placement`.
    pub fn pose(&self, index: usize, seconds: f32, placement: Mat4) -> Vec<Vertex> {
        self.pose_local(&self.local_pose(index, seconds), placement)
    }

    /// Each bone's transform (relative to its parent) in animation `index`
    /// at `seconds`.
    pub fn local_pose(&self, index: usize, seconds: f32) -> Vec<(Quat, Vec3)> {
        self.animations[index].1.sample(seconds)
    }

    /// The mesh's vertices in a pose of parent-relative bone transforms
    /// (such as two animations blended), moved by `placement`.
    pub fn pose_local(&self, local: &[(Quat, Vec3)], placement: Mat4) -> Vec<Vertex> {
        let posed = pose::model_space(&self.skeleton, local);
        let matrices: Vec<Mat4> = pose::skinning(&self.rest, &posed)
            .into_iter()
            .map(|m| placement * m)
            .collect();
        self.mesh
            .vertices
            .iter()
            .zip(&self.influences)
            .map(|(v, influences)| Vertex {
                position: pose::skin_point(&matrices, influences, v.position.into()).into(),
                normal: pose::skin_vector(&matrices, influences, v.normal.into())
                    .normalize_or_zero()
                    .into(),
                ..*v
            })
            .collect()
    }
}

/// Texture checksums of the eye textures, the same in every character.
pub mod eye_textures {
    /// `eyes.png`, on the eye mesh.
    pub const EYES: u32 = 0x2c5a_cac0;
    /// `eyes00.png`, open (the same picture as `EYES`).
    pub const OPEN: u32 = 0x1cfe_f7d0;
    /// `eyes01.png`, half shut.
    pub const HALF: u32 = 0x21fc_bf50;
    /// `eyes03.png`, shut.
    pub const SHUT: u32 = 0x2bf5_5450;
}

/// A character's blink: which texture slot the eyes use, and the frames to
/// show instead while blinking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Blink {
    pub eyes: usize,
    pub half: usize,
    pub shut: usize,
}

/// Each blink: half shut, shut, half shut (in seconds, ending at each).
const BLINK_STEPS: [(f32, bool); 3] = [(0.05, false), (0.117, true), (0.167, false)];
/// One blink per this many seconds, at a varying point in each.
const BLINK_PERIOD: f32 = 3.5;

impl Blink {
    fn find(mesh: &Level) -> Option<Self> {
        Some(Self {
            eyes: mesh.texture_slot(eye_textures::EYES)?,
            half: mesh.texture_slot(eye_textures::HALF)?,
            shut: mesh.texture_slot(eye_textures::SHUT)?,
        })
    }

    /// The texture slot the eyes show `seconds` after a character appears.
    /// Blinks come 1.5 to 5.5 seconds apart, never in a regular rhythm.
    pub fn texture_at(&self, seconds: f32) -> usize {
        let period = (seconds / BLINK_PERIOD).floor();
        // A fixed pseudo-random start within each period.
        let hash = (period as u32).wrapping_mul(0x9E37_79B9).rotate_left(13);
        let start = period * BLINK_PERIOD + 0.5 + (hash % 2000) as f32 / 1000.0;
        let into = seconds - start;
        if into < 0.0 {
            return self.eyes;
        }
        match BLINK_STEPS.iter().find(|(end, _)| into < *end) {
            Some((_, true)) => self.shut,
            Some((_, false)) => self.half,
            None => self.eyes,
        }
    }
}

/// Bones and weights for every vertex, in the order `Level` lays them out.
/// Vertices outside skinned sectors follow the root bone.
fn influences(skin: &[u8]) -> Result<Vec<[(u8, f32); 3]>> {
    let scene = Scene::parse(skin).context("could not parse the model")?;
    let mut out = Vec::new();
    for sector in &scene.sectors {
        match &sector.skin {
            Some(skin) => out.extend(skin.influences()),
            None => out.extend(std::iter::repeat_n(
                [(0, 1.0), (0, 0.0), (0, 0.0)],
                sector.positions.len(),
            )),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blinks_close_and_reopen() {
        let blink = Blink {
            eyes: 1,
            half: 2,
            shut: 3,
        };
        let frames: Vec<usize> = (0..60 * 20)
            .map(|frame| blink.texture_at(frame as f32 / 60.0))
            .collect();
        // Open most of the time, and every blink goes half, shut, half.
        let open = frames.iter().filter(|&&s| s == 1).count();
        assert!(open > frames.len() * 9 / 10);
        let mut changes: Vec<usize> = frames.clone();
        changes.dedup();
        assert!(changes.len() >= 4 * 4, "too few blinks: {changes:?}");
        for w in changes.windows(4).filter(|w| w[0] == 1) {
            assert_eq!(w[1..], [2, 3, 2]);
        }
    }
}
