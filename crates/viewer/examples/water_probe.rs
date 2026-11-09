//! Lists each level's water candidates: batches that scroll their UVs or are
//! transparent, with how much of them faces up, their area and height.
use std::path::Path;

use desa_viewer::source::GameData;

fn main() {
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    for level in data.levels() {
        let files = data.load_level(&level.id).unwrap();
        let l = desa_viewer::level::Level::from_bytes_filtered(
            &files.scene,
            files.textures.as_deref(),
            |_| true,
        )
        .unwrap();
        println!("== {}", level.id);
        for (bi, b) in l.batches.iter().enumerate().filter(|(_, b)| b.water) {
            let idx = &l.indices[b.first_index as usize..(b.first_index + b.index_count) as usize];
            let (mut lo, mut hi) = (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN));
            for &i in idx {
                let q = glam::Vec3::from(l.vertices[i as usize].position);
                lo = lo.min(q);
                hi = hi.max(q);
            }
            println!("  WATER {bi} {lo:.0} .. {hi:.0}");
        }
        if std::env::var("WATER_ONLY").is_ok() {
            continue;
        }
        for (bi, b) in l.batches.iter().enumerate() {
            let scroll = b
                .passes
                .iter()
                .any(|p| p.uv_wibble.iter().any(|v| *v != 0.0));
            let (mut area, mut up_area) = (0.0f32, 0.0f32);
            let (mut ymin, mut ymax) = (f32::MAX, f32::MIN);
            let idx = &l.indices[b.first_index as usize..(b.first_index + b.index_count) as usize];
            for t in idx.chunks(3) {
                let p = |i: u32| glam::Vec3::from(l.vertices[i as usize].position);
                let (a, bb, c) = (p(t[0]), p(t[1]), p(t[2]));
                let n = (bb - a).cross(c - a);
                let ar = n.length() / 2.0;
                area += ar;
                if ar > 0.0 && (n.y / n.length()).abs() > 0.97 {
                    up_area += ar;
                }
                for q in [a, bb, c] {
                    ymin = ymin.min(q.y);
                    ymax = ymax.max(q.y);
                }
            }
            if !(scroll || b.transparent) || area < 1.0e5 {
                continue;
            }
            let p = &b.passes[0];
            if let Ok(dir) = std::env::var("WATER_PROBE_OUT") {
                for p in &b.passes {
                    let t = &l.textures[p.texture];
                    let name = format!("{dir}/{}_{bi}_{:08x}.png", level.id, t.checksum);
                    let file = std::fs::File::create(&name).unwrap();
                    let mut enc = png::Encoder::new(file, t.width, t.height);
                    enc.set_color(png::ColorType::Rgba);
                    enc.set_depth(png::BitDepth::Eight);
                    enc.write_header()
                        .unwrap()
                        .write_image_data(&t.levels[0])
                        .unwrap();
                }
            }
            println!(
                "  batch {bi:3} tex {:08x} passes {} scroll {scroll} transp {} flat {:.2} area {:.0}m2 y {:.0}..{:.0} blend {:?} wib {:?}",
                l.textures[p.texture].checksum,
                b.passes.len(),
                b.transparent,
                up_area / area.max(1.0),
                area / 1550.0,
                ymin,
                ymax,
                b.passes.iter().map(|p| p.blend_mode).collect::<Vec<_>>(),
                b.passes
                    .iter()
                    .map(|p| [p.uv_wibble[0], p.uv_wibble[1]])
                    .collect::<Vec<_>>(),
            );
        }
    }
}
