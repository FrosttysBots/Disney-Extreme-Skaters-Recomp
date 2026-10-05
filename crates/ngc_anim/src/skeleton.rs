//! `.ske` skeletons, little-endian:
//!
//! ```text
//! u32  checksum
//! u32  bone count
//! u32 × count  bone name checksums
//! u32 × count  parent name checksums (0 for the root)
//! u32 × count  mirror-partner name checksums (0 for center bones)
//! ```
//!
//! There's no rest pose here; it comes from each character's `default`
//! animation.

use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bone {
    pub name: u32,
    pub parent: Option<usize>,
    /// The bone on the other side (left arm <-> right arm), by name.
    pub mirror: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skeleton {
    pub checksum: u32,
    pub bones: Vec<Bone>,
}

impl Skeleton {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let word = |i: usize| {
            data.get(i * 4..i * 4 + 4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                .ok_or_else(|| Error::Truncated(format!("skeleton word {i}")))
        };
        let checksum = word(0)?;
        let count = word(1)? as usize;
        if count == 0 || count > 1024 || data.len() != 8 + count * 12 {
            return Err(Error::Invalid(format!(
                "{count} bones doesn't fit a {}-byte skeleton",
                data.len()
            )));
        }
        let names: Vec<u32> = (0..count).map(|i| word(2 + i)).collect::<Result<_>>()?;
        let mut bones = Vec::with_capacity(count);
        for i in 0..count {
            let parent = match word(2 + count + i)? {
                0 => None,
                p => Some(names.iter().position(|&n| n == p).ok_or_else(|| {
                    Error::Invalid(format!("bone {i}'s parent {p:08x} isn't in the skeleton"))
                })?),
            };
            let mirror = Some(word(2 + 2 * count + i)?).filter(|&m| m != 0);
            bones.push(Bone {
                name: names[i],
                parent,
                mirror,
            });
        }
        Ok(Self { checksum, bones })
    }

    pub fn find(&self, name: u32) -> Option<usize> {
        self.bones.iter().position(|b| b.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_parents_and_mirrors() {
        let mut data = Vec::new();
        for w in [0xABCDu32, 3, 10, 20, 30, 0, 10, 10, 0, 30, 20] {
            data.extend_from_slice(&w.to_le_bytes());
        }
        let s = Skeleton::parse(&data).unwrap();
        assert_eq!(s.bones.len(), 3);
        assert_eq!(s.bones[0].parent, None);
        assert_eq!(s.bones[2].parent, Some(0));
        assert_eq!(s.bones[1].mirror, Some(30));
        assert_eq!(s.find(30), Some(2));
    }

    #[test]
    fn rejects_bad_sizes_and_parents() {
        let mut data = Vec::new();
        for w in [0u32, 2, 10, 20, 0, 99, 0, 0] {
            data.extend_from_slice(&w.to_le_bytes());
        }
        assert!(matches!(Skeleton::parse(&data), Err(Error::Invalid(_))));
        assert!(matches!(
            Skeleton::parse(&data[..12]),
            Err(Error::Invalid(_))
        ));
    }
}
