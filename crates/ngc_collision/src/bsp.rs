//! The BSP trees after the faces, which let the game find the faces near a
//! point without testing them all.
//!
//! ```text
//! u32  byte size of all the nodes
//! nodes, each object's tree depth-first, offsets counted from here:
//!   split: u8 axis (0 = x, 1 = y, 2 = z), u8 ×3 0, f32 position,
//!          u32 offset of the child below it, u32 offset of the child above
//!   leaf:  u8 0xFF, u8 0, u16 face count, f32 -1, u32 ×2 0xFFFFFFFF,
//!          u32 first index into the face list
//! u16 face list: each leaf's faces (an object's own face numbers),
//!     leaf after leaf, all objects together
//! ```
//!
//! A split's lower child always comes right after it, and its upper one
//! right after the lower one's whole subtree, so the tree is read from its
//! shape and the stored offsets only checked. A face lies in every leaf
//! whose box it touches: of the 1,020,583 (face, leaf) pairs on the disc,
//! 1,020,227 touch their leaf's box with "below" meaning a smaller
//! coordinate, which is how the axes and sides were confirmed.
//!
//! The export tool's `0x20` -> `0x00` corruption hits the child offsets, the
//! leaves' face counts and first indices too (a leaf of 32 faces reads 0).
//! Offsets are checked against the shape; a leaf's first index must follow
//! the previous leaf's, and its count must end where the next one starts.

use crate::{Error, Result, be_u16, be_u32};

const SPLIT_SIZE: usize = 16;
const LEAF_SIZE: usize = 20;
const LEAF: u8 = 0xFF;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BspNode {
    /// Faces below `at` on `axis` are under `below`, faces above under
    /// `above` (faces crossing it are under both). Children are indices
    /// into [`BspTree::nodes`].
    Split {
        axis: u8,
        at: f32,
        below: u32,
        above: u32,
    },
    /// `count` entries of [`BspTree::faces`] from `first`.
    Leaf { first: u32, count: u32 },
}

/// One collision object's tree. The root is `nodes[0]`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BspTree {
    pub nodes: Vec<BspNode>,
    /// Face numbers (indices into the object's faces), leaf by leaf.
    pub faces: Vec<u16>,
}

impl BspTree {
    /// Calls `visit` with every face in a leaf that the box from `min` to
    /// `max` reaches. A face can come up more than once.
    pub fn faces_near(&self, min: [f32; 3], max: [f32; 3], mut visit: impl FnMut(u16)) {
        if self.nodes.is_empty() {
            return;
        }
        let mut stack = vec![0u32];
        while let Some(i) = stack.pop() {
            match self.nodes[i as usize] {
                BspNode::Split {
                    axis,
                    at,
                    below,
                    above,
                } => {
                    let a = usize::from(axis);
                    if min[a] <= at {
                        stack.push(below);
                    }
                    if max[a] >= at {
                        stack.push(above);
                    }
                }
                BspNode::Leaf { first, count } => {
                    for &f in &self.faces[first as usize..(first + count) as usize] {
                        visit(f);
                    }
                }
            }
        }
    }
}

impl BspTree {
    /// Renumbers faces by `map` (indexed by the stored face number),
    /// dropping those it maps to `None` (placeholder faces).
    pub(crate) fn renumber(&mut self, map: &[Option<u16>]) {
        let old = std::mem::take(&mut self.faces);
        for node in &mut self.nodes {
            if let BspNode::Leaf { first, count } = node {
                let start = self.faces.len() as u32;
                self.faces.extend(
                    old[*first as usize..(*first + *count) as usize]
                        .iter()
                        .filter_map(|&f| map.get(usize::from(f)).copied().flatten()),
                );
                *first = start;
                *count = self.faces.len() as u32 - start;
            }
        }
    }
}

/// A leaf as stored, before its first index and count are checked.
struct RawLeaf {
    tree: usize,
    node: usize,
    first: u32,
    count: u32,
}

/// Reads one tree per root offset (in the order given) from the BSP region.
/// Returns the trees and how many fields were repaired.
pub(crate) fn read_trees(region: &[u8], roots: &[u32]) -> Result<(Vec<BspTree>, usize)> {
    let invalid = |what: String| Error::Inconsistent(format!("BSP tree: {what}"));
    let node_bytes = be_u32_at(region, 0).ok_or_else(|| invalid("no size".into()))?;
    let nodes = region.get(4..).unwrap_or_default();
    let mut repaired = 0;
    let mut trees = Vec::with_capacity(roots.len());
    let mut leaves = Vec::new();
    let mut end_of_nodes = 0;
    // Trees follow each other in object order, so each root should be
    // where the previous tree ended (roots lose 0x20 bytes too).
    let mut fixed_roots = Vec::with_capacity(roots.len());
    for (t, &root) in roots.iter().enumerate() {
        let root = fit(root, end_of_nodes as u64, &mut repaired).unwrap_or(u64::from(root));
        let mut tree = BspTree::default();
        let end = read_node(
            nodes,
            root as usize,
            &mut tree,
            t,
            &mut leaves,
            &mut repaired,
        )
        .map_err(invalid)?;
        end_of_nodes = end_of_nodes.max(end);
        fixed_roots.push(root);
        trees.push(tree);
    }

    // The face list starts after the last node.
    let size = fit(node_bytes, end_of_nodes as u64, &mut repaired).ok_or_else(|| {
        invalid(format!(
            "nodes end at {end_of_nodes:#x}, size says {node_bytes:#x}"
        ))
    })?;
    let list = nodes
        .get(size as usize..)
        .ok_or_else(|| invalid("face list missing".into()))?;

    // Leaves in face-list order: each starts where the previous ended.
    leaves.sort_by_key(|l| (fixed_roots[l.tree], l.node));
    let stored: Vec<(u32, u32)> = leaves.iter().map(|l| (l.first, l.count)).collect();
    let (fixed, leaf_repairs) = solve_leaves(&stored, (list.len() / 2) as u64)
        .ok_or_else(|| invalid("the leaves' face lists don't add up".into()))?;
    repaired += leaf_repairs;
    for (leaf, (first, count)) in leaves.iter_mut().zip(fixed) {
        leaf.first = first;
        leaf.count = count;
    }

    // Copy each leaf's faces into its tree.
    for leaf in &leaves {
        let tree = &mut trees[leaf.tree];
        let start = tree.faces.len() as u32;
        for k in 0..leaf.count {
            let at = (leaf.first + k) as usize * 2;
            tree.faces.push(be_u16(list, at));
        }
        tree.nodes[leaf.node] = BspNode::Leaf {
            first: start,
            count: leaf.count,
        };
    }
    Ok((trees, repaired))
}

/// Each leaf's true (first, count) from the stored ones: the leaves'
/// lists follow each other from 0 and the last ends at the end of the face
/// list (`entries`, or one short: the list is padded to 4 bytes). Each
/// value is a reading of what's stored, and the solution with the fewest
/// repairs wins: a leaf of 32 faces stored as 0 followed by a first index
/// of 32 stored as 0 also reads as an empty leaf, until the end doesn't fit.
fn solve_leaves(stored: &[(u32, u32)], entries: u64) -> Option<(Vec<(u32, u32)>, usize)> {
    use std::collections::HashMap;
    // For each leaf: start -> (repairs so far, previous start, count before).
    let mut steps: Vec<HashMap<u64, (usize, u64, u32)>> = Vec::with_capacity(stored.len() + 1);
    let mut start = HashMap::new();
    for first in readings(stored.first().map_or(0, |l| l.0)) {
        if first == 0 {
            start.insert(
                0,
                (
                    usize::from(Some(first) != stored.first().map(|l| l.0)),
                    0,
                    0,
                ),
            );
        }
    }
    if stored.is_empty() {
        start.insert(0, (0, 0, 0));
    }
    steps.push(start);
    for (i, &(_, count)) in stored.iter().enumerate() {
        let mut next: HashMap<u64, (usize, u64, u32)> = HashMap::new();
        for (&at, &(cost, _, _)) in &steps[i] {
            for c in readings(count) {
                let end = at + u64::from(c);
                let mut cost = cost + usize::from(c != count);
                match stored.get(i + 1) {
                    Some(&(next_first, _)) => {
                        let Some(r) = readings(next_first).find(|&r| u64::from(r) == end) else {
                            continue;
                        };
                        cost += usize::from(r != next_first);
                    }
                    None if end == entries || end + 1 == entries => {}
                    None => continue,
                }
                if next.get(&end).is_none_or(|&(best, _, _)| cost < best) {
                    next.insert(end, (cost, at, c));
                }
            }
        }
        if next.is_empty() {
            return None;
        }
        steps.push(next);
    }
    // Walk back from the cheapest end.
    let (&end, &(cost, _, _)) = steps.last()?.iter().min_by_key(|(_, (c, _, _))| *c)?;
    let mut out = vec![(0, 0); stored.len()];
    let mut at = end;
    for i in (0..stored.len()).rev() {
        let (_, previous, count) = steps[i + 1][&at];
        out[i] = (previous as u32, count);
        at = previous;
    }
    Some((out, cost))
}

/// Reads the subtree at `at` into `tree`; returns where it ends.
fn read_node(
    nodes: &[u8],
    at: usize,
    tree: &mut BspTree,
    tree_index: usize,
    leaves: &mut Vec<RawLeaf>,
    repaired: &mut usize,
) -> std::result::Result<usize, String> {
    let bytes = nodes
        .get(at..at + SPLIT_SIZE)
        .ok_or_else(|| format!("node at {at:#x} is past the end"))?;
    let index = tree.nodes.len();
    if bytes[0] == LEAF {
        if at + LEAF_SIZE > nodes.len() {
            return Err(format!("leaf at {at:#x} is past the end"));
        }
        tree.nodes.push(BspNode::Leaf { first: 0, count: 0 });
        leaves.push(RawLeaf {
            tree: tree_index,
            node: index,
            first: be_u32(nodes, at + 16),
            count: u32::from(be_u16(bytes, 2)),
        });
        return Ok(at + LEAF_SIZE);
    }
    let axis = bytes[0];
    if axis > 2 || bytes[1..4] != [0, 0, 0] {
        return Err(format!("node at {at:#x} isn't a split or a leaf"));
    }
    let position = f32::from_bits(be_u32(bytes, 4));
    if !position.is_finite() {
        return Err(format!("split at {at:#x} isn't a number"));
    }
    tree.nodes.push(BspNode::Split {
        axis,
        at: position,
        below: 0,
        above: 0,
    });
    let below_at = at + SPLIT_SIZE;
    fit(be_u32(bytes, 8), below_at as u64, repaired)
        .ok_or_else(|| format!("split at {at:#x}: its lower child isn't next"))?;
    let below = tree.nodes.len() as u32;
    let above_at = read_node(nodes, below_at, tree, tree_index, leaves, repaired)?;
    fit(be_u32(bytes, 12), above_at as u64, repaired)
        .ok_or_else(|| format!("split at {at:#x}: its upper child isn't after the lower"))?;
    let above = tree.nodes.len() as u32;
    let end = read_node(nodes, above_at, tree, tree_index, leaves, repaired)?;
    tree.nodes[index] = BspNode::Split {
        axis,
        at: position,
        below,
        above,
    };
    Ok(end)
}

/// `expected` if `stored` is it, or is it with `0x20` bytes zeroed
/// (counting a repair).
fn fit(stored: u32, expected: u64, repaired: &mut usize) -> Option<u64> {
    if u64::from(stored) == expected {
        return Some(expected);
    }
    let found = readings(stored).any(|r| u64::from(r) == expected);
    *repaired += usize::from(found);
    found.then_some(expected)
}

/// `value` and every value it could have been before `0x20` bytes became
/// `0x00`, the stored one first.
fn readings(value: u32) -> impl Iterator<Item = u32> {
    let zero_bytes: Vec<u32> = (0..4).filter(|i| (value >> (8 * i)) & 0xFF == 0).collect();
    (0u32..1 << zero_bytes.len()).map(move |mask| {
        zero_bytes
            .iter()
            .enumerate()
            .filter(|(bit, _)| mask & (1 << bit) != 0)
            .fold(value, |v, (_, byte)| v | (0x20 << (8 * byte)))
    })
}

fn be_u32_at(buf: &[u8], at: usize) -> Option<u32> {
    buf.get(at..at + 4)
        .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(axis: u8, at: f32, below: u32, above: u32) -> Vec<u8> {
        let mut out = vec![axis, 0, 0, 0];
        out.extend_from_slice(&at.to_be_bytes());
        out.extend_from_slice(&below.to_be_bytes());
        out.extend_from_slice(&above.to_be_bytes());
        out
    }

    fn leaf(count: u16, first: u32) -> Vec<u8> {
        let mut out = vec![LEAF, 0];
        out.extend_from_slice(&count.to_be_bytes());
        out.extend_from_slice(&(-1.0f32).to_be_bytes());
        out.extend_from_slice(&[0xFF; 8]);
        out.extend_from_slice(&first.to_be_bytes());
        out
    }

    /// Two trees: x < 10 → faces [0, 1], else [1, 2]; then a lone leaf [5].
    fn region(below_offset: u32, first_count: u16) -> (Vec<u8>, Vec<u32>) {
        let mut nodes = Vec::new();
        nodes.extend(split(0, 10.0, below_offset, 0x24));
        nodes.extend(leaf(first_count, 0));
        nodes.extend(leaf(2, 2));
        let second = nodes.len() as u32;
        nodes.extend(leaf(1, 4));
        let mut out = (nodes.len() as u32).to_be_bytes().to_vec();
        out.extend(nodes);
        for f in [0u16, 1, 1, 2, 5] {
            out.extend_from_slice(&f.to_be_bytes());
        }
        (out, vec![0, second])
    }

    #[test]
    fn reads_trees_and_finds_faces_near_a_box() {
        let (data, roots) = region(0x10, 2);
        let (trees, repaired) = read_trees(&data, &roots).unwrap();
        assert_eq!(repaired, 0);
        let near = |t: &BspTree, min: f32, max: f32| {
            let mut out = Vec::new();
            t.faces_near([min, 0.0, 0.0], [max, 0.0, 0.0], |f| out.push(f));
            out.sort();
            out
        };
        assert_eq!(near(&trees[0], 0.0, 5.0), [0, 1]);
        assert_eq!(near(&trees[0], 12.0, 20.0), [1, 2]);
        assert_eq!(near(&trees[0], 5.0, 12.0), [0, 1, 1, 2]);
        assert_eq!(trees[1].faces, [5]);
    }

    #[test]
    fn repairs_a_leaf_of_32_stored_as_empty() {
        // x < 10: faces 0..32; else faces [40, 41]. The first leaf's count
        // (32) and the second's first index (32) were both zeroed.
        let mut nodes = Vec::new();
        nodes.extend(split(0, 10.0, 0x10, 0x24));
        nodes.extend(leaf(0, 0));
        nodes.extend(leaf(2, 0));
        let mut data = (nodes.len() as u32).to_be_bytes().to_vec();
        data.extend(nodes);
        for f in (0u16..32).chain([40, 41]) {
            data.extend_from_slice(&f.to_be_bytes());
        }
        let (trees, repaired) = read_trees(&data, &[0]).unwrap();
        assert_eq!(repaired, 2);
        let mut above = Vec::new();
        trees[0].faces_near([20.0, 0.0, 0.0], [30.0, 0.0, 0.0], |f| above.push(f));
        assert_eq!(above, [40, 41]);
        let mut below = 0;
        trees[0].faces_near([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], |_| below += 1);
        assert_eq!(below, 32);
    }

    #[test]
    fn rejects_shapes_that_dont_fit() {
        let (data, roots) = region(0x10, 2);
        let mut broken = data.clone();
        // The upper child's offset doesn't follow the lower subtree.
        broken[4 + 12..4 + 16].copy_from_slice(&0x30u32.to_be_bytes());
        assert!(read_trees(&broken, &roots).is_err());
        // A split axis that isn't x, y or z.
        let mut broken = data;
        broken[4] = 3;
        assert!(read_trees(&broken, &roots).is_err());
    }
}
