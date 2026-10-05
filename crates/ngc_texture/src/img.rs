//! `.img.ngc`: a single image with a 36-byte big-endian header.
//!
//! ```text
//! 0x00  u32  version (2)
//! 0x04  u32  unused (always 0x0012FF5C, likely a leftover pointer)
//! 0x08  u32  width   } the visible image
//! 0x0C  u32  height  }
//! 0x10  u32  unknown (0 or 32)
//! 0x14  u32  unknown (0)
//! 0x18  u32  stored width   } padded size of the pixel data
//! 0x1C  u32  stored height  }
//! 0x20  u16  1 if a CMPR alpha mask follows the color data
//! 0x22  u16  unknown flag
//! 0x24  pixel data
//! ```
//!
//! The pixel format isn't recorded; it follows from the data size. Loading
//! screens are RGBA8; everything else is CMPR, optionally followed by an
//! alpha mask of the same size. The 8 memory-card icons use a palette
//! format that isn't supported yet.

use crate::gx::{cmpr, rgba8};
use crate::{Error, Image, MAX_DIMENSION, Result, be_u32, stored_dimension};

pub const VERSION: u32 = 2;
const HEADER_SIZE: usize = 0x24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImgFormat {
    Rgba8,
    Cmpr,
    CmprWithAlpha,
}

#[derive(Debug, Clone)]
pub struct ImgFile<'a> {
    pub width: u32,
    pub height: u32,
    pub stored_width: u32,
    pub stored_height: u32,
    pub format: ImgFormat,
    pixels: &'a [u8],
}

impl<'a> ImgFile<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let header = data
            .get(..HEADER_SIZE)
            .ok_or_else(|| Error::Truncated("missing image header".into()))?;
        let version = be_u32(header, 0);
        if version != VERSION {
            return Err(Error::UnsupportedVersion(version));
        }

        let width = stored_dimension(be_u32(header, 0x08));
        let height = stored_dimension(be_u32(header, 0x0C));
        let stored_width = stored_dimension(be_u32(header, 0x18));
        let stored_height = stored_dimension(be_u32(header, 0x1C));
        if stored_width > MAX_DIMENSION || stored_height > MAX_DIMENSION {
            return Err(Error::Unsupported(format!(
                "implausible size {stored_width}x{stored_height}"
            )));
        }
        let has_alpha_mask = u16::from_be_bytes([header[0x20], header[0x21]]) != 0;

        let pixels = &data[HEADER_SIZE..];
        let cmpr_size = cmpr::data_size(stored_width, stored_height);
        let format = if pixels.len() == rgba8::data_size(stored_width, stored_height) {
            ImgFormat::Rgba8
        } else if !has_alpha_mask && pixels.len() == cmpr_size {
            ImgFormat::Cmpr
        } else if has_alpha_mask && pixels.len() == cmpr_size * 2 {
            ImgFormat::CmprWithAlpha
        } else {
            return Err(Error::Unsupported(format!(
                "{} bytes of pixel data for a {stored_width}x{stored_height} image",
                pixels.len()
            )));
        };

        Ok(Self {
            width,
            height,
            stored_width,
            stored_height,
            format,
            pixels,
        })
    }

    /// Decodes the image, cropped to its visible size.
    pub fn decode(&self) -> Result<Image> {
        let (w, h) = (self.stored_width, self.stored_height);
        let full = match self.format {
            ImgFormat::Rgba8 => rgba8::decode(self.pixels, w, h)?,
            ImgFormat::Cmpr => cmpr::decode(self.pixels, w, h)?,
            ImgFormat::CmprWithAlpha => {
                let (color, mask) = self.pixels.split_at(cmpr::data_size(w, h));
                let mut image = cmpr::decode(color, w, h)?;
                image.apply_alpha_mask(&cmpr::decode(mask, w, h)?);
                image
            }
        };
        Ok(full.finish(self.width, self.height))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(width: u32, height: u32, stored: (u32, u32), alpha_mask: bool) -> Vec<u8> {
        let mut out = vec![0u8; HEADER_SIZE];
        out[0..4].copy_from_slice(&VERSION.to_be_bytes());
        out[0x08..0x0C].copy_from_slice(&width.to_be_bytes());
        out[0x0C..0x10].copy_from_slice(&height.to_be_bytes());
        out[0x18..0x1C].copy_from_slice(&stored.0.to_be_bytes());
        out[0x1C..0x20].copy_from_slice(&stored.1.to_be_bytes());
        out[0x20] = 0;
        out[0x21] = u8::from(alpha_mask);
        out
    }

    #[test]
    fn rgba8_image_is_cropped() {
        let mut data = header(3, 4, (4, 4), false);
        data.extend_from_slice(&[0xAB; 64]);
        let img = ImgFile::parse(&data).unwrap();
        assert_eq!(img.format, ImgFormat::Rgba8);
        let image = img.decode().unwrap();
        assert_eq!((image.width, image.height), (3, 4));
        assert_eq!(&image.rgba[..4], &[0xAB; 4]);
    }

    #[test]
    fn zero_dimensions_mean_32() {
        let mut data = header(0, 16, (0, 16), false);
        data.extend_from_slice(&vec![0; cmpr::data_size(32, 16)]);
        let img = ImgFile::parse(&data).unwrap();
        assert_eq!((img.width, img.stored_width, img.height), (32, 32, 16));
        assert_eq!(img.format, ImgFormat::Cmpr);
    }

    #[test]
    fn alpha_mask_sets_alpha_from_green() {
        let mut data = header(8, 8, (8, 8), true);
        // Color: four solid red blocks. Mask: the first stored block black,
        // the rest white. Rows are stored bottom-up, so the first stored
        // block ends up at the bottom-left of the decoded image.
        let block = |c0: u16| [c0.to_be_bytes()[0], c0.to_be_bytes()[1], 0, 0, 0, 0, 0, 0];
        for _ in 0..4 {
            data.extend_from_slice(&block(0xF800));
        }
        data.extend_from_slice(&block(0x0000));
        for _ in 0..3 {
            data.extend_from_slice(&block(0xFFFF));
        }
        let image = ImgFile::parse(&data).unwrap().decode().unwrap();
        let pixel = |x: usize, y: usize| &image.rgba[(y * 8 + x) * 4..][..4];
        assert_eq!(pixel(0, 7), &[255, 0, 0, 0]);
        assert_eq!(pixel(0, 0), &[255, 0, 0, 255]);
        assert_eq!(pixel(4, 7), &[255, 0, 0, 255]);
    }

    #[test]
    fn padding_rows_come_first_in_stored_order() {
        // 4x3 visible inside 4x4 stored. Stored rows (bottom-up) have red 0..3;
        // stored row 0 is padding, so the picture is rows 3, 2, 1.
        let mut data = header(4, 3, (4, 4), false);
        let mut tile = [0u8; 64];
        for i in 0..16 {
            tile[i * 2] = 255;
            tile[i * 2 + 1] = (i / 4) as u8;
        }
        data.extend_from_slice(&tile);
        let image = ImgFile::parse(&data).unwrap().decode().unwrap();
        let reds: Vec<u8> = image.rgba.chunks_exact(16).map(|row| row[0]).collect();
        assert_eq!(reds, [3, 2, 1]);
    }

    #[test]
    fn unknown_layout_is_reported() {
        let mut data = header(32, 32, (32, 32), false);
        data.extend_from_slice(&[0; 1536]); // palette-based memory card icon
        assert!(matches!(ImgFile::parse(&data), Err(Error::Unsupported(_))));
    }
}
