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
    let small = Font::parse(&ui.fonts["small"]).unwrap();
    for c in "PLAY GAME Options 0123".chars().filter(|c| *c != ' ') {
        assert!(small.glyph(c).is_some(), "{c}");
    }
}
