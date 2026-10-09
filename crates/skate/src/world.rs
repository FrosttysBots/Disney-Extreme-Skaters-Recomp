//! Ray casts against a level's collision, using each object's BSP tree to
//! test only the faces near the ray.

use glam::{Mat4, Vec3};
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
    /// The face's terrain (`TERRAIN_...` in `TERRAIN.q`).
    pub terrain: u16,
    /// The checksum of the collision object it belongs to (its node's
    /// name).
    pub object: u32,
}

/// Something solid that moves about, like a pedestrian: an upright
/// cylinder standing on `base`. Only lines from the side meet it (the
/// skater bumps into it but can't stand on it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Obstacle {
    pub base: Vec3,
    pub radius: f32,
    pub height: f32,
}

/// A vehicle going round the level, to skitch on: where it is, which way
/// it faces (flat), how fast it goes along that, and half its length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vehicle {
    pub position: Vec3,
    pub forward: Vec3,
    pub speed: f32,
    pub half_length: f32,
}

/// [`Hit::object`] for an [`Obstacle`].
pub const OBSTACLE: u32 = u32::MAX;

pub struct World {
    collision: Collision,
    /// The rails to grind.
    pub rails: Rails,
    /// What moves about (pedestrians), where it is now.
    obstacles: Vec<Obstacle>,
    /// The vehicles going round, where they are now.
    pub vehicles: Vec<Vehicle>,
    /// Collision objects gone (broken): passed through, triggers and all.
    disabled: std::collections::HashSet<u32>,
    /// Collision objects moved from where they're stored (the level's
    /// pieces scripts move: a door swung open): stored space to where
    /// they are, and back.
    placed: std::collections::HashMap<u32, (Mat4, Mat4)>,
}

impl World {
    pub fn new(collision: Collision) -> Self {
        World {
            collision,
            rails: Rails::default(),
            obstacles: Vec::new(),
            vehicles: Vec::new(),
            disabled: Default::default(),
            placed: Default::default(),
        }
    }

    /// Puts a collision object somewhere else: `transform` takes it from
    /// where it's stored to where it is now (`None`: back where it was).
    pub fn place(&mut self, object: u32, transform: Option<Mat4>) {
        match transform {
            Some(m) => {
                self.placed.insert(object, (m, m.inverse()));
            }
            None => {
                self.placed.remove(&object);
            }
        }
    }

    /// A line in an object's own (stored) space, if it's been moved.
    fn local(&self, object: u32, from: Vec3, to: Vec3) -> (Vec3, Vec3) {
        match self.placed.get(&object) {
            Some((_, back)) => (back.transform_point3(from), back.transform_point3(to)),
            None => (from, to),
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

    /// Puts a collision object back.
    pub fn enable(&mut self, object: u32) {
        self.disabled.remove(&object);
    }

    /// Takes a collision object out (a breakable that's been broken).
    pub fn disable(&mut self, object: u32) {
        self.disabled.insert(object);
    }

    /// What moves about, where it is now.
    pub fn obstacles(&self) -> &[Obstacle] {
        &self.obstacles
    }

    /// Where the obstacles are now.
    pub fn set_obstacles(&mut self, obstacles: Vec<Obstacle>) {
        self.obstacles = obstacles;
    }

    /// The first obstacle the line from `from` along `direction` meets
    /// from outside it, as a fraction of `direction`.
    fn obstacle_hit(&self, from: Vec3, direction: Vec3) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        let d = glam::Vec2::new(direction.x, direction.z);
        let a = d.length_squared();
        if a < 1e-6 {
            return None;
        }
        for o in &self.obstacles {
            let f = glam::Vec2::new(from.x - o.base.x, from.z - o.base.z);
            let c = f.length_squared() - o.radius * o.radius;
            if c <= 0.0 {
                // Already inside: let it go rather than trap anything.
                continue;
            }
            let b = 2.0 * f.dot(d);
            let disc = b * b - 4.0 * a * c;
            if disc < 0.0 {
                continue;
            }
            let t = (-b - disc.sqrt()) / (2.0 * a);
            if !(0.0..=1.0).contains(&t) || best.is_some_and(|h| h.fraction <= t) {
                continue;
            }
            let point = from + direction * t;
            if point.y < o.base.y || point.y > o.base.y + o.height {
                continue;
            }
            let out = f + d * t;
            best = Some(Hit {
                point,
                normal: Vec3::new(out.x, 0.0, out.y).normalize_or(Vec3::X),
                fraction: t,
                flags: face_flags::NOT_SKATABLE,
                terrain: 0,
                object: OBSTACLE,
            });
        }
        best
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

    /// The collision objects with trigger faces (flagged to run a script
    /// when the skater touches them) that the line from `from` to `to`
    /// crosses, solid or not.
    pub fn triggers(&self, world_from: Vec3, world_to: Vec3) -> Vec<u32> {
        let mut out = Vec::new();
        for object in &self.collision.objects {
            if self.disabled.contains(&object.checksum) {
                continue;
            }
            let (from, to) = self.local(object.checksum, world_from, world_to);
            let min = from.min(to) - Vec3::splat(0.5);
            let max = from.max(to) + Vec3::splat(0.5);
            let direction = to - from;
            let [x0, y0, z0, x1, y1, z1] = object.bbox;
            if max.x < x0 || min.x > x1 || max.y < y0 || min.y > y1 || max.z < z0 || min.z > z1 {
                continue;
            }
            object.bsp.faces_near(min.to_array(), max.to_array(), |f| {
                let face = &object.faces[usize::from(f)];
                if face.flags & face_flags::TRIGGER == 0 || out.contains(&object.checksum) {
                    return;
                }
                let [a, b, c] = face
                    .indices
                    .map(|i| Vec3::from(object.vertices[usize::from(i)]));
                if intersect(from, direction, a, b, c).is_some() {
                    out.push(object.checksum);
                }
            });
        }
        out
    }

    /// The first solid face between `from` and `to`. Faces flagged
    /// non-collidable (triggers, decals) are passed through.
    pub fn ray(&self, from: Vec3, to: Vec3) -> Option<Hit> {
        self.ray_requiring(from, to, 0)
    }

    /// Like [`World::ray`], hitting only faces with all of `flags` (the
    /// camera's line only meets camera-collidable faces, 0x80).
    pub fn ray_requiring(&self, world_from: Vec3, world_to: Vec3, flags: u16) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        for object in &self.collision.objects {
            if self.disabled.contains(&object.checksum) {
                continue;
            }
            // (A moved object's met in its own space: the fraction along
            // the line is the same, the point and normal go back.)
            let (from, to) = self.local(object.checksum, world_from, world_to);
            let placed = self.placed.get(&object.checksum).map(|(m, _)| *m);
            let min = from.min(to) - Vec3::splat(0.5);
            let max = from.max(to) + Vec3::splat(0.5);
            let direction = to - from;
            let [x0, y0, z0, x1, y1, z1] = object.bbox;
            if max.x < x0 || min.x > x1 || max.y < y0 || min.y > y1 || max.z < z0 || min.z > z1 {
                continue;
            }
            object.bsp.faces_near(min.to_array(), max.to_array(), |f| {
                let face = &object.faces[usize::from(f)];
                if face.flags & face_flags::NON_COLLIDABLE != 0 || face.flags & flags != flags {
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
                        let (point, normal) = match placed {
                            Some(m) => (
                                m.transform_point3(from + direction * t),
                                m.transform_vector3(normal).normalize_or_zero(),
                            ),
                            None => (from + direction * t, normal),
                        };
                        best = Some(Hit {
                            point,
                            normal,
                            fraction: t,
                            flags: face.flags,
                            terrain: face.terrain,
                            object: object.checksum,
                        });
                    }
                }
            });
        }
        // Obstacles are solid to everything but the camera.
        if flags == 0 {
            if let Some(hit) = self.obstacle_hit(world_from, world_to - world_from) {
                if best.is_none_or(|h| hit.fraction < h.fraction) {
                    best = Some(hit);
                }
            }
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
    use ngc_collision::{BspNode, BspTree, CollisionObject, Face};

    /// A world of one floor triangle round the origin, object 7.
    fn floor() -> World {
        World::new(Collision {
            objects: vec![CollisionObject {
                checksum: 7,
                flags: 0,
                bbox: [-10.0, 0.0, -10.0, 10.0, 0.0, 10.0],
                vertices: vec![[-10.0, 0.0, -10.0], [10.0, 0.0, -10.0], [0.0, 0.0, 10.0]],
                intensities: Vec::new(),
                faces: vec![Face {
                    flags: 0,
                    terrain: 0,
                    indices: [0, 1, 2],
                }],
                skipped_faces: 0,
                bsp: BspTree {
                    nodes: vec![BspNode::Leaf { first: 0, count: 1 }],
                    faces: vec![0],
                },
            }],
            repaired_fields: 0,
            repaired_bsp_fields: 0,
        })
    }

    #[test]
    fn moved_objects_are_met_where_they_are() {
        let mut world = floor();
        let down = |x: f32, z: f32| world_ray(&world, Vec3::new(x, 50.0, z));
        fn world_ray(world: &World, from: Vec3) -> Option<Vec3> {
            world.ray(from, from - Vec3::Y * 100.0).map(|h| h.point)
        }
        assert!(down(0.0, 0.0).is_some());
        // Moved 100 along x and up 5: met there, not where it was.
        world.place(7, Some(Mat4::from_translation(Vec3::new(100.0, 5.0, 0.0))));
        let down = |x: f32, z: f32| world_ray(&world, Vec3::new(x, 50.0, z));
        assert!(down(0.0, 0.0).is_none());
        let hit = down(100.0, 0.0).unwrap();
        assert!((hit.y - 5.0).abs() < 1e-4);
        // Turned on its side (about z): a ray down misses, one across hits,
        // with the normal turned too.
        world.place(7, Some(Mat4::from_rotation_z(std::f32::consts::FRAC_PI_2)));
        assert!(world_ray(&world, Vec3::new(2.0, 50.0, 0.0)).is_none());
        let hit = world
            .ray(Vec3::new(30.0, 0.0, 0.0), Vec3::new(-30.0, 0.0, 0.0))
            .unwrap();
        assert!(hit.point.x.abs() < 1e-3 && hit.normal.x > 0.99);
        // Put back.
        world.place(7, None);
        assert!(world_ray(&world, Vec3::new(0.0, 50.0, 0.0)).is_some());
    }

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
