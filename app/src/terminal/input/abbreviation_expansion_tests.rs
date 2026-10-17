//! The expected buffers and cursor positions are what fish 4.9.3 does with the same
//! abbreviations: typing the abbreviation, a space and then `-3` runs the command shown in each
//! test.
use string_offset::ByteOffset;
use warp_completer::parsers::simple::command_at_cursor_position;
use warp_util::path::EscapeChar;

use super::*;
use crate::terminal::shell::ShellType;

/// Real `abbr --show` output (fish 4.9.3).
const ABBR_SHOW: &str = r#"abbr -a -- cls 'clear && printf "\\033[3J"'
abbr -a -- ls 'eza --git'
abbr -a -- ll 'eza -la --git'
abbr -a -- cat 'bat --plain'
abbr -a --position anywhere --set-cursor=@CURSOR@ --command git -- glc "log --pretty=tformat:'%h%x09%an%x09%s' @CURSOR@ | column -t -s \\t"
abbr -a --position anywhere --command git -- summary "log --pretty=tformat:'%h%x09%an%x09%s' | column -t -s \\t"
"#;

/// Types a space after `typed` with `abbreviations` and returns the buffer and the cursor
/// afterwards, the way the input applies an expansion.
fn type_space_with(abbreviations: &[Abbreviation], typed: &str) -> (String, usize) {
    let mut buffer = format!("{typed} ");
    let cursor = buffer.len();
    let Some(command) =
        command_at_cursor_position(&buffer, EscapeChar::Backslash, ByteOffset::from(cursor))
    else {
        return (buffer, cursor);
    };
    let Some(expansion) = abbreviation_expansion_on_space(abbreviations, &buffer, &command, cursor)
    else {
        return (buffer, cursor);
    };
    let start = expansion.token.span().start();
    let replacement = format!("{} ", expansion.text);
    let cursor = start + expansion.cursor.unwrap_or(replacement.len());
    buffer.replace_range(start..buffer.len(), &replacement);
    (buffer, cursor)
}

fn type_space(typed: &str) -> (String, usize) {
    type_space_with(&ShellType::Fish.parse_abbreviations(ABBR_SHOW), typed)
}

/// Inserts `text` at `cursor`, like typing it.
fn insert_at(buffer: &str, cursor: usize, text: &str) -> String {
    let mut buffer = buffer.to_owned();
    buffer.insert_str(cursor, text);
    buffer
}

#[test]
fn test_command_position_abbreviation_expands() {
    assert_eq!(type_space("ll"), ("eza -la --git ".to_owned(), 14));
}

#[test]
fn test_command_position_abbreviation_does_not_expand_as_argument() {
    assert_eq!(type_space("git ll"), ("git ll ".to_owned(), 7));
}

#[test]
fn test_expansion_is_unescaped_like_fish() {
    let (buffer, _) = type_space("cls");
    assert_eq!(buffer, r#"clear && printf "\033[3J" "#);
}

#[test]
fn test_command_specific_abbreviation_expands_as_argument_to_its_command() {
    let (buffer, cursor) = type_space("echo x | git summary");
    assert_eq!(
        buffer,
        r"echo x | git log --pretty=tformat:'%h%x09%an%x09%s' | column -t -s \t "
    );
    assert_eq!(cursor, buffer.len());
}

#[test]
fn test_command_specific_abbreviation_does_not_expand_elsewhere() {
    assert_eq!(type_space("glc"), ("glc ".to_owned(), 4));
    assert_eq!(type_space("echo glc"), ("echo glc ".to_owned(), 9));
}

/// `--set-cursor`: the marker is removed and the cursor goes where it was; the typed space stays
/// at the end of the expansion.
#[test]
fn test_set_cursor_abbreviation_moves_cursor_to_marker() {
    let (buffer, cursor) = type_space("git glc");
    assert_eq!(
        buffer,
        r"git log --pretty=tformat:'%h%x09%an%x09%s'  | column -t -s \t "
    );
    assert_eq!(
        &buffer[..cursor],
        "git log --pretty=tformat:'%h%x09%an%x09%s' "
    );
    assert_eq!(
        insert_at(&buffer, cursor, "-3"),
        r"git log --pretty=tformat:'%h%x09%an%x09%s' -3 | column -t -s \t "
    );
}

/// The other `--set-cursor` forms checked against fish: the default `%` marker and a marker
/// followed by more text; and an abbreviation without a marker.
#[test]
fn test_set_cursor_forms_match_fish() {
    let abbreviations = ShellType::Fish.parse_abbreviations(
        r#"abbr -a --position anywhere --set-cursor='%' --command echo -- zz 'A%B'
abbr -a --position anywhere --command echo -- yy 'C D'
abbr -a --position anywhere --set-cursor=@C@ --command echo -- ww 'X@C@ | cat'
"#,
    );
    let type_space_then_q = |typed: &str| {
        let (buffer, cursor) = type_space_with(&abbreviations, typed);
        insert_at(&buffer, cursor, "Q")
    };

    assert_eq!(type_space_then_q("echo zz"), "echo AQB ");
    assert_eq!(type_space_then_q("echo yy"), "echo C D Q");
    assert_eq!(type_space_then_q("echo ww"), "echo XQ | cat ");
}

/// Like in fish, a `--regex` has to match the whole token.
#[test]
fn test_regex_abbreviation_matches_whole_token() {
    let abbreviations =
        ShellType::Fish.parse_abbreviations("abbr -a --regex 'g[0-9]+' -- gnum 'git log -n'");

    assert_eq!(
        type_space_with(&abbreviations, "g12"),
        ("git log -n ".to_owned(), 11)
    );
    assert_eq!(
        type_space_with(&abbreviations, "xg12"),
        ("xg12 ".to_owned(), 5)
    );
}

/// `--function` abbreviations need fish to run the function, so they are left to fish.
#[test]
fn test_function_abbreviation_does_not_expand() {
    let abbreviations = ShellType::Fish
        .parse_abbreviations(r"abbr -a --regex '^\\.\\.+$' --function multicd -- dotdot");

    assert_eq!(
        type_space_with(&abbreviations, "..."),
        ("... ".to_owned(), 4)
    );
}

/// When several abbreviations may expand a token, the one added last wins.
#[test]
fn test_last_matching_abbreviation_wins() {
    let abbreviations = ShellType::Fish.parse_abbreviations(
        r#"abbr -a --regex 'x.*' -- zfirst 'from zfirst'
abbr -a --regex 'x.*' -- asecond 'from asecond'
abbr -a --position anywhere --regex 'x.*' --command git -- cpos 'from git only'
"#,
    );

    assert_eq!(type_space_with(&abbreviations, "x1").0, "from asecond ");
    assert_eq!(
        type_space_with(&abbreviations, "git x1").0,
        "git from git only "
    );
}

/// Fish only expands a token as typed: quoted or escaped tokens stay as they are.
#[test]
fn test_quoted_or_escaped_token_does_not_expand() {
    let abbreviations = ShellType::Fish.parse_abbreviations(
        "abbr -a --position anywhere --command git -- co checkout
abbr -a --position anywhere -- -C --color",
    );

    assert_eq!(type_space_with(&abbreviations, "git 'co'").0, "git 'co' ");
    assert_eq!(type_space_with(&abbreviations, r"git \co").0, r"git \co ");
    assert_eq!(
        type_space_with(&abbreviations, r#"ls "-C""#).0,
        r#"ls "-C" "#
    );
    assert_eq!(type_space_with(&abbreviations, "ls -C").0, "ls --color ");
}

/// Leading `KEY=VALUE` assignments are not the command: fish expands after them.
#[test]
fn test_abbreviations_expand_after_env_var_assignments() {
    assert_eq!(type_space("FOO=1 ll").0, "FOO=1 eza -la --git ");
    assert_eq!(
        type_space("GIT_PAGER=cat git summary").0,
        r"GIT_PAGER=cat git log --pretty=tformat:'%h%x09%an%x09%s' | column -t -s \t "
    );
    assert_eq!(type_space("FOO=1 git ll").0, "FOO=1 git ll ");
}

/// Fish's keywords and decorators aren't the command: the command is the word after them. Other
/// commands that run a command (`env`, `sudo`) are the command themselves.
#[test]
fn test_abbreviations_expand_after_fish_keywords_and_decorators() {
    for prefix in ["command", "builtin", "exec", "time", "not", "and", "if"] {
        assert_eq!(
            type_space(&format!("{prefix} ll")).0,
            format!("{prefix} eza -la --git "),
            "after {prefix}"
        );
    }
    assert_eq!(
        type_space("command git summary").0,
        r"command git log --pretty=tformat:'%h%x09%an%x09%s' | column -t -s \t "
    );
    assert_eq!(type_space("env ll").0, "env ll ");
    assert_eq!(type_space("sudo ll").0, "sudo ll ");
}

/// `--command` compares against the command as typed, so a quoted command doesn't match.
#[test]
fn test_command_specific_abbreviation_does_not_match_quoted_command() {
    assert_eq!(type_space("'git' summary").0, "'git' summary ");
}
