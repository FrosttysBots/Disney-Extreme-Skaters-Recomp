//! Neversoft PRE archives, stored as `.prg` files on the GameCube disc.
//!
//! Layout (all big-endian):
//!
//! ```text
//! header:  u32 total file size, u32 version (0xABCD0002), u32 entry count
//! entry:   u32 uncompressed size
//!          u32 compressed size (0 = stored uncompressed)
//!          u16 name length (includes NUL and padding to 4 bytes)
//!          u16 always zero
//!          name, e.g. `models\arrow\ARROW.mdl.ngc`
//!          data (LZSS-compressed or stored), padded to 4 bytes
//! ```

pub mod lzss;

use std::borrow::Cow;

use thiserror::Error;

pub const VERSION: u32 = 0xABCD_0002;
const HEADER_SIZE: usize = 12;
const ENTRY_HEADER_SIZE: usize = 12;

#[derive(Debug, Error)]
pub enum Error {
    #[error("archive is truncated: {0}")]
    Truncated(String),

    #[error("unsupported archive version {0:#010x} (expected {VERSION:#010x})")]
    UnsupportedVersion(u32),

    #[error("header says the archive is {header} bytes but it is {actual}")]
    SizeMismatch { header: u32, actual: usize },

    #[error("compressed data for `{0}` ended early")]
    Decompress(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
pub struct Entry<'a> {
    /// The name as stored, using `\` separators.
    pub name: String,
    pub size: u32,
    /// `None` when the data is stored uncompressed.
    pub compressed_size: Option<u32>,
    raw: &'a [u8],
}

impl<'a> Entry<'a> {
    /// The name with `/` separators.
    pub fn path(&self) -> String {
        self.name.replace('\\', "/")
    }

    pub fn is_compressed(&self) -> bool {
        self.compressed_size.is_some()
    }

    /// The file's contents, decompressing if needed.
    pub fn contents(&self) -> Result<Cow<'a, [u8]>> {
        if self.compressed_size.is_none() {
            return Ok(Cow::Borrowed(self.raw));
        }
        lzss::decompress(self.raw, self.size as usize)
            .map(Cow::Owned)
            .map_err(|_| Error::Decompress(self.name.clone()))
    }
}

#[derive(Debug, Clone)]
pub struct Archive<'a> {
    entries: Vec<Entry<'a>>,
}

impl<'a> Archive<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let header = data
            .get(..HEADER_SIZE)
            .ok_or_else(|| Error::Truncated("missing header".into()))?;
        let total = be_u32(header, 0);
        let version = be_u32(header, 4);
        let count = be_u32(header, 8);

        if version != VERSION {
            return Err(Error::UnsupportedVersion(version));
        }
        if total as usize != data.len() {
            return Err(Error::SizeMismatch {
                header: total,
                actual: data.len(),
            });
        }

        let mut entries = Vec::new();
        let mut offset = HEADER_SIZE;
        for index in 0..count {
            let truncated =
                || Error::Truncated(format!("entry {index} of {count} at offset {offset:#x}"));

            let header = data
                .get(offset..offset + ENTRY_HEADER_SIZE)
                .ok_or_else(truncated)?;
            let size = be_u32(header, 0);
            let compressed_size = be_u32(header, 4);
            let name_len = usize::from(u16::from_be_bytes([header[8], header[9]]));

            let name_start = offset + ENTRY_HEADER_SIZE;
            let name_bytes = data
                .get(name_start..name_start + name_len)
                .ok_or_else(truncated)?;
            let name_end = name_bytes.iter().position(|&b| b == 0).unwrap_or(name_len);
            let name = String::from_utf8_lossy(&name_bytes[..name_end]).into_owned();

            let stored_len = if compressed_size != 0 {
                compressed_size
            } else {
                size
            } as usize;
            let data_start = name_start + name_len;
            let raw = data
                .get(data_start..data_start + stored_len)
                .ok_or_else(truncated)?;

            entries.push(Entry {
                name,
                size,
                compressed_size: (compressed_size != 0).then_some(compressed_size),
                raw,
            });
            offset = data_start + stored_len.next_multiple_of(4);
        }

        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[Entry<'a>] {
        &self.entries
    }

    /// Looks up an entry by name, ignoring ASCII case and separator style.
    pub fn find(&self, name: &str) -> Option<&Entry<'a>> {
        let wanted = name.replace('/', "\\");
        self.entries
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(&wanted))
    }
}

fn be_u32(buf: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(buf[offset..offset + 4].try_into().unwrap())
}
