//! Builds a colored triangle soup from a level's collision for display.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use ngc_collision::{Collision, face_flags};

use crate::level::find_ignoring_case;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct CollisionVertex {
    pub position: [f32; 3],
    pub color: [u8; 4],
}

/// How collision is shown.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum CollisionView {
    Hidden,
    /// Translucent, over the normal level.
    Overlay,
    /// Solid, without the level.
    Only,
}

impl CollisionView {
    pub fn next(self) -> Self {
        match self {
            Self::Hidden => Self::Overlay,
            Self::Overlay => Self::Only,
            Self::Only => Self::Hidden,
        }
    }
}

/// Legend, in priority order: the first matching flag picks the color.
pub const LEGEND: &str = "\
Collision colors:
  yellow   trigger or non-collidable
  red      vert (quarter pipes)
  blue     wall-ridable
  purple   not skatable
  gray     everything else";

pub fn load(path: &Path) -> Result<Vec<CollisionVertex>> {
    let data = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let collision =
        Collision::parse(&data).with_context(|| format!("could not parse {}", path.display()))?;

    let light = Vec3::new(0.4, 0.8, 0.3).normalize();
    let mut vertices = Vec::with_capacity(collision.face_count() * 3);
    for object in &collision.objects {
        for face in &object.faces {
            let corners = face
                .indices
                .map(|i| Vec3::from(object.vertices[usize::from(i)]));
            let normal = (corners[1] - corners[0])
                .cross(corners[2] - corners[0])
                .normalize_or_zero();
            let shade = 0.55 + 0.45 * normal.dot(light).abs();
            let [r, g, b, a] = color(face.flags);
            let color = [r, g, b].map(|c| (f32::from(c) * shade) as u8);
            for corner in corners {
                vertices.push(CollisionVertex {
                    position: corner.into(),
                    color: [color[0], color[1], color[2], a],
                });
            }
        }
    }
    Ok(vertices)
}

fn color(flags: u16) -> [u8; 4] {
    if flags & (face_flags::TRIGGER | face_flags::NON_COLLIDABLE) != 0 {
        [255, 220, 40, 110]
    } else if flags & face_flags::VERT != 0 {
        [235, 60, 50, 200]
    } else if flags & face_flags::WALL_RIDABLE != 0 {
        [60, 120, 255, 200]
    } else if flags & face_flags::NOT_SKATABLE != 0 {
        [170, 80, 220, 200]
    } else {
        [200, 200, 200, 170]
    }
}

/// `<root>/<X>Scn/Levels/<name>/<name>.scn.ngc` has its collision in
/// `<root>/<X>col/Levels/<name>/<name>.col.ngc`.
pub fn find(scene_path: &Path) -> Option<PathBuf> {
    let file = scene_path.file_name()?.to_str()?;
    let stem = &file[..file.to_ascii_lowercase().strip_suffix(".scn.ngc")?.len()];
    let level_dir = scene_path.parent()?;
    let scn_dir = level_dir.parent()?.parent()?;
    let scn_name = scn_dir.file_name()?.to_str()?;
    let base = &scn_name[..scn_name.to_ascii_lowercase().strip_suffix("scn")?.len()];
    let col_dir = find_ignoring_case(scn_dir.parent()?, &format!("{base}col"))?;
    let levels = find_ignoring_case(&col_dir, "levels")?;
    let level = find_ignoring_case(&levels, level_dir.file_name()?.to_str()?)?;
    find_ignoring_case(&level, &format!("{stem}.col.ngc"))
}
