use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    UnsupportedFormat(String),

    #[error("not a GameCube disc image (magic word is {0:#010x}, expected 0xc2339f3d)")]
    BadMagic(u32),

    #[error("corrupt file system table: {0}")]
    BadFst(String),

    #[error("corrupt main.dol: {0}")]
    BadDol(String),

    #[error("no file at `{0}`")]
    NotFound(String),

    #[error("`{0}` is a directory")]
    IsDirectory(String),
}

pub type Result<T> = std::result::Result<T, Error>;
