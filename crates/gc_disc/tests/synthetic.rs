//! Tests against a tiny hand-built disc image, so no real game is needed.

use std::io::Cursor;

use gc_disc::{Disc, Error, Region, SectionKind};

const DOL_OFFSET: usize = 0x3000;
const FST_OFFSET: usize = 0x4000;

fn put_u32(buf: &mut [u8], offset: usize, value: u32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn fst_entry(buf: &mut [u8], index: usize, is_dir: bool, name_offset: u32, a: u32, b: u32) {
    let at = FST_OFFSET + index * 12;
    put_u32(buf, at, (u32::from(is_dir) << 24) | name_offset);
    put_u32(buf, at + 4, a);
    put_u32(buf, at + 8, b);
}

/// Layout:
///   levels/park.pre   "PARK"
///   levels/mall.pre   "MALL"
///   readme.txt        "hello skate"
fn build_image() -> Vec<u8> {
    let mut img = vec![0u8; 0x6000];

    // Disc header
    img[0..6].copy_from_slice(b"GTSE01");
    img[7] = 1;
    put_u32(&mut img, 0x1C, 0xC233_9F3D);
    let title = b"Test Skate Game";
    img[0x20..0x20 + title.len()].copy_from_slice(title);
    put_u32(&mut img, 0x420, DOL_OFFSET as u32);
    put_u32(&mut img, 0x424, FST_OFFSET as u32);

    // Apploader header: 0x100 bytes of code, no trailer
    put_u32(&mut img, 0x2440 + 0x14, 0x100);

    // DOL: one text and one data section
    put_u32(&mut img, DOL_OFFSET, 0x100); // .text0 offset
    put_u32(&mut img, DOL_OFFSET + 0x48, 0x8000_3100); // .text0 address
    put_u32(&mut img, DOL_OFFSET + 0x90, 0x40); // .text0 size
    put_u32(&mut img, DOL_OFFSET + 0x1C, 0x140); // .data0 offset
    put_u32(&mut img, DOL_OFFSET + 0x64, 0x8000_4000); // .data0 address
    put_u32(&mut img, DOL_OFFSET + 0xAC, 0x20); // .data0 size
    put_u32(&mut img, DOL_OFFSET + 0xD8, 0x8000_5000); // bss address
    put_u32(&mut img, DOL_OFFSET + 0xDC, 0x800); // bss size
    put_u32(&mut img, DOL_OFFSET + 0xE0, 0x8000_3100); // entry point

    // FST: 5 entries, then the string table
    let names = b"levels\0park.pre\0mall.pre\0readme.txt\0";
    fst_entry(&mut img, 0, true, 0, 0, 5);
    fst_entry(&mut img, 1, true, 0, 0, 4);
    fst_entry(&mut img, 2, false, 7, 0x5000, 4);
    fst_entry(&mut img, 3, false, 16, 0x5010, 4);
    fst_entry(&mut img, 4, false, 25, 0x5020, 11);
    let strings = FST_OFFSET + 5 * 12;
    img[strings..strings + names.len()].copy_from_slice(names);
    let fst_size = (5 * 12 + names.len()) as u32;
    put_u32(&mut img, 0x428, fst_size);
    put_u32(&mut img, 0x42C, fst_size);

    // File contents
    img[0x5000..0x5004].copy_from_slice(b"PARK");
    img[0x5010..0x5014].copy_from_slice(b"MALL");
    img[0x5020..0x502B].copy_from_slice(b"hello skate");

    img
}

fn open(img: Vec<u8>) -> Result<Disc<Cursor<Vec<u8>>>, Error> {
    Disc::new(Cursor::new(img))
}

#[test]
fn parses_header() {
    let disc = open(build_image()).unwrap();
    let header = disc.header();
    assert_eq!(header.game_id(), "GTSE01");
    assert_eq!(header.title, "Test Skate Game");
    assert_eq!(header.region(), Region::NtscU);
    assert_eq!(header.version, 1);
}

#[test]
fn lists_paths() {
    let disc = open(build_image()).unwrap();
    let paths: Vec<&str> = disc.fst().entries().map(|n| n.path.as_str()).collect();
    assert_eq!(
        paths,
        ["levels", "levels/park.pre", "levels/mall.pre", "readme.txt"]
    );
    assert_eq!(disc.fst().files().count(), 3);
    assert_eq!(disc.fst().directories().count(), 1);
    assert_eq!(disc.fst().total_file_size(), 19);
}

#[test]
fn reads_files() {
    let mut disc = open(build_image()).unwrap();
    assert_eq!(disc.read_file("levels/park.pre").unwrap(), b"PARK");
    assert_eq!(disc.read_file("/LEVELS\\MALL.PRE").unwrap(), b"MALL");
    assert_eq!(disc.read_file("readme.txt").unwrap(), b"hello skate");
    assert!(matches!(
        disc.read_file("nope.bin"),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        disc.read_file("levels"),
        Err(Error::IsDirectory(_))
    ));
}

#[test]
fn parses_dol() {
    let disc = open(build_image()).unwrap();
    let dol = disc.dol();
    assert_eq!(dol.sections.len(), 2);
    assert_eq!(dol.sections[0].kind, SectionKind::Text);
    assert_eq!(dol.sections[0].name(), ".text0");
    assert_eq!(dol.sections[1].name(), ".data0");
    assert_eq!(dol.sections[1].address, 0x8000_4000);
    assert_eq!(dol.entry_point, 0x8000_3100);
    assert_eq!(dol.file_size(), 0x160);
}

#[test]
fn system_file_sizes() {
    let disc = open(build_image()).unwrap();
    let sys = disc.system_files();
    let size_of = |name: &str| sys.iter().find(|f| f.name == name).unwrap().size;
    assert_eq!(size_of("boot.bin"), 0x440);
    assert_eq!(size_of("apploader.img"), 0x120);
    assert_eq!(size_of("main.dol"), 0x160);
    assert_eq!(size_of("fst.bin"), 96);
}

#[test]
fn copy_range_streams_bytes() {
    let mut disc = open(build_image()).unwrap();
    let mut out = Vec::new();
    disc.copy_range(0x5020, 11, &mut out).unwrap();
    assert_eq!(out, b"hello skate");
}

#[test]
fn rejects_compressed_images() {
    let mut img = build_image();
    img[0..4].copy_from_slice(b"RVZ\x01");
    match open(img) {
        Err(Error::UnsupportedFormat(msg)) => assert!(msg.contains("RVZ")),
        other => panic!("expected UnsupportedFormat, got {:?}", other.err()),
    }
}

#[test]
fn rejects_wii_discs() {
    let mut img = build_image();
    put_u32(&mut img, 0x1C, 0);
    put_u32(&mut img, 0x18, 0x5D1C_9EA3);
    assert!(matches!(open(img), Err(Error::UnsupportedFormat(_))));
}

#[test]
fn rejects_non_disc_files() {
    let mut img = build_image();
    put_u32(&mut img, 0x1C, 0x1234_5678);
    assert!(matches!(open(img), Err(Error::BadMagic(0x1234_5678))));
    assert!(matches!(
        open(vec![0u8; 16]),
        Err(Error::UnsupportedFormat(_))
    ));
}

#[test]
fn rejects_bad_directory_bounds() {
    let mut img = build_image();
    fst_entry(&mut img, 1, true, 0, 0, 99);
    assert!(matches!(open(img), Err(Error::BadFst(_))));
}
