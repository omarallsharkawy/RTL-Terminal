//! A unified diff (`git diff` output) parsed into files, hunks and lines with their old and new
//! line numbers. Everything is bounded: a cut input, too many files or too many lines never
//! make the panel slow; what does not fit becomes a one-line entry or a note.

/// Most diff bytes read from git (untracked files share the budget).
pub const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024;
/// Most files listed; the rest are counted.
pub const MAX_FILES: usize = 200;
/// Most diff lines kept over all files; files after that are one-line entries.
pub const MAX_LINES: usize = 20_000;
/// A file with more diff lines than this is a one-line entry.
pub const MAX_FILE_LINES: usize = 5_000;
/// Longest line text kept, in characters (the rest is cut with `…`).
pub const MAX_LINE_CHARS: usize = 1_000;

/// The bounds a parse keeps to (tests use smaller ones).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_files: usize,
    pub max_lines: usize,
    pub max_file_lines: usize,
    pub max_line_chars: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_files: MAX_FILES,
            max_lines: MAX_LINES,
            max_file_lines: MAX_FILE_LINES,
            max_line_chars: MAX_LINE_CHARS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Add,
    Del,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// Line number in the old file (context and deleted lines).
    pub old: Option<u32>,
    /// Line number in the new file (context and added lines).
    pub new: Option<u32>,
    /// The line without its `+`/`-`/` ` prefix and line ending (a CR before the LF is dropped).
    pub text: String,
    /// Followed by `\ No newline at end of file`.
    pub no_newline: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    /// The text after the closing `@@` (usually the enclosing function).
    pub section: String,
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    /// The `@@ -a,b +c,d @@ section` header.
    pub fn header(&self) -> String {
        let mut text = format!(
            "@@ -{},{} +{},{} @@",
            self.old_start, self.old_count, self.new_start, self.new_count
        );
        if !self.section.is_empty() {
            text.push(' ');
            text.push_str(&self.section);
        }
        text
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
}

impl FileStatus {
    pub fn letter(self) -> char {
        match self {
            Self::Modified => 'M',
            Self::Added => 'A',
            Self::Deleted => 'D',
            Self::Renamed => 'R',
            Self::Copied => 'C',
        }
    }
}

/// Why a file is a one-line entry instead of its hunks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OneLine {
    Binary,
    /// More lines than the bounds allow (its `+`/`-` counts are still right).
    TooLarge,
    /// An untracked file too big to read (its size in bytes).
    TooBig(u64),
    /// An untracked symlink, FIFO or other non-regular file.
    Special,
    /// An untracked file that could not be read.
    Unreadable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    /// Path before the change (`None` for an added file).
    pub old_path: Option<String>,
    /// Path after the change (`None` for a deleted file).
    pub new_path: Option<String>,
    pub status: FileStatus,
    /// Rename or copy similarity, in percent.
    pub similarity: Option<u8>,
    pub old_mode: Option<String>,
    pub new_mode: Option<String>,
    /// Not in git yet (listed by `git ls-files --others`).
    pub untracked: bool,
    pub one_line: Option<OneLine>,
    pub added: usize,
    pub removed: usize,
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    fn new() -> Self {
        Self {
            old_path: None,
            new_path: None,
            status: FileStatus::Modified,
            similarity: None,
            old_mode: None,
            new_mode: None,
            untracked: false,
            one_line: None,
            added: 0,
            removed: 0,
            hunks: Vec::new(),
        }
    }

    /// The path comments refer to: the new path, or the old one for a deleted file.
    pub fn path(&self) -> &str {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .unwrap_or("")
    }

    /// `old → new` for renames and copies, else the path.
    pub fn display_path(&self) -> String {
        match (&self.old_path, &self.new_path, self.status) {
            (Some(old), Some(new), FileStatus::Renamed | FileStatus::Copied) if old != new => {
                format!("{old} → {new}")
            }
            _ => self.path().to_owned(),
        }
    }

    /// `old → new` file mode, when only the mode (or the mode too) changed.
    pub fn mode_change(&self) -> Option<String> {
        match (&self.old_mode, &self.new_mode) {
            (Some(old), Some(new)) if old != new => Some(format!("{old} → {new}")),
            _ => None,
        }
    }

    fn line_count(&self) -> usize {
        self.hunks.iter().map(|h| h.lines.len()).sum()
    }
}

/// A parsed diff and what did not fit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diff {
    pub files: Vec<FileDiff>,
    /// Files past `max_files`, counted but not listed.
    pub omitted_files: usize,
    /// The input was cut at its byte bound: the last file may be incomplete.
    pub truncated: bool,
    /// The line bound was reached: later files are one-line entries.
    pub lines_capped: bool,
    /// Diff lines kept over all files.
    pub stored_lines: usize,
}

impl Diff {
    /// Added and removed lines over all files.
    pub fn totals(&self) -> (usize, usize) {
        self.files
            .iter()
            .fold((0, 0), |(a, r), f| (a + f.added, r + f.removed))
    }

    /// The number of files changed, listed or not.
    pub fn file_count(&self) -> usize {
        self.files.len() + self.omitted_files
    }

    /// Largest line number shown, for the gutter width.
    pub fn max_line_number(&self) -> u32 {
        self.files
            .iter()
            .flat_map(|f| &f.hunks)
            .map(|h| {
                (h.old_start + h.old_count)
                    .max(h.new_start + h.new_count)
                    .saturating_sub(1)
            })
            .max()
            .unwrap_or(0)
    }

    /// Add a finished file, keeping to the bounds.
    fn push(&mut self, mut file: FileDiff, limits: &Limits) {
        if self.files.len() >= limits.max_files {
            self.omitted_files += 1;
            return;
        }
        let lines = file.line_count();
        if file.one_line.is_none()
            && (lines > limits.max_file_lines || self.stored_lines + lines > limits.max_lines)
        {
            if self.stored_lines + lines > limits.max_lines && lines <= limits.max_file_lines {
                self.lines_capped = true;
            }
            file.one_line = Some(OneLine::TooLarge);
        }
        if file.one_line.is_some() {
            file.hunks.clear();
        } else {
            self.stored_lines += lines;
        }
        self.files.push(file);
    }

    /// Add an untracked file as an added file. `content` is its text, or why it has none.
    pub fn push_untracked(&mut self, path: &str, content: Untracked, limits: &Limits) {
        let mut file = FileDiff::new();
        file.status = FileStatus::Added;
        file.untracked = true;
        file.new_path = Some(path.to_owned());
        match content {
            Untracked::Text(bytes) => {
                if bytes.iter().take(8000).any(|&b| b == 0) {
                    file.one_line = Some(OneLine::Binary);
                } else {
                    let mut lines = Vec::new();
                    let mut cut = false;
                    let body = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
                    let ends_with_newline = body.len() != bytes.len();
                    if !bytes.is_empty() {
                        for (i, raw) in body.split(|&b| b == b'\n').enumerate() {
                            file.added += 1;
                            if lines.len() <= limits.max_file_lines {
                                lines.push(DiffLine {
                                    kind: LineKind::Add,
                                    old: None,
                                    new: Some(i as u32 + 1),
                                    text: line_text(raw, limits.max_line_chars),
                                    no_newline: false,
                                });
                            } else {
                                cut = true;
                            }
                        }
                    }
                    if let Some(last) = lines.last_mut().filter(|_| !ends_with_newline) {
                        last.no_newline = true;
                    }
                    if !lines.is_empty() {
                        file.hunks.push(Hunk {
                            old_start: 0,
                            old_count: 0,
                            new_start: 1,
                            new_count: file.added as u32,
                            section: String::new(),
                            lines,
                        });
                    }
                    if cut {
                        file.one_line = Some(OneLine::TooLarge);
                    }
                }
            }
            Untracked::TooBig(size) => file.one_line = Some(OneLine::TooBig(size)),
            Untracked::Special => file.one_line = Some(OneLine::Special),
            Untracked::Unreadable => file.one_line = Some(OneLine::Unreadable),
        }
        self.push(file, limits);
    }
}

/// What an untracked file holds, as read by `git::load`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Untracked {
    Text(Vec<u8>),
    TooBig(u64),
    Special,
    Unreadable,
}

/// A line's text: lossy UTF-8, without a trailing CR, at most `max` characters.
fn line_text(raw: &[u8], max: usize) -> String {
    let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
    let text = String::from_utf8_lossy(raw);
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.into_owned(),
    }
}

/// Parse `git diff` output. `truncated` says the input was cut at its byte bound (its last,
/// partial line is then dropped).
pub fn parse(input: &[u8], truncated: bool, limits: &Limits) -> Diff {
    let mut diff = Diff {
        truncated,
        ..Diff::default()
    };
    let mut input = input;
    if truncated {
        // The bytes after the last newline are part of a line that was cut.
        input = match input.iter().rposition(|&b| b == b'\n') {
            Some(end) => &input[..=end],
            None => &[],
        };
    }
    let mut parser = Parser {
        file: None,
        hunk: None,
        old_left: 0,
        new_left: 0,
        old_line: 0,
        new_line: 0,
        limits: *limits,
    };
    let body = input.strip_suffix(b"\n").unwrap_or(input);
    if !body.is_empty() {
        for raw in body.split(|&b| b == b'\n') {
            parser.line(raw, &mut diff);
        }
    }
    parser.finish_file(&mut diff);
    diff
}

struct Parser {
    file: Option<FileDiff>,
    hunk: Option<Hunk>,
    old_left: u32,
    new_left: u32,
    old_line: u32,
    new_line: u32,
    limits: Limits,
}

impl Parser {
    fn finish_hunk(&mut self) {
        if let (Some(hunk), Some(file)) = (self.hunk.take(), self.file.as_mut()) {
            file.hunks.push(hunk);
        }
        self.old_left = 0;
        self.new_left = 0;
    }

    fn finish_file(&mut self, diff: &mut Diff) {
        self.finish_hunk();
        if let Some(file) = self.file.take() {
            diff.push(file, &self.limits);
        }
    }

    fn line(&mut self, raw: &[u8], diff: &mut Diff) {
        if raw.starts_with(b"diff --git ") {
            self.finish_file(diff);
            let mut file = FileDiff::new();
            let header = String::from_utf8_lossy(&raw[b"diff --git ".len()..]);
            if let Some((old, new)) = git_header_paths(header.trim_end_matches('\r')) {
                file.old_path = Some(old);
                file.new_path = Some(new);
            }
            self.file = Some(file);
            return;
        }
        if self.hunk.is_some() {
            if raw.starts_with(b"\\") {
                // `\ No newline at end of file`: about the line before it.
                if let Some(line) = self.hunk.as_mut().and_then(|h| h.lines.last_mut()) {
                    line.no_newline = true;
                }
                return;
            }
            if self.old_left > 0 || self.new_left > 0 {
                let max = self.limits.max_line_chars;
                let (kind, text) = match raw.first() {
                    Some(b'+') if self.new_left > 0 => (LineKind::Add, &raw[1..]),
                    Some(b'-') if self.old_left > 0 => (LineKind::Del, &raw[1..]),
                    Some(b' ') => (LineKind::Context, &raw[1..]),
                    // Some tools drop the space of an empty context line.
                    None => (LineKind::Context, raw),
                    _ => {
                        self.finish_hunk();
                        return self.header_line(raw);
                    }
                };
                let line = match kind {
                    LineKind::Context => {
                        self.old_left = self.old_left.saturating_sub(1);
                        self.new_left = self.new_left.saturating_sub(1);
                        let (old, new) = (self.old_line, self.new_line);
                        self.old_line += 1;
                        self.new_line += 1;
                        DiffLine {
                            kind,
                            old: Some(old),
                            new: Some(new),
                            text: line_text(text, max),
                            no_newline: false,
                        }
                    }
                    LineKind::Add => {
                        self.new_left -= 1;
                        let new = self.new_line;
                        self.new_line += 1;
                        if let Some(f) = self.file.as_mut() {
                            f.added += 1;
                        }
                        DiffLine {
                            kind,
                            old: None,
                            new: Some(new),
                            text: line_text(text, max),
                            no_newline: false,
                        }
                    }
                    LineKind::Del => {
                        self.old_left -= 1;
                        let old = self.old_line;
                        self.old_line += 1;
                        if let Some(f) = self.file.as_mut() {
                            f.removed += 1;
                        }
                        DiffLine {
                            kind,
                            old: Some(old),
                            new: None,
                            text: line_text(text, max),
                            no_newline: false,
                        }
                    }
                };
                if let Some(hunk) = self.hunk.as_mut() {
                    // Lines past the per-file bound are counted but not kept.
                    let kept: usize = self
                        .file
                        .as_ref()
                        .map_or(0, |f| f.line_count() + hunk.lines.len());
                    if kept <= self.limits.max_file_lines {
                        hunk.lines.push(line);
                    } else if let Some(f) = self.file.as_mut() {
                        f.one_line = Some(OneLine::TooLarge);
                    }
                }
                return;
            }
            self.finish_hunk();
        }
        self.header_line(raw);
    }

    /// A line outside a hunk body: a hunk start or a file header line.
    fn header_line(&mut self, raw: &[u8]) {
        let Some(file) = self.file.as_mut() else {
            return;
        };
        let text = String::from_utf8_lossy(raw);
        let text = text.trim_end_matches('\r');
        if let Some(rest) = text.strip_prefix("@@ ") {
            if let Some(hunk) = parse_hunk_header(rest) {
                self.old_left = hunk.old_count;
                self.new_left = hunk.new_count;
                self.old_line = hunk.old_start;
                self.new_line = hunk.new_start;
                self.hunk = Some(hunk);
            }
        } else if let Some(mode) = text.strip_prefix("old mode ") {
            file.old_mode = Some(mode.to_owned());
        } else if let Some(mode) = text.strip_prefix("new mode ") {
            file.new_mode = Some(mode.to_owned());
        } else if let Some(mode) = text.strip_prefix("new file mode ") {
            file.status = FileStatus::Added;
            file.new_mode = Some(mode.to_owned());
            file.old_path = None;
        } else if let Some(mode) = text.strip_prefix("deleted file mode ") {
            file.status = FileStatus::Deleted;
            file.old_mode = Some(mode.to_owned());
            file.new_path = None;
        } else if let Some(rest) = text
            .strip_prefix("similarity index ")
            .or_else(|| text.strip_prefix("copy similarity index "))
        {
            file.similarity = rest.trim_end_matches('%').parse().ok();
        } else if let Some(path) = text.strip_prefix("rename from ") {
            file.status = FileStatus::Renamed;
            file.old_path = Some(unquote(path));
        } else if let Some(path) = text.strip_prefix("rename to ") {
            file.status = FileStatus::Renamed;
            file.new_path = Some(unquote(path));
        } else if let Some(path) = text.strip_prefix("copy from ") {
            file.status = FileStatus::Copied;
            file.old_path = Some(unquote(path));
        } else if let Some(path) = text.strip_prefix("copy to ") {
            file.status = FileStatus::Copied;
            file.new_path = Some(unquote(path));
        } else if text.starts_with("Binary files ") || text == "GIT binary patch" {
            file.one_line = Some(OneLine::Binary);
        } else if let Some(path) = text.strip_prefix("--- ") {
            file.old_path = side_path(path, "a/");
        } else if let Some(path) = text.strip_prefix("+++ ") {
            file.new_path = side_path(path, "b/");
        }
    }
}

/// `-a,b +c,d @@ section` (after the leading `@@ `).
fn parse_hunk_header(rest: &str) -> Option<Hunk> {
    let (ranges, section) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(' ')?;
    let range = |s: &str, sign: char| -> Option<(u32, u32)> {
        let s = s.strip_prefix(sign)?;
        match s.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((s.parse().ok()?, 1)),
        }
    };
    let (old_start, old_count) = range(old, '-')?;
    let (new_start, new_count) = range(new, '+')?;
    Some(Hunk {
        old_start,
        old_count,
        new_start,
        new_count,
        section: section.trim().to_owned(),
        lines: Vec::new(),
    })
}

/// The path of a `---`/`+++` line: `None` for `/dev/null`, else without its `a/` or `b/` prefix
/// and the tab git adds after names with spaces.
fn side_path(text: &str, prefix: &str) -> Option<String> {
    let text = if text.starts_with('"') {
        text.to_owned()
    } else {
        text.split('\t').next().unwrap_or(text).to_owned()
    };
    if text == "/dev/null" {
        return None;
    }
    let path = unquote(&text);
    Some(path.strip_prefix(prefix).map(str::to_owned).unwrap_or(path))
}

/// The two paths of `diff --git a/x b/y` (used when no `---`/`+++` lines follow: binary,
/// mode-only and pure renames).
fn git_header_paths(rest: &str) -> Option<(String, String)> {
    let strip = |p: String, prefix: &str| p.strip_prefix(prefix).map(str::to_owned).unwrap_or(p);
    if rest.starts_with('"') {
        let (old, tail) = quoted_token(rest)?;
        let tail = tail.trim_start();
        let new = if tail.starts_with('"') {
            quoted_token(tail)?.0
        } else {
            tail.to_owned()
        };
        return Some((strip(old, "a/"), strip(new, "b/")));
    }
    // Unquoted: the same name twice is the common case; split it in the middle.
    if rest.len() % 2 == 1 {
        let half = rest.len() / 2;
        if rest.is_char_boundary(half) && rest.is_char_boundary(half + 1) {
            let (a, b) = (&rest[..half], &rest[half + 1..]);
            if let (Some(a1), Some(b1)) = (a.strip_prefix("a/"), b.strip_prefix("b/")) {
                if a1 == b1 && rest.as_bytes()[half] == b' ' {
                    return Some((a1.to_owned(), b1.to_owned()));
                }
            }
        }
    }
    if let Some((a, b)) = rest.split_once(" \"") {
        let new = quoted_token(&format!("\"{b}"))?.0;
        return Some((strip(a.to_owned(), "a/"), strip(new, "b/")));
    }
    let (a, b) = rest.split_once(" b/")?;
    Some((strip(a.to_owned(), "a/"), b.to_owned()))
}

/// A C-style quoted token at the start of `text` (git quotes names with special characters)
/// and the rest after it.
fn quoted_token(text: &str) -> Option<(String, &str)> {
    let body = text.strip_prefix('"')?;
    let bytes = body.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => return Some((String::from_utf8_lossy(&out).into_owned(), &body[i + 1..])),
            b'\\' if i + 1 < bytes.len() => {
                i += 1;
                match bytes[i] {
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    b'r' => out.push(b'\r'),
                    b'a' => out.push(7),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'v' => out.push(11),
                    d @ b'0'..=b'7' => {
                        let mut value = u32::from(d - b'0');
                        for _ in 0..2 {
                            match bytes.get(i + 1) {
                                Some(&o @ b'0'..=b'7') => {
                                    value = value * 8 + u32::from(o - b'0');
                                    i += 1;
                                }
                                _ => break,
                            }
                        }
                        out.push(value as u8);
                    }
                    other => out.push(other),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    None
}

/// A path as git prints it: unquoted when it was quoted.
pub fn unquote(text: &str) -> String {
    match quoted_token(text) {
        Some((path, _)) => path,
        None => text.to_owned(),
    }
}
