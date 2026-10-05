use std::fmt;

use crate::bytes::{be_u32, cstr};
use crate::error::{Error, Result};

pub const HEADER_SIZE: usize = 0x440;
pub const GC_MAGIC: u32 = 0xC233_9F3D;
pub const WII_MAGIC: u32 = 0x5D1C_9EA3;
pub const BI2_OFFSET: u64 = 0x440;
pub const BI2_SIZE: u64 = 0x2000;
pub const APPLOADER_OFFSET: u64 = 0x2440;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    NtscU,
    NtscJ,
    Pal,
    Unknown(u8),
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Region::NtscU => f.write_str("NTSC-U (North America)"),
            Region::NtscJ => f.write_str("NTSC-J (Japan)"),
            Region::Pal => f.write_str("PAL (Europe/Australia)"),
            Region::Unknown(code) => write!(f, "unknown ({})", *code as char),
        }
    }
}

/// The disc header, stored at offset 0 (`boot.bin`).
#[derive(Debug, Clone)]
pub struct DiscHeader {
    /// Four-character game code followed by the two-character maker code.
    pub game_id: [u8; 6],
    /// Zero-based disc number for multi-disc games.
    pub disc_number: u8,
    pub version: u8,
    pub audio_streaming: bool,
    pub stream_buffer_size: u8,
    pub title: String,
    pub dol_offset: u32,
    pub fst_offset: u32,
    pub fst_size: u32,
    pub fst_max_size: u32,
}

impl DiscHeader {
    pub fn parse(buf: &[u8]) -> Result<Self> {
        if buf.len() < HEADER_SIZE {
            return Err(Error::UnsupportedFormat(
                "file is too small to be a GameCube disc image".into(),
            ));
        }

        let magic = be_u32(buf, 0x1C);
        if magic != GC_MAGIC {
            if be_u32(buf, 0x18) == WII_MAGIC {
                return Err(Error::UnsupportedFormat(
                    "this is a Wii disc, not a GameCube disc".into(),
                ));
            }
            return Err(Error::BadMagic(magic));
        }

        let mut game_id = [0u8; 6];
        game_id.copy_from_slice(&buf[0..6]);

        Ok(Self {
            game_id,
            disc_number: buf[6],
            version: buf[7],
            audio_streaming: buf[8] != 0,
            stream_buffer_size: buf[9],
            title: cstr(&buf[0x20..0x400]),
            dol_offset: be_u32(buf, 0x420),
            fst_offset: be_u32(buf, 0x424),
            fst_size: be_u32(buf, 0x428),
            fst_max_size: be_u32(buf, 0x42C),
        })
    }

    pub fn game_id(&self) -> String {
        String::from_utf8_lossy(&self.game_id).into_owned()
    }

    pub fn region(&self) -> Region {
        match self.game_id[3] {
            b'E' => Region::NtscU,
            b'J' => Region::NtscJ,
            b'P' | b'D' | b'F' | b'S' | b'I' | b'U' | b'X' | b'Y' => Region::Pal,
            other => Region::Unknown(other),
        }
    }
}
