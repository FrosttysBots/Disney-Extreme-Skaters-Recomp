use std::path::{Path, PathBuf};

use qb::{Definition, Symbols, Token, Value, checksum, decompile, parse_definitions, tokenize};

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
    let (mut compiled, mut mismatched) = (0, Vec::new());
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
        // And the text compiles back to the same (but `Random` choices,
        // which the source compiler doesn't read).
        let random = tokens
            .iter()
            .any(|(_, t)| matches!(t, Token::Random(..) | Token::Jump(_)));
        if !random {
            let plain = |tokens: &[(usize, Token)]| -> Vec<Token> {
                tokens
                    .iter()
                    .map(|(_, t)| t.clone())
                    .filter(|t| {
                        !matches!(
                            t,
                            Token::EndOfLine | Token::LineNumber(_) | Token::ChecksumName(..)
                        )
                    })
                    .collect()
            };
            match qb::source::compile(&text) {
                Ok(again) if plain(&again) == plain(tokens) => compiled += 1,
                Ok(again) => {
                    let (a, b) = (plain(tokens), plain(&again));
                    let at = a
                        .iter()
                        .zip(&b)
                        .position(|(x, y)| x != y)
                        .unwrap_or(a.len().min(b.len()));
                    mismatched.push(format!(
                        "{}: {:?} read back as {:?}",
                        path.display(),
                        &a[at..(at + 4).min(a.len())],
                        &b[at..(at + 4).min(b.len())]
                    ));
                }
                Err(e) => mismatched.push(format!("{}: {e}", path.display())),
            }
        }
    }
    assert!(
        mismatched.is_empty(),
        "{} didn't read back:
{}",
        mismatched.len(),
        mismatched.join(
            "
"
        )
    );
    println!(
        "{} files ({compiled} compiled back), {scripts} scripts, {nodes} level nodes, {} names",
        files.len(),
        symbols.len()
    );
}
