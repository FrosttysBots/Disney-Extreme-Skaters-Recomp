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
                ..
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
    // Free Skate: down three, choose.
    if std::env::var("CHOOSE").is_ok() {
        screen.unknown.clear();
        let downs: usize = std::env::var("CHOOSE").unwrap().parse().unwrap_or(3);
        for _ in 0..downs {
            screen.pad(desa_viewer::screen::Pad::Down);
            screen.update(&program, 1.0 / 60.0);
        }
        screen.pad(desa_viewer::screen::Pad::Choose);
        for _ in 0..120 {
            screen.update(&program, 1.0 / 60.0);
        }
        let mut unknown: Vec<_> = screen
            .unknown
            .iter()
            .map(|(n, c)| (symbols.name(*n), *c))
            .collect();
        unknown.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
        println!("after choosing: unknown {unknown:?}");
        println!("{}", screen.describe());
        render(&screen, &images, "main_menu_chosen");
    }
}

/// A goal's panel message in its style (`create_panel_block ... style =
/// panel_message_actiontext`): moved into place by the style's script.
#[test]
#[ignore = "needs the game data"]
fn real_panel_message() {
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let (mut screen, program, _images) = screen_from_disc(&mut data);
    screen.run(
        qb::checksum("create_panel_block"),
        vec![
            (
                Some(qb::checksum("text")),
                qb::Value::String("Strength Grind Complete!".into()),
            ),
            (
                Some(qb::checksum("style")),
                qb::Value::Name(qb::checksum("panel_message_actiontext")),
            ),
        ],
    );
    for _ in 0..60 {
        screen.update(&program, 1.0 / 60.0);
    }
    println!("{}", screen.describe());
}

/// The skater select as the Skate Shop makes it (`launch_select_skater_menu`):
/// what it puts up and what of it isn't understood yet.
#[test]
#[ignore = "needs the game data"]
fn real_select_skater() {
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let (mut screen, mut program, images) = screen_from_disc(&mut data);
    let shop = data.load_level("SkateShop").unwrap();
    for file in &shop.scripts {
        let _ = program.add(file);
    }
    screen.level = Some(qb::checksum("load_skateshop"));
    // Jessie's the one chosen (`disney_skater` is the skater_select_array
    // index; the game sets it before this).
    program.change(qb::checksum("disney_skater"), qb::Value::Integer(7));
    screen.character = Some("jessie".into());
    screen.run(qb::checksum("launch_select_skater_menu"), Vec::new());
    for _ in 0..90 {
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
    println!("{}", screen.describe());
    render(&screen, &images, "select_skater");
    assert!(screen.exists("select_skater_anchor"));
}

/// Choosing a character at the skater select (Woody, a Disney character)
/// goes on to the level select (`chose_skater`'s
/// `launch_level_select_menu`).
#[test]
#[ignore = "needs the game data"]
fn real_select_skater_choose() {
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let (mut screen, mut program, _images) = screen_from_disc(&mut data);
    let shop = data.load_level("SkateShop").unwrap();
    for file in &shop.scripts {
        let _ = program.add(file);
    }
    screen.level = Some(qb::checksum("load_skateshop"));
    let who = std::env::var("SKATER").map_or(7, |s| s.parse().unwrap());
    program.change(qb::checksum("disney_skater"), qb::Value::Integer(who));
    screen.character = Some("jessie".into());
    let menu = std::env::var("FROM_MENU").ok();
    if let Some(downs) = menu {
        screen.run(qb::checksum("launch_main_menu"), Vec::new());
        for _ in 0..90 {
            screen.update(&program, 1.0 / 60.0);
        }
        for _ in 0..downs.parse::<u32>().unwrap() {
            screen.pad(desa_viewer::screen::Pad::Down);
            for _ in 0..10 {
                screen.update(&program, 1.0 / 60.0);
            }
        }
        screen.pad(desa_viewer::screen::Pad::Choose);
        for _ in 0..180 {
            screen.update(&program, 1.0 / 60.0);
        }
        println!(
            "after main menu: select_skater {}",
            screen.exists("select_skater_anchor")
        );
        println!("{}", screen.describe());
    } else {
        screen.run(qb::checksum("launch_select_skater_menu"), Vec::new());
    }
    for _ in 0..90 {
        screen.update(&program, 1.0 / 60.0);
    }
    // (The app gives the menus the pad only when they take it.)
    assert!(screen.takes_pad(), "the skater select doesn't take the pad");
    screen.pad(desa_viewer::screen::Pad::Choose);
    for _ in 0..90 {
        screen.update(&program, 1.0 / 60.0);
    }
    println!("{}", screen.describe());
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
    assert!(
        !screen.exists("select_skater_anchor"),
        "the skater select's gone"
    );
}

/// A cheat code typed as the Cheat Codes keyboard hands it on
/// (`TryCheatString`): the game's hash finds it in `Cheat_Array_Gamecube`
/// and its script runs (the Lion King movie unlocked); a wrong one
/// doesn't.
#[test]
#[ignore = "needs the game data"]
fn real_cheat_codes() {
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let (mut screen, program, _) = screen_from_disc(&mut data);
    let code = |text: &str| {
        qb::Value::Struct(vec![(
            Some(qb::checksum("string")),
            qb::Value::String(text.into()),
        )])
    };
    // (The GameCube's own codes for the rest aren't known; the movies'
    // are the same on every console.)
    let special = desa_viewer::behaviour::flag_number(
        &program,
        &qb::Value::Name(qb::checksum("MOVIE_UNLOCKED_LIONKING0")),
    )
    .unwrap();
    screen.command(qb::checksum("TryCheatString"), &code("notacheat"), &program);
    for _ in 0..10 {
        screen.update(&program, 1.0 / 60.0);
    }
    assert!(!screen.global_flags.contains(&special));
    screen.command(qb::checksum("TryCheatString"), &code("savannah"), &program);
    for _ in 0..10 {
        screen.update(&program, 1.0 / 60.0);
    }
    assert!(screen.global_flags.contains(&special));
    // And the other consoles' codes (the PS2's always special).
    let always = desa_viewer::behaviour::flag_number(
        &program,
        &qb::Value::Name(qb::checksum("CHEAT_ALWAYS_SPECIAL")),
    )
    .unwrap();
    screen.command(
        qb::checksum("TryCheatString"),
        &code("supercharger"),
        &program,
    );
    for _ in 0..10 {
        screen.update(&program, 1.0 / 60.0);
    }
    assert!(screen.global_flags.contains(&always));
}

/// The viewer's Video Options, made by the game's menu scripts: in the
/// shop's Options before Done, and the pause menu's Screen Mode; each
/// setting asks the viewer (`viewer_video`), and Back goes back.
#[test]
#[ignore = "needs the game data"]
fn real_video_options() {
    use desa_viewer::screen::Pad;
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let (mut screen, mut program, _) = screen_from_disc(&mut data);
    // (The shop's too: its Options.)
    for file in &data.load_level("SkateShop").unwrap().scripts {
        let _ = program.add(file);
    }
    desa_viewer::behaviour::add_viewer_menus(&mut program);
    screen.listen(&["viewer_video", "viewer_look"]);
    let frames = |screen: &mut desa_viewer::screen::Screen, n| {
        for _ in 0..n {
            screen.update(&program, 1.0 / 60.0);
        }
    };

    // The pause menu's Options: Screen Mode is the Video Options.
    screen.run(qb::checksum("create_screen_menu"), Vec::new());
    frames(&mut screen, 30);
    for id in [
        "video_display",
        "video_size",
        "video_vsync",
        "video_fps",
        "video_apply",
        "video_done",
    ] {
        assert!(screen.exists(id), "no {id}:\n{}", screen.describe());
    }
    assert!(screen.describe().contains("\"VIDEO\""));
    // Chosen, and turned left: the viewer asked, with which and which way.
    screen.pad(Pad::Choose);
    frames(&mut screen, 2);
    screen.pad(Pad::Left);
    frames(&mut screen, 2);
    let asked: Vec<_> = screen
        .requests
        .iter()
        .filter(|(n, _)| *n == qb::checksum("viewer_video"))
        .map(|(_, args)| {
            (
                args.get(qb::checksum("option"))
                    .and_then(qb::Value::as_name),
                args.get(qb::checksum("dir")).and_then(qb::Value::as_int),
            )
        })
        .collect();
    let display = Some(qb::checksum("video_display"));
    assert_eq!(asked, [(display, Some(1)), (display, Some(-1))]);
    // Apply (seven down, past Advanced): asked for by name.
    screen.requests.clear();
    for _ in 0..7 {
        screen.pad(Pad::Down);
        frames(&mut screen, 2);
    }
    screen.pad(Pad::Choose);
    frames(&mut screen, 2);
    let apply = screen
        .requests
        .iter()
        .find(|(n, _)| *n == qb::checksum("viewer_video"))
        .and_then(|(_, args)| args.get(qb::checksum("option")))
        .and_then(qb::Value::as_name);
    assert_eq!(apply, Some(qb::checksum("video_apply")));
    // Up to Advanced: its menu, Atmosphere and Colour first, the settings
    // to come greyed out.
    screen.pad(Pad::Up);
    frames(&mut screen, 2);
    screen.pad(Pad::Choose);
    frames(&mut screen, 30);
    let text = screen.describe();
    assert!(text.contains("\"ADVANCED\""), "{text}");
    for id in [
        "advanced_atmosphere",
        "advanced_colour",
        "look_reset",
        "advanced_aa",
        "advanced_done",
    ] {
        assert!(
            screen.exists(id),
            "no {id}:
{text}"
        );
    }
    assert!(!screen.exists("video_menu"));
    // Atmosphere: the sky, clouds, time of day, day cycle and fog.
    screen.pad(Pad::Choose);
    frames(&mut screen, 30);
    let text = screen.describe();
    assert!(text.contains("\"ATMOSPHERE\""), "{text}");
    for id in [
        "look_sky",
        "look_clouds",
        "look_hour",
        "look_day",
        "look_fog",
    ] {
        assert!(
            screen.exists(id),
            "no {id}:
{text}"
        );
    }
    // The sky turned right, then left: the viewer asked, which and which
    // way.
    screen.requests.clear();
    screen.pad(Pad::Right);
    frames(&mut screen, 2);
    screen.pad(Pad::Left);
    frames(&mut screen, 2);
    let asked: Vec<_> = screen
        .requests
        .iter()
        .filter(|(n, _)| *n == qb::checksum("viewer_look"))
        .map(|(_, args)| {
            (
                args.get(qb::checksum("option"))
                    .and_then(qb::Value::as_name),
                args.get(qb::checksum("dir")).and_then(qb::Value::as_int),
            )
        })
        .collect();
    let sky = Some(qb::checksum("look_sky"));
    assert_eq!(asked, [(sky, Some(1)), (sky, Some(-1))]);
    // Back: ADVANCED; down to Colour, its grade.
    screen.pad(Pad::Back);
    frames(&mut screen, 30);
    assert!(screen.exists("advanced_menu"), "{}", screen.describe());
    screen.pad(Pad::Down);
    frames(&mut screen, 2);
    screen.pad(Pad::Choose);
    frames(&mut screen, 30);
    let text = screen.describe();
    assert!(text.contains("\"COLOUR\""), "{text}");
    assert!(screen.exists("look_exposure") && screen.exists("look_saturation"));
    // Its Done (up from the first): ADVANCED; up from its first, Done
    // (the placeholders skipped): VIDEO.
    screen.pad(Pad::Up);
    frames(&mut screen, 2);
    screen.pad(Pad::Choose);
    frames(&mut screen, 30);
    assert!(screen.exists("advanced_menu"), "{}", screen.describe());
    screen.pad(Pad::Up);
    frames(&mut screen, 2);
    screen.pad(Pad::Choose);
    frames(&mut screen, 30);
    assert!(screen.exists("video_menu"), "{}", screen.describe());
    // Back: the Options.
    screen.pad(Pad::Back);
    frames(&mut screen, 30);
    assert!(!screen.exists("video_menu"));
    assert!(screen.exists("options_menu"), "{}", screen.describe());

    // The shop's Options: Video Options just before Done.
    screen.clear();
    screen.run(qb::checksum("create_setup_options_menu"), Vec::new());
    frames(&mut screen, 30);
    let text = screen.describe();
    let video = text.find("\"Video Options\"").expect("no Video Options");
    let done = text.rfind("\"Done\"").expect("no Done");
    assert!(video < done, "{text}");
}

/// The main menu's Exit Game (the viewer's, after Options): the game's
/// QUIT? box, whose Yes asks the viewer to close (`viewer_exit_game`).
#[test]
#[ignore = "needs the game data"]
fn real_exit_game() {
    use desa_viewer::screen::Pad;
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let (mut screen, mut program, _) = screen_from_disc(&mut data);
    for file in &data.load_level("SkateShop").unwrap().scripts {
        let _ = program.add(file);
    }
    desa_viewer::behaviour::add_viewer_menus(&mut program);
    screen.level = Some(qb::checksum("load_SkateShop"));
    screen.listen(&["viewer_exit_game"]);
    let frames = |screen: &mut desa_viewer::screen::Screen, n| {
        for _ in 0..n {
            screen.update(&program, 1.0 / 60.0);
        }
    };
    screen.run(qb::checksum("launch_main_menu"), Vec::new());
    frames(&mut screen, 120);
    let text = screen.describe();
    let options = text.find("\"Options\"").expect("no Options");
    let exit = text.find("\"Exit Game\"").expect("no Exit Game");
    assert!(options < exit, "{text}");
    // Up from the first: the last, Exit Game; chosen, the QUIT? box.
    screen.pad(Pad::Up);
    frames(&mut screen, 10);
    screen.pad(Pad::Choose);
    frames(&mut screen, 60);
    assert!(
        screen.describe().contains("\"Exit Game?\""),
        "{}",
        screen.describe()
    );
    assert!(screen.requests.is_empty());
    // (The box starts on No, as the game's do: `no_button`.)
    screen.pad(Pad::Up);
    frames(&mut screen, 10);
    // Yes: the viewer's asked to close.
    screen.pad(Pad::Choose);
    frames(&mut screen, 10);
    assert!(
        screen
            .requests
            .iter()
            .any(|(n, _)| *n == qb::checksum("viewer_exit_game")),
        "{}",
        screen.describe()
    );
}

/// The main menu's 2 Player greyed out for now (`not_focusable`, as the
/// game's demo builds have it): down from Free Skate is Options.
#[test]
#[ignore = "needs the game data"]
fn real_main_menu_2_player_off() {
    use desa_viewer::screen::Pad;
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let (mut screen, mut program, _) = screen_from_disc(&mut data);
    for file in &data.load_level("SkateShop").unwrap().scripts {
        let _ = program.add(file);
    }
    desa_viewer::behaviour::add_viewer_menus(&mut program);
    screen.level = Some(qb::checksum("load_SkateShop"));
    let frames = |screen: &mut desa_viewer::screen::Screen, n| {
        for _ in 0..n {
            screen.update(&program, 1.0 / 60.0);
        }
    };
    screen.run(qb::checksum("launch_main_menu"), Vec::new());
    frames(&mut screen, 120);
    assert!(screen.exists("main_menu_2_player_option"));
    // Play Game, Load, Save, Free Skate, (not 2 Player) Options.
    for _ in 0..4 {
        screen.pad(Pad::Down);
        frames(&mut screen, 10);
    }
    screen.pad(Pad::Choose);
    frames(&mut screen, 60);
    let text = screen.describe();
    assert!(text.contains("\"Video Options\""), "{text}");
    assert!(!screen.exists("main_menu"), "{text}");
}

/// The main menu's demo movie (`attract_mode_timer`): two minutes idle,
/// each key starting the wait again, and none once the menu's left.
#[test]
#[ignore = "needs the game data"]
fn real_main_menu_idle_demo() {
    use desa_viewer::screen::Pad;
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let (mut screen, mut program, _) = screen_from_disc(&mut data);
    for file in &data.load_level("SkateShop").unwrap().scripts {
        let _ = program.add(file);
    }
    desa_viewer::behaviour::add_viewer_menus(&mut program);
    screen.level = Some(qb::checksum("load_SkateShop"));
    let seconds = |screen: &mut desa_viewer::screen::Screen, n: u32| {
        for _ in 0..n * 10 {
            screen.update(&program, 0.1);
        }
    };
    screen.run(qb::checksum("launch_main_menu"), Vec::new());
    seconds(&mut screen, 100);
    assert!(screen.movies.is_empty(), "a demo at 100 seconds");
    // A key: two minutes again from here.
    screen.pad(Pad::Down);
    seconds(&mut screen, 100);
    assert!(screen.movies.is_empty(), "a demo 100 seconds after a key");
    seconds(&mut screen, 25);
    assert_eq!(screen.movies.len(), 1, "no demo two minutes after a key");
    // Back on the menu, then off it (to the Options): no more demos.
    screen.movies.clear();
    screen.clear();
    screen.run(qb::checksum("launch_main_menu"), Vec::new());
    seconds(&mut screen, 2);
    assert!(screen.describe().contains("\"Options\""));
    for _ in 0..2 {
        screen.pad(Pad::Down);
        seconds(&mut screen, 1);
    }
    screen.pad(Pad::Choose);
    seconds(&mut screen, 300);
    assert!(screen.movies.is_empty(), "a demo off the main menu");
}
