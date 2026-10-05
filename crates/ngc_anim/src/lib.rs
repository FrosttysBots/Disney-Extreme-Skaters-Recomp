//! Character skeletons (`.ske`) and animations (`.ska.ngc`).
//!
//! - [`Skeleton`]: bone names, parents and mirror partners. Little-endian.
//! - [`KeyTables`]: the two 256-entry tables of common rotations and
//!   translations that compressed keys index into
//!   (`skeletons/anims/standardkeyq.bin` and `standardkeyt.bin`).
//! - [`Animation`]: per-bone rotation and translation keys, decoded from
//!   the compressed streams and sampled at any time.
//! - [`pose`]: turns sampled bone transforms into model-space matrices
//!   and skinning matrices.
//!
//! The animation format was worked out from the data and then confirmed
//! against the game's decoder in `main.dol` (around `0x80067BE8`).

pub mod animation;
pub mod pose;
pub mod skeleton;

pub use animation::{Animation, BoneTrack, KeyTables, RotationKey, TranslationKey};
pub use skeleton::{Bone, Skeleton};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("file is truncated: {0}")]
    Truncated(String),
    #[error("invalid data: {0}")]
    Invalid(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, Error>;
