use std::path::Path;
use std::time::{Duration, Instant};

use desa_viewer::movie::{self, Movie};
use desa_viewer::source::GameData;

/// The disc's movies, and the first one played through ffmpeg (skipped
/// without it): its frames come out the right size, not all black, and
/// sound with them.
/// Run with `cargo test --release -p desa_viewer -- --ignored`; the game
/// data is a disc image or folder in `DESA_GAME_DATA`, by default `extracted`.
#[test]
#[ignore = "needs the game data"]
fn real_movies() {
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let data = GameData::open(Path::new(&path)).unwrap();
    let names = data.movies();
    println!("{names:?}");
    for name in ["ATVI", "DI_Logo", "TFBlogo", "intro"] {
        assert!(names.iter().any(|n| n.eq_ignore_ascii_case(name)), "{name}");
    }
    let source = data.movie("atvi").unwrap();
    let Some(ffmpeg) = movie::ffmpeg() else {
        println!("no ffmpeg: not played");
        return;
    };
    let (mut movie, audio) = Movie::start(&source, &ffmpeg).unwrap();
    println!("{:?}", movie.info);
    assert_eq!((movie.info.width, movie.info.height), (640, 480));
    // A second in: frames there, and some not black.
    let start = Instant::now();
    let mut lit = false;
    while start.elapsed() < Duration::from_secs(10) && !lit {
        if let Some((frame, _)) = movie.frame_at(1.0) {
            assert_eq!(frame.len(), 640 * 480 * 4);
            lit = frame
                .chunks_exact(4)
                .any(|p| p[0] > 40 || p[1] > 40 || p[2] > 40);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(lit, "no picture");
    let samples: usize = audio.unwrap().iter().take(4).map(|s| s.len()).sum();
    assert!(samples > 0, "no sound");
}
