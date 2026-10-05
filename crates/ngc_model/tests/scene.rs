use ngc_model::{Error, Scene, pass_flags, sector_flags};

/// Builds big-endian test files.
#[derive(Default)]
struct Builder(Vec<u8>);

impl Builder {
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn f32(&mut self, v: f32) -> &mut Self {
        self.u32(v.to_bits())
    }
    fn f32s(&mut self, vs: &[f32]) -> &mut Self {
        vs.iter().for_each(|&v| {
            self.f32(v);
        });
        self
    }
    fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.0.extend_from_slice(b);
        self
    }

    fn pass(&mut self, texture: u32, flags: u32) -> &mut Self {
        self.u32(texture).u32(flags).u32(0x80).u32(5).u32(0).u32(1);
        self.f32s(&[0.5, 0.25, 1.0]).f32s(&[0.5; 6]);
        if flags & pass_flags::UV_WIBBLE != 0 {
            self.f32s(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
        }
        if flags & pass_flags::VC_WIBBLE != 0 {
            // One sequence: 2 keys, phase -3.
            self.u32(1).u32(2).u32((-3i32) as u32);
            self.u32(0)
                .bytes(&[1, 2, 3, 4])
                .u32(100)
                .bytes(&[5, 6, 7, 8]);
        }
        self.u32(1).u32(4).f32(-8.0).f32(-8.0)
    }
}

const MATERIAL: u32 = 0xAAAA_0001;
const TEXTURE: u32 = 0x1234_5678;

/// One material with two passes, one sector with every optional vertex
/// array, and one mesh with two strips.
fn sample() -> Builder {
    let mut b = Builder::default();
    b.u32(1).u32(1).u32(1);
    b.u32(1); // materials
    b.u32(MATERIAL).u32(2).u32(1).u32(0).f32(1000.0).u32(1);
    b.pass(TEXTURE, pass_flags::TEXTURED | pass_flags::UV_WIBBLE);
    b.pass(0, pass_flags::VC_WIBBLE);

    b.u32(1); // sectors
    let flags = sector_flags::TEXCOORDS
        | sector_flags::COLORS
        | sector_flags::NORMALS
        | sector_flags::VC_WIBBLE_INDICES;
    b.u32(0xBEEF).u32(u32::MAX).u32(flags).u32(1);
    b.f32s(&[0.0; 6]).f32s(&[0.0; 4]);
    b.u32(4).u32(60); // 4 vertices
    for i in 0..4 {
        b.f32s(&[i as f32, 0.0, 0.0]);
    }
    for _ in 0..4 {
        b.u16(0).u16(16384).u16((-16384i16) as u16);
    }
    b.u32(2); // two UV sets, interleaved per vertex
    for i in 0..4 {
        b.f32s(&[i as f32, 0.0, 10.0 + i as f32, 0.0]);
    }
    for i in 0..4u8 {
        b.bytes(&[i, 0x80, 0x40, 0xFF]);
    }
    b.bytes(&[0, 0, 1, 1]); // vc wibble indices, no padding

    b.u32(MATERIAL).u32(0).f32s(&[0.0; 4]);
    // Strips [0,1,2,3] and [3,3,2] (the second is degenerate), then 0.
    b.u32(10)
        .u16(4)
        .u16(0)
        .u16(1)
        .u16(2)
        .u16(3)
        .u16(3)
        .u16(3)
        .u16(3)
        .u16(2)
        .u16(0);
    b
}

fn finish(mut b: Builder) -> Vec<u8> {
    b.u32(0);
    b.0
}

#[test]
fn parses_materials() {
    let scene = Scene::parse(&finish(sample())).unwrap();
    let material = scene.material(MATERIAL).unwrap();
    assert_eq!(material.alpha_cutoff, 1);
    assert_eq!(material.draw_order, 1000.0);
    assert_eq!(material.passes.len(), 2);

    let first = &material.passes[0];
    assert_eq!(first.texture, TEXTURE);
    assert_eq!(
        (first.fixed_alpha, first.blend_mode, first.v_address),
        (0x80, 5, 1)
    );
    assert_eq!(first.color, [0.5, 0.25, 1.0]);
    assert_eq!(first.uv_wibble.unwrap()[7], 8.0);
    assert_eq!(first.mip_bias, [-8.0, -8.0]);

    let second = &material.passes[1];
    assert_eq!(second.vc_wibble.len(), 1);
    assert_eq!(second.vc_wibble[0].phase, -3);
    assert_eq!(
        second.vc_wibble[0].keys,
        [(0, [1, 2, 3, 4]), (100, [5, 6, 7, 8])]
    );
}

#[test]
fn parses_vertex_arrays() {
    let scene = Scene::parse(&finish(sample())).unwrap();
    let sector = &scene.sectors[0];
    assert_eq!(sector.checksum, 0xBEEF);
    assert_eq!(sector.bone, -1);
    assert_eq!(sector.positions[3], [3.0, 0.0, 0.0]);
    assert_eq!(sector.normals[0], [0.0, 1.0, -1.0]);
    assert_eq!(sector.uv_sets, 2);
    assert_eq!(sector.uv(2, 0), Some([2.0, 0.0]));
    assert_eq!(sector.uv(2, 1), Some([12.0, 0.0]));
    assert_eq!(sector.uv(2, 2), None);
    assert_eq!(sector.colors[1], [1, 0x80, 0x40, 0xFF]);
    assert_eq!(sector.vc_wibble_indices, [0, 0, 1, 1]);
}

#[test]
fn strips_become_counter_clockwise_triangles() {
    let scene = Scene::parse(&finish(sample())).unwrap();
    let mesh = &scene.sectors[0].meshes[0];
    assert_eq!(mesh.strips, [vec![0, 1, 2, 3], vec![3, 3, 2]]);
    // Odd triangles in a strip swap their first two vertices; the
    // degenerate strip produces nothing.
    assert_eq!(mesh.triangles().collect::<Vec<_>>(), [[0, 1, 2], [2, 1, 3]]);
}

#[test]
fn rejects_out_of_range_indices() {
    let mut data = finish(sample());
    // The first strip's first index: 0 -> 9 (only 4 vertices).
    let at = data.len() - 4 - 10 * 2 + 2;
    data[at..at + 2].copy_from_slice(&9u16.to_be_bytes());
    assert!(matches!(Scene::parse(&data), Err(Error::Invalid(_))));
}

#[test]
fn rejects_truncation_and_trailing_data() {
    let data = finish(sample());
    // Cut into the trailing zero word.
    assert!(matches!(
        Scene::parse(&data[..data.len() - 2]),
        Err(Error::Truncated(_))
    ));
    // Cut into the index data: the index count no longer fits.
    assert!(matches!(
        Scene::parse(&data[..data.len() - 6]),
        Err(Error::Invalid(_))
    ));

    let mut longer = data.clone();
    longer.push(0);
    assert!(matches!(Scene::parse(&longer), Err(Error::Invalid(_))));
}

#[test]
fn rejects_absurd_counts() {
    let mut b = Builder::default();
    b.u32(1).u32(1).u32(1).u32(0x7FFF_FFFF);
    assert!(matches!(Scene::parse(&b.0), Err(Error::Invalid(_))));
}

/// Parses every model, skinned model and level from a real disc. Run with
/// `cargo test -p ngc_model -- --ignored` after unpacking into `extracted/unpacked`.
#[test]
#[ignore = "needs the unpacked game files"]
fn real_models_and_levels() {
    let root = std::env::var("DESA_UNPACKED_DIR").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted/unpacked").into()
    });
    let mut files = Vec::new();
    collect(std::path::Path::new(&root), &mut files);
    assert!(
        !files.is_empty(),
        "no .mdl.ngc / .scn.ngc files under {root}"
    );
    let (mut extra, mut repaired, mut add_up) = (0, 0, 0);
    for path in &files {
        let data = std::fs::read(path).unwrap();
        let scene = Scene::parse(&data).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        for skin in scene.sectors.iter().filter_map(|s| s.skin.as_ref()) {
            extra += skin.extra.len();
            repaired += skin.repaired_values;
            // With its third bone, a vertex's weights should add up to one.
            add_up += skin
                .influences()
                .iter()
                .filter(|i| i[2].1 > 0.0 && (i.iter().map(|w| w.1).sum::<f32>() - 1.0).abs() < 1e-3)
                .count();
        }
    }
    println!(
        "parsed {} files; {extra} extra influences ({add_up} adding up to one), {repaired} values repaired from their copies",
        files.len()
    );
    assert!(add_up * 100 >= extra * 98);
}

fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_ascii_lowercase();
        if path.is_dir() {
            collect(&path, out);
        } else if name.ends_with(".mdl.ngc")
            || name.ends_with(".scn.ngc")
            || name.ends_with(".skin.ngc")
        {
            out.push(path);
        }
    }
}
