//! Smart command completion and inline ghost suggestions for the composer:
//! declarative command specs (subcommands, options, option values, `sudo` delegation, pipelines),
//! off-thread dynamic generators, file paths relative to the pane's cwd, trusted workflows, and
//! command history, with Arabic-aware matching (`ui::palette::fold`).
//!
//! Ranking priority: **exact prefix > fuzzy > history** (followed by workflows).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::composer::generators::{GeneratedItem, GeneratorKey, GeneratorKind};
use crate::composer::highlight::{
    is_env_assignment, is_shell_builtin, match_operator, operator_is_redirect,
    operator_starts_command,
};
use crate::composer::specs::{default_registry, ArgKind, CommandSpec, OptionSpec, SpecRegistry};
use crate::ui::chrome::UiLanguage;
use crate::ui::palette::fold;
use crate::workflows::Workflow;

/// Maximum completion candidates returned for the popup list.
pub const MAX_COMPLETIONS: usize = 32;
/// Maximum directory entries read when completing a path.
pub const MAX_DIR_ENTRIES: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    Command,
    Subcommand,
    Option,
    Branch,
    Remote,
    Tag,
    Script,
    Target,
    Feature,
    Container,
    Image,
    Unit,
    Host,
    Value,
    Path,
    Workflow,
    History,
}

impl CompletionKind {
    pub fn badge_en(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Subcommand => "subcommand",
            Self::Option => "option",
            Self::Branch => "branch",
            Self::Remote => "remote",
            Self::Tag => "tag",
            Self::Script => "script",
            Self::Target => "target",
            Self::Feature => "feature",
            Self::Container => "container",
            Self::Image => "image",
            Self::Unit => "unit",
            Self::Host => "host",
            Self::Value => "value",
            Self::Path => "path",
            Self::Workflow => "workflow",
            Self::History => "history",
        }
    }

    pub fn badge_ar(self) -> &'static str {
        match self {
            Self::Command => "أمر",
            Self::Subcommand => "فرعي",
            Self::Option => "خيار",
            Self::Branch => "فرع",
            Self::Remote => "مستودع",
            Self::Tag => "وسم",
            Self::Script => "سكربت",
            Self::Target => "هدف",
            Self::Feature => "ميزة",
            Self::Container => "حاوية",
            Self::Image => "صورة",
            Self::Unit => "وحدة",
            Self::Host => "مضيف",
            Self::Value => "قيمة",
            Self::Path => "مسار",
            Self::Workflow => "سير عمل",
            Self::History => "سجل",
        }
    }
}

/// One completion candidate shown in the composer popup or applied on Tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub kind: CompletionKind,
    /// Primary text shown in the popup list.
    pub label: String,
    /// Optional English description / detail.
    pub detail: Option<String>,
    /// Optional Arabic description / detail (falls back to `detail` when `None`).
    pub detail_ar: Option<String>,
    /// Text to splice into `replace_range`.
    pub replacement: String,
    /// Byte range `(start, end)` in the editor buffer replaced when accepted.
    pub replace_range: (usize, usize),
    /// Ranking score (lower is better).
    pub score: usize,
}

impl CompletionItem {
    pub fn localized_detail(&self, lang: UiLanguage) -> Option<&str> {
        match lang {
            UiLanguage::Arabic => self.detail_ar.as_deref().or(self.detail.as_deref()),
            UiLanguage::English => self.detail.as_deref(),
        }
    }
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
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let line_before = &text[line_start..caret];

    let mut token_start_in_line = 0;
    let mut in_quote: Option<char> = None;
    let mut escaped = false;
    let bytes = line_before.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        let c = line_before[idx..].chars().next().unwrap();
        let clen = c.len_utf8();
        if escaped {
            escaped = false;
            idx += clen;
            continue;
        }
        if c == '\\' && in_quote != Some('\'') {
            escaped = true;
            idx += clen;
            continue;
        }
        match in_quote {
            Some(q) if c == q => in_quote = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => in_quote = Some(c),
            None if c.is_whitespace() => token_start_in_line = idx + clen,
            None => {
                if let Some(op_len) = match_operator(bytes, idx) {
                    idx += op_len;
                    token_start_in_line = idx;
                    continue;
                }
            }
        }
        idx += clen;
    }

    let raw_token = &line_before[token_start_in_line..];
    // Handle `--flag=path/to/file`: complete only the value after `=`.
    let (eq_offset, token_slice) = if raw_token.starts_with('-') {
        if let Some(eq) = raw_token.find('=') {
            (eq + 1, &raw_token[eq + 1..])
        } else {
            (0, raw_token)
        }
    } else {
        (0, raw_token)
    };
    let replace_start = line_start + token_start_in_line + eq_offset;
    let (quote, unquoted) = match token_slice.chars().next() {
        Some(q @ ('\'' | '"')) => (Some(q), &token_slice[1..]),
        _ => (None, token_slice),
    };

    let has_preceding_arg = !line_before[..token_start_in_line].trim().is_empty() || eq_offset > 0;
    let looks_like_path =
        unquoted.contains('/') || unquoted.starts_with('.') || unquoted.starts_with('~');
    let path_context = has_preceding_arg || looks_like_path;

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

/// What the shell parser determined is being completed at the caret.
#[derive(Clone, Debug)]
enum CompletionSlot<'a> {
    /// Completing a command name (start of buffer, after `|`/`&&`/`;`, or after `sudo`).
    Command {
        /// True when inside `sudo` or after a pipe/operator (suppresses full-buffer history).
        nested: bool,
    },
    /// Completing an option flag (`-` or `--`) for `spec` (with inherited options from `ancestors`).
    Option {
        spec: &'a CommandSpec,
        ancestors: Vec<&'a CommandSpec>,
    },
    /// Completing the value of a specific option (`--opt val` or `--opt=val`).
    OptionValue { kind: ArgKind, systemctl_user: bool },
    /// Completing a subcommand (and optionally positional args / options) of `spec`.
    SubcommandOrArg {
        spec: &'a CommandSpec,
        ancestors: Vec<&'a CommandSpec>,
        positional_index: usize,
        systemctl_user: bool,
    },
    /// Completing a positional argument with a known [`ArgKind`].
    PositionalArg { kind: ArgKind, systemctl_user: bool },
    /// Completing a path after a redirection operator (`>`, `>>`, `<`).
    RedirectPath,
}

#[derive(Clone, Debug)]
struct ParsedContext<'a> {
    /// Byte offset where the active pipeline/list segment begins.
    segment_start: usize,
    /// Current token fragment being completed (already stripped of `--opt=` prefix if applicable).
    query: String,
    /// Byte range `(start, end)` in the full buffer replaced by the completion.
    replace_range: (usize, usize),
    slot: CompletionSlot<'a>,
}

/// Token inside the active pipeline segment before the caret.
#[derive(Clone, Debug)]
struct SegToken {
    text: String,
    is_operator: bool,
}

/// Parse the active shell segment up to `caret` to determine what slot is being completed.
fn analyze_context<'a>(text: &str, caret: usize, specs: &'a SpecRegistry) -> ParsedContext<'a> {
    let caret = caret.min(text.len());
    let before = &text[..caret];
    let bytes = before.as_bytes();
    let n = bytes.len();

    // Tokenize `before`, resetting whenever a command-starting operator or newline appears.
    let mut segment_start = 0usize;
    let mut tokens: Vec<SegToken> = Vec::new();
    let mut i = 0usize;
    let mut in_quote: Option<u8> = None;
    let mut escaped = false;
    let mut tok_start: Option<usize> = None;

    while i < n {
        let b = bytes[i];
        if escaped {
            escaped = false;
            i += next_char_len(before, i);
            continue;
        }
        if let Some(q) = in_quote {
            if b == b'\\' && q == b'"' {
                escaped = true;
                i += 1;
            } else if b == q {
                in_quote = None;
                i += 1;
            } else {
                i += next_char_len(before, i);
            }
            continue;
        }
        if b == b'\\' {
            if tok_start.is_none() {
                tok_start = Some(i);
            }
            escaped = true;
            i += 1;
            continue;
        }
        if b == b'\'' || b == b'"' {
            if tok_start.is_none() {
                tok_start = Some(i);
            }
            in_quote = Some(b);
            i += 1;
            continue;
        }
        if b == b'\n' {
            tokens.clear();
            tok_start = None;
            i += 1;
            segment_start = i;
            continue;
        }
        if b == b' ' || b == b'\t' || b == b'\r' {
            if let Some(s) = tok_start.take() {
                tokens.push(SegToken {
                    text: before[s..i].to_owned(),
                    is_operator: false,
                });
            }
            i += 1;
            continue;
        }
        if let Some(op_len) = match_operator(bytes, i) {
            if let Some(s) = tok_start.take() {
                tokens.push(SegToken {
                    text: before[s..i].to_owned(),
                    is_operator: false,
                });
            }
            let op = &before[i..i + op_len];
            if operator_starts_command(op) {
                tokens.clear();
                segment_start = i + op_len;
            } else {
                tokens.push(SegToken {
                    text: op.to_owned(),
                    is_operator: true,
                });
            }
            i += op_len;
            continue;
        }
        if tok_start.is_none() {
            tok_start = Some(i);
        }
        i += next_char_len(before, i);
    }

    let (current_raw, current_start) = match tok_start {
        Some(s) => (before[s..caret].to_owned(), s),
        None => (String::new(), caret),
    };

    // Check if immediately preceded by a redirection operator (`>`, `>>`, `<`).
    if tokens
        .last()
        .is_some_and(|t| t.is_operator && operator_is_redirect(&t.text))
    {
        return ParsedContext {
            segment_start,
            query: current_raw,
            replace_range: (current_start, caret),
            slot: CompletionSlot::RedirectPath,
        };
    }

    // Filter out redirections and their targets from preceding tokens.
    let mut words: Vec<&str> = Vec::new();
    let mut skip_next = false;
    for t in &tokens {
        if skip_next {
            skip_next = false;
            continue;
        }
        if t.is_operator {
            if operator_is_redirect(&t.text) {
                skip_next = true;
            }
            continue;
        }
        words.push(t.text.as_str());
    }

    // Skip leading `VAR=val` assignments and command-prefix keywords (`time`, `nohup`, `command`).
    let mut idx = 0;
    while idx < words.len() {
        let w = words[idx];
        if is_env_assignment(w).is_some()
            || matches!(w, "time" | "nohup" | "command" | "builtin" | "exec" | "!")
        {
            idx += 1;
        } else {
            break;
        }
    }
    let nested_segment = segment_start > 0 || idx > 0;

    // Resolve command & delegate through wrappers (`sudo`) with a safety depth cap.
    let mut delegated = false;
    for _depth in 0..4 {
        if idx >= words.len() {
            return ParsedContext {
                segment_start,
                query: current_raw,
                replace_range: (current_start, caret),
                slot: CompletionSlot::Command {
                    nested: nested_segment || delegated,
                },
            };
        }

        let cmd_name = strip_outer_quotes(words[idx]);
        let Some(root_spec) = specs.get(cmd_name) else {
            // Unknown command (no spec): positional path / history fallback.
            return ParsedContext {
                segment_start,
                query: current_raw,
                replace_range: (current_start, caret),
                slot: CompletionSlot::PositionalArg {
                    kind: ArgKind::Path,
                    systemctl_user: false,
                },
            };
        };

        let mut active: &CommandSpec = root_spec;
        let mut ancestors: Vec<&CommandSpec> = Vec::new();
        let mut expect_opt: Option<&OptionSpec> = None;
        let mut positional_count = 0usize;
        let mut systemctl_user = false;
        let mut after_double_dash = false;
        let mut delegated_to_inner = false;

        let mut j = idx + 1;
        while j < words.len() {
            let w = words[j];
            if let Some(_opt) = expect_opt.take() {
                j += 1;
                continue;
            }
            if w == "--" {
                after_double_dash = true;
                j += 1;
                continue;
            }
            if !after_double_dash && w.starts_with('-') && w.len() > 1 {
                if w == "--user" {
                    systemctl_user = true;
                }
                if !w.contains('=') {
                    if let Some(opt) = find_option_in_chain(active, &ancestors, w) {
                        if opt.expects_value() {
                            expect_opt = Some(opt);
                        }
                    }
                }
                j += 1;
                continue;
            }

            // If this command delegates to an inner command (`sudo`), jump to `j` as new command!
            if active.delegates_command() {
                if is_env_assignment(w).is_some() {
                    j += 1;
                    continue;
                }
                idx = j;
                delegated = true;
                delegated_to_inner = true;
                break;
            }

            if !after_double_dash {
                if let Some(sub) = active.find_subcommand(w) {
                    ancestors.push(active);
                    active = sub;
                    positional_count = 0;
                    j += 1;
                    continue;
                }
            }
            positional_count += 1;
            j += 1;
        }

        if delegated_to_inner {
            continue;
        }

        // Case 1: Previous token was an option expecting a value (e.g. `git commit -m |`).
        if let Some(opt) = expect_opt {
            let kind = opt.value_kind().cloned().unwrap_or(ArgKind::Any);
            return ParsedContext {
                segment_start,
                query: current_raw,
                replace_range: (current_start, caret),
                slot: CompletionSlot::OptionValue {
                    kind,
                    systemctl_user,
                },
            };
        }

        // Case 2: Current token is `--opt=val` (e.g. `cargo test --features=fo|`).
        if !after_double_dash && current_raw.starts_with('-') {
            if let Some(eq) = current_raw.find('=') {
                let flag = &current_raw[..eq];
                let val_query = current_raw[eq + 1..].to_owned();
                let val_start = current_start + eq + 1;
                let kind = find_option_in_chain(active, &ancestors, flag)
                    .and_then(|o| o.value_kind().cloned())
                    .unwrap_or(ArgKind::Any);
                return ParsedContext {
                    segment_start,
                    query: val_query,
                    replace_range: (val_start, caret),
                    slot: CompletionSlot::OptionValue {
                        kind,
                        systemctl_user,
                    },
                };
            }
            return ParsedContext {
                segment_start,
                query: current_raw,
                replace_range: (current_start, caret),
                slot: CompletionSlot::Option {
                    spec: active,
                    ancestors,
                },
            };
        }

        // Case 3: Inside `sudo` (after sudo's options, before any inner command): complete inner command!
        if active.delegates_command() {
            return ParsedContext {
                segment_start,
                query: current_raw,
                replace_range: (current_start, caret),
                slot: CompletionSlot::Command { nested: true },
            };
        }

        // Case 4: Active command has subcommands and none chosen yet.
        if !after_double_dash && !active.subcommands.is_empty() && positional_count == 0 {
            return ParsedContext {
                segment_start,
                query: current_raw,
                replace_range: (current_start, caret),
                slot: CompletionSlot::SubcommandOrArg {
                    spec: active,
                    ancestors,
                    positional_index: positional_count,
                    systemctl_user,
                },
            };
        }

        // Case 5: Positional argument of `active`.
        let kind = active
            .positional_arg(positional_count)
            .cloned()
            .unwrap_or(ArgKind::Path);
        return ParsedContext {
            segment_start,
            query: current_raw,
            replace_range: (current_start, caret),
            slot: CompletionSlot::PositionalArg {
                kind,
                systemctl_user,
            },
        };
    }

    ParsedContext {
        segment_start,
        query: current_raw,
        replace_range: (current_start, caret),
        slot: CompletionSlot::Command {
            nested: nested_segment || delegated,
        },
    }
}

fn find_option_in_chain<'a>(
    active: &'a CommandSpec,
    ancestors: &[&'a CommandSpec],
    flag: &str,
) -> Option<&'a OptionSpec> {
    if let Some(opt) = active.find_option(flag) {
        return Some(opt);
    }
    for parent in ancestors.iter().rev() {
        if let Some(opt) = parent.find_option(flag) {
            return Some(opt);
        }
    }
    None
}

/// Determine which dynamic generators (if any) are needed at `(text, caret)` so the app can
/// trigger them off the UI thread.
pub fn needed_generators(
    text: &str,
    caret: usize,
    specs: &SpecRegistry,
    cwd: Option<&Path>,
) -> Vec<GeneratorKey> {
    let ctx = analyze_context(text, caret, specs);
    let mut kinds = Vec::new();
    match &ctx.slot {
        CompletionSlot::OptionValue {
            kind,
            systemctl_user,
        }
        | CompletionSlot::PositionalArg {
            kind,
            systemctl_user,
        } => {
            collect_generators_for_arg(kind, *systemctl_user, &mut kinds);
        }
        CompletionSlot::SubcommandOrArg {
            spec,
            positional_index,
            systemctl_user,
            ..
        } => {
            if let Some(arg) = spec.positional_arg(*positional_index) {
                collect_generators_for_arg(arg, *systemctl_user, &mut kinds);
            }
        }
        CompletionSlot::Command { .. }
        | CompletionSlot::Option { .. }
        | CompletionSlot::RedirectPath => {}
    }
    kinds
        .into_iter()
        .filter(|k| !k.is_cwd_scoped() || cwd.is_some())
        .map(|k| GeneratorKey::new(k, cwd))
        .collect()
}

fn collect_generators_for_arg(kind: &ArgKind, systemctl_user: bool, out: &mut Vec<GeneratorKind>) {
    match kind {
        ArgKind::GitBranch => out.push(GeneratorKind::GitBranches),
        ArgKind::GitRemote => out.push(GeneratorKind::GitRemotes),
        ArgKind::GitTag => out.push(GeneratorKind::GitTags),
        ArgKind::GitRef => {
            out.push(GeneratorKind::GitBranches);
            out.push(GeneratorKind::GitTags);
        }
        ArgKind::GitChangedFile => out.push(GeneratorKind::GitChangedFiles),
        ArgKind::NpmScript => out.push(GeneratorKind::NpmScripts),
        ArgKind::MakeTarget => out.push(GeneratorKind::MakeTargets),
        ArgKind::CargoTarget => out.push(GeneratorKind::CargoTargets),
        ArgKind::CargoFeature => out.push(GeneratorKind::CargoFeatures),
        ArgKind::DockerContainer => out.push(GeneratorKind::DockerContainers),
        ArgKind::DockerImage => out.push(GeneratorKind::DockerImages),
        ArgKind::SystemctlUnit => out.push(GeneratorKind::SystemctlUnits {
            user: systemctl_user,
        }),
        ArgKind::SshHost => out.push(GeneratorKind::SshHosts),
        ArgKind::Path
        | ArgKind::File
        | ArgKind::Dir
        | ArgKind::Command
        | ArgKind::Values(_)
        | ArgKind::Any => {}
    }
}

/// Inputs to [`complete_with`], bundling history, workflows, directory entries, specs, dynamic
/// generator results, and scanned `$PATH` executables.
pub struct CompleteInputs<'a> {
    pub history: &'a [String],
    pub workflows: &'a [Workflow],
    pub path_query: Option<&'a PathQuery>,
    pub dir_entries: &'a [DirEntryInfo],
    pub specs: &'a SpecRegistry,
    pub generated: &'a HashMap<GeneratorKind, Vec<GeneratedItem>>,
    pub path_commands: Option<&'a HashSet<String>>,
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

/// Rank completion candidates using built-in specs, history, trusted workflows, and directory
/// entries (convenience wrapper over [`complete_with`]).
pub fn complete(
    text: &str,
    caret: usize,
    history: &[String],
    workflows: &[Workflow],
    path_query: Option<&PathQuery>,
    dir_entries: &[DirEntryInfo],
) -> Vec<CompletionItem> {
    let empty_gen = HashMap::new();
    let inputs = CompleteInputs {
        history,
        workflows,
        path_query,
        dir_entries,
        specs: default_registry(),
        generated: &empty_gen,
        path_commands: None,
    };
    complete_with(text, caret, &inputs)
}

/// Full smart completion entry point accepting specs, dynamic generator snapshots, and `$PATH`
/// executables.
///
/// Ranking bands (lower score wins):
/// - `100..=599`: Exact prefix (and Arabic/case-folded prefix) matches on contextual candidates
///   (subcommands, options, commands, dynamic generators, enum values, paths).
/// - `1_000..=1_899`: Fuzzy / subsequence / substring matches on contextual candidates.
/// - `2_000..=3_999`: Command history matches (`exact prefix > folded > word/fuzzy`).
/// - `4_000..=5_999`: Trusted workflow matches (by command, English title, or Arabic title).
pub fn complete_with(text: &str, caret: usize, inputs: &CompleteInputs<'_>) -> Vec<CompletionItem> {
    let caret = caret.min(text.len());
    let ctx = analyze_context(text, caret, inputs.specs);
    let mut out = Vec::new();

    match &ctx.slot {
        CompletionSlot::Command { .. } => {
            add_command_candidates(&ctx, inputs, &mut out);
            // If the query looks like a path (`./`, `/`, `~/`), also offer path completions.
            if let Some(pq) = inputs.path_query {
                if pq.path_context {
                    add_path_candidates(pq, inputs.dir_entries, false, &mut out);
                }
            }
        }
        CompletionSlot::Option { spec, ancestors } => {
            add_option_candidates(&ctx, spec, ancestors, &mut out);
        }
        CompletionSlot::OptionValue {
            kind,
            systemctl_user,
        } => {
            add_arg_candidates(&ctx, kind, *systemctl_user, inputs, &mut out);
        }
        CompletionSlot::SubcommandOrArg {
            spec,
            ancestors,
            positional_index,
            systemctl_user,
        } => {
            add_subcommand_candidates(&ctx, spec, &mut out);
            if let Some(arg_kind) = spec.positional_arg(*positional_index) {
                add_arg_candidates(&ctx, arg_kind, *systemctl_user, inputs, &mut out);
            }
            if ctx.query.is_empty() && out.len() < 4 {
                add_option_candidates(&ctx, spec, ancestors, &mut out);
            }
        }
        CompletionSlot::PositionalArg {
            kind,
            systemctl_user,
        } => {
            add_arg_candidates(&ctx, kind, *systemctl_user, inputs, &mut out);
        }
        CompletionSlot::RedirectPath => {
            if let Some(pq) = inputs.path_query {
                add_path_candidates(pq, inputs.dir_entries, false, &mut out);
            }
        }
    }

    // History and workflows compete in the top-level command segment (`exact prefix > fuzzy > history`).
    let allow_history =
        ctx.segment_start == 0 && !matches!(ctx.slot, CompletionSlot::Command { nested: true });
    if allow_history {
        add_history_candidates(text, caret, inputs.history, &mut out);
        add_workflow_candidates(text, caret, inputs.workflows, &mut out);
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

/// Score a candidate string against `query` in the contextual band:
/// - Empty query: `250`
/// - Exact case-sensitive prefix: `100`
/// - Case/Arabic-folded prefix: `350`
/// - Substring or fuzzy subsequence match: `1_000 + penalty` (capped at `1_850`)
fn contextual_match_score(candidate: &str, query: &str) -> Option<usize> {
    if query.is_empty() {
        return Some(250);
    }
    if candidate.starts_with(query) {
        return Some(100);
    }
    let folded_cand = fold(candidate);
    let folded_query = fold(query);
    if folded_query.is_empty() {
        return Some(250);
    }
    if folded_cand.starts_with(&folded_query) {
        return Some(350);
    }
    if let Some(pos) = folded_cand.find(&folded_query) {
        let boundary_bonus = if pos == 0
            || folded_cand[..pos]
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_alphanumeric())
        {
            0
        } else {
            100
        };
        return Some((1_000 + boundary_bonus + pos * 4).min(1_850));
    }
    fuzzy_subsequence_score(&folded_cand, &folded_query).map(|s| (1_200 + s).min(1_850))
}

/// Subsequence fuzzy matcher returning a lower-is-better penalty score.
fn fuzzy_subsequence_score(candidate: &str, query: &str) -> Option<usize> {
    let mut cand_chars = candidate.char_indices().peekable();
    let mut prev_pos: Option<usize> = None;
    let mut penalty = 0usize;

    for qc in query.chars() {
        let mut matched = None;
        for (pos, cc) in cand_chars.by_ref() {
            if cc == qc {
                matched = Some(pos);
                break;
            }
        }
        let pos = matched?;
        if let Some(prev) = prev_pos {
            let gap = pos.saturating_sub(prev + 1);
            if gap > 0 {
                let is_boundary = candidate[..pos]
                    .chars()
                    .next_back()
                    .is_some_and(|c| matches!(c, '-' | '_' | '/' | '.' | ' '));
                penalty += if is_boundary { 4 } else { 12 + gap.min(40) };
            }
        } else {
            penalty += pos.min(40) * 3;
        }
        prev_pos = Some(pos);
    }
    Some(penalty)
}

fn add_command_candidates(
    ctx: &ParsedContext<'_>,
    inputs: &CompleteInputs<'_>,
    out: &mut Vec<CompletionItem>,
) {
    let query = ctx.query.as_str();
    let mut seen_names = HashSet::new();

    // 1. Spec commands (have rich descriptions and aliases).
    for cmd in inputs.specs.commands() {
        seen_names.insert(cmd.name.as_str());
        if let Some(score) = contextual_match_score(&cmd.name, query) {
            out.push(CompletionItem {
                kind: CompletionKind::Command,
                label: cmd.name.clone(),
                detail: Some(cmd.desc.clone()),
                detail_ar: cmd.desc_ar.clone(),
                replacement: cmd.name.clone(),
                replace_range: ctx.replace_range,
                score,
            });
        }
        for alias in &cmd.aliases {
            seen_names.insert(alias.as_str());
            if let Some(score) = contextual_match_score(alias, query) {
                out.push(CompletionItem {
                    kind: CompletionKind::Command,
                    label: alias.clone(),
                    detail: Some(cmd.desc.clone()),
                    detail_ar: cmd.desc_ar.clone(),
                    replacement: alias.clone(),
                    replace_range: ctx.replace_range,
                    score: score + 10,
                });
            }
        }
    }

    // 2. Executables on `$PATH` (when typed prefix is non-empty, or after `sudo`).
    if let Some(path_cmds) = inputs.path_commands {
        if !query.is_empty() {
            let mut matched: Vec<(&str, usize)> = path_cmds
                .iter()
                .filter(|name| !seen_names.contains(name.as_str()))
                .filter_map(|name| {
                    contextual_match_score(name, query).map(|s| (name.as_str(), s + 40))
                })
                .collect();
            matched.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(b.0)));
            for (name, score) in matched.into_iter().take(24) {
                let desc = if is_shell_builtin(name) {
                    Some("shell builtin".into())
                } else {
                    None
                };
                out.push(CompletionItem {
                    kind: CompletionKind::Command,
                    label: name.to_owned(),
                    detail: desc,
                    detail_ar: None,
                    replacement: name.to_owned(),
                    replace_range: ctx.replace_range,
                    score,
                });
            }
        }
    }
}

fn add_subcommand_candidates(
    ctx: &ParsedContext<'_>,
    spec: &CommandSpec,
    out: &mut Vec<CompletionItem>,
) {
    let query = ctx.query.as_str();
    for (idx, sub) in spec.subcommands.iter().enumerate() {
        let score = contextual_match_score(&sub.name, query).or_else(|| {
            sub.aliases
                .iter()
                .filter_map(|a| contextual_match_score(a, query))
                .min()
                .map(|s| s + 15)
        });
        let Some(score) = score else {
            continue;
        };
        out.push(CompletionItem {
            kind: CompletionKind::Subcommand,
            label: sub.name.clone(),
            detail: Some(sub.desc.clone()),
            detail_ar: sub.desc_ar.clone(),
            replacement: sub.name.clone(),
            replace_range: ctx.replace_range,
            score: score + idx.min(60),
        });
    }
}

fn add_option_candidates(
    ctx: &ParsedContext<'_>,
    spec: &CommandSpec,
    ancestors: &[&CommandSpec],
    out: &mut Vec<CompletionItem>,
) {
    let query = ctx.query.as_str();
    let query_body = query.trim_start_matches('-');
    let mut seen_flags = HashSet::new();

    let all_specs = std::iter::once((spec, 0usize)).chain(
        ancestors
            .iter()
            .rev()
            .enumerate()
            .map(|(i, s)| (*s, 25 + i * 10)),
    );

    for (sp, depth_penalty) in all_specs {
        for (idx, opt) in sp.options.iter().enumerate() {
            let insert = opt.insert_flag(query);
            if insert.is_empty() || !seen_flags.insert(insert.to_owned()) {
                continue;
            }
            let best_score = [opt.short.as_deref(), opt.long.as_deref()]
                .into_iter()
                .flatten()
                .filter_map(|flag| {
                    contextual_match_score(flag, query).or_else(|| {
                        if !query_body.is_empty() {
                            let flag_body = flag.trim_start_matches('-');
                            contextual_match_score(flag_body, query_body).map(|s| s + 50)
                        } else {
                            None
                        }
                    })
                })
                .min();
            let Some(score) = best_score else {
                continue;
            };
            out.push(CompletionItem {
                kind: CompletionKind::Option,
                label: opt.display_label(),
                detail: Some(opt.desc.clone()),
                detail_ar: opt.desc_ar.clone(),
                replacement: insert.to_owned(),
                replace_range: ctx.replace_range,
                score: score + depth_penalty + idx.min(60),
            });
        }
    }
}

fn add_arg_candidates(
    ctx: &ParsedContext<'_>,
    arg_kind: &ArgKind,
    systemctl_user: bool,
    inputs: &CompleteInputs<'_>,
    out: &mut Vec<CompletionItem>,
) {
    let query = ctx.query.as_str();
    match arg_kind {
        ArgKind::Path | ArgKind::File => {
            if let Some(pq) = inputs.path_query {
                add_path_candidates(pq, inputs.dir_entries, false, out);
            }
        }
        ArgKind::Dir => {
            if let Some(pq) = inputs.path_query {
                add_path_candidates(pq, inputs.dir_entries, true, out);
            }
        }
        ArgKind::Command => add_command_candidates(ctx, inputs, out),
        ArgKind::Values(values) => {
            for (idx, val) in values.iter().enumerate() {
                if let Some(score) = contextual_match_score(val, query) {
                    out.push(CompletionItem {
                        kind: CompletionKind::Value,
                        label: val.clone(),
                        detail: None,
                        detail_ar: None,
                        replacement: val.clone(),
                        replace_range: ctx.replace_range,
                        score: score + idx.min(50),
                    });
                }
            }
        }
        ArgKind::GitBranch => {
            add_generated(
                ctx,
                inputs.generated,
                GeneratorKind::GitBranches,
                CompletionKind::Branch,
                0,
                out,
            );
        }
        ArgKind::GitRemote => {
            add_generated(
                ctx,
                inputs.generated,
                GeneratorKind::GitRemotes,
                CompletionKind::Remote,
                0,
                out,
            );
        }
        ArgKind::GitTag => {
            add_generated(
                ctx,
                inputs.generated,
                GeneratorKind::GitTags,
                CompletionKind::Tag,
                0,
                out,
            );
        }
        ArgKind::GitRef => {
            add_generated(
                ctx,
                inputs.generated,
                GeneratorKind::GitBranches,
                CompletionKind::Branch,
                0,
                out,
            );
            add_generated(
                ctx,
                inputs.generated,
                GeneratorKind::GitTags,
                CompletionKind::Tag,
                20,
                out,
            );
        }
        ArgKind::GitChangedFile => {
            let before_len = out.len();
            add_generated(
                ctx,
                inputs.generated,
                GeneratorKind::GitChangedFiles,
                CompletionKind::Path,
                0,
                out,
            );
            // Also fall back to cwd paths if no changed files matched or user typed a path.
            if out.len() == before_len || query.contains('/') {
                if let Some(pq) = inputs.path_query {
                    add_path_candidates(pq, inputs.dir_entries, false, out);
                }
            }
        }
        ArgKind::NpmScript => {
            add_generated(
                ctx,
                inputs.generated,
                GeneratorKind::NpmScripts,
                CompletionKind::Script,
                0,
                out,
            );
        }
        ArgKind::MakeTarget => {
            add_generated(
                ctx,
                inputs.generated,
                GeneratorKind::MakeTargets,
                CompletionKind::Target,
                0,
                out,
            );
        }
        ArgKind::CargoTarget => {
            add_generated(
                ctx,
                inputs.generated,
                GeneratorKind::CargoTargets,
                CompletionKind::Target,
                0,
                out,
            );
        }
        ArgKind::CargoFeature => {
            add_generated(
                ctx,
                inputs.generated,
                GeneratorKind::CargoFeatures,
                CompletionKind::Feature,
                0,
                out,
            );
        }
        ArgKind::DockerContainer => {
            add_generated(
                ctx,
                inputs.generated,
                GeneratorKind::DockerContainers,
                CompletionKind::Container,
                0,
                out,
            );
        }
        ArgKind::DockerImage => {
            add_generated(
                ctx,
                inputs.generated,
                GeneratorKind::DockerImages,
                CompletionKind::Image,
                0,
                out,
            );
        }
        ArgKind::SystemctlUnit => {
            add_generated(
                ctx,
                inputs.generated,
                GeneratorKind::SystemctlUnits {
                    user: systemctl_user,
                },
                CompletionKind::Unit,
                0,
                out,
            );
        }
        ArgKind::SshHost => {
            add_generated(
                ctx,
                inputs.generated,
                GeneratorKind::SshHosts,
                CompletionKind::Host,
                0,
                out,
            );
        }
        ArgKind::Any => {}
    }
}

fn add_generated(
    ctx: &ParsedContext<'_>,
    generated: &HashMap<GeneratorKind, Vec<GeneratedItem>>,
    gen_kind: GeneratorKind,
    item_kind: CompletionKind,
    order_bias: usize,
    out: &mut Vec<CompletionItem>,
) {
    let Some(items) = generated.get(&gen_kind) else {
        return;
    };
    let query = ctx.query.as_str();
    for (idx, item) in items.iter().enumerate() {
        let Some(score) = contextual_match_score(&item.value, query) else {
            continue;
        };
        let replacement = format_path_replacement(&item.value, None);
        out.push(CompletionItem {
            kind: item_kind,
            label: item.value.clone(),
            detail: item.desc.clone(),
            detail_ar: None,
            replacement,
            replace_range: ctx.replace_range,
            score: score + order_bias + idx.min(80),
        });
    }
}

fn add_path_candidates(
    pq: &PathQuery,
    dir_entries: &[DirEntryInfo],
    dirs_only: bool,
    out: &mut Vec<CompletionItem>,
) {
    let show_hidden = pq.name_prefix.starts_with('.');
    for (idx, entry) in dir_entries.iter().enumerate() {
        if dirs_only && !entry.is_dir {
            continue;
        }
        if entry.name.starts_with('.') && !show_hidden {
            continue;
        }
        let Some(base_score) = contextual_match_score(&entry.name, &pq.name_prefix) else {
            continue;
        };
        let dir_bonus = if entry.is_dir { 0 } else { 30 };
        let mut full = format!("{}{}", pq.typed_dir, entry.name);
        if entry.is_dir {
            full.push('/');
        }
        let replacement = format_path_replacement(&full, pq.quote);
        out.push(CompletionItem {
            kind: CompletionKind::Path,
            label: full,
            detail: None,
            detail_ar: None,
            replacement,
            replace_range: pq.replace_range,
            score: base_score + dir_bonus + idx.min(120),
        });
    }
}

fn add_history_candidates(
    text: &str,
    caret: usize,
    history: &[String],
    out: &mut Vec<CompletionItem>,
) {
    let prefix = &text[..caret];
    let folded_query = fold(prefix.trim_start());
    if folded_query.is_empty() {
        return;
    }
    let query_words: Vec<String> = folded_query.split_whitespace().map(str::to_owned).collect();

    for (recency, entry) in history.iter().enumerate() {
        let recency_bonus = recency.min(400);
        let score = if entry.starts_with(prefix) && entry.len() > prefix.len() {
            2_000 + recency_bonus
        } else if folded_prefix_suffix(entry, prefix).is_some() {
            2_500 + recency_bonus
        } else if let Some(sub) = multi_word_score(&[entry.as_str()], &query_words) {
            3_200 + sub * 10 + recency_bonus
        } else {
            continue;
        };
        out.push(CompletionItem {
            kind: CompletionKind::History,
            label: single_line_preview(entry),
            detail: None,
            detail_ar: None,
            replacement: entry.clone(),
            replace_range: (0, text.len()),
            score,
        });
    }
}

fn add_workflow_candidates(
    text: &str,
    caret: usize,
    workflows: &[Workflow],
    out: &mut Vec<CompletionItem>,
) {
    let prefix = &text[..caret];
    let folded_query = fold(prefix.trim_start());
    let query_words: Vec<String> = folded_query.split_whitespace().map(str::to_owned).collect();

    for (idx, wf) in workflows.iter().filter(|w| w.trusted).enumerate() {
        let cmd = &wf.spec.command;
        let fields = [
            cmd.as_str(),
            wf.spec.name.as_str(),
            wf.spec.name_ar.as_str(),
        ];
        let score = if folded_query.is_empty() {
            4_800 + idx
        } else if cmd.starts_with(prefix) && !prefix.is_empty() {
            4_000 + idx
        } else if folded_prefix_suffix(cmd, prefix).is_some()
            || folded_prefix_suffix(&wf.spec.name, prefix).is_some()
            || folded_prefix_suffix(&wf.spec.name_ar, prefix).is_some()
        {
            4_400 + idx
        } else if let Some(sub) = multi_word_score(&fields, &query_words) {
            5_200 + sub * 10 + idx
        } else {
            continue;
        };
        let detail = (!wf.spec.name.is_empty()).then(|| wf.spec.name.clone());
        let detail_ar = (!wf.spec.name_ar.is_empty()).then(|| wf.spec.name_ar.clone());
        out.push(CompletionItem {
            kind: CompletionKind::Workflow,
            label: single_line_preview(cmd),
            detail,
            detail_ar,
            replacement: cmd.clone(),
            replace_range: (0, text.len()),
            score,
        });
    }
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

fn strip_outer_quotes(s: &str) -> &str {
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn next_char_len(s: &str, idx: usize) -> usize {
    s[idx..].chars().next().map_or(1, char::len_utf8)
}
