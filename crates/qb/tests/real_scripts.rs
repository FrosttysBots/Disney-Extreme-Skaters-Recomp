use std::path::{Path, PathBuf};

use qb::{Definition, Symbols, Value, checksum, decompile, parse_definitions, tokenize};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, out);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("qb"))
        {
            out.push(path);
        }
    }
}

/// Tokenizes, parses and decompiles every script from a real disc. Run with
/// `cargo test -p qb -- --ignored` after unpacking into `extracted/unpacked`.
#[test]
#[ignore = "needs the unpacked game files"]
fn real_scripts() {
    let root = std::env::var("DESA_UNPACKED_DIR").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../extracted/unpacked").into()
    });
    let mut files = Vec::new();
    collect(Path::new(&root), &mut files);
    assert!(!files.is_empty(), "no .qb files under {root}");

    let all: Vec<_> = files
        .iter()
        .map(|p| {
            (
                p,
                tokenize(&std::fs::read(p).unwrap())
                    .unwrap_or_else(|e| panic!("{}: {e}", p.display())),
            )
        })
        .collect();
    let mut symbols = Symbols::new();
    for (_, tokens) in &all {
        symbols.add_tokens(tokens);
    }

    let (mut scripts, mut nodes) = (0, 0);
    for (path, tokens) in &all {
        let defs = parse_definitions(tokens).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        scripts += defs
            .iter()
            .filter(|d| matches!(d, Definition::Script { .. }))
            .count();
        for def in &defs {
            if let Definition::Value { name, value } = def {
                if *name == checksum("NodeArray") {
                    nodes += value.as_array().map_or(0, <[Value]>::len);
                }
            }
        }
        let text = decompile(tokens, &symbols);
        assert!(
            !text.contains("/* jump"),
            "{}: unmatched jump",
            path.display()
        );
        assert!(
            !text.contains("/* token"),
            "{}: unknown token",
            path.display()
        );
    }
    println!(
        "{} files, {scripts} scripts, {nodes} level nodes, {} names",
        files.len(),
        symbols.len()
    );
}
