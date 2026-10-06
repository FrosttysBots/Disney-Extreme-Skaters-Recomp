//! Neversoft QB scripts: the compiled form of the game's `.q` script files.
//!
//! A `.qb` file is a stream of one-byte tokens, some followed by data.
//! Multi-byte values are **little-endian**, even on the GameCube. Names are
//! stored as 32-bit [checksums](checksum); each file ends with a symbol
//! table (`0x2B` tokens) mapping the checksums it uses back to their
//! original names.
//!
//! - [`token`] reads the token stream.
//! - [`decompile`] turns tokens back into readable script source.
//! - [`value`] parses top-level data definitions, such as a level's
//!   `NodeArray`, into structured values.

pub mod decompile;
pub mod token;
pub mod value;
pub mod vm;

use std::collections::HashMap;

pub use decompile::decompile;
pub use token::{Token, tokenize};
pub use value::{Definition, Value, parse_definitions};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unknown token {op:#04x} at {offset:#x}")]
    UnknownToken { op: u8, offset: usize },
    #[error("file is truncated at {0:#x}")]
    Truncated(usize),
    #[error("missing end-of-file token")]
    MissingEnd,
    #[error("unexpected {found} at {offset:#x}: {context}")]
    Unexpected {
        found: String,
        offset: usize,
        context: String,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Neversoft's name checksum: a CRC-32 of the lowercased name, without the
/// final bit inversion of standard CRC-32.
pub fn checksum(name: &str) -> u32 {
    let mut crc = u32::MAX;
    for byte in name.bytes().map(|b| b.to_ascii_lowercase()) {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    crc
}

/// Checksum-to-name lookups, built from the symbol tables of any number of
/// script files.
#[derive(Debug, Default, Clone)]
pub struct Symbols {
    names: HashMap<u32, String>,
}

impl Symbols {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the symbol-table entries from a token stream.
    pub fn add_tokens(&mut self, tokens: &[(usize, Token)]) {
        for (_, token) in tokens {
            if let Token::ChecksumName(checksum, name) = token {
                self.names.entry(*checksum).or_insert_with(|| name.clone());
            }
        }
    }

    /// Adds a name directly, e.g. one known from the game's code.
    pub fn add_name(&mut self, name: &str) {
        self.names
            .entry(checksum(name))
            .or_insert_with(|| name.to_string());
    }

    pub fn get(&self, checksum: u32) -> Option<&str> {
        self.names.get(&checksum).map(String::as_str)
    }

    /// The name as it would appear in source: plain when it's a simple
    /// identifier, `#"Big 1"` when it contains other characters, and
    /// `#0x1234abcd` when it isn't known.
    pub fn name(&self, checksum: u32) -> String {
        match self.get(checksum) {
            Some(name)
                if !name.is_empty()
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') =>
            {
                name.to_string()
            }
            Some(name) => format!("#\"{name}\""),
            None => format!("#{checksum:#010x}"),
        }
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_the_game() {
        // From the first token of beach.qb and its symbol table.
        assert_eq!(checksum("NodeArray"), 0xC472_ECC5);
        assert_eq!(checksum("nodearray"), checksum("NODEARRAY"));
    }

    #[test]
    fn unknown_names_fall_back_to_hex() {
        let mut symbols = Symbols::new();
        symbols.add_name("Skater");
        assert_eq!(symbols.name(checksum("skater")), "Skater");
        assert_eq!(symbols.name(0xDEAD_BEEF), "#0xdeadbeef");
        symbols.add_name("Big 1");
        assert_eq!(symbols.name(checksum("Big 1")), "#\"Big 1\"");
    }
}
