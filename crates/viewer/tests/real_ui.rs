use std::path::Path;

use desa_viewer::font::Font;
use desa_viewer::source::GameData;

/// The screen's sprites and fonts from the panel archives: every font
/// reads, with the letters it should have, and the menus' sprites are
/// there.
/// Run with `cargo test --release -p desa_viewer -- --ignored`; the game
/// data is a disc image or folder in `DESA_GAME_DATA`, by default `extracted`.
#[test]
#[ignore = "needs the game data"]
fn real_ui_files() {
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let ui = data
        .ui_files(&["panelsprites.prg", "hubpanel.prg"])
        .unwrap();
    println!(
        "{} images, fonts {:?}",
        ui.images.len(),
        ui.fonts.keys().collect::<Vec<_>>()
    );
    for name in [
        "paused",
        "pa_continue",
        "dialog_frame",
        "dialog_middle",
        "thps4",
        "mainbar",
        "balancemeter",
    ] {
        assert!(ui.images.contains_key(name), "{name}");
    }
    for (name, bytes) in &ui.fonts {
        let font = Font::parse(bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        println!(
            "{name}: {} glyphs, height {}",
            font.glyphs.len(),
            font.line_height
        );
        for g in &font.glyphs {
            assert!(
                u32::from(g.x) + u32::from(g.width) <= font.atlas_width,
                "{name}"
            );
            assert!(
                u32::from(g.y) + u32::from(g.height) <= font.atlas_height,
                "{name}"
            );
        }
    }
    for name in ["dialog_frame", "dialog_middle", "dialog_frame_b"] {
        let f = ngc_texture::img::ImgFile::parse(&ui.images[name]).unwrap();
        let img = f.decode().unwrap();
        let px = |x: u32, y: u32| &img.rgba[((y * img.width + x) * 4) as usize..][..4];
        println!(
            "{name}: {:?} {}x{} stored {}x{} corner {:?} middle {:?}",
            f.format,
            f.width,
            f.height,
            f.stored_width,
            f.stored_height,
            px(0, 0),
            px(f.width / 2, f.height / 2)
        );
    }
    let small = Font::parse(&ui.fonts["small"]).unwrap();
    {
        let out = format!(
            "{}/../../target/small_atlas.png",
            env!("CARGO_MANIFEST_DIR")
        );
        let file = std::fs::File::create(&out).unwrap();
        let mut enc = png::Encoder::new(
            std::io::BufWriter::new(file),
            small.atlas_width,
            small.atlas_height,
        );
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()
            .unwrap()
            .write_image_data(&small.atlas)
            .unwrap();
    }
    for c in "CONTI".chars() {
        println!("{c}: {:?} advance {}", small.glyph(c), small.advance(c));
    }
    for c in "PLAY GAME Options 0123".chars().filter(|c| *c != ' ') {
        assert!(small.glyph(c).is_some(), "{c}");
    }
}

/// Draws the screen's elements onto a 640x480 picture (the tests' own
/// look at a menu, written next to the build as `target/<name>.png`).
fn render(
    screen: &desa_viewer::screen::Screen,
    images: &std::collections::HashMap<u32, ngc_texture::Image>,
    name: &str,
) -> Vec<u8> {
    use desa_viewer::screen::Draw;
    let (w, h) = (640usize, 480usize);
    // A grey-blue backdrop, as if over a level.
    let mut px: Vec<[f32; 3]> = (0..w * h)
        .map(|i| {
            let y = (i / w) as f32 / h as f32;
            [0.25 + 0.2 * y, 0.3 + 0.2 * y, 0.4]
        })
        .collect();
    let mut blit = |rect: [f32; 4], src: (&[u8], usize, [f32; 4]), rgba: [f32; 4]| {
        let (data, stride, [sx, sy, sw, sh]) = src;
        let (x0, y0) = (rect[0].max(0.0) as usize, rect[1].max(0.0) as usize);
        let (x1, y1) = (
            rect[2].min(w as f32) as usize,
            rect[3].min(h as f32) as usize,
        );
        for y in y0..y1 {
            for x in x0..x1 {
                let u = sx + ((x as f32 + 0.5 - rect[0]) / (rect[2] - rect[0])) * sw;
                let v = sy + ((y as f32 + 0.5 - rect[1]) / (rect[3] - rect[1])) * sh;
                let at = ((v as usize) * stride + u as usize) * 4;
                let Some(t) = data.get(at..at + 4) else {
                    continue;
                };
                let a = (f32::from(t[3]) / 255.0 * rgba[3]).clamp(0.0, 1.0);
                let p = &mut px[y * w + x];
                for c in 0..3 {
                    let s = (f32::from(t[c]) / 255.0 * rgba[c]).min(1.0);
                    p[c] = p[c] * (1.0 - a) + s * a;
                }
            }
        }
    };
    for d in screen.draw() {
        match d {
            Draw::Sprite {
                texture,
                rect,
                rgba,
            } => {
                if let Some(img) = images.get(&texture) {
                    let src = (
                        img.rgba.as_slice(),
                        img.width as usize,
                        [0.0, 0.0, img.width as f32, img.height as f32],
                    );
                    blit(rect, src, rgba);
                }
            }
            Draw::Glyph {
                font,
                source,
                rect,
                rgba,
            } => {
                if let Some(f) = screen.font(font) {
                    let s = source.map(f32::from);
                    blit(rect, (f.atlas.as_slice(), f.atlas_width as usize, s), rgba);
                }
            }
        }
    }
    let out = format!("{}/../../target/{name}.png", env!("CARGO_MANIFEST_DIR"));
    let file = std::fs::File::create(&out).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let bytes: Vec<u8> = px
        .iter()
        .flat_map(|p| p.map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8))
        .collect();
    enc.write_header()
        .unwrap()
        .write_image_data(&bytes)
        .unwrap();
    println!("wrote {out}");
    bytes
}

/// The screen as the game's scripts make it, its sprites and fonts from
/// the disc.
fn screen_from_disc(
    data: &mut GameData,
) -> (
    desa_viewer::screen::Screen,
    qb::vm::Program,
    std::collections::HashMap<u32, ngc_texture::Image>,
) {
    let ui = data
        .ui_files(&["panelsprites.prg", "hubpanel.prg"])
        .unwrap();
    let mut images = std::collections::HashMap::new();
    let mut sizes = std::collections::HashMap::new();
    for (name, bytes) in &ui.images {
        let Ok(file) = ngc_texture::img::ImgFile::parse(bytes) else {
            continue;
        };
        let Ok(image) = file.decode() else { continue };
        let image = image.cropped(file.width, file.height);
        sizes.insert(qb::checksum(name), (file.width, file.height));
        images.insert(qb::checksum(name), image);
    }
    let fonts = ui
        .fonts
        .iter()
        .filter_map(|(n, b)| Some((qb::checksum(n), Font::parse(b).ok()?)))
        .collect();
    let mut program = qb::vm::Program::new();
    for file in data.global_scripts().unwrap() {
        let _ = program.add(&file);
    }
    (
        desa_viewer::screen::Screen::new(fonts, sizes),
        program,
        images,
    )
}

/// The game's pause menu (`create_pause_menu`): its title, a menu of
/// items, the first focused; down moves the focus.
#[test]
#[ignore = "needs the game data"]
fn real_pause_menu() {
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let (mut screen, program, images) = screen_from_disc(&mut data);
    screen.run(qb::checksum("create_pause_menu"), Vec::new());
    for _ in 0..60 {
        screen.update(&program, 1.0 / 60.0);
    }
    let mut symbols = qb::Symbols::new();
    for file in data.global_scripts().unwrap() {
        if let Ok(t) = qb::tokenize(&file) {
            symbols.add_tokens(&t);
        }
    }
    let mut unknown: Vec<_> = screen
        .unknown
        .iter()
        .map(|(n, c)| (symbols.name(*n), *c))
        .collect();
    unknown.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    println!("unknown {unknown:?}");
    assert!(screen.exists("pause_menu"));
    assert!(screen.exists("menu_continue"));
    render(&screen, &images, "pause_menu");
    for d in screen
        .draw()
        .iter()
        .filter(|d| matches!(d, desa_viewer::screen::Draw::Glyph { .. }))
        .take(3)
    {
        println!("{d:?}");
    }
    screen.pad(desa_viewer::screen::Pad::Down);
    for _ in 0..30 {
        screen.update(&program, 1.0 / 60.0);
    }
    render(&screen, &images, "pause_menu_down");
}

/// The main menu as the Skate Shop makes it (`create_main_menu`).
#[test]
#[ignore = "needs the game data"]
fn real_main_menu() {
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let (mut screen, mut program, images) = screen_from_disc(&mut data);
    let shop = data.load_level("SkateShop").unwrap();
    for file in &shop.scripts {
        let _ = program.add(file);
    }
    for key in ["parent", "Dims", "font", "text_pos"] {
        println!(
            "default {key}: {:?}",
            program.default_param(qb::checksum("main_menu_add_item"), qb::checksum(key))
        );
    }
    screen.level = Some(qb::checksum("load_skateshop"));
    screen.run(qb::checksum("create_main_menu"), Vec::new());
    for _ in 0..60 {
        screen.update(&program, 1.0 / 60.0);
    }
    let mut symbols = qb::Symbols::new();
    for file in data.global_scripts().unwrap().iter().chain(&shop.scripts) {
        if let Ok(t) = qb::tokenize(file) {
            symbols.add_tokens(&t);
        }
    }
    let mut unknown: Vec<_> = screen
        .unknown
        .iter()
        .map(|(n, c)| (symbols.name(*n), *c))
        .collect();
    unknown.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    println!("unknown {unknown:?}");
    assert!(screen.exists("main_menu"));
    println!("{}", screen.describe());
    render(&screen, &images, "main_menu");
}
