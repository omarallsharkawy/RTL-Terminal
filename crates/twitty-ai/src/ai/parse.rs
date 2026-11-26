//! Robust reply parsing for non-interactive agent responses.
//!
//! Handles fenced code blocks (backtick and tilde fences, shell-tagged or untagged), multiple
//! fenced blocks, unfenced single/multi-line replies, Arabic and English prose around the command,
//! inline backticks, ANSI escape sequences, and empty/invalid replies.

use twitty_core::shell_integration::{clean_text, MAX_COMMAND_CHARS};

/// Maximum length of the short explanation line shown in the composer.
pub const MAX_EXPLANATION_CHARS: usize = 200;

/// Maximum length of the body shown in the block error explanation popover.
pub const MAX_EXPLAIN_BODY_CHARS: usize = 4096;

/// Parsed result of a natural-language-to-command agent reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedCommand {
    /// Exactly one shell command ready for user review in the composer (never auto-run).
    pub command: String,
    /// Optional short explanation line to show alongside the generated command.
    pub explanation: Option<String>,
}

/// Parsed result of an "Explain error" or "Suggest a fix" agent reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExplainReply {
    /// Readable explanation text (cleaned, bounded).
    pub summary: String,
    /// Optional suggested replacement command extracted from a fenced block.
    pub suggested_command: Option<String>,
}

#[derive(Clone, Debug)]
struct FencedBlock {
    info: String,
    content: String,
    start_line: usize,
    end_line: usize,
}

/// Parse an agent reply into exactly one shell command plus an optional short explanation line.
///
/// Returns `None` when the reply is empty or contains no usable command.
pub fn parse_command_reply(raw: &str) -> Option<GeneratedCommand> {
    let cleaned = strip_ansi_and_controls(raw);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lines: Vec<&str> = cleaned.lines().collect();
    let blocks = extract_fenced_blocks(&lines);

    if !blocks.is_empty() {
        let chosen = choose_command_block(&blocks)?;
        let command = normalize_command(&chosen.content)?;
        let explanation = extract_explanation_around(&lines, chosen.start_line, chosen.end_line);
        return Some(GeneratedCommand {
            command,
            explanation,
        });
    }

    // Unfenced reply fallback.
    parse_unfenced_command(&lines)
}

/// Parse an agent reply for explaining/fixing a failed command block.
///
/// Extracts readable prose (cleaned and bounded) plus an optional suggested fix command from a
/// fenced block so the UI can offer "Insert fix into composer".
pub fn parse_explain_reply(raw: &str) -> Option<ExplainReply> {
    let cleaned = strip_ansi_and_controls(raw);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lines: Vec<&str> = cleaned.lines().collect();
    let blocks = extract_fenced_blocks(&lines);
    let suggested_command =
        choose_command_block(&blocks).and_then(|b| normalize_command(&b.content));

    // Build a clean readable summary without raw fence delimiter lines.
    let mut summary_lines = Vec::new();
    let mut in_fence: Option<(char, usize)> = None;
    for &line in &lines {
        let t = line.trim_start();
        if let Some((fc, flen)) = in_fence {
            if is_closing_fence(t, fc, flen) {
                in_fence = None;
                continue;
            }
            summary_lines.push(line.trim_end());
        } else if let Some((fc, flen, _)) = parse_opening_fence(t) {
            in_fence = Some((fc, flen));
        } else {
            summary_lines.push(line.trim_end());
        }
    }

    // Collapse runs of blank lines and bound total size.
    let mut compact = Vec::new();
    let mut last_blank = true;
    for line in summary_lines {
        let blank = line.trim().is_empty();
        if blank {
            if !last_blank {
                compact.push("");
                last_blank = true;
            }
        } else {
            compact.push(line);
            last_blank = false;
        }
    }
    while compact.last().is_some_and(|l| l.is_empty()) {
        compact.pop();
    }

    let mut summary = compact.join("\n");
    if summary.is_empty() {
        summary = suggested_command.clone()?;
    }
    if summary.chars().count() > MAX_EXPLAIN_BODY_CHARS {
        summary = summary
            .chars()
            .take(MAX_EXPLAIN_BODY_CHARS.saturating_sub(1))
            .collect::<String>()
            + "…";
    }

    Some(ExplainReply {
        summary,
        suggested_command,
    })
}

fn extract_fenced_blocks(lines: &[&str]) -> Vec<FencedBlock> {
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if let Some((fence_char, fence_len, info)) = parse_opening_fence(trimmed) {
            let start_line = i;
            i += 1;
            let mut body = Vec::new();
            let mut closed = false;
            while i < lines.len() {
                if is_closing_fence(lines[i].trim_start(), fence_char, fence_len) {
                    closed = true;
                    break;
                }
                body.push(lines[i]);
                i += 1;
            }
            let end_line = if closed {
                i
            } else {
                lines.len().saturating_sub(1)
            };
            blocks.push(FencedBlock {
                info: info.to_ascii_lowercase(),
                content: body.join("\n"),
                start_line,
                end_line,
            });
            i += 1;
        } else {
            i += 1;
        }
    }
    blocks
}

fn parse_opening_fence(line: &str) -> Option<(char, usize, &str)> {
    let first = line.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let count = line.chars().take_while(|&c| c == first).count();
    if count < 3 {
        return None;
    }
    let rest = line[count..].trim();
    // CommonMark: backtick info strings cannot contain backticks (that would be inline code like
    // ```echo hi``` on one line).
    if first == '`' && rest.contains('`') {
        return None;
    }
    let info = rest.split_whitespace().next().unwrap_or("");
    Some((first, count, info))
}

fn is_closing_fence(line: &str, fence_char: char, min_len: usize) -> bool {
    let count = line.chars().take_while(|&c| c == fence_char).count();
    count >= min_len && line[count..].trim().is_empty()
}

fn is_shell_tag(info: &str) -> bool {
    matches!(
        info,
        "sh" | "bash"
            | "zsh"
            | "fish"
            | "shell"
            | "console"
            | "shellsession"
            | "powershell"
            | "pwsh"
            | "cmd"
    )
}

fn choose_command_block(blocks: &[FencedBlock]) -> Option<&FencedBlock> {
    let non_empty: Vec<&FencedBlock> = blocks
        .iter()
        .filter(|b| !b.content.trim().is_empty())
        .collect();
    if non_empty.is_empty() {
        return None;
    }
    // Prefer explicitly shell-tagged blocks, then untagged blocks, then the first non-empty block.
    non_empty
        .iter()
        .copied()
        .find(|b| is_shell_tag(&b.info))
        .or_else(|| non_empty.iter().copied().find(|b| b.info.is_empty()))
        .or_else(|| non_empty.first().copied())
}

fn extract_explanation_around(
    lines: &[&str],
    block_start: usize,
    block_end: usize,
) -> Option<String> {
    // 1. Prefer an explicitly labeled explanation line after or before the block.
    let after = lines.get(block_end + 1..).unwrap_or(&[]);
    let before = lines.get(..block_start).unwrap_or(&[]);

    for slice in [after, before] {
        for &line in slice {
            if let Some(rest) = strip_explanation_label(line) {
                let clean = clean_text(rest, MAX_EXPLANATION_CHARS);
                if !clean.is_empty() {
                    return Some(clean);
                }
            }
        }
    }

    // 2. Otherwise pick the first plain non-fence prose line after the block, else before it.
    for slice in [after, before] {
        let mut in_other_fence: Option<(char, usize)> = None;
        for &line in slice {
            let t = line.trim();
            if let Some((fc, flen)) = in_other_fence {
                if is_closing_fence(t, fc, flen) {
                    in_other_fence = None;
                }
                continue;
            }
            if let Some((fc, flen, _)) = parse_opening_fence(t) {
                in_other_fence = Some((fc, flen));
                continue;
            }
            if t.is_empty() {
                continue;
            }
            let clean = clean_text(t, MAX_EXPLANATION_CHARS);
            if !clean.is_empty() {
                return Some(clean);
            }
        }
    }
    None
}

fn strip_explanation_label(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let lower = trimmed.to_ascii_lowercase();
    for prefix in ["explanation:", "note:", "summary:"] {
        if lower.starts_with(prefix) {
            return Some(trimmed[prefix.len()..].trim());
        }
    }
    for ar_prefix in ["الشرح:", "التوضيح:", "ملاحظة:", "شرح:"] {
        if let Some(rest) = trimmed.strip_prefix(ar_prefix) {
            return Some(rest.trim());
        }
    }
    None
}

fn parse_unfenced_command(lines: &[&str]) -> Option<GeneratedCommand> {
    let non_empty: Vec<&str> = lines
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    if non_empty.is_empty() {
        return None;
    }

    // Single non-empty line: may be bare command, `$ command`, inline `` `command` ``, or prose with inline `` `command` ``.
    if non_empty.len() == 1 {
        let line = non_empty[0];
        if let Some(stripped) = strip_wrapping_backticks(line) {
            let cmd = normalize_command(stripped)?;
            return Some(GeneratedCommand {
                command: cmd,
                explanation: None,
            });
        }
        if let Some((cmd_raw, prose)) = extract_inline_backtick_command(line) {
            if let Some(cmd) = normalize_command(&cmd_raw) {
                let explanation = (!prose.is_empty())
                    .then(|| clean_text(&prose, MAX_EXPLANATION_CHARS))
                    .filter(|s| !s.is_empty());
                return Some(GeneratedCommand {
                    command: cmd,
                    explanation,
                });
            }
        }
        let cmd = normalize_command(line)?;
        return Some(GeneratedCommand {
            command: cmd,
            explanation: None,
        });
    }

    // Multi-line unfenced reply:
    // 1. Check if any line has an explicit inline backtick command (e.g. Arabic explanation + `cmd`).
    for (idx, &line) in non_empty.iter().enumerate() {
        if let Some(stripped) = strip_wrapping_backticks(line) {
            if let Some(cmd) = normalize_command(stripped) {
                let explanation = pick_other_line(&non_empty, idx);
                return Some(GeneratedCommand {
                    command: cmd,
                    explanation,
                });
            }
        }
    }

    // 2. Check if there is an explicit `Explanation:` / `الشرح:` line; take the line before it as command.
    if let Some(exp_idx) = non_empty
        .iter()
        .position(|l| strip_explanation_label(l).is_some())
    {
        if exp_idx > 0 {
            let cmd_text = non_empty[..exp_idx].join("\n");
            if let Some(cmd) = normalize_command(&cmd_text) {
                let explanation = strip_explanation_label(non_empty[exp_idx])
                    .map(|s| clean_text(s, MAX_EXPLANATION_CHARS))
                    .filter(|s| !s.is_empty());
                return Some(GeneratedCommand {
                    command: cmd,
                    explanation,
                });
            }
        }
    }

    // 3. Check if any line contains an inline `` `...` `` command surrounded by prose.
    for (idx, &line) in non_empty.iter().enumerate() {
        if let Some((cmd_raw, prose)) = extract_inline_backtick_command(line) {
            if let Some(cmd) = normalize_command(&cmd_raw) {
                let explanation = if !prose.is_empty() {
                    Some(clean_text(&prose, MAX_EXPLANATION_CHARS)).filter(|s| !s.is_empty())
                } else {
                    pick_other_line(&non_empty, idx)
                };
                return Some(GeneratedCommand {
                    command: cmd,
                    explanation,
                });
            }
        }
    }

    // 4. Fallback: first non-empty line is the command, next non-empty line is the explanation.
    let cmd = normalize_command(non_empty[0])?;
    let explanation = non_empty
        .get(1)
        .map(|l| strip_explanation_label(l).unwrap_or(l))
        .map(|s| clean_text(s, MAX_EXPLANATION_CHARS))
        .filter(|s| !s.is_empty());
    Some(GeneratedCommand {
        command: cmd,
        explanation,
    })
}

fn pick_other_line(lines: &[&str], skip_idx: usize) -> Option<String> {
    for (i, &l) in lines.iter().enumerate() {
        if i == skip_idx {
            continue;
        }
        let s = strip_explanation_label(l).unwrap_or(l);
        let clean = clean_text(s, MAX_EXPLANATION_CHARS);
        if !clean.is_empty() {
            return Some(clean);
        }
    }
    None
}

fn strip_wrapping_backticks(s: &str) -> Option<&str> {
    let s = s.trim();
    if s.starts_with("```") && s.ends_with("```") && s.len() > 6 {
        return Some(s[3..s.len() - 3].trim());
    }
    if s.starts_with('`') && s.ends_with('`') && s.len() > 2 && !s[1..s.len() - 1].contains('`') {
        return Some(s[1..s.len() - 1].trim());
    }
    None
}

fn extract_inline_backtick_command(line: &str) -> Option<(String, String)> {
    let start = line.find('`')?;
    let rest = &line[start + 1..];
    let end_rel = rest.find('`')?;
    let inside = rest[..end_rel].trim();
    if inside.is_empty() {
        return None;
    }
    let before = line[..start].trim();
    let after = rest[end_rel + 1..].trim();
    let prose = match (before.is_empty(), after.is_empty()) {
        (true, true) => String::new(),
        (false, true) => before.trim_end_matches(':').trim().to_string(),
        (true, false) => after.to_string(),
        (false, false) => format!("{before} {after}"),
    };
    Some((inside.to_string(), prose))
}

fn normalize_command(raw: &str) -> Option<String> {
    let mut lines: Vec<&str> = raw.lines().map(str::trim_end).collect();
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        return None;
    }
    if lines.len() == 1 {
        let line = lines[0].trim();
        let line = strip_shell_prompt_prefix(line);
        let bounded = bound_multiline(line, MAX_COMMAND_CHARS);
        return (!bounded.trim().is_empty()).then_some(bounded);
    }
    // Keep multi-line shell commands intact, stripping leading blank/prompt prefix only if first line has `$ `.
    let mut joined = String::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx > 0 {
            joined.push('\n');
        }
        if idx == 0 {
            joined.push_str(strip_shell_prompt_prefix(line.trim_start()));
        } else {
            joined.push_str(line);
        }
    }
    let bounded = bound_multiline(joined.trim(), MAX_COMMAND_CHARS);
    (!bounded.trim().is_empty()).then_some(bounded)
}

fn strip_shell_prompt_prefix(line: &str) -> &str {
    // Strip "$ cmd" (with whitespace after '$'), never touching "$VAR=1".
    if let Some(rest) = line.strip_prefix("$ ") {
        return rest.trim_start();
    }
    line
}

fn bound_multiline(s: &str, max_chars: usize) -> String {
    s.chars()
        .filter(|&c| {
            (c == '\n' || c == '\t' || !c.is_control())
                && !twitty_core::terminal::is_bidi_control(c)
                && !matches!(c, '\u{2028}' | '\u{2029}')
        })
        .take(max_chars)
        .collect()
}

/// Remove ANSI CSI/OSC escape sequences, bidi override controls, and non-whitespace control chars.
pub fn strip_ansi_and_controls(input: &str) -> String {
    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(normalized.len());
    let mut chars = normalized.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    // CSI: parameter bytes 0x30-0x3F, intermediates 0x20-0x2F, final 0x40-0x7E.
                    for ch in chars.by_ref() {
                        if ('@'..='~').contains(&ch) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    // OSC: terminated by BEL or ST (ESC \).
                    let mut prev_esc = false;
                    for ch in chars.by_ref() {
                        if ch == '\x07' || (prev_esc && ch == '\\') {
                            break;
                        }
                        prev_esc = ch == '\x1b';
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
            continue;
        }
        if (c == '\n' || c == '\t' || !c.is_control())
            && !twitty_core::terminal::is_bidi_control(c)
            && !matches!(c, '\u{2028}' | '\u{2029}')
        {
            out.push(c);
        }
    }
    out
}
