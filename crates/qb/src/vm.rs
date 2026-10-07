//! Running scripts.
//!
//! A [`Program`] holds every script and global value from some `.qb`
//! files. A [`Thread`] runs one script (and the scripts it calls) for one
//! object, a few statements at a time: it stops when the script waits, so a
//! game can run many objects' scripts side by side each frame.
//!
//! Statements are lines of tokens:
//!
//! - `name args...` calls a script, or else asks the [`Host`] to run a
//!   command (`Obj_PlayAnim Anim = Ped_M_Idle1 Cycle`). `object:name args`
//!   runs it on another object.
//! - `name = value` sets one of the running script's parameters.
//! - `if [NOT] call ... [else ...] endif`, `begin ... repeat [n]`, `break`,
//!   `return`, and `wait n [seconds | frames]` (60 frames a second).
//!
//! Arguments are `key = value` items, bare values and flags; `<name>` reads
//! one of the script's parameters and `<...>` passes them all on. Only
//! simple arithmetic is evaluated in parentheses. Commands the host doesn't
//! know do nothing and count as false in an `if`.

use std::collections::HashMap;

use crate::token::Token;
use crate::value::{Definition, Value, parse_definitions, parse_value};
use crate::{Result, checksum, tokenize};

pub type Params = Vec<(Option<u32>, Value)>;

/// Every script and global value loaded so far.
#[derive(Default)]
pub struct Program {
    scripts: HashMap<u32, Vec<Token>>,
    values: HashMap<u32, Value>,
}

impl Program {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a `.qb` file's scripts and values (later files win).
    pub fn add(&mut self, data: &[u8]) -> Result<()> {
        let tokens = tokenize(data)?;
        for def in parse_definitions(&tokens)? {
            match def {
                Definition::Script {
                    name,
                    tokens: range,
                } => {
                    // Skip `script name` and the closing `endscript`.
                    let body = tokens[range.start + 2..range.end - 1]
                        .iter()
                        .map(|(_, t)| t.clone())
                        .collect();
                    self.scripts.insert(name, body);
                }
                Definition::Value { name, value } => {
                    self.values.insert(name, value);
                }
                Definition::Other { .. } => {}
            }
        }
        Ok(())
    }

    /// Adds one script from its body's tokens (for tests and tools).
    pub fn add_script(&mut self, name: u32, body: Vec<Token>) {
        self.scripts.insert(name, body);
    }

    /// A script's body, as tokens.
    pub fn script(&self, name: u32) -> Option<&[Token]> {
        self.scripts.get(&name).map(Vec::as_slice)
    }

    pub fn has_script(&self, name: u32) -> bool {
        self.scripts.contains_key(&name)
    }

    pub fn value(&self, name: u32) -> Option<&Value> {
        self.values.get(&name)
    }

    /// Adds or replaces a global value.
    pub fn add_value(&mut self, name: u32, value: Value) {
        self.values.insert(name, value);
    }

    /// Every global value, in no particular order.
    pub fn values(&self) -> impl Iterator<Item = (&u32, &Value)> {
        self.values.iter()
    }
}

/// What a host command did.
pub enum Outcome {
    /// Finished, with a result for `if`.
    Done(bool),
    /// The thread should pause this long before going on.
    Wait(f32),
}

/// Runs commands that aren't scripts: the game (or a viewer) implements
/// the ones it understands.
pub trait Host {
    /// `target` is the object named before a `:`, if any; `args` is a
    /// [`Value::Struct`].
    fn command(&mut self, target: Option<u32>, name: u32, args: &Value) -> Outcome;
}

/// Statements a thread runs at most per [`Thread::run`], so a loop that
/// never waits can't freeze the game.
const BUDGET: usize = 2000;
/// Calls can nest this deep.
const MAX_DEPTH: usize = 64;

struct Frame {
    script: u32,
    pc: usize,
    params: Params,
    /// Open `begin` loops: where the body starts, and how many more times
    /// it runs (`None` until the `repeat` is first reached).
    loops: Vec<(usize, Option<i64>)>,
}

pub struct Thread {
    frames: Vec<Frame>,
    wait: f32,
}

impl Thread {
    pub fn new(script: u32, params: Params) -> Self {
        Thread {
            frames: vec![Frame {
                script,
                pc: 0,
                params,
                loops: Vec::new(),
            }],
            wait: 0.0,
        }
    }

    pub fn is_finished(&self) -> bool {
        self.frames.is_empty()
    }

    /// Runs `dt` seconds' worth: counts down a wait, then runs statements
    /// until the script waits again, ends, or uses up its budget.
    pub fn run(&mut self, program: &Program, host: &mut dyn Host, dt: f32) {
        self.wait -= dt;
        if self.wait > 0.0 {
            return;
        }
        self.wait = 0.0;
        for _ in 0..BUDGET {
            if self.frames.is_empty() || self.wait > 0.0 {
                return;
            }
            self.step(program, host);
        }
    }

    fn step(&mut self, program: &Program, host: &mut dyn Host) {
        let frame = self.frames.last_mut().unwrap();
        let Some(body) = program.scripts.get(&frame.script) else {
            self.frames.pop();
            return;
        };
        // Skip line breaks.
        while matches!(
            body.get(frame.pc),
            Some(Token::EndOfLine | Token::LineNumber(_))
        ) {
            frame.pc += 1;
        }
        let Some(token) = body.get(frame.pc) else {
            self.frames.pop();
            return;
        };
        let line_end = line_end(body, frame.pc);
        match token {
            Token::EndScript | Token::Return => {
                self.frames.pop();
            }
            Token::If => {
                let mut condition = (frame.pc + 1, line_end);
                // Try the `if`, then each `elseif`, until one holds.
                let pc = loop {
                    let truth =
                        self.condition(program, host, frame_line(body, condition.0, condition.1));
                    if truth {
                        break condition.1;
                    }
                    let next = skip_branch(body, condition.1, true);
                    if body.get(next) == Some(&Token::ElseIf) {
                        condition = (next + 1, self::line_end(body, next));
                    } else {
                        break next;
                    }
                };
                if let Some(frame) = self.frames.last_mut() {
                    frame.pc = pc;
                }
            }
            Token::Else | Token::ElseIf => {
                // Reached from the branch that ran: skip to the endif.
                frame.pc = skip_branch(body, frame.pc + 1, false);
            }
            Token::EndIf => frame.pc = line_end,
            Token::Begin => {
                frame.pc = line_end;
                frame.loops.push((frame.pc, None));
            }
            Token::Repeat => {
                let count = {
                    let args = &body[frame.pc + 1..line_end];
                    let params = &frame.params;
                    first_number(args, params, program).map(|n| n as i64)
                };
                let Some((start, remaining)) = frame.loops.last_mut() else {
                    frame.pc = line_end;
                    return;
                };
                let again = match count {
                    None | Some(0) => true, // repeat forever
                    Some(n) => {
                        let left = remaining.get_or_insert(n);
                        *left -= 1;
                        *left > 0
                    }
                };
                if again {
                    frame.pc = *start;
                } else {
                    frame.loops.pop();
                    frame.pc = line_end;
                }
            }
            Token::Break => {
                frame.pc = after_repeat(body, frame.pc + 1);
                frame.loops.pop();
            }
            Token::Name(name) if body.get(frame.pc + 1) == Some(&Token::Equals) => {
                let name = *name;
                let mut args = frame_line(body, frame.pc + 2, line_end).to_vec();
                args.insert(0, Token::StartStruct);
                args.push(Token::EndStruct);
                let value = match resolve(&args, &frame.params, program) {
                    Value::Struct(mut items) if items.len() == 1 => items.remove(0).1,
                    other => other,
                };
                set_param(&mut frame.params, name, value);
                frame.pc = line_end;
            }
            Token::Name(_) => {
                let line = frame_line(body, frame.pc, line_end).to_vec();
                frame.pc = line_end;
                self.call(program, host, &line);
            }
            _ => frame.pc = line_end, // switch, random choices and the like
        }
    }

    /// Runs a call line (`[target:]name args...`); returns its result.
    fn call(&mut self, program: &Program, host: &mut dyn Host, line: &[Token]) -> bool {
        let (target, name, args) = match line {
            [Token::Name(t), Token::Colon, Token::Name(n), rest @ ..] => (Some(*t), *n, rest),
            [Token::Name(n), rest @ ..] => (None, *n, rest),
            _ => return false,
        };
        let frame = self.frames.last().unwrap();
        let mut tokens = vec![Token::StartStruct];
        tokens.extend_from_slice(args);
        tokens.push(Token::EndStruct);
        let args = resolve(&tokens, &frame.params, program);
        if target.is_none() && program.has_script(name) {
            if self.frames.len() < MAX_DEPTH {
                let Value::Struct(params) = args else {
                    return false;
                };
                self.frames.push(Frame {
                    script: name,
                    pc: 0,
                    params,
                    loops: Vec::new(),
                });
            }
            return true;
        }
        if target.is_none() && name == checksum("wait") {
            self.wait += wait_seconds(&args);
            return true;
        }
        match host.command(target, name, &args) {
            Outcome::Done(result) => result,
            Outcome::Wait(seconds) => {
                self.wait += seconds;
                true
            }
        }
    }

    /// Evaluates an `if` line: `[NOT] call` or `(expression)`.
    fn condition(&mut self, program: &Program, host: &mut dyn Host, line: &[Token]) -> bool {
        let (negate, rest) = match line {
            [Token::Not, rest @ ..] => (true, rest),
            _ => (false, line),
        };
        let value = match rest.first() {
            Some(Token::OpenParen) => {
                let params = &self.frames.last().unwrap().params;
                first_number(rest, params, program).is_some_and(|v| v != 0.0)
            }
            // A script used as a condition runs to its end (or first wait)
            // here and counts as true; scripts rarely return results.
            _ => {
                let depth = self.frames.len();
                let result = self.call(program, host, rest);
                if self.frames.len() > depth {
                    while self.frames.len() > depth && self.wait <= 0.0 {
                        self.step(program, host);
                    }
                }
                result
            }
        };
        value != negate
    }
}

/// The tokens of one statement, without line breaks.
fn frame_line(body: &[Token], start: usize, end: usize) -> &[Token] {
    &body[start.min(end)..end]
}

/// Where the line starting at `at` ends (the first line break after it).
fn line_end(body: &[Token], at: usize) -> usize {
    let mut depth = 0i32;
    let mut i = at;
    while let Some(t) = body.get(i) {
        match t {
            Token::StartStruct | Token::StartArray => depth += 1,
            Token::EndStruct | Token::EndArray => depth -= 1,
            Token::EndOfLine | Token::LineNumber(_) if depth <= 0 => return i,
            _ => {}
        }
        i += 1;
    }
    body.len()
}

/// From inside an `if` branch at `at`, finds where execution continues:
/// after the matching `else` (if `stop_at_else`) or after the `endif`.
fn skip_branch(body: &[Token], mut at: usize, stop_at_else: bool) -> usize {
    let mut depth = 0;
    while let Some(t) = body.get(at) {
        match t {
            Token::If => depth += 1,
            Token::EndIf if depth == 0 => return at + 1,
            Token::EndIf => depth -= 1,
            Token::Else if depth == 0 && stop_at_else => return at + 1,
            // `elseif` is followed by a condition: treat it as `else if`
            // without a separate endif by checking it like an `if`.
            Token::ElseIf if depth == 0 && stop_at_else => return at,
            _ => {}
        }
        at += 1;
    }
    body.len()
}

/// After the `repeat` that closes the loop containing `at`.
fn after_repeat(body: &[Token], mut at: usize) -> usize {
    let mut depth = 0;
    while let Some(t) = body.get(at) {
        match t {
            Token::Begin => depth += 1,
            Token::Repeat if depth == 0 => return line_end(body, at),
            Token::Repeat => depth -= 1,
            _ => {}
        }
        at += 1;
    }
    body.len()
}

fn set_param(params: &mut Params, name: u32, value: Value) {
    match params.iter_mut().find(|(k, _)| *k == Some(name)) {
        Some(item) => item.1 = value,
        None => params.push((Some(name), value)),
    }
}

/// Parses `tokens` (a struct) with `<name>` and `<...>` filled in from
/// `params`, global values for bare names left as names.
fn resolve(tokens: &[Token], params: &Params, program: &Program) -> Value {
    // Evaluate parenthesized arithmetic first, then parse.
    let mut flat: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Token::OpenParen => {
                let close = matching_paren(tokens, i);
                let value = evaluate(&tokens[i + 1..close], params, program);
                flat.push(match value {
                    Some(v) if v.fract() == 0.0 && v.abs() < 1e9 => Token::Integer(v as i32),
                    Some(v) => Token::Float(v as f32),
                    None => Token::Integer(0),
                });
                i = close + 1;
            }
            Token::AllArgs => {
                for (k, v) in params {
                    if let Some(k) = k {
                        flat.push(Token::Name(*k));
                        flat.push(Token::Equals);
                    }
                    flat.extend(value_tokens(v));
                }
                i += 1;
            }
            Token::Arg => {
                if let Some(Token::Name(n)) = tokens.get(i + 1) {
                    let value = params
                        .iter()
                        .find(|(k, _)| *k == Some(*n))
                        .map(|(_, v)| v.clone())
                        .unwrap_or(Value::Name(0));
                    flat.extend(value_tokens(&value));
                    i += 2;
                } else {
                    i += 1;
                }
            }
            Token::EndOfLine | Token::LineNumber(_) => i += 1,
            t => {
                flat.push(t.clone());
                i += 1;
            }
        }
    }
    let with_offsets: Vec<(usize, Token)> = flat.into_iter().map(|t| (0, t)).collect();
    let mut at = 0;
    parse_value(&with_offsets, &mut at).unwrap_or(Value::Struct(Vec::new()))
}

fn matching_paren(tokens: &[Token], open: usize) -> usize {
    let mut depth = 0;
    for (i, t) in tokens.iter().enumerate().skip(open) {
        match t {
            Token::OpenParen => depth += 1,
            Token::CloseParen => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    tokens.len().saturating_sub(1)
}

/// Simple arithmetic and comparisons over numbers and `<params>`.
fn evaluate(tokens: &[Token], params: &Params, program: &Program) -> Option<f64> {
    let mut values: Vec<f64> = Vec::new();
    let mut ops: Vec<&Token> = Vec::new();
    let mut i = 0;
    let mut negative = false;
    while i < tokens.len() {
        let v = match &tokens[i] {
            Token::Integer(v) => Some(f64::from(*v)),
            Token::HexInteger(v) => Some(f64::from(*v)),
            Token::Float(v) => Some(f64::from(*v)),
            Token::Arg => {
                i += 1;
                let Some(Token::Name(n)) = tokens.get(i) else {
                    return None;
                };
                params
                    .iter()
                    .find(|(k, _)| *k == Some(*n))
                    .and_then(|(_, v)| v.as_f32())
                    .map(f64::from)
            }
            Token::Name(n) => program.value(*n).and_then(Value::as_f32).map(f64::from),
            Token::OpenParen => {
                let close = matching_paren(tokens, i);
                let v = evaluate(&tokens[i + 1..close], params, program);
                i = close;
                v
            }
            Token::Minus if values.len() == ops.len() => {
                negative = !negative;
                i += 1;
                continue;
            }
            op @ (Token::Add
            | Token::Minus
            | Token::Multiply
            | Token::Divide
            | Token::Equals
            | Token::LessThan
            | Token::LessThanEqual
            | Token::GreaterThan
            | Token::GreaterThanEqual) => {
                ops.push(op);
                i += 1;
                continue;
            }
            _ => return None,
        };
        let v = v?;
        values.push(if negative { -v } else { v });
        negative = false;
        i += 1;
    }
    // Left to right, which is all these scripts need.
    let mut acc = *values.first()?;
    for (op, v) in ops.iter().zip(values.iter().skip(1)) {
        acc = match op {
            Token::Add => acc + v,
            Token::Minus => acc - v,
            Token::Multiply => acc * v,
            Token::Divide if *v != 0.0 => acc / v,
            Token::Equals => f64::from(u8::from(acc == *v)),
            Token::LessThan => f64::from(u8::from(acc < *v)),
            Token::LessThanEqual => f64::from(u8::from(acc <= *v)),
            Token::GreaterThan => f64::from(u8::from(acc > *v)),
            Token::GreaterThanEqual => f64::from(u8::from(acc >= *v)),
            _ => return None,
        };
    }
    Some(acc)
}

/// The first number on a line, with `<params>` and parentheses evaluated.
fn first_number(tokens: &[Token], params: &Params, program: &Program) -> Option<f64> {
    let mut struct_tokens = vec![Token::StartStruct];
    struct_tokens.extend_from_slice(tokens);
    struct_tokens.push(Token::EndStruct);
    match resolve(&struct_tokens, params, program) {
        Value::Struct(items) => items.iter().find_map(|(_, v)| v.as_f32()).map(f64::from),
        _ => None,
    }
}

/// `wait n`, `wait n seconds`, `wait n frames`.
fn wait_seconds(args: &Value) -> f32 {
    let Value::Struct(items) = args else {
        return 0.0;
    };
    let n = items.iter().find_map(|(_, v)| v.as_f32()).unwrap_or(1.0);
    let in_seconds = [checksum("seconds"), checksum("second")]
        .iter()
        .any(|&s| args.has_flag(s));
    if in_seconds { n } else { n / 60.0 }
}

/// Tokens that parse back into `value`.
fn value_tokens(value: &Value) -> Vec<Token> {
    match value {
        Value::Integer(v) => vec![Token::Integer(*v)],
        Value::Float(v) => vec![Token::Float(*v)],
        Value::String(s) => vec![Token::String(s.clone())],
        Value::LocalString(s) => vec![Token::LocalString(s.clone())],
        Value::Vector(v) => vec![Token::Vector(*v)],
        Value::Pair(v) => vec![Token::Pair(*v)],
        Value::Name(n) | Value::Arg(n) => vec![Token::Name(*n)],
        Value::Struct(items) => {
            let mut out = vec![Token::StartStruct];
            for (k, v) in items {
                if let Some(k) = k {
                    out.push(Token::Name(*k));
                    out.push(Token::Equals);
                }
                out.extend(value_tokens(v));
            }
            out.push(Token::EndStruct);
            out
        }
        Value::Array(items) => {
            let mut out = vec![Token::StartArray];
            for v in items {
                out.extend(value_tokens(v));
            }
            out.push(Token::EndArray);
            out
        }
        Value::Script(_) => vec![Token::Name(0)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(name: &str) -> Token {
        Token::Name(checksum(name))
    }

    /// Records commands, and answers `IsAlive` with `alive`.
    #[derive(Default)]
    struct Log {
        calls: Vec<(Option<u32>, u32, Value)>,
        alive: bool,
    }

    impl Host for Log {
        fn command(&mut self, target: Option<u32>, name: u32, args: &Value) -> Outcome {
            self.calls.push((target, name, args.clone()));
            Outcome::Done(name == checksum("IsAlive") && self.alive)
        }
    }

    fn names(log: &Log) -> Vec<u32> {
        log.calls.iter().map(|c| c.1).collect()
    }

    #[test]
    fn calls_scripts_with_params_and_hosts_commands() {
        let mut program = Program::new();
        // script Airplane: DefaultSpeed = 13 / Obj_SetPathVelocity <DefaultSpeed> mph
        program.add_script(
            checksum("Airplane"),
            vec![
                n("DefaultSpeed"),
                Token::Equals,
                Token::Integer(13),
                Token::EndOfLine,
                n("Obj_SetPathVelocity"),
                Token::Arg,
                n("DefaultSpeed"),
                n("mph"),
                Token::EndOfLine,
            ],
        );
        // script Start: Airplane / door:Obj_Open speed = (2 * 3)
        program.add_script(
            checksum("Start"),
            vec![
                n("Airplane"),
                Token::EndOfLine,
                n("door"),
                Token::Colon,
                n("Obj_Open"),
                n("speed"),
                Token::Equals,
                Token::OpenParen,
                Token::Integer(2),
                Token::Multiply,
                Token::Integer(3),
                Token::CloseParen,
                Token::EndOfLine,
            ],
        );
        let mut log = Log::default();
        let mut thread = Thread::new(checksum("Start"), Vec::new());
        thread.run(&program, &mut log, 0.0);
        assert!(thread.is_finished());
        assert_eq!(log.calls.len(), 2);
        let (_, name, args) = &log.calls[0];
        assert_eq!(*name, checksum("Obj_SetPathVelocity"));
        assert_eq!(
            *args,
            Value::Struct(vec![
                (None, Value::Integer(13)),
                (None, Value::Name(checksum("mph")))
            ])
        );
        let (target, _, args) = &log.calls[1];
        assert_eq!(*target, Some(checksum("door")));
        assert_eq!(args.get(checksum("speed")), Some(&Value::Integer(6)));
    }

    #[test]
    fn waits_loops_and_branches() {
        let mut program = Program::new();
        // begin / if IsAlive / A / else / B / endif / wait 1 seconds / repeat 3 / C
        program.add_script(
            checksum("Loop"),
            vec![
                Token::Begin,
                Token::EndOfLine,
                Token::If,
                n("IsAlive"),
                Token::EndOfLine,
                n("A"),
                Token::EndOfLine,
                Token::Else,
                Token::EndOfLine,
                n("B"),
                Token::EndOfLine,
                Token::EndIf,
                Token::EndOfLine,
                n("wait"),
                Token::Integer(1),
                n("seconds"),
                Token::EndOfLine,
                Token::Repeat,
                Token::Integer(3),
                Token::EndOfLine,
                n("C"),
                Token::EndOfLine,
            ],
        );
        let mut log = Log::default();
        let mut thread = Thread::new(checksum("Loop"), Vec::new());
        thread.run(&program, &mut log, 0.0);
        // One pass, then waiting.
        assert_eq!(names(&log), [checksum("IsAlive"), checksum("B")]);
        thread.run(&program, &mut log, 0.5);
        assert_eq!(log.calls.len(), 2, "still waiting");
        log.alive = true;
        thread.run(&program, &mut log, 0.6); // second pass
        thread.run(&program, &mut log, 1.0); // third pass
        assert!(!thread.is_finished());
        thread.run(&program, &mut log, 1.0); // the loop ends: C
        assert!(thread.is_finished());
        assert_eq!(
            names(&log),
            [
                checksum("IsAlive"),
                checksum("B"),
                checksum("IsAlive"),
                checksum("A"),
                checksum("IsAlive"),
                checksum("A"),
                checksum("C"),
            ]
        );
    }

    #[test]
    fn not_break_and_endless_loops() {
        let mut program = Program::new();
        // if NOT IsAlive / Gone / endif / begin / X / break / repeat / begin / repeat
        program.add_script(
            checksum("S"),
            vec![
                Token::If,
                Token::Not,
                n("IsAlive"),
                Token::EndOfLine,
                n("Gone"),
                Token::EndOfLine,
                Token::EndIf,
                Token::EndOfLine,
                Token::Begin,
                Token::EndOfLine,
                n("X"),
                Token::EndOfLine,
                Token::Break,
                Token::EndOfLine,
                Token::Repeat,
                Token::EndOfLine,
                Token::Begin,
                Token::EndOfLine,
                Token::Repeat,
                Token::EndOfLine,
            ],
        );
        let mut log = Log::default();
        let mut thread = Thread::new(checksum("S"), Vec::new());
        // The last loop never waits: the budget stops it.
        thread.run(&program, &mut log, 0.0);
        assert!(!thread.is_finished());
        assert_eq!(
            names(&log),
            [checksum("IsAlive"), checksum("Gone"), checksum("X")]
        );
    }
}
