//! Turns a parsed scene into GPU-ready arrays: one vertex pool, one index
//! list, and a draw batch per material with one entry per rendering pass.
//!
//! **Animated vertex colors.** A material pass can carry color sequences
//! (keys of a time in milliseconds and an RGBA color, 0x80 = full
//! brightness, looping at the last key), and a sector can give each vertex
//! a sequence number: 0 for none, otherwise 1-based into the sequences of
//! the material drawing it (no vertex on the disc points past them). The
//! sequence's color replaces the baked one. Since a vertex can be shared by
//! meshes with different materials, each animated (vertex, material) pair
//! gets its own copy at the end of the vertex pool, which is then all a
//! frame has to update. Only the hub, the beach and the pizza level use
//! this, for 803 vertices in all.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use ngc_model::{Pass, Scene, VcSequence, pass_flags};
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
#[derive(Clone)]
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
    /// Vertices whose color animates (see the module docs).
    pub color_animation: ColorAnimation,
}

/// Animated vertex colors: the last `vertices.len()` vertices of a level,
/// each playing one of `sequences`.
#[derive(Clone, Default)]
pub struct ColorAnimation {
    /// Where the animated vertices start in the level's pool.
    pub first_vertex: usize,
    /// The animated vertices, and the sequence each plays.
    pub vertices: Vec<Vertex>,
    pub sequence: Vec<usize>,
    pub sequences: Vec<VcSequence>,
}

impl ColorAnimation {
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }

    /// The animated vertices at `seconds`, to replace the level's from
    /// `first_vertex` on.
    pub fn at(&self, seconds: f32) -> Vec<Vertex> {
        self.vertices
            .iter()
            .zip(&self.sequence)
            .map(|(v, &s)| Vertex {
                color: sample_colors(&self.sequences[s], seconds),
                ..*v
            })
            .collect()
    }
}

/// A sequence's color `seconds` in, looping at its last key.
pub fn sample_colors(sequence: &VcSequence, seconds: f32) -> [u8; 4] {
    let keys = &sequence.keys;
    let (Some(first), Some(last)) = (keys.first(), keys.last()) else {
        return NEUTRAL_COLOR;
    };
    if last.0 == 0 {
        return first.1;
    }
    let t = (seconds * 1000.0 + sequence.phase as f32).rem_euclid(last.0 as f32);
    if t < first.0 as f32 {
        return first.1;
    }
    for pair in keys.windows(2) {
        let ((t0, c0), (t1, c1)) = (pair[0], pair[1]);
        if t < t1 as f32 {
            let f = (t - t0 as f32) / (t1 - t0).max(1) as f32;
            return std::array::from_fn(|i| {
                (f32::from(c0[i]) + (f32::from(c1[i]) - f32::from(c0[i])) * f).round() as u8
            });
        }
    }
    last.1
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
        Self::from_bytes_filtered(scene, textures, |_| true)
    }

    /// Like [`from_bytes`](Self::from_bytes), with only the sectors whose
    /// checksum passes `keep`.
    pub fn from_bytes_filtered(
        scene: &[u8],
        textures: Option<&[u8]>,
        keep: impl Fn(u32) -> bool,
    ) -> Result<Level> {
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
            color_animation: ColorAnimation::default(),
        };

        // Gather triangles per material, keeping first-seen order.
        let mut by_material: Vec<(u32, Vec<u32>)> = Vec::new();
        let mut material_slot: HashMap<u32, usize> = HashMap::new();
        // Animated copies, marked in index lists by ANIMATED | their number
        // until their place at the end of the pool is known.
        const ANIMATED: u32 = 1 << 31;
        let mut sequences_of: HashMap<u32, Option<(usize, usize)>> = HashMap::new();
        let mut copies: HashMap<(u32, u32), u32> = HashMap::new();
        let mut animated: Vec<(Vertex, usize)> = Vec::new();
        for sector in scene.sectors.iter().filter(|s| keep(s.checksum)) {
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
                let sequences = *sequences_of.entry(mesh.material).or_insert_with(|| {
                    let pass = scene
                        .material(mesh.material)?
                        .passes
                        .iter()
                        .find(|p| !p.vc_wibble.is_empty())?;
                    let anim = &mut level.color_animation.sequences;
                    anim.extend(pass.vc_wibble.iter().cloned());
                    Some((anim.len() - pass.vc_wibble.len(), pass.vc_wibble.len()))
                });
                let list = &mut by_material[slot].1;
                for tri in mesh.triangles() {
                    for &i in &tri {
                        let vertex = base + u32::from(i);
                        let number = sector.vc_wibble_indices.get(usize::from(i)).copied();
                        match (sequences, number) {
                            (Some((first, count)), Some(n)) if n > 0 && usize::from(n) <= count => {
                                let copy =
                                    *copies.entry((vertex, mesh.material)).or_insert_with(|| {
                                        animated.push((
                                            level.vertices[vertex as usize],
                                            first + usize::from(n) - 1,
                                        ));
                                        animated.len() as u32 - 1
                                    });
                                list.push(ANIMATED | copy);
                            }
                            _ => list.push(vertex),
                        }
                    }
                }
            }
        }

        let first_animated = level.vertices.len() as u32;
        level.color_animation.first_vertex = first_animated as usize;
        for (vertex, sequence) in animated {
            level.vertices.push(vertex);
            level.color_animation.vertices.push(vertex);
            level.color_animation.sequence.push(sequence);
        }
        for (_, list) in &mut by_material {
            for index in list.iter_mut().filter(|i| **i & ANIMATED != 0) {
                *index = first_animated + (*index & !ANIMATED);
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

    /// An empty level (just the white texture), to add instances to.
    pub fn empty() -> Level {
        Level {
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
            color_animation: ColorAnimation::default(),
        }
    }

    /// Adds a copy of `model` moved by `transform`. Copies of one model can
    /// share its textures: pass the slot this returned for the first copy
    /// as `textures` (`None` adds them). Different models can't share by
    /// checksum alone, since some names repeat (every character's eyes are
    /// `eyes.png`). `focus` isn't updated.
    pub fn add_instance(
        &mut self,
        model: &Level,
        transform: Mat4,
        textures: Option<usize>,
    ) -> usize {
        let base = textures.unwrap_or_else(|| {
            let base = self.textures.len() - 1;
            self.textures.extend(model.textures.iter().skip(1).cloned());
            base
        });
        let base_vertex = self.vertices.len() as u32;
        let base_index = self.indices.len() as u32;
        self.vertices.extend(model.vertices.iter().map(|v| {
            Vertex {
                position: transform.transform_point3(v.position.into()).into(),
                normal: transform
                    .transform_vector3(v.normal.into())
                    .normalize_or_zero()
                    .into(),
                ..*v
            }
        }));
        self.indices
            .extend(model.indices.iter().map(|i| i + base_vertex));
        for batch in &model.batches {
            let mut batch = batch.clone();
            batch.first_index += base_index;
            for pass in &mut batch.passes {
                if pass.texture != 0 {
                    pass.texture += base;
                }
            }
            self.batches.push(batch);
        }
        base
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sequence(phase: i32, keys: &[(u32, [u8; 4])]) -> VcSequence {
        VcSequence {
            phase,
            keys: keys.to_vec(),
        }
    }

    #[test]
    fn colors_blend_between_keys_and_loop() {
        let s = sequence(0, &[(0, [0, 0, 0, 255]), (1000, [128, 64, 0, 255])]);
        assert_eq!(sample_colors(&s, 0.0), [0, 0, 0, 255]);
        assert_eq!(sample_colors(&s, 0.5), [64, 32, 0, 255]);
        // Loops at the last key.
        assert_eq!(sample_colors(&s, 1.25), [32, 16, 0, 255]);
        // The phase shifts it, in milliseconds.
        let shifted = sequence(500, &[(0, [0, 0, 0, 255]), (1000, [128, 64, 0, 255])]);
        assert_eq!(sample_colors(&shifted, 0.0), [64, 32, 0, 255]);
    }

    #[test]
    fn colors_hold_before_the_first_key() {
        let s = sequence(0, &[(500, [10, 10, 10, 10]), (1000, [20, 20, 20, 20])]);
        assert_eq!(sample_colors(&s, 0.2), [10, 10, 10, 10]);
        assert_eq!(sample_colors(&sequence(0, &[]), 1.0), NEUTRAL_COLOR);
        assert_eq!(
            sample_colors(&sequence(0, &[(0, [1, 2, 3, 4])]), 3.0),
            [1, 2, 3, 4]
        );
    }
}
