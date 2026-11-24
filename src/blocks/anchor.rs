//! Absolute line numbers that keep pointing at the same text while output scrolls.
//!
//! A grid line number (`Line(0)` = top of the screen, negative = scrollback) changes every time
//! a line scrolls into history, and once the scrollback is full the oldest line is dropped while
//! `history_size()` stays the same, so neither number alone says how far text has moved. The
//! [`LineTracker`] counts the lines that scrolled into history: the text on grid line `l` has the
//! absolute number `top + l`, and `top` grows by one for every line that scrolls up.
//!
//! # How lines are counted
//!
//! alacritty does not report scrolling, so the tracker compares the grid before and after. At
//! every sync it remembers the newest scrollback line (`Line(-1)`) by the address of its cells
//! and a hash of its text. Scrollback lines never change and never move except by scrolling:
//! alacritty rotates its ring buffer, so a line's cells stay where they are in memory and only
//! its line number changes. At the next sync the anchor is looked up again, from `Line(-1)`
//! upwards, and the distance it moved is the number of lines that scrolled. That costs one
//! comparison per scrolled line, and nothing while nothing scrolls.
//!
//! The tracker syncs after every `process_bytes` and at every OSC 133 mark, so an anchor is
//! only lost when one batch of output scrolls more lines than the scrollback holds, the
//! scrollback is cleared (`CSI 3 J`, `reset`), or the history size changed in a way scrolling
//! cannot explain. Then every earlier line is gone (or cannot be placed), and the tracker
//! jumps `top` past every number handed out so far and bumps its [`epoch`](LineTracker::epoch):
//! nothing old can match a new line.
//!
//! # Resizes
//!
//! - Rows only: alacritty pulls lines down from the scrollback or pushes them into it, again
//!   without moving cells in memory, so the anchor is searched on the screen too.
//! - Columns (reflow): rows are rebuilt and wrapped lines join or split, so addresses mean
//!   nothing. Logical lines (rows joined by soft wraps) survive a reflow, though, and alacritty
//!   keeps the cursor on its logical line. Before the resize the tracker records where each
//!   logical line from the cursor's upwards starts; after it, the same walk on the new grid gives
//!   their new starts. A [`LineRemap`] maps old points to new ones by logical line and offset in
//!   it; users of absolute points (the blocks) apply it.
//! - Columns while the alternate screen is shown: the main screen reflows out of sight, so
//!   nothing can be mapped. Its lines are treated as lost when the main screen returns.
//!
//! The alternate screen has no scrollback and nothing on it gets an absolute line.

use alacritty_terminal::grid::{Dimensions, Grid};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};

/// A line in the scrollback or on the screen, numbered so that it stays the same while the text
/// scrolls (see the module docs).
pub type AbsLine = i64;

/// Remaps kept for users that have not caught up (one per reflowing resize).
const MAX_REMAPS: usize = 8;

/// The newest scrollback line at the last sync.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Anchor {
    /// Address of the line's first cell (identity, never dereferenced).
    addr: usize,
    /// Hash of the line's text, so a freed and reused buffer is not mistaken for it.
    hash: u64,
}

/// Where lines went in one reflowing resize, by logical line counted back from the cursor's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineRemap {
    /// Absolute first line of each logical line before the resize, newest first.
    old_starts: Vec<AbsLine>,
    /// The same logical lines after the resize (fewer when the oldest left the scrollback).
    new_starts: Vec<AbsLine>,
    old_cols: usize,
    new_cols: usize,
}

impl LineRemap {
    /// Where the cell at (`line`, `column`) before the resize is now, if it is still kept.
    pub fn map(&self, line: AbsLine, column: usize) -> Option<(AbsLine, usize)> {
        // `old_starts` is descending: the first start at or above `line` is its logical line.
        let i = self.old_starts.partition_point(|&start| start > line);
        let old_start = *self.old_starts.get(i)?;
        let new_start = *self.new_starts.get(i)?;
        let offset = (line - old_start) as usize * self.old_cols + column.min(self.old_cols);
        let mut row = (offset / self.new_cols) as AbsLine;
        let mut column = offset % self.new_cols;
        // Trailing blanks of a wrapped line are not kept by the reflow: stay inside the line.
        if let Some(&next) = i.checked_sub(1).and_then(|n| self.new_starts.get(n)) {
            let last = next - new_start - 1;
            if row > last {
                row = last.max(0);
                column = self.new_cols - 1;
            }
        }
        Some((new_start + row, column))
    }
}

/// Counts lines scrolled into history so marks keep their absolute line (see the module docs).
#[derive(Debug)]
pub struct LineTracker {
    /// Off until the first mark: shells without integration pay nothing.
    enabled: bool,
    /// Absolute number of grid line 0.
    top: AbsLine,
    anchor: Option<Anchor>,
    /// History size at the last sync.
    history: usize,
    /// Scrollback limit of the grid.
    max_history: usize,
    /// The row count changed since the last sync: the anchor may be on the screen now.
    rows_changed: bool,
    /// The main screen reflowed while the alternate screen was shown.
    reflowed_unseen: bool,
    /// Bumped whenever absolute lines handed out before lost their meaning.
    epoch: u64,
    /// Reflow remaps, oldest first, with the sequence number of the newest.
    remaps: Vec<LineRemap>,
    remap_seq: u64,
}

/// What a resize needs from the grid before it reflows.
pub struct ReflowPrep {
    old_starts: Vec<AbsLine>,
    old_cols: usize,
}

impl LineTracker {
    pub fn new(max_history: usize) -> Self {
        Self {
            enabled: false,
            top: 0,
            anchor: None,
            history: 0,
            max_history,
            rows_changed: false,
            reflowed_unseen: false,
            epoch: 0,
            remaps: Vec::new(),
            remap_seq: 0,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Start counting (on the first mark). Lines already in the scrollback get numbers from 0.
    pub fn enable(&mut self, grid: &Grid<Cell>, alt_screen: bool) {
        if self.enabled {
            return;
        }
        self.enabled = true;
        if alt_screen {
            // Nothing to anchor to yet; the main screen is placed when it returns.
            self.reflowed_unseen = true;
            return;
        }
        self.top = grid.history_size() as AbsLine;
        self.reanchor(grid);
    }

    /// Absolute number of grid line 0 (valid right after a sync).
    pub fn top(&self) -> AbsLine {
        self.top
    }

    /// Oldest line in the main screen's scrollback at the last sync.
    pub fn oldest(&self) -> AbsLine {
        self.top - self.history as AbsLine
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Reflow remaps after sequence number `seen`, or `None` when some were already dropped.
    pub fn remaps_since(&self, seen: u64) -> Option<&[LineRemap]> {
        let missing = self.remap_seq.checked_sub(seen)? as usize;
        if missing > self.remaps.len() {
            return None;
        }
        Some(&self.remaps[self.remaps.len() - missing..])
    }

    /// Sequence number of the newest remap.
    pub fn remap_seq(&self) -> u64 {
        self.remap_seq
    }

    /// Account for everything that scrolled since the last sync. Nothing happens in the
    /// alternate screen: the main screen's lines cannot move while it is hidden (except by a
    /// resize, which is recorded).
    pub fn sync(&mut self, grid: &Grid<Cell>, alt_screen: bool) {
        if !self.enabled || alt_screen {
            return;
        }
        if std::mem::take(&mut self.reflowed_unseen) {
            self.lose(grid);
            return;
        }
        let history = grid.history_size();
        let moved = match self.anchor {
            // Nothing in the scrollback last time: whatever is there now scrolled in, unless it
            // filled up (then more may have scrolled through it).
            None if history >= self.max_history && history > 0 => None,
            None => Some(history as AbsLine - self.history as AbsLine),
            Some(anchor) => self.find(grid, anchor),
        };
        // The history size must agree with the scroll count, or something else cleared it.
        let expected = moved.map(|k| (self.history as AbsLine + k).clamp(0, self.max_history as _));
        match moved.filter(|_| expected == Some(history as AbsLine)) {
            Some(k) => {
                self.top += k;
                self.reanchor(grid);
            }
            None => self.lose(grid),
        }
        self.rows_changed = false;
    }

    /// Lines the anchor moved up since the last sync (negative: pulled down by a resize).
    fn find(&self, grid: &Grid<Cell>, anchor: Anchor) -> Option<AbsLine> {
        let history = grid.history_size() as i32;
        let is_anchor =
            |l: i32| line_addr(grid, l) == anchor.addr && line_hash(grid, l) == anchor.hash;
        if let Some(l) = (1..=history).map(|d| -d).find(|&l| is_anchor(l)) {
            return Some((-1 - l) as AbsLine);
        }
        if self.rows_changed {
            let rows = grid.screen_lines() as i32;
            if let Some(l) = (0..rows).find(|&l| is_anchor(l)) {
                return Some((-1 - l) as AbsLine);
            }
        }
        None
    }

    fn reanchor(&mut self, grid: &Grid<Cell>) {
        self.history = grid.history_size();
        self.anchor = (self.history > 0).then(|| Anchor {
            addr: line_addr(grid, -1),
            hash: line_hash(grid, -1),
        });
    }

    /// Earlier lines can no longer be placed: number new ones past everything handed out.
    fn lose(&mut self, grid: &Grid<Cell>) {
        let rows = grid.screen_lines() as AbsLine;
        self.top += self.max_history as AbsLine + 2 * rows + 1;
        self.epoch += 1;
        self.remaps.clear();
        self.reanchor(grid);
    }

    /// Call before the grid is resized to `cols`. Returns what the reflow needs afterwards.
    pub fn before_resize(
        &mut self,
        grid: &Grid<Cell>,
        alt_screen: bool,
        cols: usize,
        rows: usize,
    ) -> Option<ReflowPrep> {
        if !self.enabled {
            return None;
        }
        let reflow = cols != grid.columns();
        if alt_screen {
            if reflow {
                self.reflowed_unseen = true;
            } else if rows != grid.screen_lines() {
                self.rows_changed = true;
            }
            return None;
        }
        self.sync(grid, false);
        if !reflow {
            self.rows_changed = rows != grid.screen_lines();
            return None;
        }
        let old_starts = logical_starts(grid)
            .into_iter()
            .map(|l| self.top + l as AbsLine)
            .collect();
        Some(ReflowPrep {
            old_starts,
            old_cols: grid.columns(),
        })
    }

    /// Call after the grid was resized, with what `before_resize` returned.
    pub fn after_resize(&mut self, grid: &Grid<Cell>, alt_screen: bool, prep: Option<ReflowPrep>) {
        if !self.enabled || alt_screen {
            return;
        }
        let Some(prep) = prep else {
            if self.rows_changed {
                self.sync(grid, false);
            }
            return;
        };
        let new_lines = logical_starts(grid);
        let (Some(&old_cursor), Some(&new_cursor)) = (prep.old_starts.first(), new_lines.first())
        else {
            self.lose(grid);
            return;
        };
        // The cursor's logical line keeps its number; everything else follows it.
        self.top = old_cursor - new_cursor as AbsLine;
        let new_starts = new_lines
            .into_iter()
            .map(|l| self.top + l as AbsLine)
            .collect();
        if self.remaps.len() >= MAX_REMAPS {
            self.remaps.remove(0);
        }
        self.remaps.push(LineRemap {
            old_starts: prep.old_starts,
            new_starts,
            old_cols: prep.old_cols.max(1),
            new_cols: grid.columns().max(1),
        });
        self.remap_seq += 1;
        self.rows_changed = false;
        self.reanchor(grid);
    }
}

/// Identity of a grid line: the address of its first cell. Only compared, never dereferenced.
fn line_addr(grid: &Grid<Cell>, line: i32) -> usize {
    std::ptr::from_ref(&grid[Line(line)][Column(0)]).addr()
}

/// A cheap hash of a grid line's characters and flags.
fn line_hash(grid: &Grid<Cell>, line: i32) -> u64 {
    let row = &grid[Line(line)];
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for cell in &row[..Column(grid.columns())] {
        let word = cell.c as u64 | (cell.flags.bits() as u64) << 32;
        hash = (hash ^ word).wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Grid line where each logical line starts, from the cursor's back to the oldest line.
fn logical_starts(grid: &Grid<Cell>) -> Vec<i32> {
    let top = -(grid.history_size() as i32);
    let last = Column(grid.columns() - 1);
    let wraps = |l: i32| grid[Line(l)][last].flags.contains(Flags::WRAPLINE);
    let mut starts = Vec::new();
    let mut line = grid.cursor.point.line.0;
    loop {
        while line > top && wraps(line - 1) {
            line -= 1;
        }
        starts.push(line);
        if line <= top {
            break;
        }
        line -= 1;
    }
    starts
}
