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
    /// Each script's default parameters, from its header (`script TransAm
    /// DefaultSpeed = 30`): a call gets them where it doesn't pass its own.
    defaults: HashMap<u32, Params>,
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
                    let mut body: Vec<Token> = tokens[range.start + 2..range.end - 1]
                        .iter()
                        .map(|(_, t)| t.clone())
                        .collect();
                    // The rest of the header: default parameters, to the end
                    // of the line or, written as a struct, to its close.
                    let header = if body.first() == Some(&Token::StartStruct) {
                        let mut depth = 0;
                        body.iter()
                            .position(|t| {
                                match t {
                                    Token::StartStruct => depth += 1,
                                    Token::EndStruct => depth -= 1,
                                    _ => {}
                                }
                                depth == 0
                            })
                            .map_or(body.len(), |end| end + 1)
                    } else {
                        body.iter()
                            .position(|t| matches!(t, Token::EndOfLine | Token::LineNumber(_)))
                            .unwrap_or(body.len())
                    };
                    if header > 0 {
                        let mut items = vec![(0, Token::StartStruct)];
                        items.extend(
                            body[..header]
                                .iter()
                                .filter(|t| !matches!(t, Token::EndOfLine | Token::LineNumber(_)))
                                .cloned()
                                .map(|t| (0, t)),
                        );
                        items.push((0, Token::EndStruct));
                        let mut at = 0;
                        if let Ok(Value::Struct(parsed)) = parse_value(&items, &mut at) {
                            // A header written as a struct gives its fields.
                            let mut defaults = Vec::new();
                            for (k, v) in parsed {
                                match (k, v) {
                                    (None, Value::Struct(fields)) => defaults.extend(fields),
                                    item => defaults.push(item),
                                }
                            }
                            self.defaults.insert(name, defaults);
                        }
                        body.drain(..header);
                    }
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

    /// Every script: its name's checksum and its body.
    pub fn scripts(&self) -> impl Iterator<Item = (u32, &[Token])> {
        self.scripts
            .iter()
            .map(|(name, body)| (*name, body.as_slice()))
    }

    /// A script's body, as tokens.
    pub fn script(&self, name: u32) -> Option<&[Token]> {
        self.scripts.get(&name).map(Vec::as_slice)
    }

    /// A script's default for a parameter, from its header.
    pub fn default_param(&self, script: u32, key: u32) -> Option<&Value> {
        self.defaults
            .get(&script)?
            .iter()
            .find(|(k, _)| *k == Some(key))
            .map(|(_, v)| v)
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
    /// Finished, handing the script these parameters (as
    /// `GoalManager_GetGoalParams` does): true in an `if`.
    Params(Params),
}

/// Runs commands that aren't scripts: the game (or a viewer) implements
/// the ones it understands.
pub trait Host {
    /// `target` is the object named before a `:`, if any; `args` is a
    /// [`Value::Struct`].
    fn command(&mut self, target: Option<u32>, name: u32, args: &Value) -> Outcome;

    /// Whether `target` (before a `:`) is the object this thread runs as:
    /// `self_object:script` is then an ordinary call.
    fn is_self(&self, _target: u32) -> bool {
        false
    }

    /// The tags `SetTags` and `GetTags` work on, if the host keeps them
    /// (a screen element's own); else the thread's.
    fn tags(&mut self) -> Option<&mut Params> {
        None
    }
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
    /// Whether the header's defaults are in yet.
    started: bool,
}

pub struct Thread {
    frames: Vec<Frame>,
    wait: f32,
    /// The object's tags (`SetTags`), read back with `GetTags`.
    tags: Params,
}

impl Thread {
    pub fn new(script: u32, params: Params) -> Self {
        Thread {
            frames: vec![Frame {
                script,
                pc: 0,
                params,
                loops: Vec::new(),
                started: false,
            }],
            wait: 0.0,
            tags: Vec::new(),
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
        // Starting: the header's defaults for what wasn't passed.
        if frame.pc == 0 && !frame.started {
            frame.started = true;
            for (key, value) in program.defaults.get(&frame.script).into_iter().flatten() {
                match key {
                    Some(k) if !frame.params.iter().any(|(p, _)| *p == Some(*k)) => {
                        frame.params.push((Some(*k), value.clone()));
                    }
                    None if !frame.params.iter().any(|(p, v)| p.is_none() && v == value) => {
                        frame.params.push((None, value.clone()));
                    }
                    _ => {}
                }
            }
        }
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
            // `switch <x>` / `case a` / `default` / `endswitch`: on to the
            // case that matches (or the default), and from the end of a
            // case's body out past the endswitch.
            Token::Switch => {
                let value = first_value(
                    frame_line(body, frame.pc + 1, line_end),
                    &frame.params,
                    program,
                );
                let mut at = line_end;
                let mut depth = 0;
                let mut default = None;
                let target = loop {
                    let Some(t) = body.get(at) else {
                        break body.len();
                    };
                    match t {
                        Token::Switch => depth += 1,
                        Token::EndSwitch if depth > 0 => depth -= 1,
                        Token::EndSwitch => break default.unwrap_or(self::line_end(body, at)),
                        Token::Case if depth == 0 => {
                            let end = self::line_end(body, at);
                            let case =
                                first_value(frame_line(body, at + 1, end), &frame.params, program);
                            if case == value {
                                break end;
                            }
                        }
                        Token::Default if depth == 0 => default = Some(self::line_end(body, at)),
                        _ => {}
                    }
                    at += 1;
                };
                frame.pc = target;
            }
            Token::Case | Token::Default => frame.pc = after_switch(body, frame.pc + 1),
            Token::EndSwitch => frame.pc = line_end,
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
            // `<x> = value`: a parameter set, as `x = value` is.
            Token::Arg
                if matches!(body.get(frame.pc + 1), Some(Token::Name(_)))
                    && body.get(frame.pc + 2) == Some(&Token::Equals) =>
            {
                let Some(&Token::Name(name)) = body.get(frame.pc + 1) else {
                    unreachable!()
                };
                let mut args = frame_line(body, frame.pc + 3, line_end).to_vec();
                args.insert(0, Token::StartStruct);
                args.push(Token::EndStruct);
                let value = match resolve(&args, &frame.params, program) {
                    Value::Struct(mut items) if items.len() == 1 => items.remove(0).1,
                    other => other,
                };
                set_param(&mut frame.params, name, value);
                frame.pc = line_end;
            }
            // `<script> args` or `<object>:command args`: the call named by
            // a parameter (`<goal_outro_script> <goal_outro_script_params>`).
            Token::Arg => {
                let line = frame_line(body, frame.pc, line_end).to_vec();
                frame.pc = line_end;
                let named = match line.get(1) {
                    Some(Token::Name(n)) => frame
                        .params
                        .iter()
                        .find(|(k, _)| *k == Some(*n))
                        .and_then(|(_, v)| v.as_name()),
                    _ => None,
                };
                if let Some(named) = named {
                    let mut call = vec![Token::Name(named)];
                    call.extend_from_slice(&line[2..]);
                    self.call(program, host, &call);
                }
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
        let own = target.is_none_or(|t| host.is_self(t));
        if own && program.has_script(name) {
            if self.frames.len() < MAX_DEPTH {
                let Value::Struct(items) = args else {
                    return false;
                };
                // A struct passed whole (`script { a = 1 b = 2 }`) gives
                // its fields as the script's parameters.
                let mut params = Vec::with_capacity(items.len());
                for (k, v) in items {
                    match (k, v) {
                        (None, Value::Struct(fields)) => {
                            for (fk, fv) in fields {
                                match fk {
                                    Some(fk) => set_param(&mut params, fk, fv),
                                    None => params.push((None, fv)),
                                }
                            }
                        }
                        (Some(k), v) => set_param(&mut params, k, v),
                        (None, v) => params.push((None, v)),
                    }
                }
                self.frames.push(Frame {
                    script: name,
                    pc: 0,
                    params,
                    loops: Vec::new(),
                    started: false,
                });
            }
            return true;
        }
        if target.is_none() && name == checksum("wait") {
            self.wait += wait_seconds(&args);
            return true;
        }
        // Built in: whether the script was given a parameter, and the
        // object's tags.
        if target.is_none() && name == checksum("GotParam") {
            let Value::Struct(items) = &args else {
                return false;
            };
            let Some(Value::Name(wanted)) = items.iter().find(|(k, _)| k.is_none()).map(|(_, v)| v)
            else {
                return false;
            };
            let frame = self.frames.last().unwrap();
            return frame
                .params
                .iter()
                .any(|(k, v)| *k == Some(*wanted) || (k.is_none() && *v == Value::Name(*wanted)));
        }
        if target.is_none() && name == checksum("SetTags") {
            // (The host's tags if it keeps them: a screen element's.)
            let tags = match host.tags() {
                Some(tags) => tags,
                None => &mut self.tags,
            };
            if let Value::Struct(items) = args {
                for (k, v) in items {
                    if let Some(k) = k {
                        set_param(tags, k, v);
                    }
                }
            }
            return true;
        }
        if target.is_none() && name == checksum("GetTags") {
            let tags = match host.tags() {
                Some(tags) => tags.clone(),
                None => self.tags.clone(),
            };
            let frame = self.frames.last_mut().unwrap();
            for (k, v) in tags {
                if let Some(k) = k {
                    set_param(&mut frame.params, k, v);
                }
            }
            return true;
        }
        if target.is_none() && (name == checksum("printf") || name == checksum("printstruct")) {
            return true;
        }
        // Built in: text made from a pattern (`FormatText TextName = msg
        // "%i of %n" i = 3 n = 10`), or a name made so (`ChecksumName`).
        if target.is_none() && name == checksum("FormatText") {
            let Value::Struct(items) = &args else {
                return false;
            };
            let pattern = items.iter().find_map(|(k, v)| match (k, v) {
                (None, Value::String(s) | Value::LocalString(s)) => Some(s.clone()),
                _ => None,
            });
            let (Some(pattern), Some((into, as_name))) = (
                pattern,
                args.get(checksum("TextName"))
                    .and_then(Value::as_name)
                    .map(|n| (n, false))
                    .or_else(|| {
                        args.get(checksum("ChecksumName"))
                            .and_then(Value::as_name)
                            .map(|n| (n, true))
                    }),
            ) else {
                return false;
            };
            let mut text = String::new();
            let mut chars = pattern.chars();
            while let Some(ch) = chars.next() {
                if ch != '%' {
                    text.push(ch);
                    continue;
                }
                let Some(key) = chars.next() else { break };
                match args.get(checksum(&key.to_string())) {
                    Some(Value::Integer(i)) => text.push_str(&i.to_string()),
                    Some(Value::Float(f)) => text.push_str(&f.to_string()),
                    Some(Value::String(s) | Value::LocalString(s)) => text.push_str(s),
                    Some(Value::Name(n)) => {
                        if let Some(Value::String(s) | Value::LocalString(s)) = program.value(*n) {
                            text.push_str(s);
                        }
                    }
                    _ => {}
                }
            }
            let value = if as_name {
                Value::Name(checksum(&text))
            } else {
                Value::String(text)
            };
            let frame = self.frames.last_mut().unwrap();
            set_param(&mut frame.params, into, value);
            return true;
        }
        // Built in: whether two names are the same (`ChecksumEquals a = x
        // b = y`).
        if target.is_none() && name == checksum("ChecksumEquals") {
            let (a, b) = (args.get(checksum("a")), args.get(checksum("b")));
            return a.is_some() && a == b;
        }
        // Built in: an array's items one by one (`GetNextArrayElement
        // array` gives `element`, counting with `index`; false past the
        // end).
        if target.is_none() && name == checksum("GetNextArrayElement") {
            let array = match &args {
                Value::Struct(items) => match items.iter().find(|(k, _)| k.is_none()) {
                    Some((_, Value::Array(a))) => Some(a.clone()),
                    Some((_, Value::Name(n))) => match program.value(*n) {
                        Some(Value::Array(a)) => Some(a.clone()),
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            };
            let Some(array) = array else {
                return false;
            };
            let frame = self.frames.last_mut().unwrap();
            let index = frame
                .params
                .iter()
                .find(|(k, _)| *k == Some(checksum("index")))
                .and_then(|(_, v)| v.as_int())
                .unwrap_or(0)
                .max(0) as usize;
            let Some(element) = array.get(index) else {
                frame.params.retain(|(k, _)| *k != Some(checksum("index")));
                return false;
            };
            set_param(&mut frame.params, checksum("element"), element.clone());
            set_param(
                &mut frame.params,
                checksum("index"),
                Value::Integer(index as i32 + 1),
            );
            return true;
        }
        // Built in: an array's length (`GetArraySize name`, giving
        // `array_size`), and a script run for each item of one
        // (`ForEachIn array do = script params = {...}`).
        if target.is_none() && (name == checksum("GetArraySize") || name == checksum("ForEachIn")) {
            let Value::Struct(items) = &args else {
                return false;
            };
            let array = match items.iter().find(|(k, _)| k.is_none()).map(|(_, v)| v) {
                Some(Value::Array(a)) => a.clone(),
                Some(Value::Name(n)) => match program.value(*n) {
                    Some(Value::Array(a)) => a.clone(),
                    _ => return false,
                },
                _ => return false,
            };
            if name == checksum("GetArraySize") {
                let frame = self.frames.last_mut().unwrap();
                set_param(
                    &mut frame.params,
                    checksum("array_size"),
                    Value::Integer(array.len() as i32),
                );
                return true;
            }
            let Some(Value::Name(script)) = args.get(checksum("do")) else {
                return false;
            };
            if !program.has_script(*script) {
                return false;
            }
            let base = match args.get(checksum("params")) {
                Some(Value::Struct(p)) => p.clone(),
                _ => Vec::new(),
            };
            // The last item's frame deepest, so they run in order.
            for item in array.iter().rev() {
                if self.frames.len() >= MAX_DEPTH {
                    break;
                }
                let mut params = base.clone();
                if let Value::Struct(fields) = item {
                    for (k, v) in fields {
                        match k {
                            Some(k) => set_param(&mut params, *k, v.clone()),
                            None => params.push((None, v.clone())),
                        }
                    }
                }
                self.frames.push(Frame {
                    script: *script,
                    pc: 0,
                    params,
                    loops: Vec::new(),
                    started: false,
                });
            }
            return true;
        }
        match host.command(target, name, &args) {
            Outcome::Done(result) => result,
            Outcome::Wait(seconds) => {
                self.wait += seconds;
                true
            }
            Outcome::Params(params) => {
                let frame = self.frames.last_mut().unwrap();
                for (k, v) in params {
                    if let Some(k) = k {
                        set_param(&mut frame.params, k, v);
                    }
                }
                true
            }
        }
    }

    /// Evaluates an `if` line: `[NOT] call` or `(expression)`.
    fn condition(&mut self, program: &Program, host: &mut dyn Host, line: &[Token]) -> bool {
        // `a OR b`, `a AND b` (left to right) at the top level.
        let mut depth = 0;
        let mut split = None;
        for (i, t) in line.iter().enumerate() {
            match t {
                Token::OpenParen => depth += 1,
                Token::CloseParen => depth -= 1,
                Token::Or | Token::And if depth == 0 => split = Some(i),
                _ => {}
            }
        }
        if let Some(i) = split {
            let left = self.condition(program, host, &line[..i]);
            return match line[i] {
                Token::Or => left || self.condition(program, host, &line[i + 1..]),
                _ => left && self.condition(program, host, &line[i + 1..]),
            };
        }
        let (negate, rest) = match line {
            [Token::Not, rest @ ..] => (true, rest),
            _ => (false, line),
        };
        let value = match rest.first() {
            // One group in parentheses: a condition of its own if it holds
            // a command or AND/OR (`((LevelIs a) OR (LevelIs b))`), else
            // arithmetic (`(<n> > 3)`).
            Some(Token::OpenParen) if matching_paren(rest, 0) == rest.len() - 1 => {
                let inner = &rest[1..rest.len() - 1];
                let logic = inner.iter().any(|t| matches!(t, Token::Or | Token::And))
                    || matches!(inner.first(), Some(Token::OpenParen | Token::Not))
                    || is_command(inner, program);
                if logic {
                    self.condition(program, host, inner)
                } else {
                    let params = &self.frames.last().unwrap().params;
                    first_number(rest, params, program).is_some_and(|v| v != 0.0)
                }
            }
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

/// Whether tokens are a command (a name not followed by an operator, nor
/// a global value): `LevelIs load_skateshop`, `skater:IsAlive`.
fn is_command(tokens: &[Token], program: &Program) -> bool {
    match tokens {
        [Token::Name(_), Token::Colon, Token::Name(_), ..] => true,
        [Token::Name(n), rest @ ..] => {
            program.value(*n).is_none()
                && !matches!(
                    rest.first(),
                    Some(
                        Token::Equals
                            | Token::LessThan
                            | Token::LessThanEqual
                            | Token::GreaterThan
                            | Token::GreaterThanEqual
                            | Token::Add
                            | Token::Minus
                            | Token::Multiply
                            | Token::Divide
                    )
                )
        }
        _ => false,
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
/// Past the `endswitch` closing the switch `at` is inside.
fn after_switch(body: &[Token], mut at: usize) -> usize {
    let mut depth = 0;
    while let Some(t) = body.get(at) {
        match t {
            Token::Switch => depth += 1,
            Token::EndSwitch if depth == 0 => return line_end(body, at),
            Token::EndSwitch => depth -= 1,
            _ => {}
        }
        at += 1;
    }
    body.len()
}

/// The first value on a line, `<params>` and arithmetic resolved.
fn first_value(tokens: &[Token], params: &Params, program: &Program) -> Option<Value> {
    let mut wrapped = vec![Token::StartStruct];
    wrapped.extend_from_slice(tokens);
    wrapped.push(Token::EndStruct);
    match resolve(&wrapped, params, program) {
        Value::Struct(mut items) if !items.is_empty() => Some(items.remove(0).1),
        _ => None,
    }
}

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
                let value = evaluate_num(&tokens[i + 1..close], params, program);
                flat.push(value.map_or(Token::Integer(0), Num::token));
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
                    match params.iter().find(|(k, _)| *k == Some(*n)) {
                        Some((_, value)) => flat.extend(value_tokens(value)),
                        // Not given: nothing passed (`num_items = <num_items>`
                        // leaves the script its default), a bare one too.
                        None => {
                            if matches!(flat.as_slice(), [.., Token::Name(_), Token::Equals]) {
                                flat.truncate(flat.len() - 2);
                            }
                        }
                    }
                    i += 2;
                } else {
                    i += 1;
                }
            }
            Token::EndOfLine | Token::LineNumber(_) => i += 1,
            // `Random(@a @b @c)`: one of the choices. They're separated by
            // jumps past the rest; the last has no end of its own, so it's
            // taken to be as long as the first (they're single values here).
            Token::Random(_, offsets) if !offsets.is_empty() => {
                let mut choices: Vec<&[Token]> = Vec::new();
                let mut at = i + 1;
                for _ in 0..offsets.len() - 1 {
                    let end = tokens[at..]
                        .iter()
                        .position(|t| matches!(t, Token::Jump(_)))
                        .map_or(tokens.len(), |j| at + j);
                    choices.push(&tokens[at..end]);
                    at = (end + 1).min(tokens.len());
                }
                let last = choices.first().map_or(1, |c| c.len()).max(1);
                let end = (at + last).min(tokens.len());
                choices.push(&tokens[at..end]);
                let pick = next_random() as usize % choices.len();
                flat.extend(
                    choices[pick]
                        .iter()
                        .filter(|t| !matches!(t, Token::At))
                        .cloned(),
                );
                i = end;
            }
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

/// A number for picking among `Random` choices (xorshift, shared).
fn next_random() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEED: AtomicU32 = AtomicU32::new(0x2545_F491);
    let mut x = SEED.load(Ordering::Relaxed);
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    SEED.store(x, Ordering::Relaxed);
    x
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

/// A value in arithmetic: a number, or a pair or vector (`(0.0, 12.0)`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Num {
    Scalar(f64),
    /// The components and how many there are (2 for a pair, 3).
    Vector([f64; 3], usize),
}

impl Num {
    fn of(value: &Value) -> Option<Num> {
        match value {
            Value::Pair([x, y]) => Some(Num::Vector([f64::from(*x), f64::from(*y), 0.0], 2)),
            Value::Vector([x, y, z]) => Some(Num::Vector(
                [f64::from(*x), f64::from(*y), f64::from(*z)],
                3,
            )),
            Value::Name(n) => Some(Num::Scalar(f64::from(*n))),
            v => v.as_f32().map(|f| Num::Scalar(f64::from(f))),
        }
    }

    fn scalar(self) -> f64 {
        match self {
            Num::Scalar(v) => v,
            Num::Vector(v, _) => v[0],
        }
    }

    /// Componentwise, a number applying to every component.
    fn zip(self, other: Num, f: impl Fn(f64, f64) -> f64) -> Num {
        match (self, other) {
            (Num::Scalar(a), Num::Scalar(b)) => Num::Scalar(f(a, b)),
            (Num::Vector(a, n), Num::Scalar(b)) => Num::Vector(a.map(|c| f(c, b)), n),
            (Num::Scalar(a), Num::Vector(b, n)) => Num::Vector(b.map(|c| f(a, c)), n),
            (Num::Vector(a, n), Num::Vector(b, m)) => {
                Num::Vector([f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2])], n.max(m))
            }
        }
    }

    fn token(self) -> Token {
        match self {
            Num::Scalar(v) if v.fract() == 0.0 && v.abs() < 1e9 => Token::Integer(v as i32),
            Num::Scalar(v) => Token::Float(v as f32),
            Num::Vector(v, 2) => Token::Pair([v[0] as f32, v[1] as f32]),
            Num::Vector(v, _) => Token::Vector(v.map(|c| c as f32)),
        }
    }
}

/// Arithmetic and comparisons over numbers, pairs, vectors and
/// `<params>`: `*`, `/` and `.` (the dot product) before `+` and `-`,
/// before comparisons. A name compares as its checksum.
fn evaluate_num(tokens: &[Token], params: &Params, program: &Program) -> Option<Num> {
    // Operands and operators in order.
    let mut values: Vec<Num> = Vec::new();
    let mut ops: Vec<&Token> = Vec::new();
    let mut i = 0;
    let mut negative = false;
    while i < tokens.len() {
        let v = match &tokens[i] {
            Token::Integer(v) => Some(Num::Scalar(f64::from(*v))),
            Token::HexInteger(v) => Some(Num::Scalar(f64::from(*v))),
            Token::Float(v) => Some(Num::Scalar(f64::from(*v))),
            Token::Pair(p) => Num::of(&Value::Pair(*p)),
            Token::Vector(p) => Num::of(&Value::Vector(*p)),
            Token::Arg => {
                i += 1;
                let Some(Token::Name(n)) = tokens.get(i) else {
                    return None;
                };
                params
                    .iter()
                    .find(|(k, _)| *k == Some(*n))
                    .and_then(|(_, v)| Num::of(v))
            }
            Token::Name(n) => Some(match program.value(*n).and_then(Num::of) {
                Some(Num::Scalar(_)) | None => program
                    .value(*n)
                    .and_then(Value::as_f32)
                    .map_or(Num::Scalar(f64::from(*n)), |f| Num::Scalar(f64::from(f))),
                Some(v) => v,
            }),
            Token::OpenParen => {
                let close = matching_paren(tokens, i);
                let v = evaluate_num(&tokens[i + 1..close], params, program);
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
            | Token::Dot
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
        values.push(if negative {
            v.zip(Num::Scalar(-1.0), |a, b| a * b)
        } else {
            v
        });
        negative = false;
        i += 1;
    }
    if values.is_empty() || values.len() != ops.len() + 1 {
        return values.first().copied().filter(|_| ops.is_empty());
    }
    let apply = |a: Num, op: &Token, b: Num| -> Option<Num> {
        let bool_of = |x: bool| Num::Scalar(f64::from(u8::from(x)));
        Some(match op {
            Token::Add => a.zip(b, |x, y| x + y),
            Token::Minus => a.zip(b, |x, y| x - y),
            Token::Multiply => a.zip(b, |x, y| x * y),
            Token::Divide => a.zip(b, |x, y| if y != 0.0 { x / y } else { 0.0 }),
            Token::Dot => match (a, b) {
                (Num::Vector(x, _), Num::Vector(y, _)) => {
                    Num::Scalar(x[0] * y[0] + x[1] * y[1] + x[2] * y[2])
                }
                _ => a.zip(b, |x, y| x * y),
            },
            Token::Equals => bool_of(a == b),
            Token::LessThan => bool_of(a.scalar() < b.scalar()),
            Token::LessThanEqual => bool_of(a.scalar() <= b.scalar()),
            Token::GreaterThan => bool_of(a.scalar() > b.scalar()),
            Token::GreaterThanEqual => bool_of(a.scalar() >= b.scalar()),
            _ => return None,
        })
    };
    // By precedence: products, then sums, then comparisons.
    for level in [
        &[Token::Multiply, Token::Divide, Token::Dot][..],
        &[Token::Add, Token::Minus][..],
    ] {
        let mut k = 0;
        while k < ops.len() {
            if level.contains(ops[k]) {
                let v = apply(values[k], ops[k], values[k + 1])?;
                values[k] = v;
                values.remove(k + 1);
                ops.remove(k);
            } else {
                k += 1;
            }
        }
    }
    let mut acc = values[0];
    for (op, v) in ops.iter().zip(values.iter().skip(1)) {
        acc = apply(acc, op, *v)?;
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
    fn random_picks_one_choice() {
        use crate::token::RandomKind;
        let mut program = Program::new();
        // script Say: Speak stream = Random(@LineA @LineB) Vol = 3
        program.add_script(
            checksum("Say"),
            vec![
                n("Speak"),
                n("stream"),
                Token::Equals,
                Token::Random(RandomKind::Plain, vec![0, 0]),
                Token::At,
                n("LineA"),
                Token::Jump(0),
                Token::At,
                n("LineB"),
                n("Vol"),
                Token::Equals,
                Token::Integer(3),
                Token::EndOfLine,
            ],
        );
        let mut seen = std::collections::HashSet::new();
        for _ in 0..20 {
            let mut log = Log::default();
            let mut thread = Thread::new(checksum("Say"), Vec::new());
            thread.run(&program, &mut log, 0.0);
            let args = &log.calls[0].2;
            let stream = args
                .get(checksum("stream"))
                .and_then(Value::as_name)
                .unwrap();
            assert!(stream == checksum("LineA") || stream == checksum("LineB"));
            assert_eq!(args.get(checksum("Vol")), Some(&Value::Integer(3)));
            seen.insert(stream);
        }
        assert_eq!(seen.len(), 2, "both choices come up");
    }

    #[test]
    fn got_param_and_tags() {
        let mut program = Program::new();
        // script Car: if GotParam Fast / Fast_Car / endif / SetTags Speed = 3
        // / GetTags / Report <Speed>
        program.add_script(
            checksum("Car"),
            vec![
                Token::If,
                n("GotParam"),
                n("Fast"),
                Token::EndOfLine,
                n("Fast_Car"),
                Token::EndOfLine,
                Token::EndIf,
                Token::EndOfLine,
                n("SetTags"),
                n("Speed"),
                Token::Equals,
                Token::Integer(3),
                Token::EndOfLine,
                n("GetTags"),
                Token::EndOfLine,
                n("Report"),
                Token::Arg,
                n("Speed"),
                Token::EndOfLine,
            ],
        );
        for (params, fast) in [
            (vec![(None, Value::Name(checksum("Fast")))], true),
            (vec![], false),
        ] {
            let mut log = Log::default();
            let mut thread = Thread::new(checksum("Car"), params);
            thread.run(&program, &mut log, 0.0);
            assert_eq!(names(&log).contains(&checksum("Fast_Car")), fast);
            let report = log
                .calls
                .iter()
                .find(|c| c.1 == checksum("Report"))
                .unwrap();
            assert_eq!(report.2, Value::Struct(vec![(None, Value::Integer(3))]));
        }
    }

    #[test]
    fn switch_takes_the_matching_case() {
        let mut program = Program::new();
        // switch <kind> / case apple / Report 1 / case pear / Report 2 /
        // default / Report 3 / endswitch / Report 4
        let line = |t: Vec<Token>| {
            let mut t = t;
            t.push(Token::EndOfLine);
            t
        };
        let body: Vec<Token> = [
            line(vec![Token::Switch, Token::Arg, n("kind")]),
            line(vec![Token::Case, n("apple")]),
            line(vec![n("Report"), Token::Integer(1)]),
            line(vec![Token::Case, n("pear")]),
            line(vec![n("Report"), Token::Integer(2)]),
            line(vec![Token::Default]),
            line(vec![n("Report"), Token::Integer(3)]),
            line(vec![Token::EndSwitch]),
            line(vec![n("Report"), Token::Integer(4)]),
        ]
        .concat();
        program.add_script(checksum("Pick"), body);
        for (kind, want) in [
            ("pear", vec![2, 4]),
            ("apple", vec![1, 4]),
            ("plum", vec![3, 4]),
        ] {
            let mut log = Log::default();
            let params = vec![(Some(checksum("kind")), Value::Name(checksum(kind)))];
            Thread::new(checksum("Pick"), params).run(&program, &mut log, 0.0);
            let got: Vec<i32> = log
                .calls
                .iter()
                .filter(|c| c.1 == checksum("Report"))
                .filter_map(|c| match &c.2 {
                    Value::Struct(items) => items.first().and_then(|(_, v)| v.as_int()),
                    _ => None,
                })
                .collect();
            assert_eq!(got, want, "{kind}");
        }
    }

    #[test]
    fn pairs_in_arithmetic() {
        let program = Program::new();
        let params = vec![
            (Some(checksum("pos")), Value::Pair([100.0, 20.0])),
            (Some(checksum("h")), Value::Integer(32)),
        ];
        let eval = |tokens: Vec<Token>| evaluate_num(&tokens, &params, &program);
        // (<pos> + (0.0, 12.0))
        assert_eq!(
            eval(vec![
                Token::Arg,
                n("pos"),
                Token::Add,
                Token::Pair([0.0, 12.0])
            ]),
            Some(Num::Vector([100.0, 32.0, 0.0], 2))
        );
        // (1.0, 0.0) * 2 + (0.0, 1.0) * <h> / 16: products first.
        assert_eq!(
            eval(vec![
                Token::Pair([1.0, 0.0]),
                Token::Multiply,
                Token::Integer(2),
                Token::Add,
                Token::Pair([0.0, 1.0]),
                Token::Multiply,
                Token::Arg,
                n("h"),
                Token::Divide,
                Token::Integer(16),
            ]),
            Some(Num::Vector([2.0, 2.0, 0.0], 2))
        );
        // (0.0, 1.0).<pos>: the dot product.
        assert_eq!(
            eval(vec![
                Token::Pair([0.0, 1.0]),
                Token::Dot,
                Token::Arg,
                n("pos")
            ]),
            Some(Num::Scalar(20.0))
        );
    }

    #[test]
    fn statements_through_params() {
        let mut program = Program::new();
        // script Outer: <n> = 2 / <n> = (<n> + 1) / <then> <n>
        program.add_script(
            checksum("Outer"),
            vec![
                Token::Arg,
                n("n"),
                Token::Equals,
                Token::Integer(2),
                Token::EndOfLine,
                Token::Arg,
                n("n"),
                Token::Equals,
                Token::OpenParen,
                Token::Arg,
                n("n"),
                Token::Add,
                Token::Integer(1),
                Token::CloseParen,
                Token::EndOfLine,
                Token::Arg,
                n("then"),
                Token::Arg,
                n("n"),
                Token::EndOfLine,
            ],
        );
        let mut log = Log::default();
        let params = vec![(Some(checksum("then")), Value::Name(checksum("Report")))];
        let mut thread = Thread::new(checksum("Outer"), params);
        thread.run(&program, &mut log, 0.0);
        let report = log
            .calls
            .iter()
            .find(|c| c.1 == checksum("Report"))
            .unwrap();
        assert_eq!(report.2, Value::Struct(vec![(None, Value::Integer(3))]));
    }

    #[test]
    fn array_size_and_for_each() {
        let mut program = Program::new();
        let spot =
            |id: &str| Value::Struct(vec![(Some(checksum("id")), Value::Name(checksum(id)))]);
        program.add_value(
            checksum("Spots"),
            Value::Array(vec![spot("SpotA"), spot("SpotB")]),
        );
        // script Count: GetArraySize Spots / Report <array_size> /
        // ForEachIn Spots do = Visit params = { Goal = g }
        program.add_script(
            checksum("Count"),
            vec![
                n("GetArraySize"),
                n("Spots"),
                Token::EndOfLine,
                n("Report"),
                Token::Arg,
                n("array_size"),
                Token::EndOfLine,
                n("ForEachIn"),
                n("Spots"),
                n("do"),
                Token::Equals,
                n("Visit"),
                n("params"),
                Token::Equals,
                Token::StartStruct,
                n("Goal"),
                Token::Equals,
                n("g"),
                Token::EndStruct,
                Token::EndOfLine,
            ],
        );
        // script Visit: Seen <id> <Goal>
        program.add_script(
            checksum("Visit"),
            vec![
                n("Seen"),
                Token::Arg,
                n("id"),
                Token::Arg,
                n("Goal"),
                Token::EndOfLine,
            ],
        );
        let mut log = Log::default();
        let mut thread = Thread::new(checksum("Count"), Vec::new());
        thread.run(&program, &mut log, 0.0);
        let args = |name: &str| -> Vec<Value> {
            log.calls
                .iter()
                .filter(|c| c.1 == checksum(name))
                .map(|c| c.2.clone())
                .collect()
        };
        assert_eq!(
            args("Report"),
            vec![Value::Struct(vec![(None, Value::Integer(2))])]
        );
        let seen = |id: &str| {
            Value::Struct(vec![
                (None, Value::Name(checksum(id))),
                (None, Value::Name(checksum("g"))),
            ])
        };
        assert_eq!(args("Seen"), vec![seen("SpotA"), seen("SpotB")]);
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
