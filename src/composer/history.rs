//! Command history for the composer: combines Twitty's session command blocks with the user's
//! shell history files (`$HISTFILE`, `~/.zsh_history`, `~/.bash_history`, fish history),
//! deduplicated, newest first, bounded, and read off the UI thread.

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Maximum distinct history entries kept in memory.
pub const MAX_HISTORY: usize = 5_000;
/// Maximum bytes read from the tail of any single shell history file (4 MiB).
pub const MAX_HISTORY_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Maximum characters kept for any single history command.
pub const MAX_HISTORY_COMMAND_CHARS: usize = 4_096;

/// Combined, deduplicated command history (newest first).
#[derive(Clone, Debug, Default)]
pub struct HistoryStore {
    /// Commands submitted in this Twitty session (from blocks and composer), newest first.
    session: Vec<String>,
    /// Commands loaded from shell history files, newest first.
    shell: Vec<String>,
    /// Merged, deduplicated, bounded view (`session` then `shell`), newest first.
    merged: Vec<String>,
}

impl HistoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn entries(&self) -> &[String] {
        &self.merged
    }

    pub fn is_empty(&self) -> bool {
        self.merged.is_empty()
    }

    pub fn len(&self) -> usize {
        self.merged.len()
    }

    /// Record a command submitted during this session (becomes the newest entry).
    pub fn push_session(&mut self, command: &str) {
        let Some(clean) = clean_history_command(command) else {
            return;
        };
        if self.session.first().is_some_and(|first| first == &clean) {
            return;
        }
        self.session.retain(|c| c != &clean);
        self.session.insert(0, clean);
        self.session.truncate(MAX_HISTORY);
        self.rebuild();
    }

    /// Replace the session command snapshot harvested from Twitty's command blocks across panes
    /// (`commands` in newest-first order), keeping any existing composer submissions ahead of
    /// older block entries if not already present.
    pub fn merge_session_commands(&mut self, commands: impl IntoIterator<Item = String>) {
        let mut combined = Vec::new();
        let mut seen = HashSet::new();
        for cmd in self.session.iter().cloned().chain(
            commands
                .into_iter()
                .filter_map(|c| clean_history_command(&c)),
        ) {
            if seen.insert(cmd.clone()) {
                combined.push(cmd);
                if combined.len() >= MAX_HISTORY {
                    break;
                }
            }
        }
        self.session = combined;
        self.rebuild();
    }

    /// Set the shell history loaded from disk (newest first) and rebuild the deduplicated list.
    pub fn set_shell_history(&mut self, commands: Vec<String>) {
        self.shell = dedup_newest_first(commands, MAX_HISTORY);
        self.rebuild();
    }

    fn rebuild(&mut self) {
        self.merged = dedup_newest_first(
            self.session
                .iter()
                .cloned()
                .chain(self.shell.iter().cloned()),
            MAX_HISTORY,
        );
    }
}

/// Clean a history command: strip NUL and bidi overrides, normalize `\r\n`, keep internal `\n`,
/// trim trailing/leading whitespace-only noise, and bound its length.
pub fn clean_history_command(raw: &str) -> Option<String> {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let cleaned: String = normalized
        .chars()
        .filter(|&c| {
            (c == '\n' || c == '\t' || !c.is_control())
                && !crate::terminal::is_bidi_control(c)
                && !matches!(c, '\u{2028}' | '\u{2029}')
        })
        .take(MAX_HISTORY_COMMAND_CHARS)
        .collect();
    let trimmed = cleaned.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Deduplicate `entries` preserving first-seen (newest-first) order, bounded to `limit`.
pub fn dedup_newest_first(entries: impl IntoIterator<Item = String>, limit: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for entry in entries {
        let Some(clean) = clean_history_command(&entry) else {
            continue;
        };
        if seen.insert(clean.clone()) {
            out.push(clean);
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}

/// Parse `~/.bash_history`: plain lines, skipping `#<timestamp>` lines emitted by
/// `HISTTIMEFORMAT`. Returns newest first.
pub fn parse_bash_history(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut entries = Vec::new();
    for line in text.lines() {
        if is_bash_timestamp(line) {
            continue;
        }
        if let Some(clean) = clean_history_command(line) {
            entries.push(clean);
        }
    }
    entries.reverse();
    dedup_newest_first(entries, MAX_HISTORY)
}

fn is_bash_timestamp(line: &str) -> bool {
    let Some(rest) = line.strip_prefix('#') else {
        return false;
    };
    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
}

/// Unmetafy raw zsh history bytes (`0x83, b` -> `b ^ 0x20`).
///
/// Zsh writes UTF-8 multibyte bytes in `0x83..=0x9d` (including Arabic letters such as `ل`, `م`,
/// `ك`) metafied in `~/.zsh_history`.
pub fn unmetafy_zsh(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == 0x83 && i + 1 < raw.len() {
            out.push(raw[i + 1] ^ 0x20);
            i += 2;
        } else {
            out.push(raw[i]);
            i += 1;
        }
    }
    out
}

fn decode_zsh_line(raw: &[u8]) -> String {
    let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
    if let Ok(s) = std::str::from_utf8(raw) {
        return s.to_owned();
    }
    let unmetafied = unmetafy_zsh(raw);
    String::from_utf8_lossy(&unmetafied).into_owned()
}

/// Strip zsh extended history prefix `: <timestamp>:<duration>;` if present.
fn strip_zsh_extended_prefix(line: &str) -> &str {
    let Some(rest) = line.strip_prefix(": ") else {
        return line;
    };
    let Some((ts, after_ts)) = rest.split_once(':') else {
        return line;
    };
    if ts.is_empty() || !ts.bytes().all(|b| b.is_ascii_digit()) {
        return line;
    }
    let Some((dur, cmd)) = after_ts.split_once(';') else {
        return line;
    };
    if dur.is_empty() || !dur.bytes().all(|b| b.is_ascii_digit()) {
        return line;
    }
    cmd
}

/// Parse `~/.zsh_history` in both plain and extended (`: <ts>:<dur>;<cmd>`) formats, handling
/// trailing-backslash multiline continuations and zsh metafied UTF-8 bytes. Returns newest first.
pub fn parse_zsh_history(bytes: &[u8]) -> Vec<String> {
    let mut entries = Vec::new();
    let mut current: Option<String> = None;

    for raw_line in bytes.split(|&b| b == b'\n') {
        let decoded = decode_zsh_line(raw_line);
        if let Some(mut buf) = current.take() {
            buf.push('\n');
            if let Some(cont) = decoded.strip_suffix('\\') {
                buf.push_str(cont);
                current = Some(buf);
            } else {
                buf.push_str(&decoded);
                if let Some(clean) = clean_history_command(&buf) {
                    entries.push(clean);
                }
            }
            continue;
        }
        let body = strip_zsh_extended_prefix(&decoded);
        if let Some(cont) = body.strip_suffix('\\') {
            current = Some(cont.to_owned());
        } else if let Some(clean) = clean_history_command(body) {
            entries.push(clean);
        }
    }
    if let Some(buf) = current {
        if let Some(clean) = clean_history_command(&buf) {
            entries.push(clean);
        }
    }
    entries.reverse();
    dedup_newest_first(entries, MAX_HISTORY)
}

/// Parse fish shell history (`- cmd: ...` YAML-like format, unescaping `\n` and `\\`).
/// Returns newest first.
pub fn parse_fish_history(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut entries = Vec::new();
    for line in text.lines() {
        let Some(raw_cmd) = line.strip_prefix("- cmd: ") else {
            continue;
        };
        let unescaped = unescape_fish_cmd(raw_cmd);
        if let Some(clean) = clean_history_command(&unescaped) {
            entries.push(clean);
        }
    }
    entries.reverse();
    dedup_newest_first(entries, MAX_HISTORY)
}

fn unescape_fish_cmd(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Detect history format from filename and content, then parse newest-first.
pub fn parse_auto_history(bytes: &[u8], path: Option<&Path>) -> Vec<String> {
    let name = path
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if name.contains("fish") {
        return parse_fish_history(bytes);
    }
    if name.contains("zsh") {
        return parse_zsh_history(bytes);
    }
    if name.contains("bash") {
        return parse_bash_history(bytes);
    }
    // Sniff content for arbitrary `$HISTFILE` names.
    let sample = &bytes[..bytes.len().min(4096)];
    let sample_str = String::from_utf8_lossy(sample);
    if sample_str.lines().any(|l| l.starts_with("- cmd: ")) {
        parse_fish_history(bytes)
    } else if sample_str
        .lines()
        .any(|l| strip_zsh_extended_prefix(l) != l)
        || sample.contains(&0x83)
    {
        parse_zsh_history(bytes)
    } else {
        parse_bash_history(bytes)
    }
}

/// Read up to the last `MAX_HISTORY_FILE_BYTES` of a regular file (non-blocking open on Unix so
/// a FIFO at a history path can never hang a worker thread).
fn read_tail_bounded(path: &Path) -> Option<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let mut file = options.open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.file_type().is_file() {
        return None;
    }
    let len = meta.len();
    let skip_first_partial_line = if len > MAX_HISTORY_FILE_BYTES {
        file.seek(SeekFrom::Start(len - MAX_HISTORY_FILE_BYTES))
            .ok()?;
        true
    } else {
        false
    };
    let mut buf = Vec::new();
    file.take(MAX_HISTORY_FILE_BYTES)
        .read_to_end(&mut buf)
        .ok()?;
    if skip_first_partial_line {
        if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            buf.drain(..=pos);
        }
    }
    Some(buf)
}

/// Candidate shell history files in priority order (newest/active shell first).
pub fn default_history_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut push_unique = |p: PathBuf| {
        if p.is_absolute() && !paths.contains(&p) {
            paths.push(p);
        }
    };
    if let Some(histfile) = std::env::var_os("HISTFILE").filter(|v| !v.is_empty()) {
        push_unique(PathBuf::from(histfile));
    }
    let home = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute());
    if let Some(ref home) = home {
        push_unique(home.join(".zsh_history"));
        push_unique(home.join(".bash_history"));
    }
    if let Some(xdg_data) = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        push_unique(xdg_data.join("fish").join("fish_history"));
    }
    if let Some(ref home) = home {
        push_unique(home.join(".local/share/fish/fish_history"));
    }
    paths
}

/// Load and merge history from the given file paths (in priority order, newest first).
/// Designed to run off the UI thread.
pub fn load_shell_history_from(paths: &[PathBuf]) -> Vec<String> {
    let mut all = Vec::new();
    for path in paths {
        if let Some(bytes) = read_tail_bounded(path) {
            all.extend(parse_auto_history(&bytes, Some(path)));
        }
    }
    dedup_newest_first(all, MAX_HISTORY)
}

/// Load shell history from standard user history files. Call off the UI thread.
pub fn load_shell_history_default() -> Vec<String> {
    load_shell_history_from(&default_history_paths())
}
