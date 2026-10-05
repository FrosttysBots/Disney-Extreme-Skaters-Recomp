//! The file system table (FST).
//!
//! The FST is a flat array of 12-byte entries followed by a string table.
//! Entry 0 is the root directory, and its third word is the total number of
//! entries. A directory's entries are the ones that follow it, up to (but not
//! including) the index stored in its `next` field.

use crate::bytes::{be_u32, cstr};
use crate::error::{Error, Result};

const ENTRY_SIZE: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    File { offset: u32, size: u32 },
    Directory { parent: u32, next: u32 },
}

#[derive(Debug, Clone)]
pub struct Node {
    pub index: usize,
    pub name: String,
    /// Full path using `/` separators and no leading slash, e.g. `levels/park.pre`.
    pub path: String,
    pub kind: NodeKind,
}

impl Node {
    pub fn is_dir(&self) -> bool {
        matches!(self.kind, NodeKind::Directory { .. })
    }

    /// Disc offset and size, for files only.
    pub fn file_range(&self) -> Option<(u64, u64)> {
        match self.kind {
            NodeKind::File { offset, size } => Some((offset.into(), size.into())),
            NodeKind::Directory { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Fst {
    nodes: Vec<Node>,
}

impl Fst {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < ENTRY_SIZE {
            return Err(Error::BadFst("table is smaller than one entry".into()));
        }
        if data[0] == 0 {
            return Err(Error::BadFst("root entry is not a directory".into()));
        }

        let count = be_u32(data, 8) as usize;
        let strings_start = count
            .checked_mul(ENTRY_SIZE)
            .filter(|&end| count > 0 && end <= data.len())
            .ok_or_else(|| Error::BadFst(format!("entry count {count} does not fit the table")))?;
        let strings = &data[strings_start..];

        let mut nodes = Vec::with_capacity(count);
        nodes.push(Node {
            index: 0,
            name: String::new(),
            path: String::new(),
            kind: NodeKind::Directory {
                parent: 0,
                next: count as u32,
            },
        });

        // (index just past the directory's last entry, directory path)
        let mut dirs: Vec<(usize, String)> = vec![(count, String::new())];

        for index in 1..count {
            while dirs.last().is_some_and(|&(end, _)| index >= end) {
                dirs.pop();
            }
            let parent_path = dirs.last().map_or("", |(_, path)| path.as_str());

            let entry = &data[index * ENTRY_SIZE..(index + 1) * ENTRY_SIZE];
            let name_offset = (be_u32(entry, 0) & 0x00FF_FFFF) as usize;
            if name_offset >= strings.len() {
                return Err(Error::BadFst(format!(
                    "entry {index} has name offset {name_offset:#x} outside the string table"
                )));
            }
            let name = cstr(&strings[name_offset..]);
            let path = if parent_path.is_empty() {
                name.clone()
            } else {
                format!("{parent_path}/{name}")
            };

            let a = be_u32(entry, 4);
            let b = be_u32(entry, 8);
            let kind = if entry[0] != 0 {
                let next = b as usize;
                if next <= index || next > count {
                    return Err(Error::BadFst(format!(
                        "directory `{path}` (entry {index}) ends at invalid entry {next}"
                    )));
                }
                dirs.push((next, path.clone()));
                NodeKind::Directory { parent: a, next: b }
            } else {
                NodeKind::File { offset: a, size: b }
            };

            nodes.push(Node {
                index,
                name,
                path,
                kind,
            });
        }

        Ok(Self { nodes })
    }

    /// Every entry, including the root at index 0.
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// Every file and directory, excluding the root.
    pub fn entries(&self) -> impl Iterator<Item = &Node> {
        self.nodes.iter().skip(1)
    }

    pub fn files(&self) -> impl Iterator<Item = &Node> {
        self.entries().filter(|node| !node.is_dir())
    }

    pub fn directories(&self) -> impl Iterator<Item = &Node> {
        self.entries().filter(|node| node.is_dir())
    }

    /// Looks up a path. Matching ignores ASCII case and accepts `/` or `\`
    /// separators with or without a leading slash.
    pub fn find(&self, path: &str) -> Option<&Node> {
        let wanted = path.replace('\\', "/");
        let wanted = wanted.trim_matches('/');
        self.entries()
            .find(|node| node.path.eq_ignore_ascii_case(wanted))
    }

    pub fn total_file_size(&self) -> u64 {
        self.files()
            .filter_map(Node::file_range)
            .map(|(_, size)| size)
            .sum()
    }
}
