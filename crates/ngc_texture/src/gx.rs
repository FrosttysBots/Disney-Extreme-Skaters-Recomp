//! GameCube GX texture formats.
//!
//! GX textures are stored in tiles rather than rows. Images are padded up to
//! whole tiles; the decoders take the padded size and return that full size,
//! so callers crop to the real dimensions afterwards.

use crate::{Error, Image, Result};

/// CMPR: S3TC/DXT1-style compression at 4 bits per pixel.
///
/// The image is split into 8x8 tiles, each holding four 4x4 blocks in the
/// order top-left, top-right, bottom-left, bottom-right. A block is two
/// big-endian RGB565 colors followed by four rows of 2-bit indices, with the
/// leftmost pixel in the highest bits.
pub mod cmpr {
    use super::*;

    pub fn data_size(width: u32, height: u32) -> usize {
        width.next_multiple_of(8) as usize * height.next_multiple_of(8) as usize / 2
    }

    pub fn decode(data: &[u8], width: u32, height: u32) -> Result<Image> {
        let (padded_w, padded_h) = (width.next_multiple_of(8), height.next_multiple_of(8));
        let needed = data_size(width, height);
        let data = data.get(..needed).ok_or_else(|| {
            Error::Truncated(format!(
                "CMPR {width}x{height} needs {needed} bytes, got {}",
                data.len()
            ))
        })?;

        let stride = padded_w as usize * 4;
        let mut rgba = vec![0u8; stride * padded_h as usize];
        let mut blocks = data.chunks_exact(8);
        for tile_y in (0..padded_h as usize).step_by(8) {
            for tile_x in (0..padded_w as usize).step_by(8) {
                for sub in 0..4 {
                    let block = blocks.next().expect("size checked above");
                    let palette = palette(
                        u16::from_be_bytes([block[0], block[1]]),
                        u16::from_be_bytes([block[2], block[3]]),
                    );
                    let (x0, y0) = (tile_x + (sub % 2) * 4, tile_y + (sub / 2) * 4);
                    for row in 0..4 {
                        let bits = block[4 + row];
                        for col in 0..4 {
                            let index = (bits >> (6 - col * 2)) & 3;
                            let at = (y0 + row) * stride + (x0 + col) * 4;
                            rgba[at..at + 4].copy_from_slice(&palette[index as usize]);
                        }
                    }
                }
            }
        }
        Ok(Image {
            width: padded_w,
            height: padded_h,
            rgba,
        })
    }

    /// The four colors a block can use. When the first color is larger the
    /// block has four opaque colors; otherwise the fourth is transparent.
    /// Blends use the GameCube hardware's 5/8 and 3/8 weights.
    fn palette(c0: u16, c1: u16) -> [[u8; 4]; 4] {
        let a = rgb565(c0);
        let b = rgb565(c1);
        let mix = |wa: u16, wb: u16, div: u16| -> [u8; 4] {
            let ch = |i: usize| ((u16::from(a[i]) * wa + u16::from(b[i]) * wb) / div) as u8;
            [ch(0), ch(1), ch(2), 255]
        };
        if c0 > c1 {
            [a, b, mix(5, 3, 8), mix(3, 5, 8)]
        } else {
            [a, b, mix(1, 1, 2), [0, 0, 0, 0]]
        }
    }

    fn rgb565(c: u16) -> [u8; 4] {
        let r = ((c >> 11) & 0x1F) as u8;
        let g = ((c >> 5) & 0x3F) as u8;
        let b = (c & 0x1F) as u8;
        [
            (r << 3) | (r >> 2),
            (g << 2) | (g >> 4),
            (b << 3) | (b >> 2),
            255,
        ]
    }
}

/// RGBA8: 32 bits per pixel in 4x4 tiles. Each 64-byte tile stores the 16
/// pixels' alpha/red pairs first, then their green/blue pairs.
pub mod rgba8 {
    use super::*;

    pub fn data_size(width: u32, height: u32) -> usize {
        width.next_multiple_of(4) as usize * height.next_multiple_of(4) as usize * 4
    }

    pub fn decode(data: &[u8], width: u32, height: u32) -> Result<Image> {
        let (padded_w, padded_h) = (width.next_multiple_of(4), height.next_multiple_of(4));
        let needed = data_size(width, height);
        let data = data.get(..needed).ok_or_else(|| {
            Error::Truncated(format!(
                "RGBA8 {width}x{height} needs {needed} bytes, got {}",
                data.len()
            ))
        })?;

        let stride = padded_w as usize * 4;
        let mut rgba = vec![0u8; stride * padded_h as usize];
        let mut tiles = data.chunks_exact(64);
        for tile_y in (0..padded_h as usize).step_by(4) {
            for tile_x in (0..padded_w as usize).step_by(4) {
                let tile = tiles.next().expect("size checked above");
                for i in 0..16 {
                    let at = (tile_y + i / 4) * stride + (tile_x + i % 4) * 4;
                    let (a, r) = (tile[i * 2], tile[i * 2 + 1]);
                    let (g, b) = (tile[32 + i * 2], tile[33 + i * 2]);
                    rgba[at..at + 4].copy_from_slice(&[r, g, b, a]);
                }
            }
        }
        Ok(Image {
            width: padded_w,
            height: padded_h,
            rgba,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(image: &Image, x: usize, y: usize) -> [u8; 4] {
        let at = (y * image.width as usize + x) * 4;
        image.rgba[at..at + 4].try_into().unwrap()
    }

    /// One 8x8 CMPR tile: four blocks, given as (c0, c1, index rows).
    fn cmpr_tile(blocks: [(u16, u16, [u8; 4]); 4]) -> Vec<u8> {
        let mut out = Vec::new();
        for (c0, c1, rows) in blocks {
            out.extend_from_slice(&c0.to_be_bytes());
            out.extend_from_slice(&c1.to_be_bytes());
            out.extend_from_slice(&rows);
        }
        out
    }

    const RED: u16 = 0xF800;
    const BLUE: u16 = 0x001F;
    const WHITE: u16 = 0xFFFF;
    const BLACK: u16 = 0x0000;

    #[test]
    fn cmpr_four_color_block() {
        // Row 0 uses indices 0,1,2,3 from left to right.
        let solid = (WHITE, BLACK, [0; 4]);
        let data = cmpr_tile([(RED, BLUE, [0b00_01_10_11, 0, 0, 0]), solid, solid, solid]);
        let image = cmpr::decode(&data, 8, 8).unwrap();
        assert_eq!(pixel(&image, 0, 0), [255, 0, 0, 255]);
        assert_eq!(pixel(&image, 1, 0), [0, 0, 255, 255]);
        assert_eq!(pixel(&image, 2, 0), [159, 0, 95, 255]); // 5/8 red + 3/8 blue
        assert_eq!(pixel(&image, 3, 0), [95, 0, 159, 255]);
        assert_eq!(pixel(&image, 0, 1), [255, 0, 0, 255]); // row 1 is all index 0
    }

    #[test]
    fn cmpr_transparent_block_and_tile_order() {
        // c0 <= c1 selects the three-color mode where index 3 is transparent.
        let data = cmpr_tile([
            (BLACK, WHITE, [0xFF; 4]), // top-left: all transparent
            (WHITE, BLACK, [0; 4]),    // top-right: white
            (RED, BLUE, [0; 4]),       // bottom-left: red
            (BLACK, WHITE, [0x55; 4]), // bottom-right: index 1 = white
        ]);
        let image = cmpr::decode(&data, 8, 8).unwrap();
        assert_eq!(pixel(&image, 0, 0), [0, 0, 0, 0]);
        assert_eq!(pixel(&image, 4, 0), [255, 255, 255, 255]);
        assert_eq!(pixel(&image, 0, 4), [255, 0, 0, 255]);
        assert_eq!(pixel(&image, 7, 7), [255, 255, 255, 255]);
    }

    #[test]
    fn cmpr_pads_small_images_to_a_whole_tile() {
        let solid = (RED, BLUE, [0; 4]);
        let image = cmpr::decode(&cmpr_tile([solid; 4]), 4, 4).unwrap();
        assert_eq!((image.width, image.height), (8, 8));
        assert_eq!(cmpr::data_size(4, 4), 32);
        assert_eq!(image.cropped(4, 4).rgba.len(), 4 * 4 * 4);
    }

    #[test]
    fn rgba8_tile_layout() {
        let mut tile = vec![0u8; 64];
        for i in 0..16u8 {
            tile[i as usize * 2] = 200 + i; // alpha
            tile[i as usize * 2 + 1] = i; // red
            tile[32 + i as usize * 2] = 100 + i; // green
            tile[33 + i as usize * 2] = 50 + i; // blue
        }
        let image = rgba8::decode(&tile, 4, 4).unwrap();
        assert_eq!(pixel(&image, 0, 0), [0, 100, 50, 200]);
        assert_eq!(pixel(&image, 3, 0), [3, 103, 53, 203]);
        assert_eq!(pixel(&image, 1, 2), [9, 109, 59, 209]);
    }

    #[test]
    fn short_data_is_an_error() {
        assert!(matches!(
            cmpr::decode(&[0; 31], 8, 8),
            Err(Error::Truncated(_))
        ));
        assert!(matches!(
            rgba8::decode(&[0; 63], 4, 4),
            Err(Error::Truncated(_))
        ));
    }
}
