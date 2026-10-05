//! Reading GameCube disc images (raw `.iso` / `.gcm`).
//!
//! A GameCube disc starts with a 0x440-byte header (`boot.bin`), followed by
//! `bi2.bin`, the apploader, the main executable (`main.dol`) and a file
//! system table (FST) that describes every file on the disc. All multi-byte
//! values are big-endian.

mod bytes;
pub mod disc;
pub mod dol;
pub mod error;
pub mod fst;
pub mod header;

pub use disc::{Disc, SystemFile};
pub use dol::{DolHeader, DolSection, SectionKind};
pub use error::{Error, Result};
pub use fst::{Fst, Node, NodeKind};
pub use header::{DiscHeader, Region};
