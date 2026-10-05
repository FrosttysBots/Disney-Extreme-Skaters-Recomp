//! Turns a token stream back into readable script source, in the style of
//! Neversoft's `.q` files:
//!
//! ```text
//! script Foo
//!     if GotParam speed
//!         Obj_MoveToNode name = <node> speed = <speed>
//!     endif
//! endscript
//! ```
//!
//! Names come from a [`Symbols`] table; unknown checksums print as
//! `#0x1234abcd`. Line breaks follow the compiled line-number tokens.

use std::collections::{HashMap, HashSet};

use crate::Symbols;
use crate::token::{RandomKind, Token};

/// Kinds of nested block, for indentation.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    Brace,
    Bracket,
    Script,
    If,
    Begin,
    Switch,
    Case,
    Random,
}

/// Extra text attached to a token by the `Random` reconstruction.
#[derive(Default)]
struct Markers {
    /// Printed just before the token: `@` for each choice, `)` at the end.
    before: HashMap<usize, Vec<&'static str>>,
    /// Jump tokens that belong to a `Random` and print nothing.
    hidden: HashSet<usize>,
}

pub fn decompile(tokens: &[(usize, Token)], symbols: &Symbols) -> String {
    let markers = random_markers(tokens);
    let mut out = Writer::default();
    let mut i = 0;
    while i < tokens.len() {
        if let Some(texts) = markers.before.get(&i) {
            for &text in texts {
                match text {
                    ")" => out.close_glued(")", Block::Random),
                    // A multi-line Random closes on its own line.
                    "
)" => {
                        if !out.line.is_empty() {
                            out.newline();
                        }
                        out.close_glued(")", Block::Random);
                    }
                    _ => out.piece(text, false, true),
                }
            }
        }
        if markers.hidden.contains(&i) {
            i += 1;
            continue;
        }
        let token = &tokens[i].1;
        match token {
            Token::EndOfFile => break,
            Token::EndOfLine | Token::LineNumber(_) => out.newline(),
            Token::ChecksumName(..) => {}
            Token::StartStruct => out.open("{", Block::Brace),
            Token::EndStruct => out.close("}", &[Block::Brace]),
            Token::StartArray => out.open("[", Block::Bracket),
            Token::EndArray => out.close("]", &[Block::Bracket]),
            Token::Script => out.open("script", Block::Script),
            Token::EndScript => out.close("endscript", &[Block::Script]),
            Token::If => out.open("if", Block::If),
            Token::Else => {
                out.close("", &[Block::If]);
                out.open("else", Block::If);
            }
            Token::ElseIf => {
                out.close("", &[Block::If]);
                out.open("elseif", Block::If);
            }
            Token::EndIf => out.close("endif", &[Block::If]),
            Token::Begin => out.open("begin", Block::Begin),
            Token::Repeat => out.close("repeat", &[Block::Begin]),
            Token::Switch => out.open("switch", Block::Switch),
            Token::Case | Token::Default => {
                if out.top() == Some(Block::Case) {
                    out.close("", &[Block::Case]);
                }
                out.open(
                    if *token == Token::Case {
                        "case"
                    } else {
                        "default"
                    },
                    Block::Case,
                );
            }
            Token::EndSwitch => {
                if out.top() == Some(Block::Case) {
                    out.close("", &[Block::Case]);
                }
                out.close("endswitch", &[Block::Switch]);
            }
            Token::Arg => {
                // `<` followed by a name reads as one argument reference.
                if let Some((_, Token::Name(checksum))) = tokens.get(i + 1) {
                    out.piece(&format!("<{}>", symbols.name(*checksum)), false, false);
                    i += 1;
                } else {
                    out.piece("<", false, true);
                }
            }
            Token::Random(kind, _) => {
                let name = match kind {
                    RandomKind::Plain => "Random(",
                    RandomKind::Second => "Random2(",
                    RandomKind::NoRepeat => "RandomNoRepeat(",
                    RandomKind::Permute => "RandomPermute(",
                };
                out.piece(name, false, true);
                out.blocks.push(Block::Random);
            }
            other => {
                let (text, glue_left, glue_right) = simple_text(other, symbols);
                out.piece(&text, glue_left, glue_right);
            }
        }
        i += 1;
    }
    out.finish()
}

/// Text for tokens without block structure, and whether it attaches to
/// the previous / next piece without a space.
fn simple_text(token: &Token, symbols: &Symbols) -> (String, bool, bool) {
    let plain = |s: &str| (s.to_string(), false, false);
    match token {
        Token::Equals => plain("="),
        Token::Dot => (".".into(), true, true),
        Token::Comma => (",".into(), true, false),
        Token::Colon => (":".into(), true, true),
        Token::Minus => plain("-"),
        Token::Add => plain("+"),
        Token::Divide => plain("/"),
        Token::Multiply => plain("*"),
        Token::OpenParen => ("(".into(), false, true),
        Token::CloseParen => (")".into(), true, false),
        Token::SameAs => plain("=="),
        Token::LessThan => plain("<"),
        Token::LessThanEqual => plain("<="),
        Token::GreaterThan => plain(">"),
        Token::GreaterThanEqual => plain(">="),
        Token::Name(checksum) => plain(&symbols.name(*checksum)),
        Token::Integer(v) => plain(&v.to_string()),
        Token::HexInteger(v) => plain(&format!("{v:#010x}")),
        Token::Float(v) => plain(&float(*v)),
        Token::String(s) => plain(&format!(
            "\"{}\"",
            s.replace('\\', "\\\\").replace('"', "\\\"")
        )),
        Token::LocalString(s) => plain(&format!(
            "'{}'",
            s.replace('\\', "\\\\").replace('\'', "\\'")
        )),
        Token::Vector([x, y, z]) => {
            plain(&format!("({}, {}, {})", float(*x), float(*y), float(*z)))
        }
        Token::Pair([x, y]) => plain(&format!("({}, {})", float(*x), float(*y))),
        Token::Break => plain("break"),
        Token::Return => plain("return"),
        Token::AllArgs => plain("<...>"),
        Token::At => ("@".into(), false, true),
        Token::RandomRange => plain("RandomRange"),
        Token::RandomRange2 => plain("RandomRange2"),
        Token::Or => plain("OR"),
        Token::And => plain("AND"),
        Token::Xor => plain("XOR"),
        Token::ShiftLeft => plain("<<"),
        Token::ShiftRight => plain(">>"),
        Token::Not => plain("NOT"),
        Token::Jump(n) => plain(&format!("/* jump +{n} */")),
        Token::Other(op) => plain(&format!("/* token {op:#04x} */")),
        // Handled by `decompile`.
        _ => plain(""),
    }
}

/// Shortest text that reads back as the same float, always with a decimal point.
fn float(v: f32) -> String {
    let text = format!("{v}");
    if text.contains(['.', 'e', 'i', 'N']) {
        text
    } else {
        format!("{text}.0")
    }
}

/// Works out where each `Random` choice starts and where the construct
/// ends, from its offset table and the jumps between choices.
fn random_markers(tokens: &[(usize, Token)]) -> Markers {
    let index_of: HashMap<usize, usize> = tokens
        .iter()
        .enumerate()
        .map(|(i, (pos, _))| (*pos, i))
        .collect();
    let mut markers = Markers::default();
    for (i, (pos, token)) in tokens.iter().enumerate() {
        let Token::Random(_, offsets) = token else {
            continue;
        };
        // Offset k lives at pos + 5 + 4k and counts from its own end.
        let starts: Vec<usize> = offsets
            .iter()
            .enumerate()
            .filter_map(|(k, &offset)| {
                index_of
                    .get(&(pos + 5 + 4 * k + 4 + offset as usize))
                    .copied()
            })
            .collect();
        if starts.len() != offsets.len() {
            continue; // Malformed table: print the tokens as they are.
        }
        let mut end = None;
        for &start in &starts[1..] {
            if let Some((jump_pos, Token::Jump(skip))) = tokens.get(start - 1) {
                markers.hidden.insert(start - 1);
                end = index_of
                    .get(&(jump_pos + 5 + *skip as usize))
                    .copied()
                    .or(end);
            }
        }
        // A single choice has no jumps; it runs to the end of its line.
        let end = end.unwrap_or_else(|| {
            (i + 1..tokens.len())
                .find(|&j| {
                    matches!(
                        tokens[j].1,
                        Token::LineNumber(_) | Token::EndOfLine | Token::EndOfFile
                    )
                })
                .unwrap_or(tokens.len() - 1)
        });
        for start in starts {
            markers.before.entry(start).or_default().push("@");
        }
        let multi_line = tokens[i..end]
            .iter()
            .any(|(_, t)| matches!(t, Token::LineNumber(_) | Token::EndOfLine));
        markers.before.entry(end).or_default().insert(
            0,
            if multi_line {
                "
)"
            } else {
                ")"
            },
        );
    }
    markers
}

/// Builds indented lines from pieces of text.
#[derive(Default)]
struct Writer {
    lines: Vec<String>,
    line: String,
    line_indent: usize,
    blocks: Vec<Block>,
    glue_next: bool,
}

impl Writer {
    fn top(&self) -> Option<Block> {
        self.blocks.last().copied()
    }

    fn piece(&mut self, text: &str, glue_left: bool, glue_right: bool) {
        if text.is_empty() {
            return;
        }
        if self.line.is_empty() {
            self.line_indent = self.blocks.len();
        } else if !glue_left && !self.glue_next {
            self.line.push(' ');
        }
        self.line.push_str(text);
        self.glue_next = glue_right;
    }

    fn open(&mut self, text: &str, block: Block) {
        self.piece(text, false, false);
        self.blocks.push(block);
    }

    /// Closes the innermost block if it's one of `kinds`, then prints `text`
    /// at the outer indentation.
    fn close(&mut self, text: &str, kinds: &[Block]) {
        if self.top().is_some_and(|b| kinds.contains(&b)) {
            self.blocks.pop();
        }
        self.piece(text, false, false);
    }

    /// Closes `block` and prints `text` attached to the previous piece.
    fn close_glued(&mut self, text: &str, block: Block) {
        if self.top() == Some(block) {
            self.blocks.pop();
        }
        self.piece(text, true, false);
    }

    fn newline(&mut self) {
        let indent = "    ".repeat(self.line_indent);
        self.lines
            .push(format!("{indent}{}", self.line).trim_end().to_string());
        self.line.clear();
        self.glue_next = false;
    }

    fn finish(mut self) -> String {
        if !self.line.is_empty() {
            self.newline();
        }
        // Collapse runs of blank lines left by empty source lines.
        let mut out = String::new();
        let mut blank = false;
        for line in &self.lines {
            if line.is_empty() {
                if !blank && !out.is_empty() {
                    out.push('\n');
                }
                blank = true;
            } else {
                out.push_str(line);
                out.push('\n');
                blank = false;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checksum;

    fn tokens(list: Vec<Token>) -> Vec<(usize, Token)> {
        // Positions only matter for Random; give each token a distinct one.
        list.into_iter()
            .enumerate()
            .map(|(i, t)| (i * 100, t))
            .collect()
    }

    fn symbols(names: &[&str]) -> Symbols {
        let mut s = Symbols::new();
        names.iter().for_each(|n| s.add_name(n));
        s
    }

    #[test]
    fn indents_scripts_and_conditions() {
        let n = |s: &str| Token::Name(checksum(s));
        let t = tokens(vec![
            Token::Script,
            n("Foo"),
            Token::LineNumber(2),
            Token::If,
            n("GotParam"),
            n("speed"),
            Token::LineNumber(3),
            n("Move"),
            n("speed"),
            Token::Equals,
            Token::Arg,
            n("speed"),
            Token::LineNumber(4),
            Token::Else,
            Token::LineNumber(5),
            n("Stop"),
            Token::LineNumber(6),
            Token::EndIf,
            Token::LineNumber(7),
            Token::EndScript,
            Token::EndOfFile,
        ]);
        let text = decompile(&t, &symbols(&["Foo", "GotParam", "speed", "Move", "Stop"]));
        assert_eq!(
            text,
            "script Foo\n    if GotParam speed\n        Move speed = <speed>\n    else\n        Stop\n    endif\nendscript\n"
        );
    }

    #[test]
    fn prints_values_and_structures() {
        let t = tokens(vec![
            Token::Name(checksum("Node")),
            Token::Equals,
            Token::StartStruct,
            Token::LineNumber(2),
            Token::Name(checksum("Pos")),
            Token::Equals,
            Token::Vector([1.0, -2.5, 3.0]),
            Token::LineNumber(3),
            Token::Name(checksum("Text")),
            Token::Equals,
            Token::String("say \"hi\"".into()),
            Token::LineNumber(4),
            Token::Name(0x1234_5678),
            Token::Equals,
            Token::Float(2.0),
            Token::LineNumber(5),
            Token::EndStruct,
            Token::EndOfFile,
        ]);
        let text = decompile(&t, &symbols(&["Node", "Pos", "Text"]));
        assert_eq!(
            text,
            "Node = {\n    Pos = (1.0, -2.5, 3.0)\n    Text = \"say \\\"hi\\\"\"\n    #0x12345678 = 2.0\n}\n"
        );
    }

    #[test]
    fn rebuilds_random_choices() {
        // Random with choices 1 and 2: offsets count from the end of each
        // offset field; the first choice ends with a jump past the second.
        let r = 0;
        let table_end = r + 1 + 4 + 8;
        let first = table_end; // Integer(1): 5 bytes
        let jump = first + 5; // Jump: 5 bytes
        let second = jump + 5; // Integer(2): 5 bytes
        let end = second + 5;
        let t = vec![
            (
                r,
                Token::Random(
                    RandomKind::Plain,
                    vec![(first - (r + 9)) as u32, (second - (r + 13)) as u32],
                ),
            ),
            (first, Token::Integer(1)),
            (jump, Token::Jump((end - (jump + 5)) as u32)),
            (second, Token::Integer(2)),
            (end, Token::EndOfFile),
        ];
        assert_eq!(decompile(&t, &Symbols::new()), "Random(@1 @2)\n");
    }
}
