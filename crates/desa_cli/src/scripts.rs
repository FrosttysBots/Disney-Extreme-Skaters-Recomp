//! `desa qb`: decompile QB scripts and inspect level node arrays.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use qb::{Definition, Symbols, Token, Value, checksum, decompile, parse_definitions, tokenize};

fn collect_qb(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("could not list {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            collect_qb(&path, out)?;
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("qb"))
        {
            out.push(path);
        }
    }
    Ok(())
}

fn read_tokens(path: &Path) -> Result<Vec<(usize, Token)>> {
    let data = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    tokenize(&data).with_context(|| format!("could not tokenize {}", path.display()))
}

/// Names from the symbol tables of every script under `dir`.
fn load_symbols(dirs: &[&Path]) -> Result<Symbols> {
    let mut symbols = Symbols::new();
    for dir in dirs {
        let mut files = Vec::new();
        collect_qb(dir, &mut files)?;
        for path in files {
            symbols.add_tokens(&read_tokens(&path)?);
        }
    }
    Ok(symbols)
}

/// Decompiles one `.qb`, or every `.qb` under a folder, into `.q` files.
/// Names come from all scripts under `symbols_dir` (default: the input
/// folder, or the single file's own folder).
pub fn decompile_files(input: &Path, out: &Path, symbols_dir: Option<&Path>) -> Result<()> {
    let (root, files) = if input.is_dir() {
        let mut files = Vec::new();
        collect_qb(input, &mut files)?;
        files.sort();
        (input.to_path_buf(), files)
    } else {
        (
            input.parent().unwrap_or(Path::new("")).to_path_buf(),
            vec![input.to_path_buf()],
        )
    };
    if files.is_empty() {
        bail!("no .qb files in {}", input.display());
    }
    let symbol_root = symbols_dir.unwrap_or(&root);
    let mut symbols = load_symbols(&[symbol_root])?;
    for file in &files {
        symbols.add_tokens(&read_tokens(file)?);
    }

    let mut unknown = 0usize;
    for file in &files {
        let tokens = read_tokens(file)?;
        unknown += tokens
            .iter()
            .filter(|(_, t)| matches!(t, Token::Name(c) if symbols.get(*c).is_none()))
            .count();
        let relative = file.strip_prefix(&root).unwrap_or(file);
        let dest = out.join(relative).with_extension("q");
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&dest, decompile(&tokens, &symbols))
            .with_context(|| format!("could not write {}", dest.display()))?;
    }
    println!(
        "Decompiled {} scripts to {} using {} known names ({unknown} name uses without a known name)",
        files.len(),
        out.display(),
        symbols.len()
    );
    Ok(())
}

/// Summarizes a level's `NodeArray`: how many nodes of each class, and how
/// many rail nodes are linked into rails.
pub fn nodes(path: &Path, symbols_dir: Option<&Path>) -> Result<()> {
    let tokens = read_tokens(path)?;
    let mut symbols = match symbols_dir {
        Some(dir) => load_symbols(&[dir])?,
        None => Symbols::new(),
    };
    symbols.add_tokens(&tokens);

    let defs = parse_definitions(&tokens)
        .with_context(|| format!("could not parse {}", path.display()))?;
    let node_array = defs.iter().find_map(|d| match d {
        Definition::Value { name, value } if *name == checksum("NodeArray") => Some(value),
        _ => None,
    });
    let Some(nodes) = node_array.and_then(Value::as_array) else {
        bail!("{} has no NodeArray", path.display());
    };

    let class = checksum("Class");
    let mut classes: BTreeMap<String, usize> = BTreeMap::new();
    for node in nodes {
        let name = node
            .get(class)
            .and_then(Value::as_name)
            .map_or("(no class)".into(), |c| symbols.name(c));
        *classes.entry(name).or_default() += 1;
    }
    let links = checksum("Links");
    let rail_nodes = nodes
        .iter()
        .filter(|n| n.get(class).and_then(Value::as_name) == Some(checksum("RailNode")))
        .count();
    let linked = nodes
        .iter()
        .filter(|n| n.get(class).and_then(Value::as_name) == Some(checksum("RailNode")))
        .filter(|n| {
            n.get(links)
                .and_then(Value::as_array)
                .is_some_and(|l| !l.is_empty())
        })
        .count();

    let scripts = defs
        .iter()
        .filter(|d| matches!(d, Definition::Script { .. }))
        .count();
    println!(
        "{} nodes, {} other definitions, {scripts} scripts",
        nodes.len(),
        defs.len() - 1 - scripts
    );
    println!("\nnodes by class:");
    let mut sorted: Vec<_> = classes.into_iter().collect();
    sorted.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    for (name, count) in sorted {
        println!("  {count:>6}  {name}");
    }
    println!("\n{rail_nodes} rail nodes, {linked} of them linked to a next node");
    Ok(())
}
