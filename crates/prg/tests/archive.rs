use prg::{Archive, Error, VERSION};

/// Builds an archive from (name, uncompressed size, compressed size, stored bytes).
fn build(entries: &[(&str, u32, u32, &[u8])]) -> Vec<u8> {
    let mut out = vec![0u8; 12];
    out[4..8].copy_from_slice(&VERSION.to_be_bytes());
    out[8..12].copy_from_slice(&(entries.len() as u32).to_be_bytes());
    for &(name, size, compressed, data) in entries {
        let name_len = (name.len() + 1).next_multiple_of(4);
        out.extend_from_slice(&size.to_be_bytes());
        out.extend_from_slice(&compressed.to_be_bytes());
        out.extend_from_slice(&(name_len as u16).to_be_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(name.as_bytes());
        out.resize(out.len() + name_len - name.len(), 0);
        out.extend_from_slice(data);
        out.resize(out.len().next_multiple_of(4), 0);
    }
    let total = out.len() as u32;
    out[0..4].copy_from_slice(&total.to_be_bytes());
    out
}

// "abcabcabc" as LZSS: three literals, then a 6-byte back-reference.
const COMPRESSED: &[u8] = &[0b0000_0111, b'a', b'b', b'c', 0xEE, 0xF3];

fn sample() -> Vec<u8> {
    build(&[
        ("scripts\\test.qb", 5, 0, b"hello"),
        (
            "models\\abc.mdl.ngc",
            9,
            COMPRESSED.len() as u32,
            COMPRESSED,
        ),
    ])
}

#[test]
fn reads_stored_and_compressed_entries() {
    let data = sample();
    let archive = Archive::parse(&data).unwrap();
    let entries = archive.entries();
    assert_eq!(entries.len(), 2);

    assert_eq!(entries[0].name, "scripts\\test.qb");
    assert_eq!(entries[0].path(), "scripts/test.qb");
    assert!(!entries[0].is_compressed());
    assert_eq!(&*entries[0].contents().unwrap(), b"hello");

    assert!(entries[1].is_compressed());
    assert_eq!(entries[1].size, 9);
    assert_eq!(&*entries[1].contents().unwrap(), b"abcabcabc");
}

#[test]
fn find_ignores_case_and_separators() {
    let data = sample();
    let archive = Archive::parse(&data).unwrap();
    assert!(archive.find("MODELS/ABC.mdl.ngc").is_some());
    assert!(archive.find("missing.qb").is_none());
}

#[test]
fn rejects_wrong_version() {
    let mut data = sample();
    data[4..8].copy_from_slice(&0xABCD_0003u32.to_be_bytes());
    assert!(matches!(
        Archive::parse(&data),
        Err(Error::UnsupportedVersion(0xABCD_0003))
    ));
}

#[test]
fn rejects_size_mismatch() {
    let mut data = sample();
    data.push(0);
    assert!(matches!(
        Archive::parse(&data),
        Err(Error::SizeMismatch { .. })
    ));
}

#[test]
fn rejects_truncated_entries() {
    let mut data = sample();
    data.truncate(data.len() - 8);
    let total = data.len() as u32;
    data[0..4].copy_from_slice(&total.to_be_bytes());
    assert!(matches!(Archive::parse(&data), Err(Error::Truncated(_))));
}

#[test]
fn reports_bad_compressed_data() {
    let data = build(&[("bad.bin", 100, 2, &[0xFF, b'x'])]);
    let archive = Archive::parse(&data).unwrap();
    assert!(matches!(
        archive.entries()[0].contents(),
        Err(Error::Decompress(_))
    ));
}

/// Checks every archive from a real disc. Run with
/// `cargo test -p prg -- --ignored` after extracting the game into `extracted/`.
#[test]
#[ignore = "needs the extracted game files"]
fn real_archives() {
    let dir = std::env::var("DESA_PRE_DIR").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted/files/pre").into()
    });
    let mut archives = 0;
    for entry in std::fs::read_dir(&dir).expect("extracted pre/ directory") {
        let path = entry.unwrap().path();
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("prg"))
        {
            continue;
        }
        let data = std::fs::read(&path).unwrap();
        let archive = Archive::parse(&data).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        for file in archive.entries() {
            let contents = file
                .contents()
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!(contents.len(), file.size as usize, "{}", file.name);
        }
        archives += 1;
    }
    assert!(archives > 0, "no .prg files found in {dir}");
}
