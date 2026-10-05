use ngc_collision::{Collision, Error, face_flags};

/// One object to build: vertices, faces as (flags, terrain, indices), and
/// optional corrupted values to store in place of the real counts.
struct Spec {
    vertices: Vec<[f32; 3]>,
    faces: Vec<(u16, u16, [u16; 3])>,
    small: bool,
    stored_vertex_count: Option<u16>,
    stored_face_count: Option<u16>,
    stored_vertex_offset: Option<u32>,
}

impl Spec {
    fn new(vertices: usize, faces: Vec<(u16, u16, [u16; 3])>, small: bool) -> Self {
        Self {
            vertices: (0..vertices).map(|i| [i as f32, 1.0, 2.0]).collect(),
            faces,
            small,
            stored_vertex_count: None,
            stored_face_count: None,
            stored_vertex_offset: None,
        }
    }
}

fn build(specs: &[Spec], stored_total_vertices: Option<u32>) -> Vec<u8> {
    let total_vertices: usize = specs.iter().map(|s| s.vertices.len()).sum();
    let large: usize = specs
        .iter()
        .filter(|s| !s.small)
        .map(|s| s.faces.len())
        .sum();
    let small: usize = specs
        .iter()
        .filter(|s| s.small)
        .map(|s| s.faces.len())
        .sum();

    let mut out = Vec::new();
    let u32s = |out: &mut Vec<u8>, vs: &[u32]| {
        vs.iter()
            .for_each(|v| out.extend_from_slice(&v.to_be_bytes()))
    };
    u32s(
        &mut out,
        &[
            8,
            specs.len() as u32,
            stored_total_vertices.unwrap_or(total_vertices as u32),
        ],
    );
    u32s(&mut out, &[large as u32, small as u32, 0, 0, 0]);

    let (mut vertex_offset, mut face_offset) = (0u32, 0u32);
    for (i, s) in specs.iter().enumerate() {
        out.extend_from_slice(&(0x1000 + i as u32).to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(
            &s.stored_vertex_count
                .unwrap_or(s.vertices.len() as u16)
                .to_be_bytes(),
        );
        out.extend_from_slice(
            &s.stored_face_count
                .unwrap_or(s.faces.len() as u16)
                .to_be_bytes(),
        );
        out.extend_from_slice(&[u8::from(s.small), 0]);
        out.extend_from_slice(&face_offset.to_be_bytes());
        for v in [-1.0f32, -2.0, -3.0, 1.0, 1.0, 2.0, 3.0, 1.0] {
            out.extend_from_slice(&v.to_be_bytes());
        }
        out.extend_from_slice(
            &s.stored_vertex_offset
                .unwrap_or(vertex_offset)
                .to_be_bytes(),
        );
        out.extend_from_slice(&[0; 12]);
        vertex_offset += s.vertices.len() as u32;
        face_offset += s.faces.len() as u32 * if s.small { 8 } else { 12 };
    }
    for s in specs {
        for v in &s.vertices {
            v.iter()
                .for_each(|c| out.extend_from_slice(&c.to_be_bytes()));
        }
    }
    for s in specs {
        out.extend((0..s.vertices.len()).map(|i| i as u8));
    }
    out.resize(out.len().next_multiple_of(4), 0);
    for s in specs {
        for &(flags, terrain, idx) in &s.faces {
            out.extend_from_slice(&flags.to_be_bytes());
            out.extend_from_slice(&terrain.to_be_bytes());
            if s.small {
                out.extend_from_slice(&[idx[0] as u8, idx[1] as u8, idx[2] as u8, 0]);
            } else {
                idx.iter()
                    .for_each(|i| out.extend_from_slice(&i.to_be_bytes()));
                out.extend_from_slice(&[0, 0]);
            }
        }
    }
    out.extend_from_slice(&[0xAB; 16]); // stand-in BSP data
    out
}

fn faces(count: usize, vertices: u16) -> Vec<(u16, u16, [u16; 3])> {
    (0..count)
        .map(|i| {
            let i = i as u16;
            (
                face_flags::VERT,
                i,
                [i % vertices, (i + 1) % vertices, (i + 2) % vertices],
            )
        })
        .collect()
}

#[test]
fn parses_small_and_large_faces() {
    let data = build(
        &[
            Spec::new(4, faces(2, 4), true),
            Spec::new(300, faces(3, 300), false),
        ],
        None,
    );
    let col = Collision::parse(&data).unwrap();
    assert_eq!(col.objects.len(), 2);
    assert_eq!(col.vertex_count(), 304);
    assert_eq!(col.face_count(), 5);
    assert_eq!(col.repaired_fields, 0);

    let first = &col.objects[0];
    assert_eq!(first.checksum, 0x1000);
    assert_eq!(first.bbox, [-1.0, -2.0, -3.0, 1.0, 2.0, 3.0]);
    assert_eq!(first.vertices[3], [3.0, 1.0, 2.0]);
    assert_eq!(first.intensities, [0, 1, 2, 3]);
    assert_eq!(first.faces[1].indices, [1, 2, 3]);
    assert_eq!(first.faces[1].terrain, 1);

    let second = &col.objects[1];
    assert_eq!(second.vertices[0], [0.0, 1.0, 2.0]);
    assert_eq!(second.faces[2].indices, [2, 3, 4]);
    assert_eq!(col.bsp, [0xAB; 16]);
}

#[test]
fn repairs_a_zeroed_count_of_32() {
    // A lone object with 32 vertices: the count and the header total both read 0.
    let mut spec = Spec::new(32, faces(4, 32), true);
    spec.stored_vertex_count = Some(0);
    let col = Collision::parse(&build(&[spec], Some(0))).unwrap();
    assert_eq!(col.objects[0].vertices.len(), 32);
    assert_eq!(col.objects[0].faces.len(), 4);
    assert_eq!(col.repaired_fields, 1);
}

#[test]
fn repairs_counts_and_offsets_with_a_zeroed_byte() {
    // 0x120 faces stored as 0x100, and the next object's vertex offset
    // 0x420 stored as 0x400.
    let mut first = Spec::new(0x420, faces(0x120, 0x420), false);
    first.stored_face_count = Some(0x100);
    let mut second = Spec::new(3, faces(1, 3), true);
    second.stored_vertex_offset = Some(0x400);
    let col = Collision::parse(&build(&[first, second], None)).unwrap();
    assert_eq!(col.objects[0].faces.len(), 0x120);
    assert_eq!(col.objects[1].vertices[2], [2.0, 1.0, 2.0]);
    assert_eq!(col.repaired_fields, 1);
}

#[test]
fn keeps_genuinely_empty_objects() {
    let data = build(
        &[Spec::new(0, vec![], true), Spec::new(3, faces(1, 3), true)],
        None,
    );
    let col = Collision::parse(&data).unwrap();
    assert!(col.objects[0].vertices.is_empty());
    assert_eq!(col.objects[1].vertices.len(), 3);
}

#[test]
fn skips_placeholder_faces() {
    let mut spec = Spec::new(300, faces(2, 300), false);
    spec.faces.push((0, 0, [0xFFFF, 0xFFFF, 0xFFFF]));
    let col = Collision::parse(&build(&[spec], None)).unwrap();
    assert_eq!(col.objects[0].faces.len(), 2);
    assert_eq!(col.objects[0].skipped_faces, 1);
}

#[test]
fn rejects_bad_files() {
    let mut data = build(&[Spec::new(4, faces(2, 4), true)], None);
    data[3] = 9;
    assert!(matches!(
        Collision::parse(&data),
        Err(Error::UnsupportedVersion(9))
    ));

    let data = build(&[Spec::new(4, faces(2, 4), true)], None);
    assert!(matches!(
        Collision::parse(&data[..40]),
        Err(Error::Truncated(_))
    ));

    // A vertex count that no reading can reconcile with the totals.
    let mut spec = Spec::new(4, faces(2, 4), true);
    spec.stored_vertex_count = Some(7);
    assert!(matches!(
        Collision::parse(&build(&[spec], None)),
        Err(Error::Inconsistent(_))
    ));
}

/// Parses every collision file from a real disc. Run with
/// `cargo test -p ngc_collision -- --ignored` after unpacking into `extracted/unpacked`.
#[test]
#[ignore = "needs the unpacked game files"]
fn real_collision_files() {
    let root = std::env::var("DESA_UNPACKED_DIR").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted/unpacked").into()
    });
    let mut files = Vec::new();
    collect(std::path::Path::new(&root), &mut files);
    assert!(!files.is_empty(), "no .col.ngc files under {root}");
    let (mut repaired, mut skipped, mut faces) = (0, 0, 0);
    for path in &files {
        let data = std::fs::read(path).unwrap();
        let col = Collision::parse(&data).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        repaired += col.repaired_fields;
        skipped += col.objects.iter().map(|o| o.skipped_faces).sum::<usize>();
        faces += col.face_count();
    }
    println!(
        "parsed {} files: {faces} faces, {repaired} repaired counts, {skipped} placeholder faces",
        files.len()
    );
}

fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, out);
        } else if path
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".col.ngc")
        {
            out.push(path);
        }
    }
}
