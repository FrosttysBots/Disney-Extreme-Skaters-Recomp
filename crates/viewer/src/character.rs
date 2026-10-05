//! Playable characters: a skinned model and its board, posed by an
//! animation on the CPU every frame.
//!
//! Posing follows `ngc_anim::pose`: bone matrices for the animation at
//! some time, times the inverse of the rest pose (the `default` animation
//! at time 0), blending two bones per vertex. The board is skinned to the
//! character's own skeleton (bone 1, with the trucks on bones 2 and 3), so
//! one set of matrices moves both. A character has about 2,500 vertices,
//! which takes well under a millisecond.

use anyhow::{Context, Result, bail};
use glam::{Mat4, Vec3};
use ngc_anim::{Animation, KeyTables, Skeleton, pose};
use ngc_model::Scene;

use crate::level::{Level, Vertex};
use crate::source::CharacterFiles;

pub struct Character {
    /// Materials, textures and triangles, with the vertices in the rest pose.
    pub mesh: Level,
    /// For each vertex of `mesh`: its two bones and their weights.
    influences: Vec<([u8; 2], [f32; 2])>,
    skeleton: Skeleton,
    /// Model-space bone matrices of the rest pose.
    rest: Vec<Mat4>,
    /// Sorted by name. Only animations that fit the skeleton are kept: a
    /// few files were made for another character's skeleton (Jane's
    /// `Crouch` has Jessie's 39 bones, for example).
    pub animations: Vec<(String, Animation)>,
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
        let local = self.animations[index].1.sample(seconds);
        let posed = pose::model_space(&self.skeleton, &local);
        let matrices: Vec<Mat4> = pose::skinning(&self.rest, &posed)
            .into_iter()
            .map(|m| placement * m)
            .collect();
        let matrix = |b: u8| matrices.get(usize::from(b)).copied().unwrap_or(placement);
        self.mesh
            .vertices
            .iter()
            .zip(&self.influences)
            .map(|(v, &(bones, weights))| {
                let normal = Vec3::from(v.normal);
                let normal = matrix(bones[0]).transform_vector3(normal) * weights[0]
                    + matrix(bones[1]).transform_vector3(normal) * weights[1];
                Vertex {
                    position: pose::skin_point(&matrices, bones, weights, v.position.into()).into(),
                    normal: normal.normalize_or_zero().into(),
                    ..*v
                }
            })
            .collect()
    }
}

/// Bones and weights for every vertex, in the order `Level` lays them out.
/// Vertices outside skinned sectors follow the root bone.
fn influences(skin: &[u8]) -> Result<Vec<([u8; 2], [f32; 2])>> {
    let scene = Scene::parse(skin).context("could not parse the model")?;
    let mut out = Vec::new();
    for sector in &scene.sectors {
        for i in 0..sector.positions.len() {
            out.push(match &sector.skin {
                Some(skin) => (skin.bones[i], skin.weights[i]),
                None => ([0, 0], [1.0, 0.0]),
            });
        }
    }
    Ok(out)
}
