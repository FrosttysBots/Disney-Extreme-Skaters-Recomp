use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::bytes::be_u32;
use crate::dol::{DOL_HEADER_SIZE, DolHeader};
use crate::error::{Error, Result};
use crate::fst::Fst;
use crate::header::{APPLOADER_OFFSET, BI2_OFFSET, BI2_SIZE, DiscHeader, HEADER_SIZE};

/// Refuse to allocate an FST larger than this; real ones are well under 1 MiB.
const MAX_FST_SIZE: u32 = 64 * 1024 * 1024;

/// One of the disc's system areas, which Dolphin extracts into `sys/`.
#[derive(Debug, Clone, Copy)]
pub struct SystemFile {
    pub name: &'static str,
    pub offset: u64,
    pub size: u64,
}

/// An open GameCube disc image.
pub struct Disc<R> {
    reader: R,
    header: DiscHeader,
    fst: Fst,
    dol: DolHeader,
    apploader_size: u64,
}

impl Disc<BufReader<File>> {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::new(BufReader::new(File::open(path)?))
    }
}

impl<R: Read + Seek> Disc<R> {
    pub fn new(mut reader: R) -> Result<Self> {
        let mut head = vec![0u8; HEADER_SIZE];
        read_at(&mut reader, 0, &mut head).map_err(|err| match err {
            Error::Io(io) if io.kind() == io::ErrorKind::UnexpectedEof => {
                Error::UnsupportedFormat("file is too small to be a GameCube disc image".into())
            }
            other => other,
        })?;
        detect_container(&head)?;
        let header = DiscHeader::parse(&head)?;

        let mut apploader = [0u8; 0x20];
        read_at(&mut reader, APPLOADER_OFFSET, &mut apploader)?;
        let apploader_size =
            0x20 + u64::from(be_u32(&apploader, 0x14)) + u64::from(be_u32(&apploader, 0x18));

        if header.fst_size == 0 || header.fst_size > MAX_FST_SIZE {
            return Err(Error::BadFst(format!(
                "implausible size {:#x} in disc header",
                header.fst_size
            )));
        }
        let mut fst_bytes = vec![0u8; header.fst_size as usize];
        read_at(&mut reader, header.fst_offset.into(), &mut fst_bytes)?;
        let fst = Fst::parse(&fst_bytes)?;

        let mut dol_bytes = [0u8; DOL_HEADER_SIZE];
        read_at(&mut reader, header.dol_offset.into(), &mut dol_bytes)?;
        let dol = DolHeader::parse(&dol_bytes)?;

        Ok(Self {
            reader,
            header,
            fst,
            dol,
            apploader_size,
        })
    }

    pub fn header(&self) -> &DiscHeader {
        &self.header
    }

    pub fn fst(&self) -> &Fst {
        &self.fst
    }

    pub fn dol(&self) -> &DolHeader {
        &self.dol
    }

    /// The system areas, named the way Dolphin's "Extract Entire Disc" names them.
    pub fn system_files(&self) -> [SystemFile; 5] {
        [
            SystemFile {
                name: "boot.bin",
                offset: 0,
                size: HEADER_SIZE as u64,
            },
            SystemFile {
                name: "bi2.bin",
                offset: BI2_OFFSET,
                size: BI2_SIZE,
            },
            SystemFile {
                name: "apploader.img",
                offset: APPLOADER_OFFSET,
                size: self.apploader_size,
            },
            SystemFile {
                name: "main.dol",
                offset: self.header.dol_offset.into(),
                size: self.dol.file_size(),
            },
            SystemFile {
                name: "fst.bin",
                offset: self.header.fst_offset.into(),
                size: self.header.fst_size.into(),
            },
        ]
    }

    /// Reads a whole file from the disc's file system by path.
    pub fn read_file(&mut self, path: &str) -> Result<Vec<u8>> {
        let node = self
            .fst
            .find(path)
            .ok_or_else(|| Error::NotFound(path.into()))?;
        let (offset, size) = node
            .file_range()
            .ok_or_else(|| Error::IsDirectory(path.into()))?;
        self.read_range(offset, size)
    }

    pub fn read_range(&mut self, offset: u64, size: u64) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; size as usize];
        read_at(&mut self.reader, offset, &mut buf)?;
        Ok(buf)
    }

    /// Streams a byte range of the disc into `out` without loading it all into memory.
    pub fn copy_range(&mut self, offset: u64, size: u64, out: &mut impl Write) -> Result<()> {
        self.reader.seek(SeekFrom::Start(offset))?;
        let copied = io::copy(&mut (&mut self.reader).take(size), out)?;
        if copied != size {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("disc image ended {} bytes early", size - copied),
            )));
        }
        Ok(())
    }

    pub fn into_inner(self) -> R {
        self.reader
    }
}

fn read_at(reader: &mut (impl Read + Seek), offset: u64, buf: &mut [u8]) -> Result<()> {
    reader.seek(SeekFrom::Start(offset))?;
    reader.read_exact(buf)?;
    Ok(())
}

/// Gives a helpful error for compressed formats that this crate can't read yet.
fn detect_container(head: &[u8]) -> Result<()> {
    let format = match &head[0..4] {
        b"RVZ\x01" => "RVZ",
        b"WIA\x01" => "WIA",
        b"CISO" => "CISO",
        [0x01, 0xC0, 0x0B, 0xB1] => "GCZ",
        _ => return Ok(()),
    };
    Err(Error::UnsupportedFormat(format!(
        "this is a compressed {format} image. Convert it to a plain ISO in Dolphin: \
         right-click the game, choose Convert File..., and set Format to ISO"
    )))
}
