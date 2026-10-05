//! `.tex.ngc`: a dictionary of mipmapped textures, big-endian.
//!
//! ```text
//! u32  version (1)
//! u32  texture count
//! per texture:
//!   u32  checksum of the texture's name (models refer to textures by it)
//!   u32  width, u32 height
//!   u32  mip level count
//!   u32  format: 0 = CMPR, 1 = CMPR plus a CMPR alpha mask, 2 = RGBA8
//!   u32  unknown flag (0 or 1)
//!   per level: u32 size, then the level's data
//!   for format 1, the alpha mask's levels follow all of the color levels
//! ```
//!
//! The stored level sizes hit the same quirk as dimensions (32 and 8192 are
//! stored as 0), so level sizes are computed from the dimensions instead.

use crate::gx::{cmpr, rgba8};
use crate::{Error, Image, MAX_DIMENSION, Result, be_u32, stored_dimension};

pub const VERSION: u32 = 1;
const TEXTURE_HEADER_SIZE: usize = 24;
const MAX_LEVELS: u32 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TexFormat {
    Cmpr,
    CmprWithAlpha,
    Rgba8,
}

#[derive(Debug, Clone)]
pub struct Texture<'a> {
    pub checksum: u32,
    pub width: u32,
    pub height: u32,
    pub format: TexFormat,
    pub flag: u32,
    levels: Vec<&'a [u8]>,
    alpha_levels: Vec<&'a [u8]>,
}

impl Texture<'_> {
    pub fn level_count(&self) -> usize {
        self.levels.len()
    }

    pub fn level_dimensions(&self, level: usize) -> (u32, u32) {
        level_dimensions(self.width, self.height, level)
    }

    /// Decodes one mip level (0 is full size).
    pub fn decode_level(&self, level: usize) -> Result<Image> {
        let data = self.levels.get(level).ok_or_else(|| {
            Error::Unsupported(format!(
                "no mip level {level} (texture has {})",
                self.levels.len()
            ))
        })?;
        let (w, h) = self.level_dimensions(level);
        let full = match self.format {
            TexFormat::Rgba8 => rgba8::decode(data, w, h)?,
            TexFormat::Cmpr => cmpr::decode(data, w, h)?,
            TexFormat::CmprWithAlpha => {
                let mut image = cmpr::decode(data, w, h)?;
                image.apply_alpha_mask(&cmpr::decode(self.alpha_levels[level], w, h)?);
                image
            }
        };
        Ok(full.finish(w, h))
    }
}

#[derive(Debug, Clone)]
pub struct TexDictionary<'a> {
    pub textures: Vec<Texture<'a>>,
}

impl<'a> TexDictionary<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let header = data
            .get(..8)
            .ok_or_else(|| Error::Truncated("missing dictionary header".into()))?;
        let version = be_u32(header, 0);
        if version != VERSION {
            return Err(Error::UnsupportedVersion(version));
        }
        let count = be_u32(header, 4);

        let mut textures = Vec::new();
        let mut offset = 8;
        for index in 0..count {
            let header = data
                .get(offset..offset + TEXTURE_HEADER_SIZE)
                .ok_or_else(|| {
                    Error::Truncated(format!("texture {index} header at {offset:#x}"))
                })?;
            let checksum = be_u32(header, 0);
            let width = stored_dimension(be_u32(header, 4));
            let height = stored_dimension(be_u32(header, 8));
            let level_count = be_u32(header, 12);
            let format = match be_u32(header, 16) {
                0 => TexFormat::Cmpr,
                1 => TexFormat::CmprWithAlpha,
                2 => TexFormat::Rgba8,
                other => {
                    return Err(Error::Unsupported(format!(
                        "texture {index} at {offset:#x} has format {other}"
                    )));
                }
            };
            let flag = be_u32(header, 20);
            if width > MAX_DIMENSION
                || height > MAX_DIMENSION
                || !(1..=MAX_LEVELS).contains(&level_count)
            {
                return Err(Error::Unsupported(format!(
                    "texture {index} at {offset:#x}: {width}x{height} with {level_count} levels"
                )));
            }
            offset += TEXTURE_HEADER_SIZE;

            let level_size = |level: usize| {
                let (w, h) = level_dimensions(width, height, level);
                match format {
                    TexFormat::Rgba8 => rgba8::data_size(w, h),
                    TexFormat::Cmpr | TexFormat::CmprWithAlpha => cmpr::data_size(w, h),
                }
            };
            let levels = read_levels(data, &mut offset, level_count, level_size, index)?;
            let alpha_levels = if format == TexFormat::CmprWithAlpha {
                read_levels(data, &mut offset, level_count, level_size, index)?
            } else {
                Vec::new()
            };

            textures.push(Texture {
                checksum,
                width,
                height,
                format,
                flag,
                levels,
                alpha_levels,
            });
        }

        if offset != data.len() {
            return Err(Error::TrailingData(data.len().saturating_sub(offset)));
        }
        Ok(Self { textures })
    }
}

fn level_dimensions(width: u32, height: u32, level: usize) -> (u32, u32) {
    let shift = level.min(31) as u32;
    ((width >> shift).max(1), (height >> shift).max(1))
}

fn read_levels<'a>(
    data: &'a [u8],
    offset: &mut usize,
    count: u32,
    level_size: impl Fn(usize) -> usize,
    texture: u32,
) -> Result<Vec<&'a [u8]>> {
    (0..count as usize)
        .map(|level| {
            // Skip the stored size; see the module docs.
            let start = *offset + 4;
            let end = start + level_size(level);
            let bytes = data.get(start..end).ok_or_else(|| {
                Error::Truncated(format!("texture {texture} level {level} at {start:#x}"))
            })?;
            *offset = end;
            Ok(bytes)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Spec {
        width: u32,
        height: u32,
        levels: u32,
        format: u32,
    }

    fn build(specs: &[Spec]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&VERSION.to_be_bytes());
        out.extend_from_slice(&(specs.len() as u32).to_be_bytes());
        for (i, s) in specs.iter().enumerate() {
            for value in [0x1000 + i as u32, s.width, s.height, s.levels, s.format, 0] {
                out.extend_from_slice(&value.to_be_bytes());
            }
            let (w, h) = (stored_dimension(s.width), stored_dimension(s.height));
            let sets = if s.format == 1 { 2 } else { 1 };
            for set in 0..sets {
                for level in 0..s.levels as usize {
                    let (lw, lh) = level_dimensions(w, h, level);
                    let size = if s.format == 2 {
                        rgba8::data_size(lw, lh)
                    } else {
                        cmpr::data_size(lw, lh)
                    };
                    out.extend_from_slice(&(size as u32).to_be_bytes());
                    // Fill byte identifies the set and level.
                    out.resize(out.len() + size, (set * 16 + level) as u8);
                }
            }
        }
        out
    }

    #[test]
    fn parses_mixed_formats() {
        let data = build(&[
            Spec {
                width: 16,
                height: 16,
                levels: 2,
                format: 0,
            },
            Spec {
                width: 0,
                height: 8,
                levels: 1,
                format: 1,
            }, // 0 means 32
            Spec {
                width: 4,
                height: 4,
                levels: 1,
                format: 2,
            },
        ]);
        let dict = TexDictionary::parse(&data).unwrap();
        let t = &dict.textures;
        assert_eq!(t.len(), 3);
        assert_eq!(
            (t[0].checksum, t[0].format, t[0].level_count()),
            (0x1000, TexFormat::Cmpr, 2)
        );
        assert_eq!(t[0].level_dimensions(1), (8, 8));
        assert_eq!(
            (t[1].width, t[1].height, t[1].format),
            (32, 8, TexFormat::CmprWithAlpha)
        );
        assert_eq!(t[2].format, TexFormat::Rgba8);

        let image = t[0].decode_level(1).unwrap();
        assert_eq!((image.width, image.height), (8, 8));
        assert_eq!(t[1].decode_level(0).unwrap().width, 32);
        assert_eq!(t[2].decode_level(0).unwrap().rgba.len(), 64);
    }

    #[test]
    fn alpha_levels_follow_all_color_levels() {
        let data = build(&[Spec {
            width: 16,
            height: 16,
            levels: 2,
            format: 1,
        }]);
        let dict = TexDictionary::parse(&data).unwrap();
        let t = &dict.textures[0];
        assert_eq!(t.levels[1][0], 1); // color level 1
        assert_eq!(t.alpha_levels[0][0], 16); // alpha level 0
        assert_eq!(t.alpha_levels[1][0], 17);
    }

    #[test]
    fn rejects_bad_input() {
        let mut data = build(&[Spec {
            width: 16,
            height: 16,
            levels: 1,
            format: 0,
        }]);
        data.push(0);
        assert!(matches!(
            TexDictionary::parse(&data),
            Err(Error::TrailingData(1))
        ));

        let data = build(&[Spec {
            width: 16,
            height: 16,
            levels: 1,
            format: 7,
        }]);
        assert!(matches!(
            TexDictionary::parse(&data),
            Err(Error::Unsupported(_))
        ));

        let mut data = build(&[Spec {
            width: 16,
            height: 16,
            levels: 1,
            format: 0,
        }]);
        data.truncate(data.len() - 1);
        assert!(matches!(
            TexDictionary::parse(&data),
            Err(Error::Truncated(_))
        ));

        let data = build(&[Spec {
            width: 16,
            height: 16,
            levels: 0,
            format: 0,
        }]);
        assert!(matches!(
            TexDictionary::parse(&data),
            Err(Error::Unsupported(_))
        ));
    }
}
