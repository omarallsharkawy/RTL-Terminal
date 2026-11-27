//! Command blocks: each command a shell ran, with its prompt, command line, output, exit status
//! and duration, built from OSC 133 marks (`A` prompt, `B` command, `C` output, `D;exit` end)
//! and OSC 633 `E` command lines.
//!
//! Window-free: a [`BlockList`] per pane is fed the pane's [`ShellEvent`]s and reads the
//! terminal only through [`Terminal::line_view`] and [`Terminal::text_between`]. Marks are
//! placed by absolute line (see [`anchor`]), so blocks follow their text into the scrollback
//! and are dropped once it leaves it. Programs on the alternate screen (vim, htop, agents' full
//! screen UIs) never create blocks.
//!
//! The list stays ordered by prompt line: a prompt drawn at or above an earlier block's prompt
//! means the screen was cleared or rewritten there (`clear`, `Ctrl+L`, a redrawn prompt), so the
//! blocks from that line down are dropped.

pub mod anchor;

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::shell_integration::{MarkKind, ShellEvent, Signal};
use crate::terminal::{LineView, Terminal};
use anchor::AbsLine;

/// Blocks kept per pane; the oldest are dropped first.
pub const MAX_BLOCKS: usize = 5000;

/// A cell by absolute line and column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AbsPoint {
    pub line: AbsLine,
    pub column: usize,
}

/// One command the shell ran.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    id: u64,
    /// Where the prompt starts (OSC 133 A).
    pub prompt: AbsPoint,
    /// Where the typed command starts (B).
    pub command_start: Option<AbsPoint>,
    /// Where the output starts (C).
    pub output_start: Option<AbsPoint>,
    /// Where the next prompt starts, i.e. one past the output (D).
    pub end: Option<AbsPoint>,
    /// Exit status from D, when the shell gave one.
    pub exit: Option<i32>,
    /// The command line: OSC 633 E when the shell sent it, else what was typed after the
    /// prompt (read from the grid at C). Cleaned and bounded.
    pub command: Option<String>,
    pub started: Option<Instant>,
    pub finished: Option<Instant>,
    /// User-bookmarked block for quick jump (`block.next-bookmark`).
    pub bookmarked: bool,
}

impl Block {
    fn new(id: u64, prompt: AbsPoint) -> Self {
        Self {
            id,
            prompt,
            command_start: None,
            output_start: None,
            end: None,
            exit: None,
            command: None,
            started: None,
            finished: None,
            bookmarked: false,
        }
    }

    /// Stable identifier of this block within its pane.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The command started (C) and has not ended (D).
    pub fn is_running(&self) -> bool {
        self.end.is_none()
    }

    /// How long the command ran, once it ended.
    pub fn duration(&self) -> Option<Duration> {
        Some(self.finished?.saturating_duration_since(self.started?))
    }

    pub fn status(&self) -> BlockStatus {
        match (self.end, self.exit) {
            (None, _) => BlockStatus::Running,
            (Some(_), Some(0)) => BlockStatus::Succeeded,
            (Some(_), Some(code)) => BlockStatus::Failed(code),
            (Some(_), None) => BlockStatus::Finished,
        }
    }

    /// Last line of the block: the line before the next prompt (the cursor's while running).
    fn last_line(&self, view: &LineView) -> AbsLine {
        match self.end {
            Some(end) if end.column == 0 => end.line - 1,
            Some(end) => end.line,
            None => view.cursor.0,
        }
        .max(self.prompt.line)
    }

    /// Short status text drawn at the end of the block's first line: the exit code when it is
    /// not 0 and the duration from one second up, e.g. "✗ 2 · 3.4s".
    pub fn badge(&self) -> Option<String> {
        if self.is_running() {
            return None;
        }
        let mut parts = Vec::new();
        if let Some(code) = self.exit.filter(|&c| c != 0) {
            parts.push(format!("✗ {code}"));
        }
        if let Some(d) = self.duration().filter(|d| d.as_secs() >= 1) {
            parts.push(format_duration(d));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    /// Dim duration and exit code label for the block card toolbar, e.g. `"2.3s · exit 1"`.
    pub fn toolbar_meta(&self) -> Option<String> {
        if self.is_running() {
            return None;
        }
        let mut parts = Vec::new();
        if let Some(d) = self.duration().filter(|d| d.as_millis() >= 100) {
            parts.push(format_duration(d));
        }
        if let Some(code) = self.exit.filter(|&c| c != 0) {
            parts.push(format!("exit {code}"));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    fn map_points(&mut self, f: impl Fn(AbsPoint) -> Option<AbsPoint>) -> bool {
        let Some(prompt) = f(self.prompt) else {
            return false;
        };
        self.prompt = prompt;
        for point in [
            &mut self.command_start,
            &mut self.output_start,
            &mut self.end,
        ] {
            *point = point.and_then(&f);
        }
        true
    }
}

/// "3.4s", "2m 5s", "1h 2m".
pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{:.1}s", d.as_secs_f64())
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, secs % 3600 / 60)
    }
}

/// What a block's gutter shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockStatus {
    /// The prompt waiting for a command: only its separator is drawn.
    Prompt,
    Running,
    Succeeded,
    Failed(i32),
    /// Ended without an exit status.
    Finished,
}

/// A block as the renderer draws it, in viewport rows of its pane (rows can lie outside the
/// viewport: a block that starts above it has a negative `first_row`).
#[derive(Clone, Debug, PartialEq)]
pub struct BlockDecor {
    /// Stable per-pane block id (`0` for the waiting prompt).
    pub id: u64,
    /// Row of the prompt line, which gets the separator above it and the badge.
    pub first_row: i64,
    /// Last row of the block (inclusive).
    pub last_row: i64,
    pub status: BlockStatus,
    pub selected: bool,
    pub badge: Option<String>,
    /// Dim duration/exit label for the card toolbar (e.g. `"2.3s · exit 1"`).
    pub meta: Option<String>,
    /// Whether this block has a known command string (enables copy-command and rerun).
    pub has_command: bool,
    /// Whether this block is bookmarked/pinned by the user.
    pub bookmarked: bool,
}

/// A block's text, e.g. for asking an agent about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockText {
    pub command: Option<String>,
    /// The output as plain logical text, trailing blank lines trimmed.
    pub output: String,
    pub exit: Option<i32>,
    pub running: bool,
}

/// Where navigating to another block leaves the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockNav {
    /// A block was selected: scroll so this line is the top row.
    ScrollTo(AbsLine),
    /// Past the newest block: the selection was cleared, back to the live screen.
    Bottom,
    None,
}

/// The blocks of one pane, oldest first.
#[derive(Debug, Default)]
pub struct BlockList {
    blocks: VecDeque<Block>,
    /// The prompt on screen, before its command runs (A seen, C not yet).
    pending: Option<Block>,
    selected: Option<u64>,
    next_id: u64,
    /// The terminal's line epoch and remap sequence the points are in.
    epoch: u64,
    remap_seq: u64,
}

impl BlockList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn blocks(&self) -> impl ExactSizeIterator<Item = &Block> + DoubleEndedIterator {
        self.blocks.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// Feed one shell signal of this pane (`Terminal::take_shell_events`), in order.
    pub fn on_event(&mut self, event: &ShellEvent, term: &Terminal, now: Instant) {
        self.refresh(term);
        match &event.signal {
            Signal::Mark(kind) => {
                // No line on the alternate screen; a line older than the scrollback was numbered
                // before the terminal lost track of its lines (a flood in the same batch).
                let oldest = term.oldest_line();
                let Some(line) = event.abs_line.filter(|&l| oldest.is_some_and(|o| l >= o)) else {
                    return;
                };
                let point = AbsPoint {
                    line,
                    column: event.column,
                };
                self.on_mark(*kind, point, now);
            }
            Signal::CommandLine(text) => {
                if let Some(pending) = self.pending.as_mut() {
                    pending.command = Some(text.clone());
                } else if let Some(last) = self.blocks.back_mut().filter(|b| b.is_running()) {
                    last.command.get_or_insert_with(|| text.clone());
                }
            }
            Signal::Cwd(_) | Signal::Notify { .. } => {}
        }
    }

    fn on_mark(&mut self, kind: MarkKind, point: AbsPoint, now: Instant) {
        match kind {
            MarkKind::PromptStart => {
                self.end_running(point, None, now);
                // A prompt at or above earlier ones: those lines were cleared or rewritten.
                while self
                    .blocks
                    .back()
                    .is_some_and(|b| b.prompt.line >= point.line)
                {
                    self.blocks.pop_back();
                }
                self.pending = Some(Block::new(self.take_id(), point));
            }
            MarkKind::CommandStart => {
                if let Some(pending) = self.pending.as_mut().filter(|p| p.prompt <= point) {
                    pending.command_start = Some(point);
                }
            }
            MarkKind::OutputStart => {
                let mut block = match self.pending.take() {
                    Some(pending) if pending.prompt <= point => pending,
                    // A repeated C (two integrations at once) changes nothing.
                    _ if self.blocks.back().is_some_and(Block::is_running) => return,
                    // C without a prompt mark: the block starts at its output.
                    _ => Block::new(self.take_id(), point),
                };
                block.output_start = Some(point);
                block.started = Some(now);
                self.push(block);
            }
            MarkKind::CommandEnd { exit } => self.end_running(point, exit, now),
        }
    }

    fn end_running(&mut self, point: AbsPoint, exit: Option<i32>, now: Instant) {
        let Some(last) = self.blocks.back_mut().filter(|b| b.is_running()) else {
            return;
        };
        if last.output_start.is_some_and(|start| point < start) {
            // The end is above the output: the screen was cleared under it.
            self.blocks.pop_back();
            return;
        }
        last.end = Some(point);
        last.exit = exit;
        last.finished = Some(now);
    }

    fn take_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn push(&mut self, block: Block) {
        if self.blocks.len() >= MAX_BLOCKS {
            self.blocks.pop_front();
        }
        self.blocks.push_back(block);
    }

    /// Bring the points up to date with the terminal: follow reflows, drop blocks whose lines
    /// left the scrollback, forget everything when the terminal lost track of its lines. Call
    /// after output and resizes (`on_event` calls it too).
    pub fn refresh(&mut self, term: &Terminal) {
        let Some(view) = term.line_view() else {
            return;
        };
        if view.epoch != self.epoch {
            self.clear();
            self.epoch = view.epoch;
            self.remap_seq = view.remap_seq;
        }
        if view.remap_seq != self.remap_seq {
            match term.line_remaps_since(self.remap_seq) {
                Some(remaps) => {
                    for remap in remaps {
                        let map = |p: AbsPoint| {
                            let (line, column) = remap.map(p.line, p.column)?;
                            Some(AbsPoint { line, column })
                        };
                        self.blocks.retain_mut(|b| b.map_points(map));
                        if let Some(pending) = self.pending.as_mut() {
                            if !pending.map_points(map) {
                                self.pending = None;
                            }
                        }
                    }
                }
                None => self.clear(),
            }
            self.remap_seq = view.remap_seq;
        }
        while self
            .blocks
            .front()
            .is_some_and(|b| b.prompt.line < view.oldest)
        {
            self.blocks.pop_front();
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.prompt.line < view.oldest)
        {
            self.pending = None;
        }
        if let Some(id) = self.selected {
            if !self.blocks.iter().any(|b| b.id == id) {
                self.selected = None;
            }
        }
    }

    fn clear(&mut self) {
        self.blocks.clear();
        self.pending = None;
        self.selected = None;
    }

    /// The points are in the terminal's current numbering (no refresh missed).
    fn current_view(&self, term: &Terminal) -> Option<LineView> {
        term.line_view()
            .filter(|v| v.epoch == self.epoch && v.remap_seq == self.remap_seq)
    }

    /// Whether block navigation has anything to do: blocks exist and the main screen is shown.
    pub fn navigable(&self, term: &Terminal) -> bool {
        !self.blocks.is_empty() && self.current_view(term).is_some()
    }

    fn selected_index(&self) -> Option<usize> {
        let id = self.selected?;
        self.blocks.iter().position(|b| b.id == id)
    }

    pub fn selected(&self) -> Option<&Block> {
        self.selected_index().map(|i| &self.blocks[i])
    }

    /// Select the block before the selected one (the newest one when none is).
    pub fn select_prev(&mut self) -> BlockNav {
        let index = match self.selected_index() {
            Some(i) => i.saturating_sub(1),
            None => match self.blocks.len() {
                0 => return BlockNav::None,
                n => n - 1,
            },
        };
        self.select_index(index)
    }

    /// Select the block after the selected one; past the newest the selection is cleared.
    pub fn select_next(&mut self) -> BlockNav {
        match self.selected_index() {
            Some(i) if i + 1 < self.blocks.len() => self.select_index(i + 1),
            Some(_) => {
                self.selected = None;
                BlockNav::Bottom
            }
            None => BlockNav::None,
        }
    }

    fn select_index(&mut self, index: usize) -> BlockNav {
        let block = &self.blocks[index];
        self.selected = Some(block.id);
        BlockNav::ScrollTo(block.prompt.line)
    }

    /// Toggle the bookmark flag on block `id`. Returns the new bookmarked state, or `None` if
    /// no block with `id` exists.
    pub fn toggle_bookmark(&mut self, id: u64) -> Option<bool> {
        let block = self.blocks.iter_mut().find(|b| b.id == id)?;
        block.bookmarked = !block.bookmarked;
        Some(block.bookmarked)
    }

    /// Jump to the next bookmarked block (wrapping around). Returns [`BlockNav::None`] if no
    /// blocks are bookmarked.
    pub fn select_next_bookmark(&mut self) -> BlockNav {
        let bookmarked: Vec<usize> = self
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(i, b)| b.bookmarked.then_some(i))
            .collect();
        if bookmarked.is_empty() {
            return BlockNav::None;
        }
        let target = match self.selected_index() {
            Some(cur) => bookmarked
                .iter()
                .copied()
                .find(|&i| i > cur)
                .unwrap_or(bookmarked[0]),
            None => *bookmarked.last().unwrap(),
        };
        self.select_index(target)
    }

    /// Select the block drawn on viewport row `row`. Returns false when no block is there.
    pub fn select_at_row(&mut self, term: &Terminal, row: usize) -> bool {
        let Some(view) = self.current_view(term) else {
            return false;
        };
        let line = view.viewport_top + row as AbsLine;
        let hit = self
            .blocks
            .iter()
            .enumerate()
            .rev()
            .find(|(i, b)| b.prompt.line <= line && line <= self.block_end(*i, &view));
        match hit {
            Some((_, block)) => {
                self.selected = Some(block.id);
                true
            }
            None => false,
        }
    }

    /// Clear the selection. Returns true when one was cleared.
    pub fn clear_selection(&mut self) -> bool {
        self.selected.take().is_some()
    }

    /// The selected block, else the newest finished one: what "copy output" and "ask an agent"
    /// act on.
    pub fn target(&self) -> Option<&Block> {
        self.selected()
            .or_else(|| self.blocks.iter().rev().find(|b| !b.is_running()))
    }

    /// The command, output and exit status of `block`.
    pub fn text(&self, block: &Block, term: &Terminal) -> Option<BlockText> {
        let view = self.current_view(term)?;
        let start = block.output_start?;
        let end = block.end.unwrap_or(AbsPoint {
            line: view.cursor.0,
            column: view.cursor.1,
        });
        let output = term.text_between((start.line, start.column), (end.line, end.column))?;
        Some(BlockText {
            command: block.command.clone(),
            output: trim_trailing_blank_lines(&output),
            exit: block.exit,
            running: block.is_running(),
        })
    }

    /// The text of the selected block, else of the newest finished one.
    pub fn target_text(&self, term: &Terminal) -> Option<BlockText> {
        self.text(self.target()?, term)
    }

    /// The selected block, else the newest one, running or not (e.g. to ask an agent about
    /// the command that is still printing).
    pub fn selected_or_last(&self) -> Option<&Block> {
        self.selected().or_else(|| self.blocks.back())
    }

    /// Look up a block by its stable per-pane `id`.
    pub fn block_by_id(&self, id: u64) -> Option<&Block> {
        self.blocks.iter().find(|b| b.id == id)
    }

    /// The command, output and exit status of the block with `id`.
    pub fn block_text_by_id(&self, id: u64, term: &Terminal) -> Option<BlockText> {
        self.text(self.block_by_id(id)?, term)
    }

    /// The text of `selected_or_last`.
    pub fn selected_or_last_text(&self, term: &Terminal) -> Option<BlockText> {
        self.text(self.selected_or_last()?, term)
    }

    /// Last line of block `index`: before the next block's prompt at the latest.
    fn block_end(&self, index: usize, view: &LineView) -> AbsLine {
        let block = &self.blocks[index];
        let mut last = block.last_line(view);
        let next = self
            .blocks
            .get(index + 1)
            .or(self.pending.as_ref())
            .map(|b| b.prompt.line);
        if let Some(next) = next {
            last = last.min(next - 1);
        }
        last.max(block.prompt.line)
    }

    /// The blocks on screen, for drawing. Empty for a pane without blocks.
    pub fn decor(&self, term: &Terminal) -> Vec<BlockDecor> {
        let Some(view) = self.current_view(term).filter(|_| !self.blocks.is_empty()) else {
            return Vec::new();
        };
        let rows = view.rows as AbsLine;
        let row = |line: AbsLine| line - view.viewport_top;
        let mut out = Vec::new();
        // Blocks are ordered by prompt: start at the last one that begins above the viewport.
        let start = self
            .blocks
            .partition_point(|b| b.prompt.line < view.viewport_top)
            .saturating_sub(1);
        for (i, block) in self.blocks.iter().enumerate().skip(start) {
            let (first, last) = (row(block.prompt.line), row(self.block_end(i, &view)));
            if first >= rows {
                break;
            }
            if last < 0 {
                continue;
            }
            out.push(BlockDecor {
                id: block.id,
                first_row: first,
                last_row: last,
                status: block.status(),
                selected: self.selected == Some(block.id),
                badge: block.badge(),
                meta: block.toolbar_meta(),
                has_command: block
                    .command
                    .as_deref()
                    .is_some_and(|c| !rerun_text(c).is_empty()),
                bookmarked: block.bookmarked,
            });
        }
        if let Some(pending) = &self.pending {
            let first = row(pending.prompt.line);
            if (0..rows).contains(&first) {
                out.push(BlockDecor {
                    id: 0,
                    first_row: first,
                    last_row: first,
                    status: BlockStatus::Prompt,
                    selected: false,
                    badge: None,
                    meta: None,
                    has_command: false,
                    bookmarked: false,
                });
            }
        }
        out
    }
}

/// Prepare a command line to be inserted into the shell prompt for rerun: never contains a
/// newline, carriage return, NUL, or control/bidi-override characters, and stays bounded.
pub fn rerun_text(command: &str) -> String {
    let single_line: String = command
        .chars()
        .map(|c| match c {
            '\r' | '\n' | '\u{2028}' | '\u{2029}' => ' ',
            _ => c,
        })
        .collect();
    crate::shell_integration::clean_text(&single_line, crate::shell_integration::MAX_COMMAND_CHARS)
}

/// Encode a command for rerun insertion into the focused prompt without executing it (no newline;
/// respects bracketed paste and strips paste-end escape sequences).
pub fn encode_rerun(command: &str, bracketed: bool) -> Vec<u8> {
    let text = rerun_text(command);
    if text.is_empty() {
        return Vec::new();
    }
    crate::paste::encode_paste(&text, bracketed)
}

/// Format a command block's command, exit status, and output as a fenced Markdown snippet
/// ready to paste into an issue, doc, or coding agent.
pub fn format_block_markdown(block: &BlockText) -> String {
    let fence = safe_markdown_fence(
        block.command.as_deref().unwrap_or(""),
        block.output.as_str(),
    );
    let mut out = String::new();
    if let Some(cmd) = block
        .command
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        out.push_str(&fence);
        out.push_str("sh\n$ ");
        out.push_str(cmd);
        out.push('\n');
        out.push_str(&fence);
        out.push('\n');
    }
    if let Some(code) = block.exit.filter(|&c| c != 0) {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("Exit code: {code}\n"));
    }
    if !block.output.trim().is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&fence);
        out.push_str("text\n");
        out.push_str(&block.output);
        if !block.output.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&fence);
        out.push('\n');
    }
    out
}

fn safe_markdown_fence(a: &str, b: &str) -> String {
    let mut run = 3usize;
    for s in [a, b] {
        let mut cur = 0usize;
        for ch in s.chars() {
            if ch == '`' {
                cur += 1;
                if cur >= run {
                    run = cur + 1;
                }
            } else {
                cur = 0;
            }
        }
    }
    "`".repeat(run)
}

/// `text` without the blank lines (and spaces) it ends with.
fn trim_trailing_blank_lines(text: &str) -> String {
    let mut lines: Vec<&str> = text.split('\n').collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}
