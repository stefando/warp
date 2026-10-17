//! Expanding fish abbreviations when the user types a space after them, following their
//! `--position`, `--command`, `--regex` and `--set-cursor` options.
use warp_completer::meta::{HasSpan, Spanned};
use warp_completer::parsers::LiteCommand;
use warp_completer::parsers::simple::is_env_var_assignment;

use super::{Executing, Input};
use crate::terminal::shell::{Abbreviation, AbbreviationExpansion};

#[cfg(test)]
#[path = "abbreviation_expansion_tests.rs"]
mod tests;

/// Keywords and decorators that fish doesn't treat as the command of a command line: the command
/// is the word after them.
const FISH_COMMAND_PREFIXES: [&str; 10] = [
    "and", "or", "not", "!", "time", "if", "while", "command", "builtin", "exec",
];

/// The text to replace a token of the input with.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Expansion<'a> {
    pub token: &'a Spanned<String>,
    pub text: String,
    /// Where the cursor goes in [`Self::text`]. `None`: after the expansion.
    pub cursor: Option<usize>,
}

/// Returns the expansion of the abbreviation that may expand in place of the token of `command`
/// that ends right before the space at `cursor_pos` in `buffer`, if any. Like in fish, the last
/// one wins when several do (`abbr --show` lists them in the order they were added).
///
/// `--function` abbreviations need fish to run the function, so they don't expand here: fish
/// expands them itself when the command is executed with the cursor on them.
pub(super) fn abbreviation_expansion_on_space<'a>(
    abbreviations: &[Abbreviation],
    buffer: &str,
    command: &'a LiteCommand,
    cursor_pos: usize,
) -> Option<Expansion<'a>> {
    // Fish matches what was typed, so quoted or escaped words are neither abbreviations nor
    // keywords, and `--command` compares against the command as typed.
    let typed_parts = command
        .parts
        .iter()
        .map(|part| buffer.get(part.span().start()..part.span().end()))
        .collect::<Option<Vec<_>>>()?;
    let command_index = typed_parts.iter().position(|typed_part| {
        !is_env_var_assignment(typed_part) && !FISH_COMMAND_PREFIXES.contains(typed_part)
    })?;
    let token_index = command
        .parts
        .iter()
        .position(|part| Input::is_potential_expansion(part, cursor_pos, Executing::No))?;
    if token_index < command_index {
        return None;
    }
    let token = &command.parts[token_index];
    let typed_token = typed_parts[token_index];
    let argument_of = (token_index > command_index).then_some(typed_parts[command_index]);

    let abbreviation = abbreviations.iter().rev().find(|abbreviation| {
        abbreviation.matches_token(typed_token) && abbreviation.may_expand_at(argument_of)
    })?;
    let AbbreviationExpansion::Text(text) = &abbreviation.expansion else {
        return None;
    };

    let marker = abbreviation
        .set_cursor
        .as_deref()
        .and_then(|marker| text.find(marker).map(|index| (marker, index)));
    Some(match marker {
        Some((marker, index)) => Expansion {
            token,
            text: text.replacen(marker, "", 1),
            cursor: Some(index),
        },
        None => Expansion {
            token,
            text: text.clone(),
            cursor: None,
        },
    })
}
