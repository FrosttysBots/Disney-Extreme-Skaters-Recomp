//! The game's fonts (`fonts/<theme>/<name>/<name>.fnt.ngc`, in the panel
//! archives: `title`, `small`, `dialog`, `helper`, `timer`, and the
//! controller buttons).
//!
//! ```text
//! u32  offset of the end of the texture (from the start)
//! u32  characters (n)
//! u32  line height
//! u32  baseline
//! n x (u16 the character's top above its row's bottom, u16 code)
//!      (all little-endian, as is the texture header)
//! texture:
//!   u32 size, u16 width, u16 height, u16 bits per pixel (8), 6 bytes
//!   width x height bytes: palette indices in GX's C8 layout (8x4 tiles)
//!   256 x u16 palette, big-endian RGB5A3
//! 32 bytes (0), u32 characters again
//! n x (u16 x, y, width, height): each character's place in the texture
//! ```
//! The glyphs are white with black outlines, tinted by the text's colour.

use anyhow::{Result, bail};

/// One character: where it is in the atlas, its size, and how far its
/// top sits above the bottom of its row (`baseline` of the font).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glyph {
    pub code: u16,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub top: u16,
    /// The letter's body inside its outline (the light columns), from its
    /// left: letters go by these, their outlines overlapping. The whole
    /// cell for glyphs without one (the coloured buttons).
    pub ink_left: u16,
    pub ink_width: u16,
}

/// A font: its glyphs and their atlas (RGBA, `atlas_width` wide).
#[derive(Clone, Debug)]
pub struct Font {
    pub line_height: u32,
    pub baseline: u32,
    pub glyphs: Vec<Glyph>,
    pub atlas_width: u32,
    pub atlas_height: u32,
    pub atlas: Vec<u8>,
}

impl Font {
    pub fn parse(data: &[u8]) -> Result<Font> {
        let u32_at = |at: usize| -> Result<u32> {
            match data.get(at..at + 4) {
                Some(b) => Ok(u32::from_le_bytes(b.try_into().unwrap())),
                None => bail!("font truncated"),
            }
        };
        let u16_at = |at: usize| -> Result<u16> {
            match data.get(at..at + 2) {
                Some(b) => Ok(u16::from_le_bytes(b.try_into().unwrap())),
                None => bail!("font truncated"),
            }
        };
        let n = u32_at(4)? as usize;
        let line_height = u32_at(8)?;
        let baseline = u32_at(12)?;
        if n == 0 || n > 4096 {
            bail!("implausible character count {n}");
        }
        let texture = 16 + n * 4;
        let width = u32::from(u16_at(texture + 4)?);
        let height = u32::from(u16_at(texture + 6)?);
        let bpp = u16_at(texture + 8)?;
        if bpp != 8 || width == 0 || height == 0 || width > 1024 || height > 1024 {
            bail!("unsupported font texture {width}x{height} at {bpp} bits");
        }
        let pixels_at = texture + 16;
        let pixel_count = (width * height) as usize;
        let palette_at = pixels_at + pixel_count;
        let (Some(pixels), Some(palette)) = (
            data.get(pixels_at..palette_at),
            data.get(palette_at..palette_at + 512),
        ) else {
            bail!("font texture truncated");
        };
        let rects_at = palette_at + 512 + 36;
        let mut glyphs = Vec::with_capacity(n);
        for i in 0..n {
            let r = rects_at + i * 8;
            glyphs.push(Glyph {
                code: u16_at(16 + i * 4 + 2)?,
                top: u16_at(16 + i * 4)?,
                x: u16_at(r)?,
                y: u16_at(r + 2)?,
                width: u16_at(r + 4)?,
                height: u16_at(r + 6)?,
                ink_left: 0,
                ink_width: 0,
            });
        }
        // The palette's colours, then the pixels out of their 8x4 tiles.
        let colours: Vec<[u8; 4]> = palette
            .chunks_exact(2)
            .map(|c| rgb5a3(u16::from_be_bytes([c[0], c[1]])))
            .collect();
        let mut atlas = vec![0; pixel_count * 4];
        let tiles_across = width as usize / 8;
        for (i, &index) in pixels.iter().enumerate() {
            let (tile, within) = (i / 32, i % 32);
            let x = (tile % tiles_across) * 8 + within % 8;
            let y = (tile / tiles_across) * 4 + within / 8;
            let at = (y * width as usize + x) * 4;
            atlas[at..at + 4].copy_from_slice(&colours[usize::from(index)]);
        }
        // Each letter's body: the light core inside its outline (letters
        // go by these, their outlines overlapping, as the game sets them:
        // "CHANGE LEVEL" fits the pause menu's bar that way). Glyphs with
        // no light core (the coloured buttons) go by the whole cell.
        for g in &mut glyphs {
            let light = |x: u32| {
                (u32::from(g.y)..u32::from(g.y) + u32::from(g.height)).any(|y| {
                    let at = ((y * width + x) * 4) as usize;
                    x < width
                        && y < height
                        && atlas
                            .get(at..at + 4)
                            .is_some_and(|p| p[3] > 200 && p[0] > 180 && p[1] > 180 && p[2] > 180)
                })
            };
            let cols: Vec<u32> = (u32::from(g.x)..u32::from(g.x) + u32::from(g.width))
                .filter(|x| light(*x))
                .collect();
            match (cols.first(), cols.last()) {
                (Some(first), Some(last)) if (last - first + 1) * 5 >= u32::from(g.width) * 2 => {
                    g.ink_left = (first - u32::from(g.x)) as u16;
                    g.ink_width = (last - first + 1) as u16;
                }
                _ => {
                    g.ink_left = 0;
                    g.ink_width = g.width;
                }
            }
        }
        Ok(Font {
            line_height,
            baseline,
            glyphs,
            atlas_width: width,
            atlas_height: height,
            atlas,
        })
    }

    /// The glyph for a character, if the font has it.
    pub fn glyph(&self, c: char) -> Option<&Glyph> {
        let code = u32::from(c);
        self.glyphs.iter().find(|g| u32::from(g.code) == code)
    }

    /// How far a character moves the pen on: what of it shows, letters'
    /// outlines just meeting. A space is a third of the line height.
    pub fn advance(&self, c: char) -> f32 {
        match (c, self.glyph(c)) {
            // The body and a little: the outlines meet between letters.
            (_, Some(g)) if g.ink_width < g.width => f32::from(g.ink_width) + 1.0,
            (_, Some(g)) => f32::from(g.width) - 1.0,
            (' ', None) => self.line_height as f32 / 3.0,
            _ => 0.0,
        }
    }

    /// How wide a line of text is (characters it lacks are left out).
    pub fn width(&self, text: &str) -> f32 {
        text.chars().map(|c| self.advance(c)).sum()
    }
}

/// GX's RGB5A3: opaque RGB555 with the top bit set, else 3-bit alpha and
/// RGB444.
fn rgb5a3(v: u16) -> [u8; 4] {
    if v & 0x8000 != 0 {
        let c = |shift: u16| (((v >> shift) & 31) * 255 / 31) as u8;
        [c(10), c(5), c(0), 255]
    } else {
        let c = |shift: u16| (((v >> shift) & 15) * 17) as u8;
        [c(8), c(4), c(0), (((v >> 12) & 7) * 255 / 7) as u8]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb5a3_both_ways() {
        assert_eq!(rgb5a3(0xffff), [255, 255, 255, 255]);
        assert_eq!(rgb5a3(0x7fff), [255, 255, 255, 255]);
        assert_eq!(rgb5a3(0x0f00), [255, 0, 0, 0]);
        assert_eq!(rgb5a3(0x8000), [0, 0, 0, 255]);
    }
}
