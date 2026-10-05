//! The DOL executable header.
//!
//! A DOL has up to 7 text (code) and 11 data sections. The 0x100-byte header
//! stores, for all 18 slots in order, the file offsets, then the load
//! addresses, then the sizes. These addresses are what Ghidra needs to map
//! `main.dol` into memory correctly.

use crate::bytes::be_u32;
use crate::error::{Error, Result};

pub const DOL_HEADER_SIZE: usize = 0x100;
const TEXT_SECTIONS: usize = 7;
const DATA_SECTIONS: usize = 11;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionKind {
    Text,
    Data,
}

#[derive(Debug, Clone, Copy)]
pub struct DolSection {
    pub kind: SectionKind,
    /// Slot number within its kind (`.text0`..`.text6`, `.data0`..`.data10`).
    pub index: usize,
    pub file_offset: u32,
    pub address: u32,
    pub size: u32,
}

impl DolSection {
    pub fn name(&self) -> String {
        match self.kind {
            SectionKind::Text => format!(".text{}", self.index),
            SectionKind::Data => format!(".data{}", self.index),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DolHeader {
    /// Only the slots that are in use.
    pub sections: Vec<DolSection>,
    pub bss_address: u32,
    pub bss_size: u32,
    pub entry_point: u32,
}

impl DolHeader {
    pub fn parse(buf: &[u8]) -> Result<Self> {
        if buf.len() < DOL_HEADER_SIZE {
            return Err(Error::BadDol("header is truncated".into()));
        }

        let mut sections = Vec::new();
        for slot in 0..TEXT_SECTIONS + DATA_SECTIONS {
            let (kind, index) = if slot < TEXT_SECTIONS {
                (SectionKind::Text, slot)
            } else {
                (SectionKind::Data, slot - TEXT_SECTIONS)
            };
            let section = DolSection {
                kind,
                index,
                file_offset: be_u32(buf, slot * 4),
                address: be_u32(buf, 0x48 + slot * 4),
                size: be_u32(buf, 0x90 + slot * 4),
            };
            if section.size == 0 {
                continue;
            }
            if (section.file_offset as usize) < DOL_HEADER_SIZE {
                return Err(Error::BadDol(format!(
                    "{} starts inside the header (offset {:#x})",
                    section.name(),
                    section.file_offset
                )));
            }
            sections.push(section);
        }

        if sections.is_empty() {
            return Err(Error::BadDol("no sections".into()));
        }

        Ok(Self {
            sections,
            bss_address: be_u32(buf, 0xD8),
            bss_size: be_u32(buf, 0xDC),
            entry_point: be_u32(buf, 0xE0),
        })
    }

    /// Size of the whole DOL file: the end of its furthest section.
    pub fn file_size(&self) -> u64 {
        self.sections
            .iter()
            .map(|s| u64::from(s.file_offset) + u64::from(s.size))
            .max()
            .unwrap_or(0)
            .max(DOL_HEADER_SIZE as u64)
    }
}
