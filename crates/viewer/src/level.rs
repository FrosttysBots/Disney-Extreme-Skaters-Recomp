//! Turns a parsed scene into GPU-ready arrays: one vertex pool, one index
//! list, and a draw batch per material with one entry per rendering pass.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use ngc_model::{Pass, Scene, pass_flags};
use ngc_texture::TexDictionary;

/// The most UV sets a vertex carries; sectors with fewer repeat their last set.
pub const UV_SETS: usize = 3;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    /// Zero when the sector has no normals.
    pub normal: [f32; 3],
    pub uvs: [[f32; 2]; UV_SETS],
    /// Raw vertex color; 0x80 is full brightness (the shader rescales).
    pub color: [u8; 4],
}

/// RGBA8 pixels for every mip level, top row first.
pub struct TextureData {
    /// The texture's name checksum (0 for the built-in white texture).
    pub checksum: u32,
    pub width: u32,
    pub height: u32,
    pub levels: Vec<Vec<u8>>,
}

/// The engine's blend modes, as stored in each pass. "Fixed" variants use
/// the pass's fixed alpha instead of the texture and vertex alpha.
pub mod blend {
    pub const DIFFUSE: u32 = 0;
    pub const ADD: u32 = 1;
    pub const ADD_FIXED: u32 = 2;
    pub const SUBTRACT: u32 = 3;
    pub const SUB_FIXED: u32 = 4;
    #[allow(dead_code)] // the default; listed for completeness
    pub const BLEND: u32 = 5;
    pub const BLEND_FIXED: u32 = 6;
    pub const MODULATE: u32 = 7;
    pub const MODULATE_FIXED: u32 = 8;
    pub const BRIGHTEN: u32 = 9;
    pub const BRIGHTEN_FIXED: u32 = 10;

    pub fn is_fixed(mode: u32) -> bool {
        matches!(
            mode,
            ADD_FIXED | SUB_FIXED | BLEND_FIXED | MODULATE_FIXED | BRIGHTEN_FIXED
        )
    }
}

#[derive(Clone)]
pub struct PassDraw {
    pub texture: usize,
    pub uv_set: u32,
    pub blend_mode: u32,
    /// For the "fixed" blend modes: the constant alpha (1.0 = opaque).
    pub fixed_alpha: Option<f32>,
    /// Texels with less alpha are discarded (opaque passes only).
    pub alpha_threshold: f32,
    pub environment: bool,
    /// UV velocity, frequency, amplitude and phase (u and v each).
    pub uv_wibble: [f32; 8],
}

#[derive(Clone)]
pub struct Batch {
    pub first_index: u32,
    pub index_count: u32,
    pub passes: Vec<PassDraw>,
    /// Drawn after all opaque materials, in `draw_order`.
    pub transparent: bool,
    pub draw_order: f32,
}

pub struct Level {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub batches: Vec<Batch>,
    /// Index 0 is always a 1x1 white texture for untextured passes.
    pub textures: Vec<TextureData>,
    /// Center and radius of the middle 90% of vertices, which ignores
    /// huge ground or ocean planes when framing the camera.
    pub focus: (Vec3, f32),
}

const NEUTRAL_COLOR: [u8; 4] = [0x80, 0x80, 0x80, 0x80];

impl Level {
    /// Loads `scene_path` and its textures (`textures_path`, or by default
    /// the `.tex.ngc` with the same name in the same folder).
    pub fn load(scene_path: &Path, textures_path: Option<&Path>) -> Result<Level> {
        let data = fs::read(scene_path)
            .with_context(|| format!("could not read {}", scene_path.display()))?;
        let tex_path = match textures_path {
            Some(p) => Some(p.to_path_buf()),
            None => sibling(scene_path, "scn.ngc", "tex.ngc"),
        };
        let tex_data = match &tex_path {
            Some(p) => {
                Some(fs::read(p).with_context(|| format!("could not read {}", p.display()))?)
            }
            None => {
                eprintln!("warning: no textures found for {}", scene_path.display());
                None
            }
        };
        Self::from_bytes(&data, tex_data.as_deref())
            .with_context(|| format!("could not load {}", scene_path.display()))
    }

    /// Builds a level from the contents of a `.scn.ngc` and its `.tex.ngc`.
    pub fn from_bytes(scene: &[u8], textures: Option<&[u8]>) -> Result<Level> {
        let scene = Scene::parse(scene).context("could not parse the scene")?;
        let dictionary = match textures {
            Some(data) => {
                Some(TexDictionary::parse(data).context("could not parse the texture file")?)
            }
            None => None,
        };

        let mut level = Level {
            vertices: Vec::new(),
            indices: Vec::new(),
            batches: Vec::new(),
            textures: vec![TextureData {
                checksum: 0,
                width: 1,
                height: 1,
                levels: vec![vec![255; 4]],
            }],
            focus: (Vec3::ZERO, 1.0),
        };

        // Gather triangles per material, keeping first-seen order.
        let mut by_material: Vec<(u32, Vec<u32>)> = Vec::new();
        let mut material_slot: HashMap<u32, usize> = HashMap::new();
        for sector in &scene.sectors {
            let base = level.vertices.len() as u32;
            for (i, position) in sector.positions.iter().enumerate() {
                let mut uvs = [[0.0; 2]; UV_SETS];
                for (set, uv) in uvs.iter_mut().enumerate() {
                    let available = set.min(sector.uv_sets.saturating_sub(1));
                    *uv = sector.uv(i, available).unwrap_or([0.0, 0.0]);
                }
                level.vertices.push(Vertex {
                    position: *position,
                    normal: sector.normals.get(i).copied().unwrap_or([0.0; 3]),
                    uvs,
                    color: sector.colors.get(i).copied().unwrap_or(NEUTRAL_COLOR),
                });
            }
            for mesh in &sector.meshes {
                let slot = *material_slot.entry(mesh.material).or_insert_with(|| {
                    by_material.push((mesh.material, Vec::new()));
                    by_material.len() - 1
                });
                let list = &mut by_material[slot].1;
                for tri in mesh.triangles() {
                    list.extend(tri.iter().map(|&i| base + u32::from(i)));
                }
            }
        }

        let mut textures = TextureCache {
            dictionary: dictionary.as_ref(),
            slots: HashMap::new(),
        };
        for (material_checksum, triangles) in by_material {
            let material = scene.material(material_checksum);
            let alpha_cutoff = material.map_or(0, |m| m.alpha_cutoff);
            let mut passes = Vec::new();
            let mut uv_set = 0;
            for pass in material.map(|m| m.passes.as_slice()).unwrap_or_default() {
                let environment = pass.flags & pass_flags::ENVIRONMENT != 0;
                passes.push(pass_draw(
                    pass,
                    textures.slot(&mut level, pass.texture),
                    uv_set,
                    alpha_cutoff,
                ));
                // Environment-mapped passes compute their UVs instead of using a set.
                if !environment {
                    uv_set = (uv_set + 1).min(UV_SETS as u32 - 1);
                }
            }
            if passes.is_empty() {
                passes.push(pass_draw_untextured());
            }
            level.batches.push(Batch {
                first_index: level.indices.len() as u32,
                index_count: triangles.len() as u32,
                transparent: passes[0].blend_mode != blend::DIFFUSE,
                draw_order: material.map_or(0.0, |m| m.draw_order),
                passes,
            });
            level.indices.extend(triangles);
        }

        level.focus = focus(&level.vertices);
        Ok(level)
    }

    /// The slot in `textures` of the texture with this name checksum.
    pub fn texture_slot(&self, checksum: u32) -> Option<usize> {
        self.textures
            .iter()
            .skip(1)
            .position(|t| t.checksum == checksum)
            .map(|i| i + 1)
    }

    /// Adds `other`'s geometry after this one's (a character and its board).
    /// Its vertices keep their order, starting at this level's old count.
    pub fn append(&mut self, other: Level) {
        let base_vertex = self.vertices.len() as u32;
        let base_index = self.indices.len() as u32;
        // Both share slot 0, the white texture.
        let base_texture = self.textures.len() - 1;
        self.vertices.extend(other.vertices);
        self.indices
            .extend(other.indices.iter().map(|i| i + base_vertex));
        self.textures.extend(other.textures.into_iter().skip(1));
        for mut batch in other.batches {
            batch.first_index += base_index;
            for pass in &mut batch.passes {
                if pass.texture != 0 {
                    pass.texture += base_texture;
                }
            }
            self.batches.push(batch);
        }
        self.focus = focus(&self.vertices);
    }
}

fn pass_draw(pass: &Pass, texture: usize, uv_set: u32, alpha_cutoff: u32) -> PassDraw {
    PassDraw {
        texture,
        uv_set,
        blend_mode: pass.blend_mode,
        // Fixed alpha uses the console convention where 128 is opaque.
        fixed_alpha: blend::is_fixed(pass.blend_mode).then(|| pass.fixed_alpha as f32 / 128.0),
        // Without a cutoff, still drop clearly transparent texels so
        // cut-out textures (leaves, fences) don't show their backgrounds.
        alpha_threshold: if alpha_cutoff > 0 {
            alpha_cutoff.min(255) as f32 / 255.0
        } else {
            0.5
        },
        environment: pass.flags & pass_flags::ENVIRONMENT != 0,
        uv_wibble: pass.uv_wibble.unwrap_or([0.0; 8]),
    }
}

fn pass_draw_untextured() -> PassDraw {
    PassDraw {
        texture: 0,
        uv_set: 0,
        blend_mode: blend::DIFFUSE,
        fixed_alpha: None,
        alpha_threshold: 0.5,
        environment: false,
        uv_wibble: [0.0; 8],
    }
}

/// Decodes each texture once and remembers its slot in `Level::textures`.
struct TextureCache<'a> {
    dictionary: Option<&'a TexDictionary<'a>>,
    slots: HashMap<u32, usize>,
}

impl TextureCache<'_> {
    fn slot(&mut self, level: &mut Level, checksum: u32) -> usize {
        if checksum == 0 {
            return 0;
        }
        if let Some(&slot) = self.slots.get(&checksum) {
            return slot;
        }
        let slot = match self.dictionary.and_then(|d| decode_texture(d, checksum)) {
            Some(data) => {
                level.textures.push(data);
                level.textures.len() - 1
            }
            None => 0,
        };
        self.slots.insert(checksum, slot);
        slot
    }
}

/// Decodes every usable mip level of one texture, or `None` if it's
/// missing or fails to decode.
fn decode_texture(dict: &TexDictionary, checksum: u32) -> Option<TextureData> {
    let texture = dict.textures.iter().find(|t| t.checksum == checksum)?;
    // wgpu allows at most log2(largest side) + 1 levels.
    let max_levels = 32 - texture.width.max(texture.height).leading_zeros();
    let count = texture.level_count().min(max_levels as usize);
    let levels = (0..count)
        .map(|level| texture.decode_level(level).map(|image| image.rgba))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| eprintln!("warning: texture {checksum:08x}: {e}"))
        .ok()?;
    Some(TextureData {
        checksum,
        width: texture.width,
        height: texture.height,
        levels,
    })
}

fn focus(vertices: &[Vertex]) -> (Vec3, f32) {
    if vertices.is_empty() {
        return (Vec3::ZERO, 1.0);
    }
    let mut axes: [Vec<f32>; 3] = Default::default();
    for v in vertices {
        for (axis, values) in axes.iter_mut().enumerate() {
            values.push(v.position[axis]);
        }
    }
    let mut lo = [0.0; 3];
    let mut hi = [0.0; 3];
    for (axis, values) in axes.iter_mut().enumerate() {
        values.sort_by(f32::total_cmp);
        lo[axis] = values[values.len() / 20];
        hi[axis] = values[values.len() * 19 / 20];
    }
    let (lo, hi) = (Vec3::from(lo), Vec3::from(hi));
    ((lo + hi) / 2.0, ((hi - lo) / 2.0).max_element().max(1.0))
}

/// Finds a file next to `path` whose name swaps `suffix` for `replacement`,
/// ignoring case (the disc mixes `Levels/HUB` and `levels/hub`).
pub fn sibling(path: &Path, suffix: &str, replacement: &str) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let stem = &name[..name.to_ascii_lowercase().strip_suffix(suffix)?.len()];
    find_ignoring_case(path.parent()?, &format!("{stem}{replacement}"))
}

pub fn find_ignoring_case(dir: &Path, name: &str) -> Option<PathBuf> {
    let wanted = name.to_ascii_lowercase();
    fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.to_ascii_lowercase() == wanted)
        })
}

/// The sky for `levels/<name>/<name>.scn.ngc` is `levels/<name>_sky/<name>_sky.scn.ngc`.
pub fn find_sky(scene_path: &Path) -> Option<PathBuf> {
    let name = scene_path.file_name()?.to_str()?;
    let stem = &name[..name.to_ascii_lowercase().strip_suffix(".scn.ngc")?.len()];
    let levels_dir = scene_path.parent()?.parent()?;
    let sky_dir = find_ignoring_case(levels_dir, &format!("{stem}_sky"))?;
    find_ignoring_case(&sky_dir, &format!("{stem}_sky.scn.ngc"))
}
