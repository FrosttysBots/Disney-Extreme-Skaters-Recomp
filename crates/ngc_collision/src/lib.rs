//! Collision meshes (`.col.ngc`), big-endian.
//!
//! ```text
//! header (32 bytes)
//!   u32  version (8)
//!   u32  object count
//!   u32  total vertices
//!   u32  total large faces, u32 total small faces
//!   u32 ×3  unknown (always 0)
//! objects (64 bytes each)
//!   u32  checksum (matches the sector with the same name in the .scn.ngc)
//!   u16  flags, u16 vertex count, u16 face count
//!   u8   1 = small faces (8-bit indices), u8 1 = fixed-point vertices (unused)
//!   u32  byte offset of the object's first face
//!   f32 ×8  bounding box: min x, y, z, w, max x, y, z, w
//!   u32  index of the object's first vertex
//!   u32  byte offset into the BSP tree, u32 unknown, u32 padding
//! vertices         f32 ×3 each
//! intensities      u8 per vertex (baked brightness), padded to 4 bytes
//! faces            small: u16 flags, u16 terrain, u8 ×3 indices, u8 pad
//!                  large: u16 flags, u16 terrain, u16 ×3 indices, u16 pad
//! BSP tree         for fast queries; not decoded yet
//! ```
//!
//! **Corrupted counts.** The tool that wrote these files replaced `0x20`
//! bytes (ASCII space) with `0x00` in some count and offset fields, so 32 can
//! read as 0, 288 (0x120) as 256, 1056 (0x420) as 1024 and so on. Vertex,
//! face and BSP data are unaffected. The parser rebuilds the true counts by
//! choosing, for each object, the reading that keeps every running offset
//! and the header totals consistent while assuming the fewest corrupted
//! fields.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("file is truncated: {0}")]
    Truncated(String),
    #[error("unsupported version {0} (expected 8)")]
    UnsupportedVersion(u32),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("inconsistent counts: {0}")]
    Inconsistent(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub const VERSION: u32 = 8;
const HEADER_SIZE: usize = 32;
const OBJECT_SIZE: usize = 64;
const SMALL_FACE_SIZE: u64 = 8;
const LARGE_FACE_SIZE: u64 = 12;

/// Face flags. Names follow the Neversoft engine family; the ones marked
/// "probably" match how often and where the bits appear in this game's
/// levels but aren't confirmed.
pub mod face_flags {
    pub const SKATABLE: u16 = 0x0001;
    pub const NOT_SKATABLE: u16 = 0x0002;
    pub const WALL_RIDABLE: u16 = 0x0004;
    /// Quarter pipes and other vert surfaces that launch the skater upward.
    pub const VERT: u16 = 0x0008;
    /// Doesn't stop the skater (triggers, decals).
    pub const NON_COLLIDABLE: u16 = 0x0010;
    /// Runs a script when the skater touches it.
    pub const TRIGGER: u16 = 0x0040;
    /// Probably: the camera bumps against it.
    pub const CAMERA_COLLIDABLE: u16 = 0x0080;
    /// Probably: the skater's shadow isn't drawn on it.
    pub const NO_SKATER_SHADOW: u16 = 0x0100;
    /// Probably: not drawn, collision only.
    pub const INVISIBLE: u16 = 0x1000;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Face {
    pub flags: u16,
    /// Surface type, which picks sounds and particles (concrete, wood, grass...).
    pub terrain: u16,
    pub indices: [u16; 3],
}

#[derive(Debug, Clone)]
pub struct CollisionObject {
    pub checksum: u32,
    pub flags: u16,
    /// min x, y, z, max x, y, z
    pub bbox: [f32; 6],
    pub vertices: Vec<[f32; 3]>,
    pub intensities: Vec<u8>,
    pub faces: Vec<Face>,
    /// Faces dropped because an index was out of range (some files pad
    /// with 0xFFFF placeholder faces).
    pub skipped_faces: usize,
    pub bsp_offset: u32,
}

#[derive(Debug, Clone)]
pub struct Collision {
    pub objects: Vec<CollisionObject>,
    /// Raw BSP tree bytes, which `CollisionObject::bsp_offset` points into.
    pub bsp: Vec<u8>,
    /// How many count fields had to be repaired (see the module docs).
    pub repaired_fields: usize,
}

struct RawObject {
    checksum: u32,
    flags: u16,
    vertex_count: u16,
    face_count: u16,
    small_faces: bool,
    fixed_vertices: bool,
    face_offset: u32,
    bbox: [f32; 6],
    vertex_offset: u32,
    bsp_offset: u32,
}

/// An object's repaired layout.
struct Layout {
    vertex_start: u64,
    vertex_count: u64,
    face_start: u64,
    face_count: u64,
}

impl Collision {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let header = data
            .get(..HEADER_SIZE)
            .ok_or_else(|| Error::Truncated("missing header".into()))?;
        let version = be_u32(header, 0);
        if version != VERSION {
            return Err(Error::UnsupportedVersion(version));
        }
        let object_count = be_u32(header, 4) as usize;
        let objects_end = object_count
            .checked_mul(OBJECT_SIZE)
            .and_then(|size| size.checked_add(HEADER_SIZE))
            .filter(|&end| end <= data.len())
            .ok_or_else(|| Error::Truncated(format!("{object_count} object records")))?;

        let raw: Vec<RawObject> = data[HEADER_SIZE..objects_end]
            .chunks_exact(OBJECT_SIZE)
            .map(read_object)
            .collect();
        if let Some(o) = raw.iter().find(|o| o.fixed_vertices) {
            return Err(Error::Unsupported(format!(
                "object {:08x} uses fixed-point vertices",
                o.checksum
            )));
        }

        let totals = (be_u32(header, 8), be_u32(header, 12), be_u32(header, 16));
        let (layouts, repaired_fields) = repair_counts(&raw, totals)?;

        let total_vertices: u64 = layouts.iter().map(|l| l.vertex_count).sum();
        let vertex_base = objects_end as u64;
        let intensity_base = vertex_base + total_vertices * 12;
        let face_base = (intensity_base + total_vertices).next_multiple_of(4);
        let face_bytes: u64 = raw
            .iter()
            .zip(&layouts)
            .map(|(o, l)| l.face_count * face_size(o.small_faces))
            .sum();
        let bsp_base = face_base + face_bytes;
        if bsp_base > data.len() as u64 {
            return Err(Error::Truncated(format!(
                "vertices and faces need {bsp_base} bytes, file has {}",
                data.len()
            )));
        }

        let objects = raw
            .iter()
            .zip(&layouts)
            .map(|(o, l)| {
                let range = |start: u64, len: u64| &data[start as usize..(start + len) as usize];
                let vertices = range(vertex_base + l.vertex_start * 12, l.vertex_count * 12)
                    .chunks_exact(12)
                    .map(|v| [0, 4, 8].map(|i| f32::from_bits(be_u32(v, i))))
                    .collect();
                let intensities = range(intensity_base + l.vertex_start, l.vertex_count).to_vec();

                let size = face_size(o.small_faces);
                let mut faces = Vec::with_capacity(l.face_count as usize);
                let mut skipped_faces = 0;
                for f in
                    range(face_base + l.face_start, l.face_count * size).chunks_exact(size as usize)
                {
                    let indices = if o.small_faces {
                        [u16::from(f[4]), u16::from(f[5]), u16::from(f[6])]
                    } else {
                        [be_u16(f, 4), be_u16(f, 6), be_u16(f, 8)]
                    };
                    if indices.iter().any(|&i| u64::from(i) >= l.vertex_count) {
                        skipped_faces += 1;
                        continue;
                    }
                    faces.push(Face {
                        flags: be_u16(f, 0),
                        terrain: be_u16(f, 2),
                        indices,
                    });
                }

                CollisionObject {
                    checksum: o.checksum,
                    flags: o.flags,
                    bbox: o.bbox,
                    vertices,
                    intensities,
                    faces,
                    skipped_faces,
                    bsp_offset: o.bsp_offset,
                }
            })
            .collect();

        Ok(Self {
            objects,
            bsp: data[bsp_base as usize..].to_vec(),
            repaired_fields,
        })
    }

    pub fn face_count(&self) -> usize {
        self.objects.iter().map(|o| o.faces.len()).sum()
    }

    pub fn vertex_count(&self) -> usize {
        self.objects.iter().map(|o| o.vertices.len()).sum()
    }
}

fn read_object(o: &[u8]) -> RawObject {
    let f = |at: usize| f32::from_bits(be_u32(o, at));
    RawObject {
        checksum: be_u32(o, 0),
        flags: be_u16(o, 4),
        vertex_count: be_u16(o, 6),
        face_count: be_u16(o, 8),
        small_faces: o[10] != 0,
        fixed_vertices: o[11] != 0,
        face_offset: be_u32(o, 12),
        // Skip the w components at 28 and 44.
        bbox: [f(16), f(20), f(24), f(32), f(36), f(40)],
        vertex_offset: be_u32(o, 48),
        bsp_offset: be_u32(o, 52),
    }
}

fn face_size(small: bool) -> u64 {
    if small {
        SMALL_FACE_SIZE
    } else {
        LARGE_FACE_SIZE
    }
}

/// Every value `stored` could have been before `0x20` bytes were zeroed,
/// paired with how many bytes that assumes were corrupted. The stored value
/// itself comes first.
fn readings(stored: u64, bytes: u32) -> Vec<(u64, u32)> {
    let zero_bytes: Vec<u32> = (0..bytes)
        .filter(|i| (stored >> (8 * i)) & 0xFF == 0)
        .collect();
    (0u32..1 << zero_bytes.len())
        .map(|mask| {
            let mut value = stored;
            for (bit, byte) in zero_bytes.iter().enumerate() {
                if mask & (1 << bit) != 0 {
                    value |= 0x20 << (8 * byte);
                }
            }
            (value, mask.count_ones())
        })
        .collect()
}

/// Rebuilds each object's vertex and face counts (see the module docs).
/// Returns the layouts and how many fields were repaired.
fn repair_counts(raw: &[RawObject], totals: (u32, u32, u32)) -> Result<(Vec<Layout>, usize)> {
    let (total_vertices, total_large, total_small) = totals;
    let mut layouts = Vec::with_capacity(raw.len());
    let (mut vertex_sum, mut face_sum) = (0u64, 0u64);
    let mut repaired = 0;

    for (i, object) in raw.iter().enumerate() {
        let inconsistent = |what: &str| {
            Error::Inconsistent(format!("object {i} ({:08x}): {what}", object.checksum))
        };
        if !readings(object.vertex_offset.into(), 4)
            .iter()
            .any(|&(v, _)| v == vertex_sum)
            || !readings(object.face_offset.into(), 4)
                .iter()
                .any(|&(v, _)| v == face_sum)
        {
            return Err(inconsistent("its offsets don't follow the previous object"));
        }

        // Where the next object starts (or the totals, for the last one),
        // with the cost of each reading.
        let next: Vec<(u64, u64, u32)> = match raw.get(i + 1) {
            Some(n) => {
                let faces = readings(n.face_offset.into(), 4);
                readings(n.vertex_offset.into(), 4)
                    .into_iter()
                    .flat_map(|(v, cv)| faces.iter().map(move |&(f, cf)| (v, f, cv + cf)))
                    .collect()
            }
            None => {
                let mut out = Vec::new();
                for (v, cv) in readings(total_vertices.into(), 4) {
                    for (large, cl) in readings(total_large.into(), 4) {
                        for (small, cs) in readings(total_small.into(), 4) {
                            let bytes = large * LARGE_FACE_SIZE + small * SMALL_FACE_SIZE;
                            out.push((v, bytes, cv + cl + cs));
                        }
                    }
                }
                out
            }
        };

        let size = face_size(object.small_faces);
        let mut best: Option<(u32, u64, u64)> = None; // (cost, vertices, faces)
        for (vertices, cost_v) in readings(object.vertex_count.into(), 2) {
            for (faces, cost_f) in readings(object.face_count.into(), 2) {
                // An object has both vertices and faces, or neither.
                if (vertices == 0) != (faces == 0) {
                    continue;
                }
                for &(next_v, next_f, cost_n) in &next {
                    if vertex_sum + vertices == next_v && face_sum + faces * size == next_f {
                        let candidate = (cost_v + cost_f + cost_n, vertices, faces);
                        // Fewest corrupted fields, then the smallest counts.
                        let key = |c: (u32, u64, u64)| (c.0, c.1 + c.2);
                        if best.is_none_or(|b| key(candidate) < key(b)) {
                            best = Some(candidate);
                        }
                    }
                }
            }
        }
        let (_, vertices, faces) =
            best.ok_or_else(|| inconsistent("no reading of its counts fits the next offsets"))?;
        repaired += usize::from(vertices != u64::from(object.vertex_count))
            + usize::from(faces != u64::from(object.face_count));

        layouts.push(Layout {
            vertex_start: vertex_sum,
            vertex_count: vertices,
            face_start: face_sum,
            face_count: faces,
        });
        vertex_sum += vertices;
        face_sum += faces * size;
    }
    Ok((layouts, repaired))
}

fn be_u16(buf: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([buf[offset], buf[offset + 1]])
}

fn be_u32(buf: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(buf[offset..offset + 4].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readings_restore_zeroed_space_bytes() {
        let values: Vec<u64> = readings(0x0100, 2).into_iter().map(|(v, _)| v).collect();
        assert_eq!(values, [0x0100, 0x0120]);
        assert_eq!(
            readings(0, 2),
            [(0, 0), (0x20, 1), (0x2000, 1), (0x2020, 2)]
        );
        assert_eq!(readings(0x1234, 2), [(0x1234, 0)]);
    }
}
