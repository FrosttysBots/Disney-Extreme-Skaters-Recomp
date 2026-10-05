//! Turning sampled bone transforms into matrices.
//!
//! Animation keys are relative to each bone's parent. Characters are
//! modeled in the pose of their `default` animation, so a vertex follows a
//! bone by `pose * inverse(rest)`: undo the rest pose, apply the new one.

use glam::{Mat4, Quat, Vec3};

use crate::Skeleton;

/// Model-space matrix for every bone, from parent-relative transforms.
pub fn model_space(skeleton: &Skeleton, local: &[(Quat, Vec3)]) -> Vec<Mat4> {
    let mut world: Vec<Option<Mat4>> = vec![None; skeleton.bones.len()];
    fn resolve(
        i: usize,
        skeleton: &Skeleton,
        local: &[(Quat, Vec3)],
        world: &mut Vec<Option<Mat4>>,
    ) -> Mat4 {
        if let Some(m) = world[i] {
            return m;
        }
        let (rotation, translation) = local
            .get(i)
            .copied()
            .unwrap_or((Quat::IDENTITY, Vec3::ZERO));
        let own = Mat4::from_rotation_translation(rotation, translation);
        let m = match skeleton.bones[i].parent {
            // Guard against malformed cycles: a bone can't be its own ancestor.
            Some(p) if p != i => resolve(p, skeleton, local, world) * own,
            _ => own,
        };
        world[i] = Some(m);
        m
    }
    (0..skeleton.bones.len())
        .map(|i| resolve(i, skeleton, local, &mut world))
        .collect()
}

/// Matrices that move rest-pose vertices into the given pose.
pub fn skinning(rest: &[Mat4], posed: &[Mat4]) -> Vec<Mat4> {
    rest.iter()
        .zip(posed)
        .map(|(r, p)| *p * r.inverse())
        .collect()
}

/// Moves a rest-pose position by weighted bones (bone index, weight).
/// Weights are normalized; with none at all, the first bone moves it.
pub fn skin_point(matrices: &[Mat4], influences: &[(u8, f32)], point: Vec3) -> Vec3 {
    blend(matrices, influences, |m| m.transform_point3(point))
}

/// Like [`skin_point`] for a direction, such as a normal (not renormalized).
pub fn skin_vector(matrices: &[Mat4], influences: &[(u8, f32)], vector: Vec3) -> Vec3 {
    blend(matrices, influences, |m| m.transform_vector3(vector))
}

fn blend(matrices: &[Mat4], influences: &[(u8, f32)], apply: impl Fn(&Mat4) -> Vec3) -> Vec3 {
    let m = |b: u8| {
        matrices
            .get(usize::from(b))
            .copied()
            .unwrap_or(Mat4::IDENTITY)
    };
    let total: f32 = influences.iter().map(|&(_, w)| w).sum();
    if total <= 0.0 {
        return influences
            .first()
            .map_or(apply(&Mat4::IDENTITY), |&(b, _)| apply(&m(b)));
    }
    influences
        .iter()
        .filter(|&&(_, w)| w != 0.0)
        .map(|&(b, w)| apply(&m(b)) * (w / total))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Bone;

    fn chain() -> Skeleton {
        Skeleton {
            checksum: 0,
            bones: vec![
                Bone {
                    name: 1,
                    parent: None,
                    mirror: None,
                },
                Bone {
                    name: 2,
                    parent: Some(0),
                    mirror: None,
                },
            ],
        }
    }

    #[test]
    fn children_follow_their_parents() {
        let local = [
            (
                Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
                Vec3::ZERO,
            ),
            (Quat::IDENTITY, Vec3::new(10.0, 0.0, 0.0)),
        ];
        let world = model_space(&chain(), &local);
        assert!(
            world[1]
                .transform_point3(Vec3::ZERO)
                .abs_diff_eq(Vec3::new(0.0, 10.0, 0.0), 1e-5)
        );
    }

    #[test]
    fn the_rest_pose_leaves_vertices_in_place() {
        let local = [
            (Quat::from_rotation_x(0.3), Vec3::Y),
            (Quat::from_rotation_y(0.2), Vec3::X),
        ];
        let rest = model_space(&chain(), &local);
        let skin = skinning(&rest, &rest);
        let p = Vec3::new(1.0, 2.0, 3.0);
        assert!(skin_point(&skin, &[(0, 0.5), (1, 0.5)], p).abs_diff_eq(p, 1e-5));
    }

    #[test]
    fn weights_blend_and_normalize() {
        let matrices = [
            Mat4::IDENTITY,
            Mat4::from_translation(Vec3::X * 10.0),
            Mat4::from_translation(Vec3::Y * 10.0),
        ];
        let p = skin_point(&matrices, &[(0, 0.5), (1, 0.25), (2, 0.25)], Vec3::ZERO);
        assert!(p.abs_diff_eq(Vec3::new(2.5, 2.5, 0.0), 1e-5));
        // Weights that don't add up to one are scaled to.
        let p = skin_point(&matrices, &[(1, 0.2), (2, 0.2)], Vec3::ZERO);
        assert!(p.abs_diff_eq(Vec3::new(5.0, 5.0, 0.0), 1e-5));
        // Translations don't move directions.
        assert_eq!(skin_vector(&matrices, &[(1, 1.0)], Vec3::Z), Vec3::Z);
    }
}
