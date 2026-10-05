//! Skinned vertices (sector flag `0x10`, used by `.skin.ngc` characters).
//!
//! Instead of separate position and normal arrays, a skinned sector stores
//! its vertices in groups that share a pair of bones (usually a bone and
//! its parent), the way GameCube hardware skinning batches them:
//!
//! ```text
//! u32  byte size of the groups that follow
//! u32  group count
//! per group:
//!   u32  vertex count
//!   u16  0, u8 bone, u8 bone
//!   count × (f32 ×3 position, f32 ×3 normal)    model space, rest pose
//!   count × (f32 weight, f32 weight)            in reverse bone order: the
//!                                               first weight is the second
//!                                               bone's
//! u32  byte size, u32 entry count               a second block, apparently
//! ...                                           extra influences for the few
//!                                               vertices with more than two
//!                                               bones; kept raw for now
//! ```
//!
//! Groups are concatenated in vertex order, so mesh indices refer to them
//! directly. The original tool's habit of turning `0x20` bytes into `0x00`
//! (see `ngc_collision`) hits these counts too: a group of 32 vertices can
//! read as 0. [`read`] tries every reading of each count and keeps the one
//! where every group header lines up and the totals match.

use crate::{Error, Result};

const GROUP_HEADER: usize = 8;
/// Position and normal, then two weights.
const BYTES_PER_VERTEX: usize = 24 + 8;

#[derive(Debug, Clone, PartialEq)]
pub struct Skin {
    /// The two bones each vertex follows.
    pub bones: Vec<[u8; 2]>,
    /// How strongly each vertex follows each of its two bones.
    pub weights: Vec<[f32; 2]>,
    /// The second block's raw bytes (entry count, then entries).
    pub extra_influences: Vec<u8>,
    /// Counts that had to be repaired (see the module docs).
    pub repaired_counts: usize,
}

pub(crate) struct SkinnedVertices {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub skin: Skin,
    /// Offset just past both blocks.
    pub end: usize,
}

/// Reads both skinning blocks starting at `start`. `stored_count` is the
/// sector's vertex count as stored (it may itself be corrupted).
pub(crate) fn read(data: &[u8], start: usize, stored_count: u32) -> Result<SkinnedVertices> {
    let truncated = |what: &str| Error::Truncated(format!("{what} at {start:#x}"));
    let size = be_u32(data, start).ok_or_else(|| truncated("skin block size"))?;
    let group_count = be_u32(data, start + 4).ok_or_else(|| truncated("skin group count"))?;

    let ends: Vec<usize> = readings(size)
        .iter()
        .map(|&s| start + GROUP_HEADER + s as usize)
        .collect();
    let totals = readings(stored_count);
    let mut counts = Vec::new();
    let found = readings(group_count).into_iter().any(|groups| {
        counts.clear();
        walk(
            data,
            start + GROUP_HEADER,
            groups,
            &ends,
            &totals,
            0,
            &mut counts,
        )
    });
    if !found {
        return Err(Error::Invalid(format!(
            "no consistent reading of the skinned vertex groups at {start:#x}"
        )));
    }

    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut bones = Vec::new();
    let mut weights = Vec::new();
    let mut repaired = 0;
    let mut at = start + GROUP_HEADER;
    for &count in &counts {
        let stored = be_u32(data, at).unwrap_or(0);
        repaired += usize::from(stored != count);
        let pair = [data[at + 6], data[at + 7]];
        at += GROUP_HEADER;
        for v in 0..count as usize {
            let base = at + v * 24;
            let f = |i: usize| f32_at(data, base + i * 4);
            positions.push([f(0), f(1), f(2)]);
            normals.push([f(3), f(4), f(5)]);
            bones.push(pair);
        }
        at += count as usize * 24;
        for v in 0..count as usize {
            // Stored in reverse bone order (see the module docs). Posing a
            // character the other way tears the mesh apart at the joints.
            weights.push([f32_at(data, at + v * 8 + 4), f32_at(data, at + v * 8)]);
        }
        at += count as usize * 8;
    }
    let total = positions.len() as u32;
    repaired += usize::from(total != stored_count);

    // The second block: skipped by its size, kept raw.
    let extra_size = be_u32(data, at).ok_or_else(|| truncated("extra influence size"))? as usize;
    let extra_start = at + 4;
    let extra_end = extra_start + 4 + extra_size;
    let extra_influences = data
        .get(extra_start..extra_end)
        .ok_or_else(|| truncated("extra influences"))?
        .to_vec();

    Ok(SkinnedVertices {
        positions,
        normals,
        skin: Skin {
            bones,
            weights,
            extra_influences,
            repaired_counts: repaired,
        },
        end: extra_end,
    })
}

/// Depth-first search over the readings of each group's count. Succeeds
/// when every group header is plausible and the walk ends exactly at one
/// of `ends` with a vertex total in `totals`.
fn walk(
    data: &[u8],
    at: usize,
    remaining: u32,
    ends: &[usize],
    totals: &[u32],
    sum: u32,
    counts: &mut Vec<u32>,
) -> bool {
    if remaining == 0 {
        return ends.contains(&at) && totals.contains(&sum);
    }
    let Some(stored) = be_u32(data, at) else {
        return false;
    };
    let Some(&max_end) = ends.iter().max() else {
        return false;
    };
    for count in readings(stored) {
        let next = at + GROUP_HEADER + count as usize * BYTES_PER_VERTEX;
        if next > max_end || (remaining > 1 && !plausible_header(data, next)) {
            continue;
        }
        counts.push(count);
        if walk(data, next, remaining - 1, ends, totals, sum + count, counts) {
            return true;
        }
        counts.pop();
    }
    false
}

/// A group header has a zero u16 and two small bone indices.
fn plausible_header(data: &[u8], at: usize) -> bool {
    match data.get(at..at + GROUP_HEADER) {
        Some(h) => h[4] == 0 && h[5] == 0 && h[6] < 128 && h[7] < 128,
        None => false,
    }
}

/// Every value `stored` could have been before `0x20` bytes were zeroed;
/// the stored value comes first.
fn readings(stored: u32) -> Vec<u32> {
    let zero_bytes: Vec<u32> = (0..4).filter(|i| (stored >> (8 * i)) & 0xFF == 0).collect();
    (0u32..1 << zero_bytes.len())
        .map(|mask| {
            zero_bytes
                .iter()
                .enumerate()
                .filter(|(bit, _)| mask & (1 << bit) != 0)
                .fold(stored, |v, (_, byte)| v | (0x20 << (8 * byte)))
        })
        .collect()
}

fn be_u32(data: &[u8], at: usize) -> Option<u32> {
    data.get(at..at + 4)
        .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
}

fn f32_at(data: &[u8], at: usize) -> f32 {
    f32::from_bits(be_u32(data, at).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds both blocks from groups of (bones, vertex count), storing
    /// `stored_counts` in the group headers.
    fn build(groups: &[([u8; 2], u32)], stored_counts: &[u32], extra: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        let mut index = 0.0f32;
        for (i, &(bones, count)) in groups.iter().enumerate() {
            body.extend_from_slice(&stored_counts[i].to_be_bytes());
            body.extend_from_slice(&[0, 0, bones[0], bones[1]]);
            for _ in 0..count {
                for v in [index, 1.0, 2.0, 0.0, 1.0, 0.0] {
                    body.extend_from_slice(&v.to_be_bytes());
                }
                index += 1.0;
            }
            for _ in 0..count {
                body.extend_from_slice(&0.75f32.to_be_bytes());
                body.extend_from_slice(&0.25f32.to_be_bytes());
            }
        }
        let mut out = Vec::new();
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&(groups.len() as u32).to_be_bytes());
        out.extend(body);
        out.extend_from_slice(&(extra.len() as u32).to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(extra);
        out
    }

    #[test]
    fn reads_groups_in_order() {
        let data = build(&[([3, 2], 2), ([5, 4], 3)], &[2, 3], &[9, 9]);
        let skinned = read(&data, 0, 5).unwrap();
        assert_eq!(skinned.positions.len(), 5);
        assert_eq!(skinned.positions[3], [3.0, 1.0, 2.0]);
        assert_eq!(skinned.normals[0], [0.0, 1.0, 0.0]);
        assert_eq!(skinned.skin.bones, [[3, 2], [3, 2], [5, 4], [5, 4], [5, 4]]);
        // Stored as (0.75, 0.25): the first weight is the second bone's.
        assert_eq!(skinned.skin.weights[4], [0.25, 0.75]);
        assert_eq!(skinned.skin.extra_influences, [0, 0, 0, 0, 9, 9]);
        assert_eq!(skinned.end, data.len());
        assert_eq!(skinned.skin.repaired_counts, 0);
    }

    #[test]
    fn repairs_a_group_of_32_stored_as_0() {
        let data = build(&[([3, 2], 32), ([5, 4], 1)], &[0, 1], &[]);
        let skinned = read(&data, 0, 33).unwrap();
        assert_eq!(skinned.positions.len(), 33);
        assert_eq!(skinned.skin.bones[32], [5, 4]);
        assert_eq!(skinned.skin.repaired_counts, 1);
    }

    #[test]
    fn rejects_inconsistent_counts() {
        let data = build(&[([3, 2], 2)], &[2], &[]);
        assert!(matches!(read(&data, 0, 7), Err(Error::Invalid(_))));
    }
}
