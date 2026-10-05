use std::path::{Path, PathBuf};

use ngc_anim::{Animation, CameraPath, KeyTables, Skeleton, is_camera_path};

fn collect(dir: &Path, suffix: &str, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, suffix, out);
        } else if path
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(suffix)
        {
            out.push(path);
        }
    }
}

/// Parses every skeleton, bone animation and camera path from a real disc. Run with
/// `cargo test -p ngc_anim -- --ignored` after unpacking into `extracted/unpacked`.
#[test]
#[ignore = "needs the unpacked game files"]
fn real_skeletons_and_animations() {
    let root = std::env::var("DESA_UNPACKED_DIR").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted/unpacked").into()
    });
    let root = Path::new(&root);
    let tables_dir = root.join("skeletons/anims");
    let tables = KeyTables::parse(
        &std::fs::read(tables_dir.join("standardkeyq.bin")).unwrap(),
        &std::fs::read(tables_dir.join("standardkeyt.bin")).unwrap(),
    )
    .unwrap();

    let mut skeletons = Vec::new();
    collect(root, ".ske", &mut skeletons);
    for path in &skeletons {
        Skeleton::parse(&std::fs::read(path).unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }

    let mut anims = Vec::new();
    collect(root, ".ska.ngc", &mut anims);
    let (mut bone_anims, mut cameras, mut repaired) = (0, 0, 0);
    for path in &anims {
        let data = std::fs::read(path).unwrap();
        if is_camera_path(&data) {
            let camera =
                CameraPath::parse(&data).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            for keys in [
                camera
                    .rotations
                    .iter()
                    .map(|k| i32::from(k.frame))
                    .collect::<Vec<_>>(),
                camera
                    .translations
                    .iter()
                    .map(|k| i32::from(k.frame))
                    .collect(),
            ] {
                assert!(
                    keys.windows(2).all(|w| w[0] < w[1]),
                    "{} frames: {keys:?}",
                    path.display()
                );
            }
            let at = camera.sample(camera.duration / 2.0);
            assert!(at.rotation.is_normalized() && at.position.is_finite());
            repaired += camera.repaired_frames;
            cameras += 1;
            continue;
        }
        match Animation::parse(&data, &tables) {
            Ok(a) => {
                bone_anims += 1;
                let pose = a.sample(a.duration / 2.0);
                assert!(
                    pose.iter().all(|(q, t)| q.is_finite() && t.is_finite()),
                    "{}",
                    path.display()
                );
            }
            Err(e) => panic!("{}: {e}", path.display()),
        }
    }
    println!(
        "{} skeletons, {bone_anims} bone animations, {cameras} camera paths ({repaired} frames repaired)",
        skeletons.len()
    );
}
