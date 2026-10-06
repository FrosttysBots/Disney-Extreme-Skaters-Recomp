//! Structured values from a script's top-level definitions.
//!
//! A `.qb` file is a list of definitions: data such as
//! `NodeArray = [ { Pos = (1.0, 2.0, 3.0) Class = RailNode } ]`, and
//! scripts. Data definitions become [`Value`] trees; scripts are kept as a
//! token range for [`decompile`](crate::decompile).

use std::ops::Range;

use crate::token::Token;
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Integer(i32),
    Float(f32),
    String(String),
    LocalString(String),
    Vector([f32; 3]),
    Pair([f32; 2]),
    /// A name (checksum), such as a node class or a flag.
    Name(u32),
    /// `<name>`: a reference to a script argument.
    Arg(u32),
    /// Items in order. Named items are `name = value`; unnamed ones are
    /// usually bare names used as flags (`CreatedAtStart`).
    Struct(Vec<(Option<u32>, Value)>),
    Array(Vec<Value>),
    /// A script written inside a struct (`script name ... endscript`),
    /// stored under its name; the range indexes the token list.
    Script(Range<usize>),
}

impl Value {
    /// The value of the named item, for structs.
    pub fn get(&self, key: u32) -> Option<&Value> {
        match self {
            Value::Struct(items) => items.iter().find(|(k, _)| *k == Some(key)).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Whether a struct contains the bare name `flag`.
    pub fn has_flag(&self, flag: u32) -> bool {
        match self {
            Value::Struct(items) => items
                .iter()
                .any(|(k, v)| k.is_none() && *v == Value::Name(flag)),
            _ => false,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_name(&self) -> Option<u32> {
        match self {
            Value::Name(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i32> {
        match self {
            Value::Integer(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Value::Float(v) => Some(*v),
            Value::Integer(v) => Some(*v as f32),
            _ => None,
        }
    }

    pub fn as_vector(&self) -> Option<[f32; 3]> {
        match self {
            Value::Vector(v) => Some(*v),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Definition {
    /// `name = value`
    Value { name: u32, value: Value },
    /// `script name ... endscript`; `tokens` indexes the token list and
    /// spans from `script` to `endscript` inclusive.
    Script { name: u32, tokens: Range<usize> },
    /// `name = ...` whose right side isn't plain data (for example an
    /// expression); `tokens` covers the whole line.
    Other { name: u32, tokens: Range<usize> },
}

impl Definition {
    pub fn name(&self) -> u32 {
        match self {
            Definition::Value { name, .. }
            | Definition::Script { name, .. }
            | Definition::Other { name, .. } => *name,
        }
    }
}

/// Parses one value starting at `*at` (skipping line breaks before it) and
/// moves `*at` past it.
pub(crate) fn parse_value(tokens: &[(usize, Token)], at: &mut usize) -> Result<Value> {
    let mut parser = Parser { tokens, at: *at };
    let value = parser.value();
    *at = parser.at;
    value
}

/// Splits a token list into its top-level definitions.
pub fn parse_definitions(tokens: &[(usize, Token)]) -> Result<Vec<Definition>> {
    let mut defs = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i].1 {
            Token::EndOfFile => break,
            Token::EndOfLine | Token::LineNumber(_) | Token::ChecksumName(..) => i += 1,
            Token::Script => {
                let start = i;
                let name = match tokens.get(i + 1) {
                    Some((_, Token::Name(n))) => *n,
                    _ => return Err(unexpected(tokens, i + 1, "expected a name after `script`")),
                };
                let end = (i..tokens.len())
                    .find(|&j| tokens[j].1 == Token::EndScript)
                    .ok_or_else(|| unexpected(tokens, i, "`script` without `endscript`"))?;
                defs.push(Definition::Script {
                    name,
                    tokens: start..end + 1,
                });
                i = end + 1;
            }
            Token::Name(name) if tokens.get(i + 1).map(|t| &t.1) == Some(&Token::Equals) => {
                let start = i;
                let mut parser = Parser { tokens, at: i + 2 };
                let parsed = parser.value();
                match parsed {
                    Ok(value) if parser.at_line_end() => {
                        defs.push(Definition::Value { name: *name, value });
                        i = parser.at;
                    }
                    _ => {
                        // Not plain data: keep the rest of the line as tokens.
                        let end = (start..tokens.len())
                            .find(|&j| {
                                matches!(
                                    tokens[j].1,
                                    Token::LineNumber(_) | Token::EndOfLine | Token::EndOfFile
                                )
                            })
                            .unwrap_or(tokens.len());
                        defs.push(Definition::Other {
                            name: *name,
                            tokens: start..end,
                        });
                        i = end;
                    }
                }
            }
            _ => return Err(unexpected(tokens, i, "expected a definition")),
        }
    }
    Ok(defs)
}

fn unexpected(tokens: &[(usize, Token)], i: usize, context: &str) -> Error {
    let (offset, found) = tokens
        .get(i)
        .map(|(pos, t)| (*pos, format!("{t:?}")))
        .unwrap_or((0, "end of file".into()));
    Error::Unexpected {
        found,
        offset,
        context: context.into(),
    }
}

struct Parser<'a> {
    tokens: &'a [(usize, Token)],
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at).map(|(_, t)| t)
    }

    fn skip_separators(&mut self) {
        while matches!(
            self.peek(),
            Some(Token::LineNumber(_) | Token::EndOfLine | Token::Comma)
        ) {
            self.at += 1;
        }
    }

    fn at_line_end(&self) -> bool {
        matches!(
            self.peek(),
            None | Some(Token::LineNumber(_) | Token::EndOfLine | Token::EndOfFile)
        )
    }

    fn value(&mut self) -> Result<Value> {
        // A value can start on the next line, as in `NodeArray =` then `[`.
        self.skip_separators();
        let start = self.at;
        let token = self
            .peek()
            .cloned()
            .ok_or_else(|| unexpected(self.tokens, start, "expected a value"))?;
        self.at += 1;
        Ok(match token {
            Token::Integer(v) => Value::Integer(v),
            Token::HexInteger(v) => Value::Integer(v as i32),
            Token::Float(v) => Value::Float(v),
            Token::String(s) => Value::String(s),
            Token::LocalString(s) => Value::LocalString(s),
            Token::Vector(v) => Value::Vector(v),
            Token::Pair(v) => Value::Pair(v),
            Token::Name(n) => Value::Name(n),
            Token::Minus => match self.value()? {
                Value::Integer(v) => Value::Integer(-v),
                Value::Float(v) => Value::Float(-v),
                _ => return Err(unexpected(self.tokens, start, "`-` before a non-number")),
            },
            Token::Arg => match self.peek() {
                Some(Token::Name(n)) => {
                    let n = *n;
                    self.at += 1;
                    Value::Arg(n)
                }
                _ => return Err(unexpected(self.tokens, start, "`<` without a name")),
            },
            Token::StartStruct => {
                let mut items = Vec::new();
                loop {
                    self.skip_separators();
                    match self.peek() {
                        Some(Token::EndStruct) => {
                            self.at += 1;
                            break;
                        }
                        Some(Token::Script) => {
                            let script = self.at;
                            let Some((_, Token::Name(name))) = self.tokens.get(script + 1) else {
                                return Err(unexpected(
                                    self.tokens,
                                    script + 1,
                                    "expected a name after `script`",
                                ));
                            };
                            let end = (script..self.tokens.len())
                                .find(|&j| self.tokens[j].1 == Token::EndScript)
                                .ok_or_else(|| {
                                    unexpected(self.tokens, script, "`script` without `endscript`")
                                })?;
                            items.push((Some(*name), Value::Script(script..end + 1)));
                            self.at = end + 1;
                        }
                        Some(Token::Name(key))
                            if self.tokens.get(self.at + 1).map(|t| &t.1)
                                == Some(&Token::Equals) =>
                        {
                            let key = *key;
                            self.at += 2;
                            items.push((Some(key), self.value()?));
                        }
                        Some(_) => items.push((None, self.value()?)),
                        None => return Err(unexpected(self.tokens, start, "unclosed `{`")),
                    }
                }
                Value::Struct(items)
            }
            Token::StartArray => {
                let mut items = Vec::new();
                loop {
                    self.skip_separators();
                    match self.peek() {
                        Some(Token::EndArray) => {
                            self.at += 1;
                            break;
                        }
                        Some(_) => items.push(self.value()?),
                        None => return Err(unexpected(self.tokens, start, "unclosed `[`")),
                    }
                }
                Value::Array(items)
            }
            _ => return Err(unexpected(self.tokens, start, "expected a value")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checksum;

    fn t(list: Vec<Token>) -> Vec<(usize, Token)> {
        list.into_iter().enumerate().collect()
    }

    #[test]
    fn parses_a_node_array() {
        let n = |s: &str| Token::Name(checksum(s));
        let tokens = t(vec![
            n("NodeArray"),
            Token::Equals,
            Token::StartArray,
            Token::LineNumber(2),
            Token::StartStruct,
            Token::LineNumber(3),
            n("Pos"),
            Token::Equals,
            Token::Vector([1.0, 2.0, 3.0]),
            Token::LineNumber(4),
            n("Class"),
            Token::Equals,
            n("RailNode"),
            Token::LineNumber(5),
            n("CreatedAtStart"),
            Token::LineNumber(6),
            n("Links"),
            Token::Equals,
            Token::StartArray,
            Token::Integer(1),
            Token::Integer(2),
            Token::EndArray,
            Token::LineNumber(7),
            n("Height"),
            Token::Equals,
            Token::Minus,
            Token::Float(1.5),
            Token::LineNumber(8),
            Token::EndStruct,
            Token::LineNumber(9),
            Token::EndArray,
            Token::LineNumber(10),
            Token::Script,
            n("Foo"),
            n("Bar"),
            Token::LineNumber(11),
            Token::EndScript,
            Token::EndOfFile,
        ]);
        let defs = parse_definitions(&tokens).unwrap();
        assert_eq!(defs.len(), 2);
        let Definition::Value { name, value } = &defs[0] else {
            panic!("{:?}", defs[0])
        };
        assert_eq!(*name, checksum("NodeArray"));
        let node = &value.as_array().unwrap()[0];
        assert_eq!(
            node.get(checksum("Pos")).and_then(Value::as_vector),
            Some([1.0, 2.0, 3.0])
        );
        assert_eq!(
            node.get(checksum("Class")).and_then(Value::as_name),
            Some(checksum("RailNode"))
        );
        assert!(node.has_flag(checksum("CreatedAtStart")));
        assert_eq!(
            node.get(checksum("Links")),
            Some(&Value::Array(vec![Value::Integer(1), Value::Integer(2)]))
        );
        assert_eq!(
            node.get(checksum("Height")).and_then(Value::as_f32),
            Some(-1.5)
        );
        assert!(
            matches!(&defs[1], Definition::Script { name, tokens } if *name == checksum("Foo") && *tokens == (32..37))
        );
    }

    #[test]
    fn parses_scripts_inside_structs() {
        let tokens = t(vec![
            Token::Name(1),
            Token::Equals,
            Token::StartStruct,
            Token::Name(2),
            Token::Equals,
            Token::Integer(5),
            Token::LineNumber(2),
            Token::Script,
            Token::Name(3),
            Token::Name(4),
            Token::EndScript,
            Token::EndStruct,
            Token::EndOfFile,
        ]);
        let defs = parse_definitions(&tokens).unwrap();
        let Definition::Value { value, .. } = &defs[0] else {
            panic!("{:?}", defs[0])
        };
        assert_eq!(value.get(2), Some(&Value::Integer(5)));
        assert_eq!(value.get(3), Some(&Value::Script(7..11)));
    }

    #[test]
    fn keeps_expressions_as_tokens() {
        let tokens = t(vec![
            Token::Name(1),
            Token::Equals,
            Token::OpenParen,
            Token::Integer(1),
            Token::Add,
            Token::Integer(2),
            Token::CloseParen,
            Token::LineNumber(2),
            Token::Name(2),
            Token::Equals,
            Token::Integer(3),
            Token::EndOfFile,
        ]);
        let defs = parse_definitions(&tokens).unwrap();
        assert_eq!(
            defs[0],
            Definition::Other {
                name: 1,
                tokens: 0..7
            }
        );
        assert_eq!(
            defs[1],
            Definition::Value {
                name: 2,
                value: Value::Integer(3)
            }
        );
    }
}
