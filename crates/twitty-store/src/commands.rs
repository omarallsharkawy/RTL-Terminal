//! Frequent project commands derived from local command history with success rates.
//!
//! Trivial navigation and shell-housekeeping commands (`cd`, `ls`, `pwd`, `clear`, `exit`, etc.)
//! are filtered out so project memory captures meaningful build, test, lint, and dev commands
//! rather than copying raw shell history blindly.

use std::collections::HashMap;

use crate::store::HistoryEntry;

/// Source tag stored on auto-computed frequent command memory rows.
pub const AUTO_COMMAND_SOURCE: &str = "auto:history";

/// Default minimum executions required for a command to qualify as frequent.
pub const DEFAULT_MIN_COMMAND_RUNS: usize = 2;

/// Default maximum frequent commands surfaced per project.
pub const DEFAULT_FREQUENT_COMMAND_LIMIT: usize = 10;

/// One frequent command derived from a project's history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrequentCommand {
    pub command: String,
    pub runs: u64,
    pub successes: u64,
    pub known_exits: u64,
    /// Rounded percentage (`0..=100`) when at least one run recorded an exit code.
    pub success_rate_pct: Option<u8>,
    pub last_run_at: i64,
}

impl FrequentCommand {
    /// Human- and AI-readable summary line for this frequent command.
    pub fn format_note(&self) -> String {
        match self.success_rate_pct {
            Some(pct) => format!("`{}` ({} runs, {}% ok)", self.command, self.runs, pct),
            None => format!("`{}` ({} runs)", self.command, self.runs),
        }
    }
}

/// Compute frequent, non-trivial commands with success rates from `history`.
///
/// Order is strictly deterministic: highest run count first, then highest success rate, then most
/// recently executed, then lexicographical by normalized command.
pub fn compute_frequent_commands(
    history: &[HistoryEntry],
    min_runs: usize,
    limit: usize,
) -> Vec<FrequentCommand> {
    if limit == 0 || history.is_empty() {
        return Vec::new();
    }
    let min_runs = min_runs.max(1) as u64;

    struct Acc {
        runs: u64,
        successes: u64,
        known_exits: u64,
        last_run_at: i64,
    }

    let mut counts: HashMap<String, Acc> = HashMap::new();
    for entry in history {
        let norm = normalize_command(&entry.command);
        if norm.is_empty() || is_trivial_command(&norm) {
            continue;
        }
        let acc = counts.entry(norm).or_insert(Acc {
            runs: 0,
            successes: 0,
            known_exits: 0,
            last_run_at: entry.started_at,
        });
        acc.runs = acc.runs.saturating_add(1);
        if let Some(code) = entry.exit_code {
            acc.known_exits = acc.known_exits.saturating_add(1);
            if code == 0 {
                acc.successes = acc.successes.saturating_add(1);
            }
        }
        if entry.started_at > acc.last_run_at {
            acc.last_run_at = entry.started_at;
        }
    }

    let mut out: Vec<FrequentCommand> = counts
        .into_iter()
        .filter(|(_, acc)| acc.runs >= min_runs)
        .map(|(command, acc)| {
            let success_rate_pct = (acc.known_exits > 0).then(|| {
                let pct =
                    (acc.successes.saturating_mul(100) + acc.known_exits / 2) / acc.known_exits;
                pct.min(100) as u8
            });
            FrequentCommand {
                command,
                runs: acc.runs,
                successes: acc.successes,
                known_exits: acc.known_exits,
                success_rate_pct,
                last_run_at: acc.last_run_at,
            }
        })
        .collect();

    out.sort_by(|a, b| {
        b.runs
            .cmp(&a.runs)
            .then_with(|| b.success_rate_pct.cmp(&a.success_rate_pct))
            .then_with(|| b.last_run_at.cmp(&a.last_run_at))
            .then_with(|| a.command.cmp(&b.command))
    });
    out.truncate(limit);
    out
}

fn normalize_command(raw: &str) -> String {
    let mut out = String::new();
    for part in raw.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(part);
    }
    out
}

fn is_trivial_command(norm: &str) -> bool {
    let first = norm.split_whitespace().next().unwrap_or("");
    matches!(
        first,
        "cd" | "ls"
            | "ll"
            | "la"
            | "l"
            | "pwd"
            | "clear"
            | "cls"
            | "exit"
            | "logout"
            | "history"
            | "true"
            | "false"
            | "which"
            | "whoami"
            | "id"
            | "date"
            | "pushd"
            | "popd"
            | "dirs"
    )
}
