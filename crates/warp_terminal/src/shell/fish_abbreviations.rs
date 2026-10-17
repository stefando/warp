//! Parsing fish's `abbr --show` output, keeping the options that decide where an abbreviation
//! expands (`--position`, `--command`, `--regex`), what it expands to (an expansion or
//! `--function`) and where the cursor goes (`--set-cursor`).
//!
//! `abbr --show` prints one `abbr -a [OPTIONS] -- NAME [EXPANSION]` command per abbreviation,
//! quoted the way fish quotes words (see `fish_words`).
use std::collections::HashMap;
use std::iter::Peekable;
use std::str::Chars;

use regex::Regex;
use smol_str::SmolStr;
use warp_core::safe_warn;

/// Where an abbreviation may expand (`--position`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbbreviationPosition {
    /// Only as the command of a command line (fish's default).
    Command,
    /// At any position.
    Anywhere,
}

/// What an abbreviation expands to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AbbreviationExpansion {
    /// A fixed text.
    Text(String),
    /// The output of this fish function (`--function`), which is called with the matched token.
    Function(String),
}

/// A `--regex` pattern, which like in fish has to match the whole token.
#[derive(Clone, Debug)]
pub struct AbbreviationRegex {
    pattern: String,
    regex: Regex,
}

impl AbbreviationRegex {
    /// Fails for patterns the `regex` crate doesn't support, e.g. PCRE2 lookarounds.
    pub fn new(pattern: &str) -> Result<Self, regex::Error> {
        Ok(Self {
            pattern: pattern.to_owned(),
            regex: Regex::new(&format!("^(?:{pattern})$"))?,
        })
    }

    pub fn is_match(&self, token: &str) -> bool {
        self.regex.is_match(token)
    }
}

impl PartialEq for AbbreviationRegex {
    fn eq(&self, other: &Self) -> bool {
        self.pattern == other.pattern
    }
}

impl Eq for AbbreviationRegex {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Abbreviation {
    pub name: SmolStr,
    pub expansion: AbbreviationExpansion,
    pub position: AbbreviationPosition,
    /// The abbreviation only expands as an argument to one of these commands (`--command`). An
    /// empty command means it only expands when there is no command. Empty: any command.
    pub commands: Vec<String>,
    /// The token must match this instead of being equal to [`Self::name`] (`--regex`).
    pub regex: Option<AbbreviationRegex>,
    /// The cursor goes to the first occurrence of this marker in the expansion, which is removed
    /// (`--set-cursor`).
    pub set_cursor: Option<String>,
}

impl Abbreviation {
    /// Whether `token` is this abbreviation.
    pub fn matches_token(&self, token: &str) -> bool {
        match &self.regex {
            Some(regex) => regex.is_match(token),
            None => self.name == token,
        }
    }

    /// Whether the abbreviation may expand for a token that is an argument to the command
    /// `argument_of`, or is the command itself (`None`).
    pub fn may_expand_at(&self, argument_of: Option<&str>) -> bool {
        if !self.commands.is_empty() {
            let argument_of = argument_of.unwrap_or_default();
            return self.commands.iter().any(|command| command == argument_of);
        }
        match self.position {
            AbbreviationPosition::Command => argument_of.is_none(),
            AbbreviationPosition::Anywhere => true,
        }
    }
}

/// The default `--set-cursor` marker.
const DEFAULT_SET_CURSOR_MARKER: &str = "%";

/// Parses the output of fish's `abbr --show`. Lines that can't be parsed are skipped.
pub fn parse_fish_abbreviations(abbr_output: &str) -> Vec<Abbreviation> {
    abbr_output
        .lines()
        .filter_map(|line| {
            let abbreviation = parse_abbr_line(line);
            if abbreviation.is_none() && !line.trim().is_empty() {
                safe_warn!(
                    safe: ("Could not parse a fish abbreviation"),
                    full: ("Could not parse fish abbreviation: {line}")
                );
            }
            abbreviation
        })
        .collect()
}

/// The plain-text abbreviations that may expand as a command, by name.
pub fn command_abbreviations(abbreviations: &[Abbreviation]) -> HashMap<SmolStr, String> {
    abbreviations
        .iter()
        .filter(|abbreviation| abbreviation.regex.is_none() && abbreviation.may_expand_at(None))
        .filter_map(|abbreviation| match &abbreviation.expansion {
            AbbreviationExpansion::Text(expansion) => {
                Some((abbreviation.name.clone(), expansion.clone()))
            }
            AbbreviationExpansion::Function(_) => None,
        })
        .collect()
}

fn parse_abbr_line(line: &str) -> Option<Abbreviation> {
    let words = fish_words(line)?;
    let mut words = words.into_iter();
    if words.next()? != "abbr" {
        return None;
    }

    let mut position = AbbreviationPosition::Command;
    let mut commands = Vec::new();
    let mut regex = None;
    let mut set_cursor = None;
    let mut function = None;
    loop {
        let word = words.next()?;
        let (option, inline_value) = match word.split_once('=') {
            Some((option, value)) if option.starts_with("--") => (option, Some(value.to_owned())),
            _ => (word.as_str(), None),
        };
        let mut value = || inline_value.clone().or_else(|| words.next());
        match option {
            "--" => break,
            "--position" | "-p" => {
                position = match value()?.as_str() {
                    "anywhere" => AbbreviationPosition::Anywhere,
                    _ => AbbreviationPosition::Command,
                }
            }
            "--command" | "-c" => commands.push(value()?),
            "--regex" | "-r" => regex = Some(AbbreviationRegex::new(&value()?).ok()?),
            "--function" | "-f" => function = Some(value()?),
            "--set-cursor" => {
                set_cursor = Some(
                    inline_value
                        .clone()
                        .unwrap_or_else(|| DEFAULT_SET_CURSOR_MARKER.to_owned()),
                )
            }
            // `-a`/`--add`, scope flags (`-g`, `-U`) and anything newer that doesn't affect
            // expansion.
            _ => {}
        }
    }

    let name = SmolStr::from(words.next()?);
    let expansion = match function {
        Some(function) => AbbreviationExpansion::Function(function),
        None => AbbreviationExpansion::Text(words.next()?),
    };
    if !commands.is_empty() {
        // `--command` implies `--position anywhere`.
        position = AbbreviationPosition::Anywhere;
    }
    Some(Abbreviation {
        name,
        expansion,
        position,
        commands,
        regex,
        set_cursor,
    })
}

/// Splits a line into words the way fish does, removing quotes and escapes:
/// - `'...'`: only `\'` and `\\` are escapes;
/// - `"..."`: only `\"`, `\\`, `\$` and an escaped newline are escapes;
/// - unquoted: see `push_unquoted_escape`.
///
/// An unquoted `#` at the start of a word starts a comment. Returns `None` for an unterminated quote.
fn fish_words(line: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut chars = line.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        match chars.peek() {
            None | Some('#') => return Some(words),
            Some(_) => {}
        }

        let mut word = String::new();
        while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
            match c {
                '\'' => loop {
                    match chars.next()? {
                        '\'' => break,
                        '\\' => match chars.next_if(|c| matches!(c, '\'' | '\\')) {
                            Some(escaped) => word.push(escaped),
                            None => word.push('\\'),
                        },
                        c => word.push(c),
                    }
                },
                '"' => loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next_if(|c| matches!(c, '"' | '\\' | '$' | '\n')) {
                            Some('\n') => {}
                            Some(escaped) => word.push(escaped),
                            None => word.push('\\'),
                        },
                        c => word.push(c),
                    }
                },
                '\\' => match chars.next() {
                    Some(escaped) => push_unquoted_escape(escaped, &mut chars, &mut word),
                    None => word.push('\\'),
                },
                c => word.push(c),
            }
        }
        words.push(word);
    }
}

/// Pushes what the unquoted fish escape `\<escaped>...` stands for onto `word`: a control character
/// (`\n`, `\t`, `\r`, `\e`, `\a`, `\b`, `\f`, `\v`, `\cX`), a code point (`\uXXXX`, `\UXXXXXXXX`,
/// octal `\NNN`), UTF-8 bytes (`\xHH`, `\XHH`, adjacent ones decoded together) or the escaped
/// character itself (`\ `, `\*`, `\$`, ...). Fish prints non-printable characters in these forms,
/// e.g. DEL as `\x7f` and Ctrl-A as `\ca`. Invalid escapes, which fish rejects, keep `escaped`.
fn push_unquoted_escape(escaped: char, chars: &mut Peekable<Chars>, word: &mut String) {
    let code_point = match escaped {
        'n' => Some('\n'),
        't' => Some('\t'),
        'r' => Some('\r'),
        'e' => Some('\u{1b}'),
        'a' => Some('\u{07}'),
        'b' => Some('\u{08}'),
        'f' => Some('\u{0c}'),
        'v' => Some('\u{0b}'),
        'c' => chars
            .next_if(|c| ('@'..'\u{7f}').contains(c))
            .and_then(|control| char::from_u32(u32::from(control) % 32)),
        'x' | 'X' => {
            let bytes = read_byte_escapes(chars);
            if !bytes.is_empty() {
                word.push_str(&String::from_utf8_lossy(&bytes));
                return;
            }
            None
        }
        'u' => read_number(chars, 16, 4, None).and_then(char::from_u32),
        'U' => read_number(chars, 16, 8, None).and_then(char::from_u32),
        '0'..='7' => read_number(chars, 8, 2, escaped.to_digit(8))
            .filter(|value| *value <= 0x7f)
            .and_then(char::from_u32),
        _ => None,
    };
    word.push(code_point.unwrap_or(escaped));
}

/// Reads the hex digits of a `\x` escape and of the `\x`/`\X` escapes right after it, as bytes.
fn read_byte_escapes(chars: &mut Peekable<Chars>) -> Vec<u8> {
    let mut bytes = Vec::new();
    while let Some(byte) =
        read_number(chars, 16, 2, None).and_then(|value| u8::try_from(value).ok())
    {
        bytes.push(byte);
        let mut ahead = chars.clone();
        if ahead.next() != Some('\\') || !matches!(ahead.next(), Some('x' | 'X')) {
            break;
        }
        chars.nth(1);
    }
    bytes
}

/// Reads up to `max_digits` digits in `radix` after the already-read `value`, if any. `None` if
/// there are no digits at all.
fn read_number(
    chars: &mut Peekable<Chars>,
    radix: u32,
    max_digits: usize,
    mut value: Option<u32>,
) -> Option<u32> {
    for _ in 0..max_digits {
        let Some(digit) = chars.peek().and_then(|c| c.to_digit(radix)) else {
            break;
        };
        chars.next();
        value = Some(value.unwrap_or_default() * radix + digit);
    }
    value
}
