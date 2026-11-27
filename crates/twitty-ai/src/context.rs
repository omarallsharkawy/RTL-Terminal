//! Project context assembly for AI requests.
//!
//! Produces a deterministic, token-budgeted, secret-redacted [`ContextBundle`] from:
//! 1. Approved project rules files (`AGENTS.md`, `CLAUDE.md`, `TWITTY.md`, `.twitty/rules.md`),
//!    read only when their current SHA-256 matches the user's stored per-project approval.
//! 2. User-written project rules (`MemoryKind::Rule`).
//! 3. Top FTS5 memory hits matching the request text.
//! 4. Project environment notes (`MemoryKind::Env`).
//! 5. Frequent project commands (`MemoryKind::Command` / computed from history with success rate).
//! 6. User-selected command blocks (tail-trimmed to fit the remaining token budget).
//!
//! ## Token Estimator
//!
//! [`estimate_tokens`] uses a fast, deterministic script-weighted heuristic:
//! - ASCII / Latin characters count at **~4 characters per token** (weight `1` quarter-token).
//! - Arabic script and other non-ASCII characters count at **~2 characters per token** (weight `2`
//!   quarter-tokens), reflecting typical BPE tokenization of Arabic UTF-8 text.
//! - Total tokens = `ceil(quarter_tokens / 4)`, returning `0` for empty strings and at least `1`
//!   for any non-empty string.
//!
//! Excluded directories (`config.memory_exclude`) produce an empty [`ContextBundle`] with no
//! project root and no items.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use twitty_core::redact::redact;
use twitty_store::{
    is_path_excluded, is_project_excluded, resolve_project_root, FrequentCommand, MemoryEntry,
    MemoryKind, RulesFileInfo, RulesFileState, Store, AUTO_COMMAND_SOURCE,
    DEFAULT_MIN_COMMAND_RUNS,
};

use crate::engine::model::{ContextBundle, ContextItem, ContextKind};

/// Default token budget for assembled context.
pub const DEFAULT_BUDGET_TOKENS: u32 = 4096;

/// Maximum FTS5 memory hits considered per request.
const MAX_FTS_HITS: usize = 8;

/// Maximum frequent commands included per request.
const MAX_FREQUENT_COMMANDS: usize = 5;

/// Maximum characters kept in a command block's inspector label.
const MAX_BLOCK_LABEL_CHARS: usize = 40;

/// Prefix prepended when a command block is tail-trimmed to fit the remaining budget.
const TRIM_PREFIX: &str = "[... trimmed]\n";

/// A user-selected terminal command block to include in context assembly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedBlock {
    pub command: Option<String>,
    pub exit_code: Option<i32>,
    pub output: String,
}

/// Input to [`assemble`].
pub struct AssembleInput<'a> {
    /// Working directory of the active pane (resolved to git root when `project_root` is `None`).
    pub cwd: Option<&'a Path>,
    /// Optional pre-resolved project root.
    pub project_root: Option<&'a Path>,
    /// User request text (drives FTS5 retrieval over approved memories).
    pub request: &'a str,
    /// Maximum total estimated tokens across all `included == true` items.
    pub budget_tokens: u32,
    /// Glob/path exclusion patterns from `config.memory_exclude`.
    pub memory_exclude: &'a [String],
    /// Optional synchronous store handle for loading rules approvals, memories, and history.
    pub store: Option<&'a Store>,
    /// User-selected terminal blocks (tail-trimmed when needed to fit remaining budget).
    pub selected_blocks: &'a [SelectedBlock],
    /// Per-item `included` toggles keyed by [`ContextItem::label`], used when re-assembling after
    /// the user unticks/reticks items in the context inspector.
    pub included_overrides: &'a [(String, bool)],
}

/// Pre-loaded inputs for pure, I/O-free context assembly (useful when data was fetched off-thread
/// via `StoreHandle`).
pub struct AssembleParts<'a> {
    pub project_root: Option<PathBuf>,
    pub budget_tokens: u32,
    pub rules_files: &'a [RulesFileInfo],
    pub user_rules: &'a [MemoryEntry],
    pub fts_hits: &'a [MemoryEntry],
    pub env_notes: &'a [MemoryEntry],
    pub command_memories: &'a [MemoryEntry],
    pub frequent_commands: &'a [FrequentCommand],
    pub selected_blocks: &'a [SelectedBlock],
    pub included_overrides: &'a [(String, bool)],
}

/// Estimate the token count of `text` using Twitty's script-weighted heuristic:
/// ~4 chars/token for Latin/ASCII, ~2 chars/token for Arabic and other non-ASCII scripts.
pub fn estimate_tokens(text: &str) -> u32 {
    if text.is_empty() {
        return 0;
    }
    let mut quarter_tokens: u64 = 0;
    for ch in text.chars() {
        if is_arabic_or_wide_token_char(ch) {
            quarter_tokens += 2;
        } else {
            quarter_tokens += 1;
        }
    }
    let tokens = quarter_tokens.div_ceil(4).max(1);
    tokens.min(u32::MAX as u64) as u32
}

#[inline]
fn is_arabic_or_wide_token_char(ch: char) -> bool {
    !ch.is_ascii()
}

/// Tail-trim `text` so `estimate_tokens(result) <= max_tokens`, keeping the end of `text` (where
/// terminal errors and summaries usually appear).
///
/// Returns `None` when `max_tokens == 0` or `text` is empty.
pub fn tail_trim_to_tokens(text: &str, max_tokens: u32) -> Option<String> {
    if max_tokens == 0 || text.is_empty() {
        return None;
    }
    if estimate_tokens(text) <= max_tokens {
        return Some(text.to_owned());
    }

    let prefix_tokens = estimate_tokens(TRIM_PREFIX);
    let (use_prefix, target_tokens) = if max_tokens > prefix_tokens + 2 {
        (true, max_tokens - prefix_tokens)
    } else {
        (false, max_tokens)
    };
    let max_quarters = u64::from(target_tokens) * 4;

    // Walk backwards from the end accumulating quarter-tokens without exceeding `max_quarters`.
    let mut used_quarters: u64 = 0;
    let mut cut_byte = text.len();
    for (byte_idx, ch) in text.char_indices().rev() {
        let w = if is_arabic_or_wide_token_char(ch) {
            2u64
        } else {
            1u64
        };
        if used_quarters + w > max_quarters {
            break;
        }
        used_quarters += w;
        cut_byte = byte_idx;
    }

    if cut_byte >= text.len() {
        return None;
    }

    // Prefer snapping forward to a newline boundary if that keeps at least half of our slice so
    // we don't start mid-line when clean lines are available.
    let mut slice = &text[cut_byte..];
    if cut_byte > 0 && !text.as_bytes()[cut_byte - 1].eq(&b'\n') {
        if let Some(nl) = slice.find('\n') {
            let after_nl = &slice[nl + 1..];
            if !after_nl.is_empty() && after_nl.len() * 2 >= slice.len() {
                slice = after_nl;
            }
        }
    }

    let mut out = if use_prefix {
        let mut s = String::with_capacity(TRIM_PREFIX.len() + slice.len());
        s.push_str(TRIM_PREFIX);
        s.push_str(slice);
        s
    } else {
        slice.to_owned()
    };

    // Guarantee the strict upper bound even under prefix rounding, trimming body characters after
    // `TRIM_PREFIX` so the prefix itself is never corrupted.
    let prefix_len = if use_prefix { TRIM_PREFIX.len() } else { 0 };
    while out.len() > prefix_len && estimate_tokens(&out) > max_tokens {
        if let Some((offset, _)) = out[prefix_len..].char_indices().nth(1) {
            out.drain(prefix_len..prefix_len + offset);
        } else {
            out.truncate(prefix_len);
        }
    }
    if use_prefix && out.len() <= prefix_len {
        return None;
    }
    (!out.is_empty()).then_some(out)
}

/// Sum estimated tokens across all `included == true` items in `bundle`.
pub fn total_included_tokens(bundle: &ContextBundle) -> u32 {
    bundle
        .items
        .iter()
        .filter(|item| item.included)
        .fold(0u32, |acc, item| acc.saturating_add(item.est_tokens))
}

/// Iterate over only the items currently marked `included == true`.
pub fn included_items(bundle: &ContextBundle) -> impl Iterator<Item = &ContextItem> {
    bundle.items.iter().filter(|item| item.included)
}

/// Assemble a budgeted, redacted, deterministically ordered [`ContextBundle`] for an AI request.
pub fn assemble(input: &AssembleInput<'_>) -> ContextBundle {
    let resolved_root = input
        .project_root
        .map(Path::to_path_buf)
        .or_else(|| input.cwd.map(resolve_project_root));

    // Check exclusions against both cwd and resolved project root.
    let excluded = match (input.cwd, resolved_root.as_deref()) {
        (Some(cwd), Some(root)) => is_project_excluded(cwd, root, input.memory_exclude),
        (Some(cwd), None) => is_path_excluded(cwd, input.memory_exclude),
        (None, Some(root)) => is_path_excluded(root, input.memory_exclude),
        (None, None) => false,
    };
    if excluded {
        return ContextBundle {
            project_root: None,
            items: Vec::new(),
            budget_tokens: input.budget_tokens,
        };
    }

    let mut rules_files = Vec::new();
    let mut user_rules = Vec::new();
    let mut fts_hits = Vec::new();
    let mut env_notes = Vec::new();
    let mut command_memories = Vec::new();
    let mut frequent_commands = Vec::new();

    if let (Some(store), Some(root)) = (input.store, resolved_root.as_deref()) {
        let key = root.to_string_lossy();
        rules_files = store.inspect_rules_files(root).unwrap_or_default();

        user_rules = store
            .list_memory_by_kind(&key, Some(MemoryKind::Rule), true, 100)
            .unwrap_or_default();
        user_rules.sort_by_key(|e| e.id);

        if !input.request.trim().is_empty() {
            fts_hits = store
                .search_approved_memory(&key, input.request, None, MAX_FTS_HITS)
                .unwrap_or_default();
        }

        env_notes = store
            .list_memory_by_kind(&key, Some(MemoryKind::Env), true, 50)
            .unwrap_or_default();
        env_notes.sort_by_key(|e| e.id);

        command_memories = store
            .list_memory_by_kind(&key, Some(MemoryKind::Command), true, 50)
            .unwrap_or_default();
        command_memories.sort_by_key(|e| e.id);

        let has_auto_commands = command_memories
            .iter()
            .any(|e| e.source == AUTO_COMMAND_SOURCE);
        if !has_auto_commands {
            frequent_commands = store
                .frequent_commands(&key, DEFAULT_MIN_COMMAND_RUNS, MAX_FREQUENT_COMMANDS)
                .unwrap_or_default();
        }
    }

    assemble_from_parts(&AssembleParts {
        project_root: resolved_root,
        budget_tokens: input.budget_tokens,
        rules_files: &rules_files,
        user_rules: &user_rules,
        fts_hits: &fts_hits,
        env_notes: &env_notes,
        command_memories: &command_memories,
        frequent_commands: &frequent_commands,
        selected_blocks: input.selected_blocks,
        included_overrides: input.included_overrides,
    })
}

/// Re-assemble `input` while preserving every per-item `included` toggle from `previous`.
///
/// Unticking an item in the inspector frees its token budget for lower-priority memories and
/// longer command-block tails on re-assembly.
pub fn reassemble_with_bundle(
    input: &AssembleInput<'_>,
    previous: &ContextBundle,
) -> ContextBundle {
    let overrides: Vec<(String, bool)> = previous
        .items
        .iter()
        .map(|item| (item.label.clone(), item.included))
        .collect();
    let updated = AssembleInput {
        included_overrides: &overrides,
        ..*input
    };
    assemble(&updated)
}

/// Pure, I/O-free context assembly from pre-fetched components.
pub fn assemble_from_parts(parts: &AssembleParts<'_>) -> ContextBundle {
    let mut items: Vec<ContextItem> = Vec::new();
    let mut used_tokens: u32 = 0;
    let mut seen_memory_ids: HashSet<i64> = HashSet::new();
    let mut used_labels: HashSet<String> = HashSet::new();

    let override_for = |label: &str| -> Option<bool> {
        parts
            .included_overrides
            .iter()
            .rev()
            .find(|(l, _)| l == label)
            .map(|(_, inc)| *inc)
    };

    let mut try_push = |kind: ContextKind, raw_label: String, raw_text: &str| {
        let label = make_unique_label(redact(&raw_label).into_owned(), &mut used_labels);
        let text = redact(raw_text).trim().to_owned();
        if text.is_empty() {
            return;
        }
        let est_tokens = estimate_tokens(&text);
        if override_for(&label) == Some(false) {
            items.push(ContextItem {
                kind,
                label,
                text,
                est_tokens,
                included: false,
            });
            return;
        }
        let remaining = parts.budget_tokens.saturating_sub(used_tokens);
        if est_tokens <= remaining {
            used_tokens = used_tokens.saturating_add(est_tokens);
            items.push(ContextItem {
                kind,
                label,
                text,
                est_tokens,
                included: true,
            });
        }
    };

    // 1. Approved rules files (never read unapproved or changed files).
    for file in parts.rules_files {
        if file.state != RulesFileState::Approved {
            continue;
        }
        if let Some(content) = file.approved_content.as_deref() {
            try_push(ContextKind::Rule, file.rel_path.clone(), content);
        }
    }

    // 2. User-written rules (`MemoryKind::Rule`, approved only).
    for rule in parts.user_rules {
        if !rule.approved || rule.kind != MemoryKind::Rule {
            continue;
        }
        if seen_memory_ids.insert(rule.id) {
            try_push(ContextKind::Rule, format!("rule #{}", rule.id), &rule.text);
        }
    }

    // 3. Top FTS memory hits for the request text (approved only, deduplicated against rules).
    for hit in parts.fts_hits {
        if !hit.approved {
            continue;
        }
        if !seen_memory_ids.insert(hit.id) {
            continue;
        }
        let kind = if hit.kind == MemoryKind::Rule {
            ContextKind::Rule
        } else {
            ContextKind::Memory
        };
        let prefix = match hit.kind {
            MemoryKind::Rule => "rule",
            MemoryKind::Fact => "fact",
            MemoryKind::Command => "cmd",
            MemoryKind::Env => "env",
        };
        try_push(kind, format!("{prefix} #{}", hit.id), &hit.text);
    }

    // 4. Environment notes (`MemoryKind::Env`, approved only).
    for env in parts.env_notes {
        if !env.approved || env.kind != MemoryKind::Env {
            continue;
        }
        if seen_memory_ids.insert(env.id) {
            try_push(ContextKind::Memory, format!("env #{}", env.id), &env.text);
        }
    }

    // 5. Frequent commands (synced `MemoryKind::Command` rows, then live-computed fallback).
    for cmd in parts.command_memories {
        if !cmd.approved || cmd.kind != MemoryKind::Command {
            continue;
        }
        if seen_memory_ids.insert(cmd.id) {
            try_push(ContextKind::Memory, format!("cmd #{}", cmd.id), &cmd.text);
        }
    }
    for freq in parts.frequent_commands {
        let note = freq.format_note();
        let short = truncate_label(&freq.command, MAX_BLOCK_LABEL_CHARS);
        try_push(ContextKind::Memory, format!("cmd: {short}"), &note);
    }

    // 6. User-selected blocks (tail-trimmed to fit the remaining token budget).
    for (idx, block) in parts.selected_blocks.iter().enumerate() {
        let raw_label = match block
            .command
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(cmd) => format!(
                "block: {}",
                truncate_label(&redact(cmd), MAX_BLOCK_LABEL_CHARS)
            ),
            None => format!("block #{}", idx + 1),
        };
        let label = make_unique_label(raw_label, &mut used_labels);
        let full_text = format_block_text(block);
        if full_text.is_empty() {
            continue;
        }
        if override_for(&label) == Some(false) {
            let est_tokens = estimate_tokens(&full_text);
            items.push(ContextItem {
                kind: ContextKind::Block,
                label,
                text: full_text,
                est_tokens,
                included: false,
            });
            continue;
        }
        let remaining = parts.budget_tokens.saturating_sub(used_tokens);
        if remaining == 0 {
            break;
        }
        if let Some(fitted) = fit_block_tail(block, &full_text, remaining) {
            let est_tokens = estimate_tokens(&fitted);
            used_tokens = used_tokens.saturating_add(est_tokens);
            items.push(ContextItem {
                kind: ContextKind::Block,
                label,
                text: fitted,
                est_tokens,
                included: true,
            });
        }
    }

    ContextBundle {
        project_root: parts.project_root.clone(),
        items,
        budget_tokens: parts.budget_tokens,
    }
}

fn format_block_header(block: &SelectedBlock) -> Option<String> {
    let cmd = block
        .command
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|c| redact(c).into_owned());
    match (cmd, block.exit_code) {
        (Some(c), Some(code)) => Some(format!("$ {c} (exit {code})")),
        (Some(c), None) => Some(format!("$ {c}")),
        (None, Some(code)) => Some(format!("(exit {code})")),
        (None, None) => None,
    }
}

fn format_block_text(block: &SelectedBlock) -> String {
    let header = format_block_header(block);
    let output = redact(&block.output).trim_end().to_owned();
    match (header, output.is_empty()) {
        (Some(h), true) => h,
        (Some(h), false) => format!("{h}\n{output}"),
        (None, false) => output,
        (None, true) => String::new(),
    }
}

fn fit_block_tail(block: &SelectedBlock, full_text: &str, remaining: u32) -> Option<String> {
    if estimate_tokens(full_text) <= remaining {
        return Some(full_text.to_owned());
    }
    if let Some(header) = format_block_header(block) {
        let header_with_nl = format!("{header}\n");
        let header_tokens = estimate_tokens(&header_with_nl);
        let output = redact(&block.output).trim_end().to_owned();
        if !output.is_empty() && remaining > header_tokens + 2 {
            let out_budget = remaining - header_tokens;
            if let Some(trimmed_out) = tail_trim_to_tokens(&output, out_budget) {
                let candidate = format!("{header_with_nl}{trimmed_out}");
                if estimate_tokens(&candidate) <= remaining {
                    return Some(candidate);
                }
            }
        }
    }
    tail_trim_to_tokens(full_text, remaining)
}

fn make_unique_label(base: String, used: &mut HashSet<String>) -> String {
    if used.insert(base.clone()) {
        return base;
    }
    for n in 2..=999 {
        let candidate = format!("{base} ({n})");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    base
}

fn truncate_label(s: &str, max_chars: usize) -> String {
    let single_line: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = single_line.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_owned();
    }
    let mut out: String = trimmed.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}
