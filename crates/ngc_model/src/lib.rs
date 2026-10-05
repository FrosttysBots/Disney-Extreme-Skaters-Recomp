//! Static models (`.mdl.ngc`) and level scenes (`.scn.ngc`).
//!
//! Both use one big-endian layout, reverse-engineered from the US disc:
//!
//! ```text
//! u32 ×3     unknown (always 1)
//! u32        material count, then the materials (see [`Material`])
//! u32        sector count, then the sectors (see [`Sector`])
//! u32        always 0, probably an empty hierarchy list
//! ```
//!
//! A sector is one object: a vertex pool plus meshes, each mesh drawing
//! triangle strips from the pool with one material. Textures are referenced
//! by checksum and live in the matching `.tex.ngc` file.
//!
//! Skinned characters (`.skin.ngc`) use the same layout, except that
//! sectors with flag `0x10` store their vertices in bone groups; see
//! [`skin`].

mod reader;
pub mod skin;

use reader::Reader;
pub use skin::{ExtraInfluence, Skin};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("file is truncated: {0}")]
    Truncated(String),

    #[error("invalid data: {0}")]
    Invalid(String),

    #[error("unsupported: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Material pass flags (the Neversoft engine family's bit assignments).
pub mod pass_flags {
    /// The pass scrolls or wobbles its UVs; an 8-float block follows.
    pub const UV_WIBBLE: u32 = 0x01;
    /// Vertex colors animate; a sequence block follows.
    pub const VC_WIBBLE: u32 = 0x02;
    pub const TEXTURED: u32 = 0x04;
    pub const ENVIRONMENT: u32 = 0x08;
    pub const SMOOTH: u32 = 0x20;
    pub const TRANSPARENT: u32 = 0x40;
}

/// Sector flags describing which vertex arrays are present.
pub mod sector_flags {
    pub const TEXCOORDS: u32 = 0x01;
    pub const COLORS: u32 = 0x02;
    pub const NORMALS: u32 = 0x04;
    /// Skinned: positions and normals come in bone groups (see `skin`).
    pub const WEIGHTS: u32 = 0x10;
    /// One byte per vertex selecting a vertex-color animation sequence.
    pub const VC_WIBBLE_INDICES: u32 = 0x800;
}

#[derive(Debug, Clone)]
pub struct Scene {
    pub materials: Vec<Material>,
    pub sectors: Vec<Sector>,
}

/// ```text
/// u32 checksum, u32 pass count, u32 alpha cutoff, u32 sorted,
/// f32 draw order, u32 unknown flag, then the passes
/// ```
#[derive(Debug, Clone)]
pub struct Material {
    pub checksum: u32,
    pub alpha_cutoff: u32,
    pub sorted: bool,
    pub draw_order: f32,
    pub unknown_flag: u32,
    pub passes: Vec<Pass>,
}

/// ```text
/// u32 texture checksum (0 = untextured), u32 flags,
/// u32 fixed alpha?, u32 blend mode?, u32 u address?, u32 v address?,
/// f32 ×3 color (0.5 = full brightness), f32 ×6 unknown,
/// [UV_WIBBLE: f32 ×8] [VC_WIBBLE: sequences],
/// u32 ×2 filtering?, f32 ×2 mipmap bias?
/// ```
/// Fields marked `?` are named after the matching fields in the engine's
/// other platforms and their observed values; they're not confirmed yet.
#[derive(Debug, Clone)]
pub struct Pass {
    pub texture: u32,
    pub flags: u32,
    pub fixed_alpha: u32,
    pub blend_mode: u32,
    pub u_address: u32,
    pub v_address: u32,
    pub color: [f32; 3],
    pub unknown: [f32; 6],
    /// UV velocity, frequency, amplitude and phase (u and v each).
    pub uv_wibble: Option<[f32; 8]>,
    pub vc_wibble: Vec<VcSequence>,
    pub filtering: [u32; 2],
    pub mip_bias: [f32; 2],
}

#[derive(Debug, Clone)]
pub struct VcSequence {
    pub phase: i32,
    /// (time, RGBA color) pairs.
    pub keys: Vec<(u32, [u8; 4])>,
}

/// ```text
/// u32 checksum, i32 bone (-1), u32 flags, u32 mesh count,
/// f32 ×6 bounding box, f32 ×4 bounding sphere,
/// u32 vertex count, u32 in-memory vertex size,
/// positions (f32 ×3), [NORMALS: i16 ×3, 1.0 = 16384],
/// [TEXCOORDS: u32 set count, then f32 ×2 per set, interleaved per vertex],
/// [COLORS: RGBA, 0x80 = full brightness], [VC_WIBBLE_INDICES: u8],
/// then the meshes
/// ```
#[derive(Debug, Clone)]
pub struct Sector {
    pub checksum: u32,
    pub bone: i32,
    pub flags: u32,
    pub bbox: [f32; 6],
    pub bsphere: [f32; 4],
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Number of UV sets; `uvs` holds `uv_sets` pairs per vertex.
    pub uv_sets: usize,
    pub uvs: Vec<[f32; 2]>,
    pub colors: Vec<[u8; 4]>,
    pub vc_wibble_indices: Vec<u8>,
    /// Bones and weights, for skinned sectors.
    pub skin: Option<Skin>,
    pub meshes: Vec<Mesh>,
}

impl Sector {
    /// UV set `set` of vertex `vertex`, if present.
    pub fn uv(&self, vertex: usize, set: usize) -> Option<[f32; 2]> {
        (set < self.uv_sets)
            .then(|| self.uvs.get(vertex * self.uv_sets + set).copied())
            .flatten()
    }
}

/// ```text
/// u32 material checksum, u32 flags, f32 ×4 bounding sphere,
/// u32 index count, u16 ×count:
///   repeated [length, length indices] triangle strips, ended by a 0
/// ```
#[derive(Debug, Clone)]
pub struct Mesh {
    pub material: u32,
    pub flags: u32,
    pub bsphere: [f32; 4],
    pub strips: Vec<Vec<u16>>,
}

impl Mesh {
    /// Triangles with counter-clockwise front faces. Degenerate triangles,
    /// used to stitch strips together, are skipped.
    pub fn triangles(&self) -> impl Iterator<Item = [u16; 3]> + '_ {
        self.strips.iter().flat_map(|strip| {
            strip.windows(3).enumerate().filter_map(|(i, w)| {
                let tri = if i % 2 == 0 {
                    [w[0], w[1], w[2]]
                } else {
                    [w[1], w[0], w[2]]
                };
                (tri[0] != tri[1] && tri[1] != tri[2] && tri[0] != tri[2]).then_some(tri)
            })
        })
    }
}

impl Scene {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        for _ in 0..3 {
            r.u32("header")?;
        }

        let material_count = r.count("material count", 24)?;
        let materials = (0..material_count)
            .map(|_| read_material(&mut r))
            .collect::<Result<_>>()?;

        let sector_count = r.count("sector count", 64)?;
        let sectors = (0..sector_count)
            .map(|_| read_sector(&mut r))
            .collect::<Result<_>>()?;

        let trailer = r.u32("trailer")?;
        if trailer != 0 || r.remaining() != 0 {
            return Err(Error::Invalid(format!(
                "expected the file to end with a zero word at {:#x}",
                r.offset - 4
            )));
        }
        Ok(Self { materials, sectors })
    }

    pub fn material(&self, checksum: u32) -> Option<&Material> {
        self.materials.iter().find(|m| m.checksum == checksum)
    }
}

fn read_material(r: &mut Reader) -> Result<Material> {
    let checksum = r.u32("material checksum")?;
    let pass_count = r.count("pass count", 76)?;
    let alpha_cutoff = r.u32("alpha cutoff")?;
    let sorted = r.u32("sorted")? != 0;
    let draw_order = r.f32("draw order")?;
    let unknown_flag = r.u32("material flag")?;
    let passes = (0..pass_count)
        .map(|_| read_pass(r))
        .collect::<Result<_>>()?;
    Ok(Material {
        checksum,
        alpha_cutoff,
        sorted,
        draw_order,
        unknown_flag,
        passes,
    })
}

fn read_pass(r: &mut Reader) -> Result<Pass> {
    let texture = r.u32("texture checksum")?;
    let flags = r.u32("pass flags")?;
    let fixed_alpha = r.u32("fixed alpha")?;
    let blend_mode = r.u32("blend mode")?;
    let u_address = r.u32("u address")?;
    let v_address = r.u32("v address")?;
    let color = r.f32s("pass color")?;
    let unknown = r.f32s("pass values")?;

    let uv_wibble = if flags & pass_flags::UV_WIBBLE != 0 {
        Some(r.f32s("uv wibble")?)
    } else {
        None
    };

    let mut vc_wibble = Vec::new();
    if flags & pass_flags::VC_WIBBLE != 0 {
        let sequences = r.count("vc wibble sequence count", 8)?;
        for _ in 0..sequences {
            let key_count = r.count("vc wibble key count", 8)?;
            let phase = r.i32("vc wibble phase")?;
            let keys = (0..key_count)
                .map(|_| {
                    let time = r.u32("vc wibble time")?;
                    let color = r.bytes(4, "vc wibble color")?.try_into().unwrap();
                    Ok((time, color))
                })
                .collect::<Result<_>>()?;
            vc_wibble.push(VcSequence { phase, keys });
        }
    }

    let filtering = [r.u32("filtering")?, r.u32("filtering")?];
    let mip_bias = r.f32s("mipmap bias")?;
    Ok(Pass {
        texture,
        flags,
        fixed_alpha,
        blend_mode,
        u_address,
        v_address,
        color,
        unknown,
        uv_wibble,
        vc_wibble,
        filtering,
        mip_bias,
    })
}

fn read_sector(r: &mut Reader) -> Result<Sector> {
    let start = r.offset;
    let checksum = r.u32("sector checksum")?;
    let bone = r.i32("bone index")?;
    let flags = r.u32("sector flags")?;
    let mesh_count = r.count("mesh count", 28)?;
    let bbox = r.f32s("bounding box")?;
    let bsphere = r.f32s("bounding sphere")?;
    let stored_vertex_count = r.u32("vertex count")?;
    let _vertex_size = r.u32("vertex size")?;

    let mut normals = Vec::new();
    let mut skin = None;
    let positions: Vec<[f32; 3]>;
    if flags & sector_flags::WEIGHTS != 0 {
        let skinned = skin::read(r.data(), r.offset, stored_vertex_count)?;
        positions = skinned.positions;
        normals = skinned.normals;
        skin = Some(skinned.skin);
        r.offset = skinned.end;
    } else {
        let count = stored_vertex_count as usize;
        if count.saturating_mul(12) > r.remaining() {
            return Err(Error::Invalid(format!(
                "vertex count {count} of sector {checksum:08x} at {start:#x} is larger than the file"
            )));
        }
        positions = (0..count)
            .map(|_| r.f32s("position"))
            .collect::<Result<_>>()?;
    }
    let vertex_count = positions.len();

    if flags & sector_flags::NORMALS != 0 && skin.is_none() {
        let raw = r.bytes(vertex_count * 6, "normals")?;
        normals = raw
            .chunks_exact(6)
            .map(|n| {
                let c = |i: usize| f32::from(i16::from_be_bytes([n[i], n[i + 1]])) / 16384.0;
                [c(0), c(2), c(4)]
            })
            .collect();
    }

    let mut uv_sets = 0;
    let mut uvs = Vec::new();
    if flags & sector_flags::TEXCOORDS != 0 {
        uv_sets = r.count("uv set count", vertex_count * 8)?;
        uvs = (0..vertex_count * uv_sets)
            .map(|_| r.f32s("uv"))
            .collect::<Result<_>>()?;
    }

    let mut colors = Vec::new();
    if flags & sector_flags::COLORS != 0 {
        colors = r
            .bytes(vertex_count * 4, "vertex colors")?
            .chunks_exact(4)
            .map(|c| c.try_into().unwrap())
            .collect();
    }

    let mut vc_wibble_indices = Vec::new();
    if flags & sector_flags::VC_WIBBLE_INDICES != 0 {
        vc_wibble_indices = r.bytes(vertex_count, "vc wibble indices")?.to_vec();
    }

    let meshes = (0..mesh_count)
        .map(|_| read_mesh(r, vertex_count))
        .collect::<Result<_>>()?;

    Ok(Sector {
        checksum,
        bone,
        flags,
        bbox,
        bsphere,
        positions,
        normals,
        uv_sets,
        uvs,
        colors,
        vc_wibble_indices,
        skin,
        meshes,
    })
}

fn read_mesh(r: &mut Reader, vertex_count: usize) -> Result<Mesh> {
    let material = r.u32("mesh material")?;
    let flags = r.u32("mesh flags")?;
    let bsphere = r.f32s("mesh bounding sphere")?;
    let start = r.offset;
    let index_count = r.count("index count", 2)?;
    let raw = r.bytes(index_count * 2, "indices")?;
    let indices: Vec<u16> = raw
        .chunks_exact(2)
        .map(|i| u16::from_be_bytes([i[0], i[1]]))
        .collect();

    let invalid = |why: &str| Error::Invalid(format!("index list at {start:#x}: {why}"));
    let mut strips = Vec::new();
    let mut rest = indices.as_slice();
    loop {
        let (&len, tail) = rest
            .split_first()
            .ok_or_else(|| invalid("missing terminating 0"))?;
        if len == 0 {
            if !tail.is_empty() {
                return Err(invalid("data after the terminating 0"));
            }
            break;
        }
        let strip = tail
            .get(..len as usize)
            .ok_or_else(|| invalid("strip runs past the end"))?;
        if let Some(&bad) = strip.iter().find(|&&i| i as usize >= vertex_count) {
            return Err(invalid(&format!(
                "index {bad} with only {vertex_count} vertices"
            )));
        }
        strips.push(strip.to_vec());
        rest = &tail[len as usize..];
    }
    Ok(Mesh {
        material,
        flags,
        bsphere,
        strips,
    })
}
