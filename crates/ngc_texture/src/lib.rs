//! Decoders for the game's GameCube texture files.
//!
//! - `.img.ngc`: a single image (UI, loading screens), see [`img`].
//! - `.tex.ngc`: a dictionary of mipmapped textures for a model or level, see [`tex`].
//!
//! Pixel data uses the GameCube's tiled GX formats, decoded in [`gx`].
//!
//! Both file types share two quirks of the tool that wrote them:
//!
//! - Pixel rows are stored bottom to top (the OpenGL convention). The
//!   decoders flip them, so every [`Image`] they return is top row first.
//! - A dimension or size of exactly 32 (and a size of exactly 8192) is
//!   stored as 0. Every other value is stored as-is, so a stored 0 is read
//!   back as 32.

pub mod gx;
pub mod img;
pub mod tex;

use thiserror::Error;

pub use img::{ImgFile, ImgFormat};
pub use tex::{TexDictionary, TexFormat, Texture};

#[derive(Debug, Error)]
pub enum Error {
    #[error("file is truncated: {0}")]
    Truncated(String),

    #[error("unsupported version {0}")]
    UnsupportedVersion(u32),

    #[error("unsupported texture layout: {0}")]
    Unsupported(String),

    #[error("{0} unexpected bytes after the last texture")]
    TrailingData(usize),
}

pub type Result<T> = std::result::Result<T, Error>;

/// A decoded image: tightly packed 8-bit RGBA, rows top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// Keeps the top-left `width` x `height` pixels.
    pub fn cropped(&self, width: u32, height: u32) -> Image {
        let (width, height) = (width.min(self.width), height.min(self.height));
        if (width, height) == (self.width, self.height) {
            return self.clone();
        }
        let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
        for row in self
            .rgba
            .chunks_exact(self.width as usize * 4)
            .take(height as usize)
        {
            rgba.extend_from_slice(&row[..width as usize * 4]);
        }
        Image {
            width,
            height,
            rgba,
        }
    }

    /// Reverses the row order.
    pub fn flipped_vertically(&self) -> Image {
        let stride = self.width as usize * 4;
        let rgba = self
            .rgba
            .chunks_exact(stride)
            .rev()
            .flatten()
            .copied()
            .collect();
        Image {
            width: self.width,
            height: self.height,
            rgba,
        }
    }

    /// Turns a decoded, padded GX image into the final picture: flips it
    /// upright (see the crate docs), then keeps the top-left `width` x
    /// `height` pixels. In stored order, the padding rows come first.
    pub(crate) fn finish(&self, width: u32, height: u32) -> Image {
        self.flipped_vertically().cropped(width, height)
    }

    /// Replaces this image's alpha with the green channel of `mask`, which
    /// is how the game stores alpha for CMPR textures (CMPR itself only has
    /// 1-bit alpha). Both images must be the same size.
    fn apply_alpha_mask(&mut self, mask: &Image) {
        for (pixel, mask) in self.rgba.chunks_exact_mut(4).zip(mask.rgba.chunks_exact(4)) {
            pixel[3] = mask[1];
        }
    }
}

/// Larger than any real GameCube texture; guards against garbage headers.
pub(crate) const MAX_DIMENSION: u32 = 4096;

/// Reads a stored dimension or size, undoing the 32 -> 0 quirk.
pub(crate) fn stored_dimension(value: u32) -> u32 {
    if value == 0 { 32 } else { value }
}

pub(crate) fn be_u32(buf: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(buf[offset..offset + 4].try_into().unwrap())
}
