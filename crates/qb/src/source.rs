//! Script source, as [`decompile()`](crate::decompile) writes it, back into
//! tokens: for scripts of a tool's own (the viewer's menus) to run beside
//! the game's.
//!
//! It reads what the decompiler writes but `Random` choices: names (and
//! `#"odd name"` or `#0x1234abcd` checksums), `<arg>` and `<...>`, numbers
//! (`-5`, `1.5`, `0x10`), `(x, y)` pairs and `(x, y, z)` vectors, `"text"`
//! and `'local text'`, the keywords (any case), the operators, and `//`
//! comments to the end of the line.

use crate::token::Token;
use crate::{Error, Result, checksum};

/// Tokens from script source, each with its byte offset, the names'
/// symbol table after them and [`Token::EndOfFile`] last (as a `.qb`
/// file's).
pub fn compile(source: &str) -> Result<Vec<(usize, Token)>> {
    let bytes = source.as_bytes();
    let mut out: Vec<(usize, Token)> = Vec::new();
    let mut names: Vec<(u32, String)> = Vec::new();
    let mut i = 0;
    let (mut depth, mut in_script) = (0i32, false);
    let error = |at: usize, context: &str| Error::Unexpected {
        found: source[at..]
            .chars()
            .take(12)
            .collect::<String>()
            .escape_debug()
            .to_string(),
        offset: at,
        context: context.into(),
    };
    while i < bytes.len() {
        let start = i;
        let ch = bytes[i];
        let next = bytes.get(i + 1).copied();
        // (`switch`, `case` and `default` are names too, but starting a
        // line of a script's own, not set: `switch = 16`, a list's `switch`.)
        let line_start = depth == 0
            && in_script
            && matches!(out.last().map(|t| &t.1), None | Some(Token::EndOfLine));
        let token = match ch {
            b' ' | b'\t' | b'\r' => {
                i += 1;
                continue;
            }
            b'\n' => {
                i += 1;
                Token::EndOfLine
            }
            b'/' if next == Some(b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'{' => one(&mut i, Token::StartStruct),
            b'}' => one(&mut i, Token::EndStruct),
            b'[' => one(&mut i, Token::StartArray),
            b']' => one(&mut i, Token::EndArray),
            b'=' if next == Some(b'=') => two(&mut i, Token::SameAs),
            b'=' => one(&mut i, Token::Equals),
            b'.' => one(&mut i, Token::Dot),
            b',' => one(&mut i, Token::Comma),
            b':' => one(&mut i, Token::Colon),
            b'+' => one(&mut i, Token::Add),
            b'*' => one(&mut i, Token::Multiply),
            b'/' => one(&mut i, Token::Divide),
            b'@' => one(&mut i, Token::At),
            b')' => one(&mut i, Token::CloseParen),
            // (The decompiler spaces a minus: `-1` is a number, after a
            // value too.)
            b'-' if next.is_some_and(|c| c.is_ascii_digit()) => {
                number(source, &mut i).ok_or_else(|| error(start, "a number"))?
            }
            b'-' => one(&mut i, Token::Minus),
            b'0'..=b'9' => number(source, &mut i).ok_or_else(|| error(start, "a number"))?,
            b'(' => match coordinates(source, i + 1) {
                Some((token, end)) => {
                    i = end;
                    token
                }
                None => one(&mut i, Token::OpenParen),
            },
            b'<' if source[i..].starts_with("<...>") => {
                i += 5;
                Token::AllArgs
            }
            b'<' if next == Some(b'=') => two(&mut i, Token::LessThanEqual),
            b'<' if next == Some(b'<') => two(&mut i, Token::ShiftLeft),
            b'<' => {
                let end = word_end(bytes, i + 1);
                if end > i + 1 && bytes.get(end) == Some(&b'>') {
                    let name = &source[i + 1..end];
                    out.push((start, Token::Arg));
                    names.push((checksum(name), name.to_string()));
                    i = end + 1;
                    Token::Name(checksum(name))
                } else {
                    one(&mut i, Token::LessThan)
                }
            }
            b'>' if next == Some(b'=') => two(&mut i, Token::GreaterThanEqual),
            b'>' if next == Some(b'>') => two(&mut i, Token::ShiftRight),
            b'>' => one(&mut i, Token::GreaterThan),
            b'"' | b'\'' => {
                let (text, end) =
                    quoted(source, i).ok_or_else(|| error(start, "an unclosed string"))?;
                i = end;
                if ch == b'"' {
                    Token::String(text)
                } else {
                    Token::LocalString(text)
                }
            }
            b'#' if next == Some(b'"') => {
                let (text, end) =
                    quoted(source, i + 1).ok_or_else(|| error(start, "an unclosed name"))?;
                i = end;
                names.push((checksum(&text), text.clone()));
                Token::Name(checksum(&text))
            }
            b'#' => {
                let end = word_end(bytes, i + 1);
                let hex = source[i + 1..end].trim_start_matches("0x");
                let value = u32::from_str_radix(hex, 16).map_err(|_| error(start, "a checksum"))?;
                i = end;
                Token::Name(value)
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let end = word_end(bytes, i);
                let word = &source[i..end];
                i = end;
                let rest = source[end..].trim_start_matches([' ', '\t']);
                let set = rest.starts_with('=') && !rest.starts_with("==");
                let block = matches!(
                    word.to_ascii_lowercase().as_str(),
                    "switch" | "case" | "default"
                );
                match keyword(word).filter(|_| !block || (line_start && !set)) {
                    Some(token) => token,
                    None => {
                        names.push((checksum(word), word.to_string()));
                        Token::Name(checksum(word))
                    }
                }
            }
            _ => return Err(error(start, "something script source doesn't have")),
        };
        match token {
            Token::StartStruct | Token::StartArray => depth += 1,
            Token::EndStruct | Token::EndArray => depth -= 1,
            Token::Script => in_script = true,
            Token::EndScript => in_script = false,
            _ => {}
        }
        out.push((start, token));
    }
    // The symbol table, as the game's files end.
    names.sort();
    names.dedup_by_key(|(c, _)| *c);
    for (c, name) in names {
        out.push((bytes.len(), Token::ChecksumName(c, name)));
    }
    out.push((bytes.len(), Token::EndOfFile));
    Ok(out)
}

fn one(i: &mut usize, token: Token) -> Token {
    *i += 1;
    token
}

fn two(i: &mut usize, token: Token) -> Token {
    *i += 2;
    token
}

fn word_end(bytes: &[u8], from: usize) -> usize {
    let mut end = from;
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        end += 1;
    }
    end
}

fn keyword(word: &str) -> Option<Token> {
    Some(match word.to_ascii_lowercase().as_str() {
        "script" => Token::Script,
        "endscript" => Token::EndScript,
        "if" => Token::If,
        "else" => Token::Else,
        "elseif" => Token::ElseIf,
        "endif" => Token::EndIf,
        "begin" => Token::Begin,
        "repeat" => Token::Repeat,
        "break" => Token::Break,
        "return" => Token::Return,
        "switch" => Token::Switch,
        "endswitch" => Token::EndSwitch,
        "case" => Token::Case,
        "default" => Token::Default,
        "not" => Token::Not,
        "and" => Token::And,
        "or" => Token::Or,
        "xor" => Token::Xor,
        "randomrange" => Token::RandomRange,
        "randomrange2" => Token::RandomRange2,
        _ => return None,
    })
}

/// A number at `i` (signed, decimal or `0x` hex), past it.
fn number(source: &str, i: &mut usize) -> Option<Token> {
    let bytes = source.as_bytes();
    let start = *i;
    if source[start..].starts_with("0x") {
        let end = word_end(bytes, start + 2);
        let value = u32::from_str_radix(&source[start + 2..end], 16).ok()?;
        *i = end;
        return Some(Token::HexInteger(value));
    }
    let mut end = start + 1;
    let mut float = false;
    while end < bytes.len() {
        match bytes[end] {
            b'0'..=b'9' => {}
            b'.' if bytes.get(end + 1).is_some_and(u8::is_ascii_digit) => float = true,
            b'e' | b'E' if float => {
                if matches!(bytes.get(end + 1), Some(b'-' | b'+')) {
                    end += 1;
                }
            }
            _ => break,
        }
        end += 1;
    }
    let text = &source[start..end];
    *i = end;
    Some(if float {
        Token::Float(text.parse().ok()?)
    } else {
        Token::Integer(text.parse().ok()?)
    })
}

/// `(x, y)` or `(x, y, z)` from just inside the paren: the pair or vector
/// and where it ends, if it's one.
fn coordinates(source: &str, from: usize) -> Option<(Token, usize)> {
    let close = from + source[from..].find(')')?;
    let parts: Vec<&str> = source[from..close].split(',').map(str::trim).collect();
    let numbers: Option<Vec<f32>> = parts
        .iter()
        .map(|p| {
            let ok = !p.is_empty()
                && p.trim_start_matches('-')
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b == b'.' || b == b'e' || b == b'-');
            ok.then(|| p.parse().ok()).flatten()
        })
        .collect();
    let token = match numbers?.as_slice() {
        [x, y] => Token::Pair([*x, *y]),
        [x, y, z] => Token::Vector([*x, *y, *z]),
        _ => return None,
    };
    Some((token, close + 1))
}

/// Text in quotes at `i` (`"` or `'`, `\` escaping), and where it ends.
fn quoted(source: &str, i: usize) -> Option<(String, usize)> {
    let quote = source[i..].chars().next()?;
    let mut text = String::new();
    let mut chars = source[i + 1..].char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '\\' => text.push(chars.next()?.1),
            c if c == quote => return Some((text, i + 1 + at + 1)),
            c => text.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decompile;

    fn plain(tokens: &[(usize, Token)]) -> Vec<Token> {
        tokens
            .iter()
            .map(|(_, t)| t.clone())
            .filter(|t| !matches!(t, Token::ChecksumName(..)))
            .collect()
    }

    #[test]
    fn reads_what_the_decompiler_writes() {
        let source = r#"script menu_item { pos = (160.0, -89.5) text = "Done" }
    if NOT GotParam id
        <id> = #"odd name"
    endif
    skater:Obj_MoveToNode Name = TRG_Start_P1 orient
    <pos> = (<pos> + (0.0, 12.0) * -2)
    other <...> num = 0x10 local = 'Hi \'you\''
endscript
"#;
        let tokens = compile(source).unwrap();
        let n = |s: &str| Token::Name(checksum(s));
        let want = vec![
            Token::Script,
            n("menu_item"),
            Token::StartStruct,
            n("pos"),
            Token::Equals,
            Token::Pair([160.0, -89.5]),
            n("text"),
            Token::Equals,
            Token::String("Done".into()),
            Token::EndStruct,
            Token::EndOfLine,
            Token::If,
            Token::Not,
            n("GotParam"),
            n("id"),
            Token::EndOfLine,
            Token::Arg,
            n("id"),
            Token::Equals,
            n("odd name"),
            Token::EndOfLine,
            Token::EndIf,
            Token::EndOfLine,
            n("skater"),
            Token::Colon,
            n("Obj_MoveToNode"),
            n("Name"),
            Token::Equals,
            n("TRG_Start_P1"),
            n("orient"),
            Token::EndOfLine,
            Token::Arg,
            n("pos"),
            Token::Equals,
            Token::OpenParen,
            Token::Arg,
            n("pos"),
            Token::Add,
            Token::Pair([0.0, 12.0]),
            Token::Multiply,
            Token::Integer(-2),
            Token::CloseParen,
            Token::EndOfLine,
            n("other"),
            Token::AllArgs,
            n("num"),
            Token::Equals,
            Token::HexInteger(0x10),
            n("local"),
            Token::Equals,
            Token::LocalString("Hi 'you'".into()),
            Token::EndOfLine,
            Token::EndScript,
            Token::EndOfLine,
            Token::EndOfFile,
        ];
        assert_eq!(plain(&tokens), want);
        // (And the names, for the symbol table.)
        assert!(tokens.contains(&(
            source.len(),
            Token::ChecksumName(checksum("orient"), "orient".into())
        )));
    }

    #[test]
    fn a_spaced_minus_takes_away_and_a_sign_is_a_number() {
        let tokens = plain(&compile("(<a> - 1)").unwrap());
        assert_eq!(tokens[3], Token::Minus);
        assert_eq!(tokens[4], Token::Integer(1));
        let tokens = plain(&compile("(<a> -1)").unwrap());
        assert_eq!(tokens[3], Token::Integer(-1));
    }

    #[test]
    fn switch_and_default_are_names_but_starting_a_line() {
        let n = |s: &str| Token::Name(checksum(s));
        let tokens = plain(
            &compile(
                "script s
switch = 16
switch <a>
default
x default y
endscript
switch
",
            )
            .unwrap(),
        );
        assert_eq!(
            &tokens[3..6],
            &[n("switch"), Token::Equals, Token::Integer(16)]
        );
        assert_eq!(tokens[7], Token::Switch);
        assert_eq!(tokens[11], Token::Default);
        assert_eq!(tokens[14], n("default"));
        assert_eq!(tokens[19], n("switch"));
    }

    #[test]
    fn decompiled_source_reads_back_the_same() {
        let tokens = compile(
            "script a\n    b c = (1.0, 2.0) d = [ 1 2 ]\n    if (<e> = 3)\n        f\n    endif\nendscript\n",
        )
        .unwrap();
        let mut symbols = crate::Symbols::new();
        symbols.add_tokens(&tokens);
        let again = compile(&decompile(&tokens, &symbols)).unwrap();
        assert_eq!(plain(&again), plain(&tokens));
    }
}
