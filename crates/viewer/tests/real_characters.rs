use std::path::Path;

use desa_viewer::character::Character;
use desa_viewer::level::Vertex;
use desa_viewer::source::GameData;
use glam::{Mat4, Vec3};

fn longest_edge(indices: &[u32], vertices: &[Vertex]) -> f32 {
    indices
        .chunks_exact(3)
        .flat_map(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])])
        .map(|(a, b)| {
            Vec3::from(vertices[a as usize].position)
                .distance(Vec3::from(vertices[b as usize].position))
        })
        .fold(0.0, f32::max)
}

/// Loads every playable character from a real disc and poses it with every
/// animation, checking the mesh never tears apart. Run with
/// `cargo test --release -p desa_viewer -- --ignored`; the game data is a
/// disc image or folder in `DESA_GAME_DATA`, by default `extracted`.
#[test]
#[ignore = "needs the game data"]
fn real_characters() {
    let path = std::env::var("DESA_GAME_DATA")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted").into());
    let mut data = GameData::open(Path::new(&path)).unwrap();
    let characters = data.characters();
    assert_eq!(characters.len(), 12);

    let mut poses = 0;
    for info in &characters {
        let files = data.load_character(&info.id).unwrap();
        let character =
            Character::from_files(&files).unwrap_or_else(|e| panic!("{}: {e:#}", info.id));
        assert!(files.board.is_some(), "{} has no board", info.id);
        // A few files were made for another character's skeleton (such as
        // Jane's Crouch, with Jessie's 39 bones) and are left out.
        println!(
            "{}: {} of {} animations",
            info.id,
            character.animations.len(),
            files.animations.len()
        );
        assert!(character.animations.len() >= 100);

        let mesh = &character.mesh;
        let rest = longest_edge(&mesh.indices, &mesh.vertices);
        for (i, (name, animation)) in character.animations.iter().enumerate() {
            for t in [0.0, 0.5, 1.0] {
                let posed = character.pose(i, animation.duration * t, Mat4::IDENTITY);
                assert!(posed.iter().all(|v| Vec3::from(v.position).is_finite()));
                // Joints stretch a little; a wrong bone or weight stretches
                // triangles across the whole body.
                let longest = longest_edge(&mesh.indices, &posed);
                assert!(
                    longest < rest * 2.0,
                    "{} {name} at {t}: edge of {longest:.1} (rest {rest:.1})",
                    info.id
                );
                poses += 1;
            }
        }
    }
    println!("{} characters, {poses} poses", characters.len());
}
