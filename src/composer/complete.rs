//! Command completion and inline ghost suggestions for the composer:
//! history prefix matches, workflow commands, and file paths relative to the pane's cwd, with
//! Arabic-aware matching (`ui::palette::fold`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::ui::palette::fold;
use crate::workflows::Workflow;

/// Maximum completion candidates returned for the popup list.
pub const MAX_COMPLETIONS: usize = 32;
/// Maximum directory entries read when completing a path.
pub const MAX_DIR_ENTRIES: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    History,
    Workflow,
    Path,
}

impl CompletionKind {
    pub fn badge_en(self) -> &'static str {
        match self {
            Self::History => "history",
            Self::Workflow => "workflow",
            Self::Path => "path",
        }
    }

    pub fn badge_ar(self) -> &'static str {
        match self {
            Self::History => "سجل",
            Self::Workflow => "أمر",
            Self::Path => "مسار",
        }
    }
}

/// One completion candidate shown in the composer popup or applied on Tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub kind: CompletionKind,
    /// Primary text shown in the popup list.
    pub label: String,
    /// Optional secondary detail (e.g. workflow bilingual title).
    pub detail: Option<String>,
    /// Text to splice into `replace_range`.
    pub replacement: String,
    /// Byte range `(start, end)` in the editor buffer replaced when accepted.
    pub replace_range: (usize, usize),
    /// Ranking score (lower is better).
    pub score: usize,
}

/// Lightweight directory entry snapshot produced off the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntryInfo {
    pub name: String,
    pub is_dir: bool,
}

/// Read up to `limit` entries of `dir`, sorted with directories first then case-insensitive name.
/// Safe to call off the UI thread.
pub fn read_dir_entries(dir: &Path, limit: usize) -> Vec<DirEntryInfo> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for item in rd.flatten() {
        let Some(name) = item.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let is_dir = item.file_type().is_ok_and(|ft| ft.is_dir());
        entries.push(DirEntryInfo { name, is_dir });
        if entries.len() >= limit {
            break;
        }
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    entries
}

/// Parsed path token at the caret, used both to trigger off-thread directory loading and to
/// filter/format path completions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathQuery {
    /// Directory on disk to list.
    pub dir_to_read: PathBuf,
    /// Prefix typed before the filename fragment (e.g. `"src/"`, `"./"`, `"~/"`, or `""`).
    pub typed_dir: String,
    /// Partial filename being completed.
    pub name_prefix: String,
    /// Byte range of the token in the editor buffer.
    pub replace_range: (usize, usize),
    /// True when the token explicitly looks like a path (contains `/`, starts with `.` or `~`)
    /// or sits after an initial command token (argument position).
    pub path_context: bool,
    /// Quote character the token started with, if any (`'` or `"`).
    pub quote: Option<char>,
}

/// Extract the path completion context at `caret` in `text`, relative to `cwd`.
pub fn extract_path_query(text: &str, caret: usize, cwd: Option<&Path>) -> Option<PathQuery> {
    let caret = caret.min(text.len());
    let before = &text[..caret];
    // Stay on the current line.
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let line_before = &text[line_start..caret];

    // Find start of current shell token (respecting simple quotes).
    let mut token_start_in_line = 0;
    let mut in_quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in line_before.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' && in_quote != Some('\'') {
            escaped = true;
            continue;
        }
        match in_quote {
            Some(q) if c == q => in_quote = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => in_quote = Some(c),
            None if c.is_whitespace() => token_start_in_line = i + c.len_utf8(),
            None => {}
        }
    }

    let raw_token = &line_before[token_start_in_line..];
    let replace_start = line_start + token_start_in_line;
    let (quote, unquoted) = match raw_token.chars().next() {
        Some(q @ ('\'' | '"')) => (Some(q), &raw_token[1..]),
        _ => (None, raw_token),
    };

    let has_preceding_arg = !line_before[..token_start_in_line].trim().is_empty();
    let looks_like_path =
        unquoted.contains('/') || unquoted.starts_with('.') || unquoted.starts_with('~');
    let path_context = has_preceding_arg || looks_like_path;

    // Don't read cwd on a completely empty buffer.
    if !path_context && unquoted.is_empty() {
        return None;
    }

    let (typed_dir, name_prefix) = match unquoted.rfind('/') {
        Some(slash) => (&unquoted[..=slash], &unquoted[slash + 1..]),
        None => ("", unquoted),
    };

    let dir_to_read = if typed_dir.starts_with('/') {
        PathBuf::from(typed_dir)
    } else if let Some(rest) = typed_dir.strip_prefix("~/") {
        let home = std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)?;
        if rest.is_empty() {
            home
        } else {
            home.join(rest)
        }
    } else {
        let base = cwd?;
        if typed_dir.is_empty() {
            base.to_path_buf()
        } else {
            base.join(typed_dir)
        }
    };

    Some(PathQuery {
        dir_to_read,
        typed_dir: typed_dir.to_owned(),
        name_prefix: name_prefix.to_owned(),
        replace_range: (replace_start, caret),
        path_context,
        quote,
    })
}

/// If `folded(candidate)` starts with `folded(prefix)`, return the remaining unmatched suffix of
/// `candidate` (on a valid UTF-8 char boundary).
pub fn folded_prefix_suffix<'a>(candidate: &'a str, prefix: &str) -> Option<&'a str> {
    if prefix.is_empty() {
        return None;
    }
    if let Some(rest) = candidate.strip_prefix(prefix) {
        return (!rest.is_empty()).then_some(rest);
    }
    let folded_prefix = fold(prefix);
    if folded_prefix.is_empty() {
        return None;
    }
    let mut folded_so_far = String::new();
    for (idx, c) in candidate.char_indices() {
        crate::ui::palette::fold_into(c, &mut folded_so_far);
        if folded_so_far.len() >= folded_prefix.len() {
            if folded_so_far == folded_prefix {
                let mut end = idx + c.len_utf8();
                // Skip trailing combining/tashkeel marks attached to the last matched char.
                while let Some(next) = candidate[end..].chars().next() {
                    let mut tmp = String::new();
                    crate::ui::palette::fold_into(next, &mut tmp);
                    if tmp.is_empty() {
                        end += next.len_utf8();
                    } else {
                        break;
                    }
                }
                let rest = &candidate[end..];
                return (!rest.is_empty()).then_some(rest);
            }
            return None;
        }
    }
    None
}

/// Inline ghost suggestion suffix for the current input (newest matching history entry, else
/// trusted workflow command). Only produced when the caret is at the end of a non-empty buffer.
pub fn ghost_suggestion(
    text: &str,
    caret: usize,
    history: &[String],
    workflows: &[Workflow],
) -> Option<String> {
    if text.is_empty() || caret != text.len() {
        return None;
    }
    // History entries are newest first; prefer exact prefix first, then Arabic-folded prefix.
    for entry in history {
        if let Some(rest) = entry.strip_prefix(text) {
            if !rest.is_empty() && !rest.contains('\n') {
                return Some(rest.to_owned());
            }
        }
    }
    for entry in history {
        if let Some(rest) = folded_prefix_suffix(entry, text) {
            if !rest.contains('\n') {
                return Some(rest.to_owned());
            }
        }
    }
    for wf in workflows.iter().filter(|w| w.trusted) {
        if let Some(rest) = folded_prefix_suffix(&wf.spec.command, text) {
            if !rest.contains('\n') {
                return Some(rest.to_owned());
            }
        }
    }
    None
}

/// Rank completion candidates from history, trusted workflows, and cached directory entries.
///
/// Ranking priorities (lower score wins):
/// - In path argument context (e.g. `cat src/co`, `cd ./`): path prefix matches compete directly
///   with history prefix matches.
/// - At command start: exact prefix > Arabic-folded prefix > word/substring match; history
///   recency breaks ties, followed by workflows, then paths.
pub fn complete(
    text: &str,
    caret: usize,
    history: &[String],
    workflows: &[Workflow],
    path_query: Option<&PathQuery>,
    dir_entries: &[DirEntryInfo],
) -> Vec<CompletionItem> {
    let caret = caret.min(text.len());
    let prefix = &text[..caret];
    let folded_query = fold(prefix.trim_start());
    let query_words: Vec<String> = folded_query.split_whitespace().map(str::to_owned).collect();
    let in_path_ctx = path_query.is_some_and(|q| q.path_context);

    let mut out = Vec::new();

    // 1. History matches.
    if !folded_query.is_empty() {
        for (recency, entry) in history.iter().enumerate() {
            let recency_bonus = recency.min(999);
            let score = if entry.starts_with(prefix) && entry.len() > prefix.len() {
                // Exact prefix match: highest priority.
                100 + recency_bonus
            } else if folded_prefix_suffix(entry, prefix).is_some() {
                // Arabic/case-folded prefix match.
                1_200 + recency_bonus
            } else if let Some(sub) = multi_word_score(&[entry.as_str()], &query_words) {
                // Arabic-aware word/substring match (like palette).
                5_000 + sub * 10 + recency_bonus
            } else {
                continue;
            };
            out.push(CompletionItem {
                kind: CompletionKind::History,
                label: single_line_preview(entry),
                detail: None,
                replacement: entry.clone(),
                replace_range: (0, text.len()),
                score,
            });
        }
    }

    // 2. Trusted workflow commands (matched on command, English name, and Arabic name).
    for (idx, wf) in workflows.iter().filter(|w| w.trusted).enumerate() {
        let cmd = &wf.spec.command;
        let fields = [
            cmd.as_str(),
            wf.spec.name.as_str(),
            wf.spec.name_ar.as_str(),
        ];
        let score = if folded_query.is_empty() {
            3_000 + idx
        } else if cmd.starts_with(prefix) && !prefix.is_empty() {
            400 + idx
        } else if folded_prefix_suffix(cmd, prefix).is_some()
            || folded_prefix_suffix(&wf.spec.name, prefix).is_some()
            || folded_prefix_suffix(&wf.spec.name_ar, prefix).is_some()
        {
            1_600 + idx
        } else if let Some(sub) = multi_word_score(&fields, &query_words) {
            5_500 + sub * 10 + idx
        } else {
            continue;
        };
        let detail = match (wf.spec.name.is_empty(), wf.spec.name_ar.is_empty()) {
            (false, false) => Some(format!("{} / {}", wf.spec.name, wf.spec.name_ar)),
            (false, true) => Some(wf.spec.name.clone()),
            (true, false) => Some(wf.spec.name_ar.clone()),
            (true, true) => None,
        };
        out.push(CompletionItem {
            kind: CompletionKind::Workflow,
            label: single_line_preview(cmd),
            detail,
            replacement: cmd.clone(),
            replace_range: (0, text.len()),
            score,
        });
    }

    // 3. File paths relative to the pane's cwd.
    if let Some(pq) = path_query {
        let folded_name = fold(&pq.name_prefix);
        let show_hidden = pq.name_prefix.starts_with('.');
        for (idx, entry) in dir_entries.iter().enumerate() {
            if entry.name.starts_with('.') && !show_hidden {
                continue;
            }
            let base_score = if pq.name_prefix.is_empty() {
                if in_path_ctx {
                    300
                } else {
                    4_000
                }
            } else if entry.name.starts_with(&pq.name_prefix) {
                if in_path_ctx {
                    200
                } else {
                    2_000
                }
            } else if folded_prefix_suffix(&entry.name, &pq.name_prefix).is_some()
                || fold(&entry.name) == folded_name
            {
                if in_path_ctx {
                    1_400
                } else {
                    3_000
                }
            } else if let Some(pos) = fold(&entry.name).find(&folded_name) {
                if in_path_ctx {
                    3_500 + pos
                } else {
                    6_000 + pos
                }
            } else {
                continue;
            };
            let dir_bonus = if entry.is_dir { 0 } else { 50 };
            let mut full = format!("{}{}", pq.typed_dir, entry.name);
            if entry.is_dir {
                full.push('/');
            }
            let replacement = format_path_replacement(&full, pq.quote);
            out.push(CompletionItem {
                kind: CompletionKind::Path,
                label: full,
                detail: None,
                replacement,
                replace_range: pq.replace_range,
                score: base_score + dir_bonus + idx.min(200),
            });
        }
    }

    out.sort_by(|a, b| {
        a.score
            .cmp(&b.score)
            .then_with(|| a.label.len().cmp(&b.label.len()))
            .then_with(|| a.label.cmp(&b.label))
    });

    // Deduplicate by (replace_range, replacement), keeping highest-ranked entry.
    let mut seen = HashSet::new();
    out.retain(|item| seen.insert((item.replace_range, item.replacement.clone())));
    out.truncate(MAX_COMPLETIONS);
    out
}

fn multi_word_score(fields: &[&str], query_words: &[String]) -> Option<usize> {
    if query_words.is_empty() {
        return None;
    }
    let folded_fields: Vec<String> = fields.iter().map(|f| fold(f)).collect();
    let mut total = 0;
    for word in query_words {
        let best = folded_fields
            .iter()
            .filter_map(|f| {
                let at = f.find(word.as_str())?;
                let word_start = at == 0
                    || f[..at]
                        .chars()
                        .next_back()
                        .is_some_and(|c| !c.is_alphanumeric());
                Some(if word_start { at.min(50) } else { 100 + at })
            })
            .min()?;
        total += best;
    }
    Some(total)
}

fn single_line_preview(s: &str) -> String {
    s.replace('\n', " ↵ ")
}

fn format_path_replacement(path: &str, quote: Option<char>) -> String {
    if let Some(q) = quote {
        return format!("{q}{path}");
    }
    if path.chars().any(char::is_whitespace) {
        let mut out = String::with_capacity(path.len() + 4);
        for c in path.chars() {
            if c.is_whitespace() {
                out.push('\\');
            }
            out.push(c);
        }
        out
    } else {
        path.to_owned()
    }
}
