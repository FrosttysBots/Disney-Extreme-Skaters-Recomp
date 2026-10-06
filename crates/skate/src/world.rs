//! Ray casts against a level's collision, using each object's BSP tree to
//! test only the faces near the ray.

use glam::Vec3;
use ngc_collision::{Collision, face_flags};

use crate::rails::Rails;

/// Where a ray hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub point: Vec3,
    /// Facing the ray's start.
    pub normal: Vec3,
    /// How far along the ray, from 0 to 1.
    pub fraction: f32,
    pub flags: u16,
}

pub struct World {
    collision: Collision,
    /// The rails to grind.
    pub rails: Rails,
}

impl World {
    pub fn new(collision: Collision) -> Self {
        World {
            collision,
            rails: Rails::default(),
        }
    }

    /// The bottom of the level's collision; nothing to land on below it
    /// (no bottom without any collision).
    pub fn floor(&self) -> f32 {
        self.collision
            .objects
            .iter()
            .map(|o| o.bbox[1])
            .reduce(f32::min)
            .unwrap_or(f32::NEG_INFINITY)
    }

    pub fn with_rails(mut self, rails: Rails) -> Self {
        self.rails = rails;
        self
    }

    /// The first face between `from` and `to` that `keep` accepts, looking
    /// past (up to a few) faces it doesn't.
    pub fn ray_past(&self, from: Vec3, to: Vec3, keep: impl Fn(&Hit) -> bool) -> Option<Hit> {
        let direction = (to - from).normalize_or_zero();
        let mut start = from;
        for _ in 0..8 {
            let hit = self.ray(start, to)?;
            if keep(&hit) {
                return Some(hit);
            }
            start = hit.point + direction * 0.01;
        }
        None
    }

    /// The first solid face between `from` and `to`. Faces flagged
    /// non-collidable (triggers, decals) are passed through.
    pub fn ray(&self, from: Vec3, to: Vec3) -> Option<Hit> {
        let min = from.min(to) - Vec3::splat(0.5);
        let max = from.max(to) + Vec3::splat(0.5);
        let direction = to - from;
        let mut best: Option<Hit> = None;
        for object in &self.collision.objects {
            let [x0, y0, z0, x1, y1, z1] = object.bbox;
            if max.x < x0 || min.x > x1 || max.y < y0 || min.y > y1 || max.z < z0 || min.z > z1 {
                continue;
            }
            object.bsp.faces_near(min.to_array(), max.to_array(), |f| {
                let face = &object.faces[usize::from(f)];
                if face.flags & face_flags::NON_COLLIDABLE != 0 {
                    return;
                }
                let [a, b, c] = face
                    .indices
                    .map(|i| Vec3::from(object.vertices[usize::from(i)]));
                if let Some(t) = intersect(from, direction, a, b, c) {
                    if best.is_none_or(|h| t < h.fraction) {
                        let mut normal = (b - a).cross(c - a).normalize_or_zero();
                        if normal.dot(direction) > 0.0 {
                            normal = -normal;
                        }
                        best = Some(Hit {
                            point: from + direction * t,
                            normal,
                            fraction: t,
                            flags: face.flags,
                        });
                    }
                }
            });
        }
        best
    }
}

/// Möller-Trumbore: the fraction along `direction` where the ray from
/// `origin` crosses triangle `a b c`, if it does within the segment.
fn intersect(origin: Vec3, direction: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let (e1, e2) = (b - a, c - a);
    let p = direction.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = origin - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = direction.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (0.0..=1.0).contains(&t).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rays_hit_triangles_within_the_segment() {
        let (a, b, c) = (
            Vec3::new(-10.0, 0.0, -10.0),
            Vec3::new(10.0, 0.0, -10.0),
            Vec3::new(0.0, 0.0, 10.0),
        );
        let down = Vec3::new(0.0, -20.0, 0.0);
        assert_eq!(
            intersect(Vec3::new(0.0, 10.0, 0.0), down, a, b, c),
            Some(0.5)
        );
        // Too short, or beside the triangle.
        assert_eq!(intersect(Vec3::new(0.0, 30.0, 0.0), down, a, b, c), None);
        assert_eq!(intersect(Vec3::new(50.0, 10.0, 0.0), down, a, b, c), None);
    }
}
