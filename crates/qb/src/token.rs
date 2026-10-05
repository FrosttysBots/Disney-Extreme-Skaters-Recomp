//! The QB token stream.
//!
//! Token codes follow the Neversoft script compiler shared across the THPS
//! series. Every code below appears in this game's scripts except where
//! noted; their sizes were checked by tokenizing all 347 scripts on the US
//! disc, each of which ends exactly on its end-of-file token.

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RandomKind {
    /// `Random`
    Plain,
    /// `Random2` (not seen in this game)
    Second,
    /// `RandomNoRepeat`
    NoRepeat,
    /// `RandomPermute` (not seen in this game)
    Permute,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    EndOfFile,
    EndOfLine,
    /// End of a line, carrying the next line's number in the source file.
    LineNumber(u32),
    StartStruct,
    EndStruct,
    StartArray,
    EndArray,
    Equals,
    Dot,
    Comma,
    Minus,
    Add,
    Divide,
    Multiply,
    OpenParen,
    CloseParen,
    SameAs,
    LessThan,
    LessThanEqual,
    GreaterThan,
    GreaterThanEqual,
    Name(u32),
    Integer(i32),
    HexInteger(u32),
    Float(f32),
    String(String),
    /// A localized string, written with single quotes.
    LocalString(String),
    Vector([f32; 3]),
    Pair([f32; 2]),
    Begin,
    Repeat,
    Break,
    Script,
    EndScript,
    If,
    Else,
    ElseIf,
    EndIf,
    Return,
    /// Symbol-table entry: a checksum and the name it came from.
    ChecksumName(u32, String),
    /// `<...>`: pass along all of the script's arguments.
    AllArgs,
    /// `<`: the next name is one of the script's arguments, as in `<speed>`.
    Arg,
    /// Skip forward this many bytes (counted from the end of the operand).
    /// Only used inside `Random` to jump past the other choices.
    Jump(u32),
    /// A random choice. Each offset points at one choice, counted from the
    /// end of that offset's own 4 bytes.
    Random(RandomKind, Vec<u32>),
    RandomRange,
    At,
    Or,
    And,
    Xor,
    ShiftLeft,
    ShiftRight,
    RandomRange2,
    Not,
    Switch,
    EndSwitch,
    Case,
    Default,
    Colon,
    /// A one-byte token this decoder has no name for.
    Other(u8),
}

/// Reads a whole script. Returns each token with its byte offset; the last
/// token is always [`Token::EndOfFile`].
pub fn tokenize(data: &[u8]) -> Result<Vec<(usize, Token)>> {
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let start = at;
        let op = data[at];
        at += 1;
        let mut take = |len: usize| -> Result<&[u8]> {
            let bytes = data.get(at..at + len).ok_or(Error::Truncated(start))?;
            at += len;
            Ok(bytes)
        };
        let u32_le = |b: &[u8]| u32::from_le_bytes(b[..4].try_into().unwrap());
        let f32_le = |b: &[u8], i: usize| f32::from_le_bytes(b[i..i + 4].try_into().unwrap());

        let token = match op {
            0x00 => Token::EndOfFile,
            0x01 => Token::EndOfLine,
            0x02 => Token::LineNumber(u32_le(take(4)?)),
            0x03 => Token::StartStruct,
            0x04 => Token::EndStruct,
            0x05 => Token::StartArray,
            0x06 => Token::EndArray,
            0x07 => Token::Equals,
            0x08 => Token::Dot,
            0x09 => Token::Comma,
            0x0A => Token::Minus,
            0x0B => Token::Add,
            0x0C => Token::Divide,
            0x0D => Token::Multiply,
            0x0E => Token::OpenParen,
            0x0F => Token::CloseParen,
            0x11 => Token::SameAs,
            0x12 => Token::LessThan,
            0x13 => Token::LessThanEqual,
            0x14 => Token::GreaterThan,
            0x15 => Token::GreaterThanEqual,
            0x16 => Token::Name(u32_le(take(4)?)),
            0x17 => Token::Integer(u32_le(take(4)?) as i32),
            0x18 => Token::HexInteger(u32_le(take(4)?)),
            0x1A => Token::Float(f32_le(take(4)?, 0)),
            0x1B | 0x1C => {
                let len = u32_le(take(4)?) as usize;
                let bytes = take(len)?;
                let text: String = bytes
                    .strip_suffix(&[0])
                    .unwrap_or(bytes)
                    .iter()
                    .map(|&b| char::from(b))
                    .collect();
                if op == 0x1B {
                    Token::String(text)
                } else {
                    Token::LocalString(text)
                }
            }
            0x1E => {
                let b = take(12)?;
                Token::Vector([f32_le(b, 0), f32_le(b, 4), f32_le(b, 8)])
            }
            0x1F => {
                let b = take(8)?;
                Token::Pair([f32_le(b, 0), f32_le(b, 4)])
            }
            0x20 => Token::Begin,
            0x21 => Token::Repeat,
            0x22 => Token::Break,
            0x23 => Token::Script,
            0x24 => Token::EndScript,
            0x25 => Token::If,
            0x26 => Token::Else,
            0x27 => Token::ElseIf,
            0x28 => Token::EndIf,
            0x29 => Token::Return,
            0x2B => {
                let checksum = u32_le(take(4)?);
                let rest = &data[at..];
                let len = rest
                    .iter()
                    .position(|&b| b == 0)
                    .ok_or(Error::Truncated(start))?;
                let name = rest[..len].iter().map(|&b| char::from(b)).collect();
                at += len + 1;
                Token::ChecksumName(checksum, name)
            }
            0x2C => Token::AllArgs,
            0x2D => Token::Arg,
            0x2E => Token::Jump(u32_le(take(4)?)),
            0x2F | 0x37 | 0x40 | 0x41 => {
                let count = u32_le(take(4)?) as usize;
                let table = take(count.checked_mul(4).ok_or(Error::Truncated(start))?)?;
                let offsets = table.chunks_exact(4).map(u32_le).collect();
                let kind = match op {
                    0x2F => RandomKind::Plain,
                    0x37 => RandomKind::Second,
                    0x40 => RandomKind::NoRepeat,
                    _ => RandomKind::Permute,
                };
                Token::Random(kind, offsets)
            }
            0x30 => Token::RandomRange,
            0x31 => Token::At,
            0x32 => Token::Or,
            0x33 => Token::And,
            0x34 => Token::Xor,
            0x35 => Token::ShiftLeft,
            0x36 => Token::ShiftRight,
            0x38 => Token::RandomRange2,
            0x39 => Token::Not,
            0x3C => Token::Switch,
            0x3D => Token::EndSwitch,
            0x3E => Token::Case,
            0x3F => Token::Default,
            0x42 => Token::Colon,
            // Single-byte codes from the token list that this game doesn't use.
            0x10 | 0x19 | 0x1D | 0x2A | 0x3A | 0x3B | 0x43..=0x47 => Token::Other(op),
            _ => return Err(Error::UnknownToken { op, offset: start }),
        };
        let done = token == Token::EndOfFile;
        tokens.push((start, token));
        if done {
            return Ok(tokens);
        }
    }
    Err(Error::MissingEnd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_values_little_endian() {
        let mut data = vec![0x16];
        data.extend_from_slice(&0xC472_ECC5u32.to_le_bytes());
        data.push(0x07);
        data.push(0x17);
        data.extend_from_slice(&(-5i32).to_le_bytes());
        data.push(0x1E);
        for v in [1.0f32, 2.0, 3.0] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        data.push(0x1B);
        data.extend_from_slice(&3u32.to_le_bytes());
        data.extend_from_slice(b"hi\0");
        data.push(0x00);
        let tokens: Vec<Token> = tokenize(&data)
            .unwrap()
            .into_iter()
            .map(|(_, t)| t)
            .collect();
        assert_eq!(
            tokens,
            [
                Token::Name(0xC472_ECC5),
                Token::Equals,
                Token::Integer(-5),
                Token::Vector([1.0, 2.0, 3.0]),
                Token::String("hi".into()),
                Token::EndOfFile,
            ]
        );
    }

    #[test]
    fn reads_symbol_entries_and_random_tables() {
        let mut data = vec![0x2F];
        data.extend_from_slice(&2u32.to_le_bytes());
        data.extend_from_slice(&4u32.to_le_bytes());
        data.extend_from_slice(&9u32.to_le_bytes());
        data.push(0x2B);
        data.extend_from_slice(&0x1234u32.to_le_bytes());
        data.extend_from_slice(b"Name\0");
        data.push(0x00);
        let tokens = tokenize(&data).unwrap();
        assert_eq!(tokens[0], (0, Token::Random(RandomKind::Plain, vec![4, 9])));
        assert_eq!(tokens[1], (13, Token::ChecksumName(0x1234, "Name".into())));
    }

    #[test]
    fn rejects_unknown_and_truncated_input() {
        assert!(matches!(
            tokenize(&[0x99]),
            Err(Error::UnknownToken {
                op: 0x99,
                offset: 0
            })
        ));
        assert!(matches!(tokenize(&[0x16, 1, 2]), Err(Error::Truncated(0))));
        assert!(matches!(tokenize(&[0x01, 0x01]), Err(Error::MissingEnd)));
    }
}
