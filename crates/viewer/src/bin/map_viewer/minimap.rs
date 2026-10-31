//! A map of the level from above, drawn from its collision: each pixel the
//! height of the highest face over it, shaded, so ramps, ledges and drops
//! read at a glance. The panel shows the part round the skater, with what's
//! there to find marked on it.
use glam::Vec3;

use desa_viewer::collision::ColorVertex;

/// Pixels across the whole level's map, the longer way.
const SIZE: usize = 512;

/// The level seen from above.
pub struct Minimap {
    pub width: usize,
    pub height: usize,
    /// Grey-blue pixels, row by row (RGBA).
    pub pixels: Vec<u8>,
    /// The level's corner (lowest x and z) and units per pixel.
    pub origin: (f32, f32),
    pub scale: f32,
}

impl Minimap {
    /// Draws the map from the collision's triangles.
    pub fn new(collision: &[ColorVertex]) -> Option<Self> {
        let mut min = (f32::INFINITY, f32::INFINITY, f32::INFINITY);
        let mut max = (f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for v in collision {
            let [x, y, z] = v.position;
            min = (min.0.min(x), min.1.min(y), min.2.min(z));
            max = (max.0.max(x), max.1.max(y), max.2.max(z));
        }
        if !(max.0 > min.0 && max.2 > min.2) {
            return None;
        }
        let scale = (max.0 - min.0).max(max.2 - min.2) / SIZE as f32;
        let width = ((max.0 - min.0) / scale).ceil() as usize + 1;
        let height = ((max.2 - min.2) / scale).ceil() as usize + 1;
        let mut top = vec![f32::NEG_INFINITY; width * height];
        for tri in collision.chunks_exact(3) {
            let p = [&tri[0], &tri[1], &tri[2]].map(|v| {
                let [x, y, z] = v.position;
                ((x - min.0) / scale, y, (z - min.2) / scale)
            });
            let (x0, x1) = (
                p.iter()
                    .map(|q| q.0)
                    .fold(f32::INFINITY, f32::min)
                    .floor()
                    .max(0.0) as usize,
                p.iter()
                    .map(|q| q.0)
                    .fold(f32::NEG_INFINITY, f32::max)
                    .ceil() as usize,
            );
            let (z0, z1) = (
                p.iter()
                    .map(|q| q.2)
                    .fold(f32::INFINITY, f32::min)
                    .floor()
                    .max(0.0) as usize,
                p.iter()
                    .map(|q| q.2)
                    .fold(f32::NEG_INFINITY, f32::max)
                    .ceil() as usize,
            );
            let area =
                (p[1].0 - p[0].0) * (p[2].2 - p[0].2) - (p[2].0 - p[0].0) * (p[1].2 - p[0].2);
            for z in z0..=z1.min(height - 1) {
                for x in x0..=x1.min(width - 1) {
                    let (px, pz) = (x as f32 + 0.5, z as f32 + 0.5);
                    // Inside: the same side of all three edges.
                    let edge = |a: (f32, f32, f32), b: (f32, f32, f32)| {
                        (b.0 - a.0) * (pz - a.2) - (px - a.0) * (b.2 - a.2)
                    };
                    let (e0, e1, e2) = (edge(p[0], p[1]), edge(p[1], p[2]), edge(p[2], p[0]));
                    let inside = (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0)
                        || (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0);
                    if !inside {
                        continue;
                    }
                    // The height there (or the triangle's highest, edge on).
                    let y = if area.abs() > 1e-6 {
                        (e1 * p[0].1 + e2 * p[1].1 + e0 * p[2].1) / (e0 + e1 + e2)
                    } else {
                        p[0].1.max(p[1].1).max(p[2].1)
                    };
                    let cell = &mut top[z * width + x];
                    *cell = cell.max(y);
                }
            }
        }
        // Shaded by height, and lit from the top left so slopes show.
        let span = (max.1 - min.1).max(1.0);
        let mut pixels = vec![0u8; width * height * 4];
        for z in 0..height {
            for x in 0..width {
                let h = top[z * width + x];
                let out = &mut pixels[(z * width + x) * 4..][..4];
                if h == f32::NEG_INFINITY {
                    out.copy_from_slice(&[12, 16, 26, 200]);
                    continue;
                }
                let up = if z > 0 { top[(z - 1) * width + x] } else { h };
                let left = if x > 0 { top[z * width + x - 1] } else { h };
                let slope = ((h - up.max(min.1)) + (h - left.max(min.1))) / scale * 0.15;
                let level = 0.35 + 0.5 * (h - min.1) / span + slope.clamp(-0.25, 0.25);
                let level = level.clamp(0.0, 1.0);
                out.copy_from_slice(&[
                    (70.0 * level + 20.0) as u8,
                    (150.0 * level + 25.0) as u8,
                    (190.0 * level + 35.0) as u8,
                    230,
                ]);
            }
        }
        Some(Minimap {
            width,
            height,
            pixels,
            origin: (min.0, min.2),
            scale,
        })
    }

    /// Where a point is on the map, in pixels.
    pub fn pixel(&self, p: Vec3) -> (f32, f32) {
        (
            (p.x - self.origin.0) / self.scale,
            (p.z - self.origin.1) / self.scale,
        )
    }
}
