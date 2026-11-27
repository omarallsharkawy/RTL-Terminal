//! Shell-aware syntax highlighting for the command composer.
//!
//! Produces non-overlapping, sorted [`HighlightSpan`]s across the buffer for command words
//! (known on `$PATH`, shell builtins, unknown commands), spec-known subcommands, options,
//! strings, variables, existing filesystem paths, operators, and comments.
//!
//! Colors are derived from the active [`Palette`] and [`UiStyle`] so they remain legible in both
//! dark and light themes. Arabic text inside strings stays within a single color span (never
//! splitting an Arabic word mid-shaping) so `RowShaper` preserves Arabic joining and bidi order.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use unicode_bidi::{bidi_class, BidiClass};

use crate::color::{Palette, Rgba};
use crate::composer::specs::{default_registry, CommandSpec, SpecRegistry};
use twitty_render::ui_style::UiStyle;

/// Semantic token category for syntax highlighting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// Executable found on `$PATH`, in specs, or an existing explicit path.
    CommandKnown,
    /// Shell builtin or control-flow keyword (`cd`, `echo`, `export`, `if`, ...).
    CommandBuiltin,
    /// Command word not found on `$PATH`, not a builtin, and not a known local alias/function.
    CommandUnknown,
    /// Recognized subcommand of the active command (including nested subcommands).
    Subcommand,
    /// Short or long option flag (`-m`, `--release`).
    Option,
    /// Single-quoted, double-quoted, or `$'...'` string literal.
    String,
    /// Variable expansion (`$VAR`, `${VAR}`, `$?`) or inline `VAR=` assignment name.
    Variable,
    /// Argument path that exists on disk.
    ExistingPath,
    /// Pipe, boolean list, separator, or redirection operator (`|`, `&&`, `||`, `;`, `>`, `<`).
    Operator,
    /// Shell comment (`# ...`).
    Comment,
    /// Ordinary argument text.
    Plain,
}

impl TokenKind {
    /// Resolve the foreground color for this token kind from the active theme.
    pub fn color(self, palette: &Palette, style: UiStyle) -> Rgba {
        match self {
            Self::CommandKnown => palette.ansi[2],
            Self::CommandBuiltin => palette.ansi[4],
            Self::CommandUnknown => palette.ansi[1],
            Self::Subcommand => palette.ansi[6],
            Self::Option => palette.ansi[4].mix(style.text_dim, 0.35),
            Self::String => palette.ansi[3],
            Self::Variable => palette.ansi[5],
            Self::ExistingPath => palette.ansi[6],
            Self::Operator => style.accent,
            Self::Comment => style.text_faint,
            Self::Plain => style.text,
        }
    }

    /// True when this token should render with a subtle underline (existing paths).
    pub fn underline(self) -> bool {
        matches!(self, Self::ExistingPath)
    }

    /// True when this token should render italic (comments).
    pub fn italic(self) -> bool {
        matches!(self, Self::Comment)
    }
}

/// One highlighted byte range `(start..end)` in the composer buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    pub start: usize,
    pub end: usize,
    pub kind: TokenKind,
}

/// Resolution result for a command word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandStatus {
    Builtin,
    Known,
    Unknown,
    /// `$PATH` has not finished scanning yet, so unknown commands are not flagged as errors.
    Unresolved,
}

/// Pure lookup context used by [`highlight_command`] and [`crate::composer::lint::lint_command`].
pub struct HighlightContext<'a> {
    pub specs: &'a SpecRegistry,
    /// Cached executable names on `$PATH`, or `None` while the initial off-thread scan is running.
    pub path_commands: Option<&'a HashSet<String>>,
    /// Predicate checking whether a shell path token exists (resolved relative to the pane's cwd).
    pub path_exists: &'a dyn Fn(&str) -> bool,
}

impl<'a> HighlightContext<'a> {
    pub fn with_defaults(
        path_commands: Option<&'a HashSet<String>>,
        path_exists: &'a dyn Fn(&str) -> bool,
    ) -> Self {
        Self {
            specs: default_registry(),
            path_commands,
            path_exists,
        }
    }

    /// Classify a command word, considering builtins, local buffer definitions, explicit paths,
    /// and `$PATH` executables.
    pub fn classify_command(&self, word: &str, local_defs: &HashSet<String>) -> CommandStatus {
        if word.is_empty() {
            return CommandStatus::Unresolved;
        }
        if is_shell_builtin(word) {
            return CommandStatus::Builtin;
        }
        if local_defs.contains(word) {
            return CommandStatus::Known;
        }
        if word.starts_with('$') {
            return CommandStatus::Known;
        }
        // Explicit path command (`./script.sh`, `/usr/bin/env`, `~/bin/tool`).
        if word.contains('/') {
            return if (self.path_exists)(word) {
                CommandStatus::Known
            } else if self.path_commands.is_some() {
                CommandStatus::Unknown
            } else {
                CommandStatus::Unresolved
            };
        }
        match self.path_commands {
            Some(cmds) => {
                if cmds.contains(word) {
                    CommandStatus::Known
                } else {
                    CommandStatus::Unknown
                }
            }
            None => {
                if self.specs.contains(word) {
                    CommandStatus::Known
                } else {
                    CommandStatus::Unresolved
                }
            }
        }
    }
}

/// Shell builtins and reserved keywords recognized across bash, zsh, and fish.
pub fn is_shell_builtin(word: &str) -> bool {
    matches!(
        word,
        "alias"
            | "bg"
            | "bind"
            | "break"
            | "builtin"
            | "case"
            | "cd"
            | "command"
            | "compgen"
            | "complete"
            | "continue"
            | "declare"
            | "dirs"
            | "disown"
            | "do"
            | "done"
            | "echo"
            | "elif"
            | "else"
            | "enable"
            | "end"
            | "esac"
            | "eval"
            | "exec"
            | "exit"
            | "export"
            | "false"
            | "fc"
            | "fg"
            | "fi"
            | "for"
            | "function"
            | "getopts"
            | "hash"
            | "help"
            | "history"
            | "if"
            | "in"
            | "jobs"
            | "kill"
            | "let"
            | "local"
            | "logout"
            | "mapfile"
            | "nohup"
            | "popd"
            | "printf"
            | "pushd"
            | "pwd"
            | "read"
            | "readarray"
            | "readonly"
            | "return"
            | "select"
            | "set"
            | "shift"
            | "shopt"
            | "source"
            | "suspend"
            | "test"
            | "then"
            | "time"
            | "times"
            | "trap"
            | "true"
            | "type"
            | "typeset"
            | "ulimit"
            | "umask"
            | "unalias"
            | "unset"
            | "until"
            | "wait"
            | "while"
            | "."
            | ":"
            | "["
            | "[["
            | "]"
            | "]]"
            | "{"
            | "}"
            | "!"
    )
}

/// Collect function and alias names defined earlier inside `text` (e.g. `foo() {`, `function foo`,
/// `alias ll=...`) so neither the highlighter nor the linter flags them as unknown commands.
pub fn collect_local_definitions(text: &str) -> HashSet<String> {
    let mut defs = HashSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("alias ") {
            if let Some((name, _)) = rest.trim_start().split_once('=') {
                let name = name.trim();
                if !name.is_empty() && !name.contains(char::is_whitespace) {
                    defs.insert(name.to_owned());
                }
            }
        }
        if let Some(rest) = trimmed.strip_prefix("function ") {
            let name = rest
                .trim_start()
                .split(|c: char| c.is_whitespace() || c == '(' || c == '{')
                .next()
                .unwrap_or("");
            if !name.is_empty() {
                defs.insert(name.to_owned());
            }
        }
        if let Some((lhs, _)) = trimmed.split_once("()") {
            let name = lhs.trim();
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                defs.insert(name.to_owned());
            }
        }
    }
    defs
}

/// Raw lexical token emitted by the shell scanner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RawTokenKind {
    Word,
    Operator,
    Comment,
    Newline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawToken {
    pub start: usize,
    pub end: usize,
    pub kind: RawTokenKind,
}

/// Tokenize shell text into words, operators, comments, and newlines, preserving exact byte ranges.
pub fn scan_shell_tokens(text: &str) -> Vec<RawToken> {
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut tokens = Vec::new();
    let mut i = 0;

    while i < n {
        let b = bytes[i];
        if b == b'\n' {
            tokens.push(RawToken {
                start: i,
                end: i + 1,
                kind: RawTokenKind::Newline,
            });
            i += 1;
            continue;
        }
        if b == b' ' || b == b'\t' || b == b'\r' {
            i += 1;
            continue;
        }
        // Comment starts at `#` when at start of a token.
        if b == b'#' {
            let start = i;
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            tokens.push(RawToken {
                start,
                end: i,
                kind: RawTokenKind::Comment,
            });
            continue;
        }
        // Multi-char and single-char shell operators and redirections.
        if let Some(op_len) = match_operator(bytes, i) {
            tokens.push(RawToken {
                start: i,
                end: i + op_len,
                kind: RawTokenKind::Operator,
            });
            i += op_len;
            continue;
        }
        // Scan a word (respecting quotes and backslash escapes).
        let start = i;
        let mut quote: Option<u8> = None;
        let mut escaped = false;
        while i < n {
            let ch = bytes[i];
            if escaped {
                escaped = false;
                i += next_char_len(text, i);
                continue;
            }
            if let Some(q) = quote {
                if ch == b'\\' && q == b'"' {
                    escaped = true;
                    i += 1;
                } else if ch == q {
                    quote = None;
                    i += 1;
                } else {
                    i += next_char_len(text, i);
                }
                continue;
            }
            if ch == b'\\' {
                escaped = true;
                i += 1;
                continue;
            }
            if ch == b'\'' || ch == b'"' || ch == b'`' {
                quote = Some(ch);
                i += 1;
                continue;
            }
            if ch == b' ' || ch == b'\t' || ch == b'\r' || ch == b'\n' {
                break;
            }
            if match_operator(bytes, i).is_some() {
                break;
            }
            i += next_char_len(text, i);
        }
        if i > start {
            tokens.push(RawToken {
                start,
                end: i,
                kind: RawTokenKind::Word,
            });
        }
    }
    tokens
}

fn next_char_len(s: &str, idx: usize) -> usize {
    s[idx..].chars().next().map_or(1, char::len_utf8)
}

/// Match a shell operator or redirection starting at `bytes[i]`.
pub fn match_operator(bytes: &[u8], i: usize) -> Option<usize> {
    let rem = &bytes[i..];
    // File-descriptor redirections like `2>&1`, `2>>`, `2>`, `1>&2`, `1>`, `0<`.
    if rem.len() >= 2 && rem[0].is_ascii_digit() && matches!(rem[1], b'>' | b'<') {
        if rem.len() >= 4 && &rem[1..3] == b">&" && rem[3].is_ascii_digit() {
            return Some(4);
        }
        if rem.len() >= 3 && rem[1] == b'>' && rem[2] == b'>' {
            return Some(3);
        }
        return Some(2);
    }
    if rem.starts_with(b"<<<") || rem.starts_with(b"&>>") {
        return Some(3);
    }
    if rem.starts_with(b"&&")
        || rem.starts_with(b"||")
        || rem.starts_with(b"|&")
        || rem.starts_with(b">>")
        || rem.starts_with(b"<<")
        || rem.starts_with(b"&>")
        || rem.starts_with(b"<>")
        || rem.starts_with(b">&")
        || rem.starts_with(b"<&")
        || rem.starts_with(b";;")
    {
        return Some(2);
    }
    if matches!(rem[0], b'|' | b';' | b'&' | b'>' | b'<') {
        return Some(1);
    }
    None
}

/// True if an operator starts a new command position (as opposed to a redirection).
pub fn operator_starts_command(op: &str) -> bool {
    matches!(op, "|" | "|&" | "&&" | "||" | ";" | "&" | ";;")
}

/// True if an operator is a redirection whose next token is a file path, not a command.
pub fn operator_is_redirect(op: &str) -> bool {
    op.contains('>') || op.contains('<')
}

/// True if `word` is a shell `NAME=value` assignment prefix (before a command).
pub fn is_env_assignment(word: &str) -> Option<usize> {
    let eq = word.find('=')?;
    if eq == 0 {
        return None;
    }
    let name = &word[..eq];
    let mut chars = name.chars();
    let first = chars.next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    if chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Some(eq)
    } else {
        None
    }
}

/// Highlight the entire composer buffer into sorted, non-overlapping [`HighlightSpan`]s.
pub fn highlight_command(text: &str, ctx: &HighlightContext<'_>) -> Vec<HighlightSpan> {
    let tokens = scan_shell_tokens(text);
    let local_defs = collect_local_definitions(text);
    let mut spans = Vec::new();

    let mut expect_command = true;
    let mut expect_redirect_target = false;
    let mut expect_opt_value = false;
    let mut active_spec: Option<&CommandSpec> = None;
    let mut delegated_wrapper = false;

    for tok in &tokens {
        let slice = &text[tok.start..tok.end];
        match tok.kind {
            RawTokenKind::Newline => {
                expect_command = true;
                expect_redirect_target = false;
                expect_opt_value = false;
                active_spec = None;
                delegated_wrapper = false;
            }
            RawTokenKind::Comment => {
                spans.push(HighlightSpan {
                    start: tok.start,
                    end: tok.end,
                    kind: TokenKind::Comment,
                });
            }
            RawTokenKind::Operator => {
                spans.push(HighlightSpan {
                    start: tok.start,
                    end: tok.end,
                    kind: TokenKind::Operator,
                });
                if operator_starts_command(slice) {
                    expect_command = true;
                    expect_redirect_target = false;
                    expect_opt_value = false;
                    active_spec = None;
                    delegated_wrapper = false;
                } else if operator_is_redirect(slice) {
                    expect_redirect_target = true;
                }
            }
            RawTokenKind::Word => {
                if expect_redirect_target {
                    expect_redirect_target = false;
                    push_argument_spans(text, tok.start, tok.end, ctx, &mut spans);
                    continue;
                }

                if expect_command {
                    // Inline `VAR=value` before command.
                    if let Some(eq) = is_env_assignment(slice) {
                        spans.push(HighlightSpan {
                            start: tok.start,
                            end: tok.start + eq,
                            kind: TokenKind::Variable,
                        });
                        spans.push(HighlightSpan {
                            start: tok.start + eq,
                            end: tok.start + eq + 1,
                            kind: TokenKind::Operator,
                        });
                        if tok.start + eq + 1 < tok.end {
                            push_argument_spans(text, tok.start + eq + 1, tok.end, ctx, &mut spans);
                        }
                        continue;
                    }

                    // Strip leading grouping punctuation (`(`, `!`) when checking command word.
                    let unquoted = strip_outer_quotes(slice);
                    let status = ctx.classify_command(unquoted, &local_defs);
                    let kind = match status {
                        CommandStatus::Builtin => TokenKind::CommandBuiltin,
                        CommandStatus::Known => TokenKind::CommandKnown,
                        CommandStatus::Unknown => TokenKind::CommandUnknown,
                        CommandStatus::Unresolved => TokenKind::Plain,
                    };
                    spans.push(HighlightSpan {
                        start: tok.start,
                        end: tok.end,
                        kind,
                    });

                    // Keywords that precede another command (`then`, `do`, `else`, `elif`, `!`, `time`, `nohup`, `command`, `builtin`, `exec`, `{`).
                    if matches!(
                        unquoted,
                        "then"
                            | "do"
                            | "else"
                            | "elif"
                            | "!"
                            | "time"
                            | "nohup"
                            | "command"
                            | "builtin"
                            | "exec"
                            | "{"
                    ) {
                        expect_command = true;
                        active_spec = None;
                        delegated_wrapper = false;
                    } else {
                        active_spec = ctx.specs.get(unquoted);
                        delegated_wrapper = active_spec.is_some_and(CommandSpec::delegates_command);
                        expect_command = false;
                    }
                    expect_opt_value = false;
                    continue;
                }

                // Inside arguments of a command.
                if expect_opt_value {
                    expect_opt_value = false;
                    push_argument_spans(text, tok.start, tok.end, ctx, &mut spans);
                    continue;
                }

                if slice == "--" {
                    spans.push(HighlightSpan {
                        start: tok.start,
                        end: tok.end,
                        kind: TokenKind::Operator,
                    });
                    continue;
                }

                // Option flag (`-x`, `--long`, `--long=value`).
                if slice.starts_with('-') && slice.len() > 1 {
                    if let Some(eq) = slice.find('=') {
                        spans.push(HighlightSpan {
                            start: tok.start,
                            end: tok.start + eq,
                            kind: TokenKind::Option,
                        });
                        spans.push(HighlightSpan {
                            start: tok.start + eq,
                            end: tok.start + eq + 1,
                            kind: TokenKind::Operator,
                        });
                        if tok.start + eq + 1 < tok.end {
                            push_argument_spans(text, tok.start + eq + 1, tok.end, ctx, &mut spans);
                        }
                    } else {
                        spans.push(HighlightSpan {
                            start: tok.start,
                            end: tok.end,
                            kind: TokenKind::Option,
                        });
                        if let Some(spec) = active_spec {
                            if spec.find_option(slice).is_some_and(|o| o.expects_value()) {
                                expect_opt_value = true;
                            }
                        }
                    }
                    continue;
                }

                // Delegated wrapper (`sudo`): first non-option positional token is the inner command!
                if delegated_wrapper {
                    if let Some(eq) = is_env_assignment(slice) {
                        spans.push(HighlightSpan {
                            start: tok.start,
                            end: tok.start + eq,
                            kind: TokenKind::Variable,
                        });
                        spans.push(HighlightSpan {
                            start: tok.start + eq,
                            end: tok.start + eq + 1,
                            kind: TokenKind::Operator,
                        });
                        if tok.start + eq + 1 < tok.end {
                            push_argument_spans(text, tok.start + eq + 1, tok.end, ctx, &mut spans);
                        }
                        continue;
                    }
                    let unquoted = strip_outer_quotes(slice);
                    let status = ctx.classify_command(unquoted, &local_defs);
                    let kind = match status {
                        CommandStatus::Builtin => TokenKind::CommandBuiltin,
                        CommandStatus::Known => TokenKind::CommandKnown,
                        CommandStatus::Unknown => TokenKind::CommandUnknown,
                        CommandStatus::Unresolved => TokenKind::Plain,
                    };
                    spans.push(HighlightSpan {
                        start: tok.start,
                        end: tok.end,
                        kind,
                    });
                    active_spec = ctx.specs.get(unquoted);
                    delegated_wrapper = active_spec.is_some_and(CommandSpec::delegates_command);
                    continue;
                }

                // Subcommand lookup on active spec.
                if let Some(spec) = active_spec {
                    if let Some(sub) = spec.find_subcommand(slice) {
                        spans.push(HighlightSpan {
                            start: tok.start,
                            end: tok.end,
                            kind: TokenKind::Subcommand,
                        });
                        active_spec = Some(sub);
                        continue;
                    }
                }

                // Ordinary argument: string / variable / existing path / plain.
                push_argument_spans(text, tok.start, tok.end, ctx, &mut spans);
            }
        }
    }

    merge_adjacent_spans(spans)
}

/// Highlight an argument slice `text[start..end]`, handling strings, variables, and existing paths
/// without breaking Arabic shaping runs.
fn push_argument_spans(
    text: &str,
    start: usize,
    end: usize,
    ctx: &HighlightContext<'_>,
    out: &mut Vec<HighlightSpan>,
) {
    let slice = &text[start..end];
    if slice.is_empty() {
        return;
    }

    // Whole single-quoted or ANSI-C quoted string: one unbroken String span.
    if slice.starts_with('\'') || slice.starts_with("$'") {
        out.push(HighlightSpan {
            start,
            end,
            kind: TokenKind::String,
        });
        return;
    }

    // Whole double-quoted string: keep Arabic words unbroken inside one String span; highlight
    // standalone `$VAR` / `${VAR}` expansions when they don't touch Arabic letters.
    if slice.starts_with('"') {
        push_double_quoted_spans(text, start, end, out);
        return;
    }

    // Check if the entire unquoted token is an existing filesystem path.
    if !slice.starts_with('$') && (ctx.path_exists)(slice) {
        out.push(HighlightSpan {
            start,
            end,
            kind: TokenKind::ExistingPath,
        });
        return;
    }

    // Scan mixed unquoted argument for embedded quotes and `$VAR` / `${VAR}`.
    let bytes = text.as_bytes();
    let mut i = start;
    let mut plain_start = start;

    let flush_plain = |p_start: usize, p_end: usize, out: &mut Vec<HighlightSpan>| {
        if p_end > p_start {
            out.push(HighlightSpan {
                start: p_start,
                end: p_end,
                kind: TokenKind::Plain,
            });
        }
    };

    while i < end {
        let b = bytes[i];
        if b == b'\\' && i + 1 < end {
            i += 1 + next_char_len(text, i + 1);
            continue;
        }
        if b == b'\'' {
            flush_plain(plain_start, i, out);
            let q_start = i;
            i += 1;
            while i < end && bytes[i] != b'\'' {
                i += next_char_len(text, i);
            }
            if i < end {
                i += 1;
            }
            out.push(HighlightSpan {
                start: q_start,
                end: i,
                kind: TokenKind::String,
            });
            plain_start = i;
            continue;
        }
        if b == b'"' {
            flush_plain(plain_start, i, out);
            let q_start = i;
            i += 1;
            let mut esc = false;
            while i < end {
                if esc {
                    esc = false;
                    i += next_char_len(text, i);
                } else if bytes[i] == b'\\' {
                    esc = true;
                    i += 1;
                } else if bytes[i] == b'"' {
                    i += 1;
                    break;
                } else {
                    i += next_char_len(text, i);
                }
            }
            push_double_quoted_spans(text, q_start, i, out);
            plain_start = i;
            continue;
        }
        if b == b'$' {
            if let Some(var_end) = scan_variable_end(text, i, end) {
                flush_plain(plain_start, i, out);
                out.push(HighlightSpan {
                    start: i,
                    end: var_end,
                    kind: TokenKind::Variable,
                });
                i = var_end;
                plain_start = i;
                continue;
            }
        }
        i += next_char_len(text, i);
    }
    flush_plain(plain_start, end, out);
}

/// Emit highlight spans for a double-quoted string `text[start..end]`.
/// Keeps Arabic words in a single `String` span; highlights `$VAR` / `${VAR}` only when not
/// directly glued to an RTL character inside the same whitespace-delimited word.
fn push_double_quoted_spans(text: &str, start: usize, end: usize, out: &mut Vec<HighlightSpan>) {
    let bytes = text.as_bytes();
    let mut seg_start = start;
    let mut i = start;
    let mut escaped = false;

    while i < end {
        let b = bytes[i];
        if escaped {
            escaped = false;
            i += next_char_len(text, i);
            continue;
        }
        if b == b'\\' {
            escaped = true;
            i += 1;
            continue;
        }
        if b == b'$' {
            if let Some(var_end) = scan_variable_end(text, i, end) {
                if !touches_rtl_in_word(text, start, end, i, var_end) {
                    if i > seg_start {
                        out.push(HighlightSpan {
                            start: seg_start,
                            end: i,
                            kind: TokenKind::String,
                        });
                    }
                    out.push(HighlightSpan {
                        start: i,
                        end: var_end,
                        kind: TokenKind::Variable,
                    });
                    i = var_end;
                    seg_start = i;
                    continue;
                }
            }
        }
        i += next_char_len(text, i);
    }
    if end > seg_start {
        out.push(HighlightSpan {
            start: seg_start,
            end,
            kind: TokenKind::String,
        });
    }
}

/// True if the whitespace-delimited word containing `var_start..var_end` inside `bound_start..bound_end`
/// contains an RTL character (splitting colors inside an RTL word would break cosmic-text ordering).
fn touches_rtl_in_word(
    text: &str,
    bound_start: usize,
    bound_end: usize,
    var_start: usize,
    var_end: usize,
) -> bool {
    let before = text[bound_start..var_start]
        .rsplit(|c: char| c.is_whitespace() || c == '"')
        .next()
        .unwrap_or("");
    let after = text[var_end..bound_end]
        .split(|c: char| c.is_whitespace() || c == '"')
        .next()
        .unwrap_or("");
    before.chars().any(is_rtl) || after.chars().any(is_rtl)
}

fn is_rtl(c: char) -> bool {
    matches!(bidi_class(c), BidiClass::R | BidiClass::AL)
}

/// If `text[start..limit]` starts with `$VAR`, `${...}`, or special `$?`/`$1`/etc., return end byte.
fn scan_variable_end(text: &str, start: usize, limit: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if start + 1 >= limit || bytes[start] != b'$' {
        return None;
    }
    let next = bytes[start + 1];
    if next == b'{' {
        let mut i = start + 2;
        while i < limit && bytes[i] != b'}' {
            i += next_char_len(text, i);
        }
        let end = if i < limit { i + 1 } else { limit };
        return Some(end);
    }
    if next.is_ascii_alphabetic() || next == b'_' {
        let mut i = start + 2;
        while i < limit && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
            i += 1;
        }
        return Some(i);
    }
    if next.is_ascii_digit() || matches!(next, b'?' | b'$' | b'!' | b'@' | b'*' | b'#' | b'-') {
        return Some(start + 2);
    }
    None
}

fn strip_outer_quotes(s: &str) -> &str {
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn merge_adjacent_spans(spans: Vec<HighlightSpan>) -> Vec<HighlightSpan> {
    let mut out: Vec<HighlightSpan> = Vec::with_capacity(spans.len());
    for span in spans {
        if span.end <= span.start {
            continue;
        }
        if let Some(last) = out.last_mut() {
            if last.kind == span.kind && last.end == span.start {
                last.end = span.end;
                continue;
            }
        }
        out.push(span);
    }
    out
}

/// Project full-buffer [`HighlightSpan`]s onto a single line `[line_start, line_end)` with
/// line-relative byte offsets.
pub fn line_highlight_spans(
    spans: &[HighlightSpan],
    line_start: usize,
    line_end: usize,
) -> Vec<HighlightSpan> {
    let mut out = Vec::new();
    for &span in spans {
        if span.end <= line_start {
            continue;
        }
        if span.start >= line_end {
            break;
        }
        let s = span.start.max(line_start) - line_start;
        let e = span.end.min(line_end) - line_start;
        if e > s {
            out.push(HighlightSpan {
                start: s,
                end: e,
                kind: span.kind,
            });
        }
    }
    out
}

/// Resolve a shell path token against `cwd` and `$HOME` (for `~` expansion), returning `None`
/// if it cannot be resolved to a local path.
pub fn resolve_shell_path(token: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    let unquoted = strip_outer_quotes(token);
    if unquoted.is_empty() || unquoted.starts_with('-') || unquoted.starts_with('$') {
        return None;
    }
    if unquoted == "~" {
        return std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
    }
    if let Some(rest) = unquoted.strip_prefix("~/") {
        let home = std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)?;
        return Some(home.join(rest));
    }
    if unquoted.starts_with('/') {
        return Some(PathBuf::from(unquoted));
    }
    Some(cwd?.join(unquoted))
}

/// Extract candidate filesystem paths referenced in `text` (bounded) so the app can stat them
/// off the UI thread for [`TokenKind::ExistingPath`] highlighting and explicit-path linting.
pub fn extract_candidate_paths(text: &str, cwd: Option<&Path>) -> Vec<PathBuf> {
    const MAX_CANDIDATES: usize = 32;
    let tokens = scan_shell_tokens(text);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for tok in tokens {
        if tok.kind != RawTokenKind::Word {
            continue;
        }
        let raw = &text[tok.start..tok.end];
        let candidates: [&str; 2] = if let Some((_, rhs)) = raw.split_once('=') {
            [raw, rhs]
        } else {
            [raw, ""]
        };
        for cand in candidates {
            if cand.is_empty() {
                continue;
            }
            if let Some(path) = resolve_shell_path(cand, cwd) {
                if seen.insert(path.clone()) {
                    out.push(path);
                    if out.len() >= MAX_CANDIDATES {
                        return out;
                    }
                }
            }
        }
    }
    out
}
