//! Pre-run static linter for the command composer.
//!
//! Marks likely problems with a wavy underline and a bilingual tooltip line without ever blocking
//! submission:
//! - Unknown commands (not on `$PATH`, not a shell builtin, not an existing explicit path or
//!   locally defined alias/function).
//! - Unclosed quotes (`'`, `"`, `` ` ``) and brackets (`(`, `[`, `{`), plus unmatched closing
//!   brackets.
//! - Trailing pipe or boolean operators (`|`, `|&`, `&&`, `||`).

use crate::composer::highlight::{
    collect_local_definitions, is_env_assignment, operator_is_redirect, operator_starts_command,
    scan_shell_tokens, CommandStatus, HighlightContext, RawTokenKind,
};
use crate::composer::specs::CommandSpec;
use crate::ui::chrome::UiLanguage;
use twitty_core::language::fill;

/// Severity of a composer lint mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LintSeverity {
    Warning,
    Error,
}

/// Category of pre-run problem detected in the composer buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LintKind {
    UnknownCommand(String),
    UnclosedQuote(char),
    UnclosedBracket(char),
    UnexpectedClose(char),
    TrailingOperator(String),
}

/// One diagnostic mark covering byte range `(start..end)` in the composer buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LintMark {
    pub start: usize,
    pub end: usize,
    pub severity: LintSeverity,
    pub kind: LintKind,
}

impl LintMark {
    /// The mark's message in `language`.
    pub fn message_in(&self, language: UiLanguage) -> String {
        match &self.kind {
            LintKind::UnknownCommand(cmd) => fill(language.t("Unknown command: `{}`"), &[&cmd]),
            LintKind::UnclosedQuote('\'') => language.t("Unclosed single quote (')").into(),
            LintKind::UnclosedQuote('"') => language.t("Unclosed double quote (\")").into(),
            LintKind::UnclosedQuote('`') => language.t("Unclosed backtick (`)").into(),
            LintKind::UnclosedQuote(q) => fill(language.t("Unclosed quote ({})"), &[&q]),
            LintKind::UnclosedBracket(c) => fill(language.t("Unclosed bracket `{}`"), &[&c]),
            LintKind::UnexpectedClose(c) => fill(language.t("Unexpected closing `{}`"), &[&c]),
            LintKind::TrailingOperator(op) => {
                fill(language.t("Trailing `{}` expects another command"), &[&op])
            }
        }
    }

    pub fn message_en(&self) -> String {
        self.message_in(UiLanguage::English)
    }

    pub fn message_ar(&self) -> String {
        self.message_in(UiLanguage::Arabic)
    }

    pub fn message(&self, lang: UiLanguage) -> String {
        match lang {
            UiLanguage::Arabic => self.message_ar(),
            UiLanguage::English => self.message_en(),
        }
    }
}

/// Analyze `text` for pre-run problems and return sorted [`LintMark`]s.
/// Never prevents execution; callers render underlines and tooltip messages only.
pub fn lint_command(text: &str, ctx: &HighlightContext<'_>) -> Vec<LintMark> {
    if text.trim().is_empty() {
        return Vec::new();
    }

    let mut marks = Vec::new();
    check_quotes_and_brackets(text, &mut marks);
    check_commands_and_operators(text, ctx, &mut marks);

    marks.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.end.cmp(&b.end)));
    marks
}

/// Pick the best lint mark to display in the composer tooltip line for `caret_byte`:
/// prefers a mark containing or touching the caret, otherwise the first mark.
pub fn active_lint_for_caret(marks: &[LintMark], caret_byte: usize) -> Option<&LintMark> {
    marks
        .iter()
        .find(|m| caret_byte >= m.start && caret_byte <= m.end)
        .or_else(|| marks.first())
}

fn check_quotes_and_brackets(text: &str, marks: &mut Vec<LintMark>) {
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut i = 0;

    // Active quote `(quote_char, byte_offset)` when inside quotes.
    let mut open_quote: Option<(char, usize)> = None;
    // Stack of open brackets `(open_char, byte_offset)`.
    let mut brackets: Vec<(char, usize)> = Vec::new();
    // Track whether we saw `case` so pattern-arm `)` is not misreported as an unmatched paren.
    let has_case = text.split_whitespace().any(|w| w == "case");

    let mut at_token_start = true;
    let mut escaped = false;

    while i < n {
        let b = bytes[i];

        if escaped {
            escaped = false;
            at_token_start = false;
            i += next_char_len(text, i);
            continue;
        }

        if let Some((q, q_start)) = open_quote {
            if b == b'\\' && (q == '"' || q == '`') {
                escaped = true;
                i += 1;
                continue;
            }
            // Track `${...}` and `$(...)` inside double quotes.
            if q == '"' && b == b'$' && i + 1 < n {
                if bytes[i + 1] == b'{' {
                    brackets.push(('{', i + 1));
                    i += 2;
                    continue;
                } else if bytes[i + 1] == b'(' {
                    brackets.push(('(', i + 1));
                    i += 2;
                    continue;
                }
            }
            if q == '"' && !brackets.is_empty() {
                if b == b'}' && brackets.last().is_some_and(|&(c, _)| c == '{') {
                    brackets.pop();
                    i += 1;
                    continue;
                }
                if b == b')' && brackets.last().is_some_and(|&(c, _)| c == '(') {
                    brackets.pop();
                    i += 1;
                    continue;
                }
            }
            if b == q as u8 {
                let _ = q_start;
                open_quote = None;
            }
            i += next_char_len(text, i);
            continue;
        }

        // Outside quotes:
        if b == b'\\' {
            escaped = true;
            at_token_start = false;
            i += 1;
            continue;
        }
        if b == b'#' && at_token_start {
            // Skip to end of line.
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            at_token_start = true;
            continue;
        }
        if b == b' ' || b == b'\t' || b == b'\r' || b == b'\n' {
            at_token_start = true;
            i += 1;
            continue;
        }
        if matches!(b, b'|' | b'&' | b';' | b'>' | b'<') {
            at_token_start = true;
            i += 1;
            continue;
        }

        match b {
            b'\'' | b'"' | b'`' => {
                open_quote = Some((b as char, i));
                at_token_start = false;
                i += 1;
            }
            b'(' | b'[' | b'{' => {
                brackets.push((b as char, i));
                at_token_start = true;
                i += 1;
            }
            b')' | b']' | b'}' => {
                let expected_open = match b {
                    b')' => '(',
                    b']' => '[',
                    _ => '{',
                };
                if brackets.last().is_some_and(|&(c, _)| c == expected_open) {
                    brackets.pop();
                } else if !(b == b')' && has_case) {
                    marks.push(LintMark {
                        start: i,
                        end: i + 1,
                        severity: LintSeverity::Error,
                        kind: LintKind::UnexpectedClose(b as char),
                    });
                }
                at_token_start = false;
                i += 1;
            }
            _ => {
                at_token_start = false;
                i += next_char_len(text, i);
            }
        }
    }

    if let Some((q, start)) = open_quote {
        // Span from opening quote to end of its line (or end of buffer) so the underline is clear.
        let line_end = text[start..].find('\n').map_or(n, |off| start + off);
        marks.push(LintMark {
            start,
            end: line_end.max(start + 1).min(n),
            severity: LintSeverity::Error,
            kind: LintKind::UnclosedQuote(q),
        });
    }

    for (open_ch, start) in brackets {
        marks.push(LintMark {
            start,
            end: (start + 1).min(n),
            severity: LintSeverity::Error,
            kind: LintKind::UnclosedBracket(open_ch),
        });
    }
}

fn check_commands_and_operators(text: &str, ctx: &HighlightContext<'_>, marks: &mut Vec<LintMark>) {
    let tokens = scan_shell_tokens(text);
    let local_defs = collect_local_definitions(text);

    // 1. Check trailing pipe / boolean operator (`|`, `|&`, `&&`, `||`), ignoring trailing
    //    comments and newlines.
    if let Some(last) = tokens
        .iter()
        .rev()
        .find(|t| !matches!(t.kind, RawTokenKind::Comment | RawTokenKind::Newline))
    {
        if last.kind == RawTokenKind::Operator {
            let op = &text[last.start..last.end];
            if matches!(op, "|" | "|&" | "&&" | "||") {
                marks.push(LintMark {
                    start: last.start,
                    end: last.end,
                    severity: LintSeverity::Error,
                    kind: LintKind::TrailingOperator(op.to_owned()),
                });
            }
        }
    }

    // 2. Check unknown commands (only when `$PATH` scan is available).
    if ctx.path_commands.is_none() {
        return;
    }

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
            RawTokenKind::Comment => {}
            RawTokenKind::Operator => {
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
                    continue;
                }

                if expect_command {
                    if is_env_assignment(slice).is_some() {
                        continue;
                    }
                    // Function definition `foo() {` is a declaration, not an invocation;
                    // keep `expect_command = true` so the following `{` starts the body cleanly.
                    if slice.ends_with("()") {
                        continue;
                    }
                    // Strip leading `(` / `{` / `!` if attached to the command token.
                    let (cmd_word, cmd_start, cmd_end) = trim_command_wrappers(slice, tok.start);
                    if cmd_word.is_empty() {
                        continue;
                    }
                    if matches!(
                        cmd_word,
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
                        continue;
                    }
                    if ctx.classify_command(cmd_word, &local_defs) == CommandStatus::Unknown {
                        marks.push(LintMark {
                            start: cmd_start,
                            end: cmd_end,
                            severity: LintSeverity::Warning,
                            kind: LintKind::UnknownCommand(cmd_word.to_owned()),
                        });
                    }
                    active_spec = ctx.specs.get(cmd_word);
                    delegated_wrapper = active_spec.is_some_and(CommandSpec::delegates_command);
                    expect_command = false;
                    expect_opt_value = false;
                    continue;
                }

                if expect_opt_value {
                    expect_opt_value = false;
                    continue;
                }

                if slice == "--" {
                    continue;
                }

                if slice.starts_with('-') && slice.len() > 1 {
                    if !slice.contains('=') {
                        if let Some(spec) = active_spec {
                            if spec.find_option(slice).is_some_and(|o| o.expects_value()) {
                                expect_opt_value = true;
                            }
                        }
                    }
                    continue;
                }

                if delegated_wrapper {
                    if is_env_assignment(slice).is_some() {
                        continue;
                    }
                    let (cmd_word, cmd_start, cmd_end) = trim_command_wrappers(slice, tok.start);
                    if !cmd_word.is_empty()
                        && ctx.classify_command(cmd_word, &local_defs) == CommandStatus::Unknown
                    {
                        marks.push(LintMark {
                            start: cmd_start,
                            end: cmd_end,
                            severity: LintSeverity::Warning,
                            kind: LintKind::UnknownCommand(cmd_word.to_owned()),
                        });
                    }
                    active_spec = ctx.specs.get(cmd_word);
                    delegated_wrapper = active_spec.is_some_and(CommandSpec::delegates_command);
                    continue;
                }
            }
        }
    }
}

fn trim_command_wrappers(slice: &str, base: usize) -> (&str, usize, usize) {
    let mut s = slice;
    let mut start = base;
    let mut end = base + slice.len();
    while let Some(rest) = s.strip_prefix('(') {
        s = rest;
        start += 1;
    }
    while let Some(rest) = s.strip_suffix(')') {
        s = rest;
        end = end.saturating_sub(1);
    }
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        s = &s[1..s.len() - 1];
        start += 1;
        end = end.saturating_sub(1);
    }
    (s, start, end)
}

fn next_char_len(s: &str, idx: usize) -> usize {
    s[idx..].chars().next().map_or(1, char::len_utf8)
}
