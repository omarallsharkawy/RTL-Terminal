//! Terminal scrollback search, cached match counting, and logical text scanning.

use std::cell::RefCell;

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::term::cell::{Flags as CellFlags, LineLength};
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::{point_to_viewport, viewport_to_point};

use super::{
    drawn_marks, is_bidi_control, Terminal, TextChar, TextLinePos, TextScan, MAX_JOINED_ROWS,
};
use crate::blocks::anchor::AbsLine;

pub(super) struct SearchState {
    query: String,
    regex: RefCell<RegexSearch>,
    focus: Option<Match>,
    /// 1-based index of the focused match (oldest first) and the total count.
    position: (usize, usize),
    /// `content_generation` at which `cached_starts` was computed (`0` = stale).
    cached_generation: u64,
    /// Sorted start points of all matches (up to `MAX_COUNTED_MATCHES`) for fast Next/Prev jumps.
    cached_starts: Vec<Point>,
}

/// Stop counting beyond this many matches.
const MAX_COUNTED_MATCHES: usize = 100_000;

/// Case-insensitive literal pattern for `query`.
fn literal_pattern(query: &str) -> String {
    let mut pattern = String::from("(?i)");
    for c in query.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern
}

impl Terminal {
    /// Find the next match of `query` (literal, case-insensitive) and scroll it into view.
    ///
    /// `Direction::Left` goes towards older output, `Direction::Right` towards newer. A changed
    /// query keeps the current match when it still matches there (incremental typing); a first
    /// search starts at the bottom of the viewport. Returns false when nothing matches.
    pub fn search(&mut self, query: &str, direction: Direction) -> bool {
        if query.is_empty() {
            self.clear_search();
            return false;
        }
        let same_query = self.search.as_ref().is_some_and(|s| s.query == query);
        if !same_query {
            let Ok(regex) = RegexSearch::new(&literal_pattern(query)) else {
                self.clear_search();
                return false;
            };
            let focus = self.search.take().and_then(|s| s.focus);
            self.search = Some(SearchState {
                query: query.to_owned(),
                regex: RefCell::new(regex),
                focus,
                position: (0, 0),
                cached_generation: 0,
                cached_starts: Vec::new(),
            });
        }
        let Some(state) = self.search.as_mut() else {
            return false;
        };
        let (origin, direction) = match (&state.focus, same_query) {
            (Some(m), true) => match direction {
                Direction::Left => (m.start().sub(&self.term, Boundary::Grid, 1), direction),
                Direction::Right => (m.end().add(&self.term, Boundary::Grid, 1), direction),
            },
            // The query changed: stay on the current match if it still matches there.
            (Some(m), false) => (*m.start(), Direction::Left),
            (None, _) => {
                let bottom = viewport_to_point(
                    self.term.grid().display_offset(),
                    Point::new(
                        self.rows.saturating_sub(1),
                        Column(self.cols.saturating_sub(1)),
                    ),
                );
                (bottom, Direction::Left)
            }
        };
        let found = self.term.search_next(
            &mut state.regex.borrow_mut(),
            origin,
            direction,
            Side::Left,
            None,
        );
        state.focus = found.clone();
        let Some(found) = found else {
            state.position = (0, 0);
            return false;
        };
        self.term.scroll_to_point(*found.start());
        self.count_matches();
        true
    }

    /// Fill `position` for the focused match, reusing cached match offsets while terminal content
    /// has not changed (O(log M) binary search on Next/Prev jumps).
    fn count_matches(&mut self) {
        let Some(state) = self.search.as_mut() else {
            return;
        };
        let Some(focus) = state.focus.clone() else {
            return;
        };
        if state.cached_generation != self.content_generation {
            let start = Point::new(self.term.topmost_line(), Column(0));
            let end = Point::new(self.term.bottommost_line(), self.term.last_column());
            let mut regex = state.regex.borrow_mut();
            state.cached_starts.clear();
            for m in RegexIter::new(start, end, Direction::Right, &self.term, &mut regex)
                .take(MAX_COUNTED_MATCHES)
            {
                state.cached_starts.push(*m.start());
            }
            drop(regex);
            state.cached_generation = self.content_generation;
        }
        let target = *focus.start();
        let total = state.cached_starts.len();
        let index = match state.cached_starts.binary_search(&target) {
            Ok(pos) => pos + 1,
            Err(_) => state
                .cached_starts
                .iter()
                .position(|&p| p == target)
                .map_or(0, |pos| pos + 1),
        };
        state.position = (index, total);
    }

    /// (index, total) of the focused match, `None` without a query.
    pub fn search_position(&self) -> Option<(usize, usize)> {
        self.search.as_ref().map(|s| s.position)
    }

    pub fn has_search_match(&self) -> bool {
        self.search.as_ref().is_some_and(|s| s.focus.is_some())
    }

    /// Drop the query and all highlights.
    pub fn clear_search(&mut self) {
        self.search = None;
    }

    /// Call `f` with each logical line (soft-wrapped rows joined, at most [`MAX_JOINED_ROWS`])
    /// of the shown screen and its scrollback, newest first: from the row at `from` (the
    /// bottom row when `None`) upwards, at most `max_rows` rows, until `f` returns false. Blank
    /// lines are skipped, trailing blanks dropped, control and bidi control characters left
    /// out. Turns on absolute line numbers for the main screen, so the positions handed out
    /// (and [`TextScan::next`], to go on later) stay valid while output scrolls.
    pub fn for_each_text_line(
        &mut self,
        from: Option<TextLinePos>,
        max_rows: usize,
        mut f: impl FnMut(TextLinePos, &[TextChar]) -> bool,
    ) -> TextScan {
        let alt = self.is_alt_screen();
        if !alt {
            self.lines.enable(self.term.grid(), false);
            self.sync_lines();
        }
        let done = TextScan {
            rows: 0,
            next: None,
        };
        if max_rows == 0 {
            return done;
        }
        let bottom = match from {
            None => self.term.bottommost_line().0,
            Some(pos) => match self.text_point(pos, 0, 0) {
                Some(point) => point.line.0,
                None => return done,
            },
        };
        let grid = self.term.grid();
        let top = self.term.topmost_line().0;
        let span = i32::try_from(max_rows.saturating_sub(1)).unwrap_or(i32::MAX);
        let lowest = top.max(bottom.saturating_sub(span));
        let last = Column(self.cols - 1);
        let wraps = |line: i32| grid[Line(line)][last].flags.contains(CellFlags::WRAPLINE);
        let (abs_top, epoch, remap_seq) =
            (self.lines.top(), self.lines.epoch(), self.lines.remap_seq());
        let pos_of = |line: i32| {
            if alt {
                TextLinePos::Alt(line)
            } else {
                TextLinePos::Main {
                    line: abs_top + AbsLine::from(line),
                    epoch,
                    remap_seq,
                }
            }
        };
        let shown = |c: char| {
            !c.is_control() && !is_bidi_control(c) && !matches!(c, '\u{2028}' | '\u{2029}')
        };
        let spacer = CellFlags::WIDE_CHAR_SPACER | CellFlags::LEADING_WIDE_CHAR_SPACER;
        let mut chars: Vec<TextChar> = Vec::with_capacity(self.cols * 2);
        let mut end = bottom;
        while end >= lowest {
            let mut start = end;
            while start > lowest
                && wraps(start - 1)
                && ((end - start) as usize) < MAX_JOINED_ROWS - 1
            {
                start -= 1;
            }
            chars.clear();
            for line in start..=end {
                let row = &grid[Line(line)];
                // Only the last physical row can lose padding; inside a wrapped line those
                // columns preserve the logical spacing.
                let cols = if line == end {
                    row.line_length().0
                } else {
                    self.cols
                };
                for c in 0..cols {
                    let cell = &row[Column(c)];
                    if cell.flags.intersects(spacer) {
                        continue;
                    }
                    let (row, col) = ((line - start) as u16, c as u16);
                    let wide = cell.flags.contains(CellFlags::WIDE_CHAR);
                    // A cell that holds no visible text still takes its column.
                    let base = if shown(cell.c) { cell.c } else { ' ' };
                    chars.push(TextChar {
                        c: base,
                        mark: false,
                        row,
                        col,
                        wide,
                    });
                    for &z in drawn_marks(cell).iter().filter(|&&z| shown(z)) {
                        chars.push(TextChar {
                            c: z,
                            mark: true,
                            row,
                            col,
                            wide,
                        });
                    }
                }
            }
            while chars.last().is_some_and(|t| !t.mark && t.c == ' ') {
                chars.pop();
            }
            let go_on = chars.is_empty() || f(pos_of(start), &chars);
            let Some(prev) = start.checked_sub(1) else {
                return TextScan {
                    rows: (i64::from(bottom) - i64::from(start) + 1).max(0) as usize,
                    next: None,
                };
            };
            end = prev;
            if !go_on {
                break;
            }
        }
        TextScan {
            rows: (i64::from(bottom) - i64::from(end)).max(0) as usize,
            next: (end >= lowest).then(|| pos_of(end)),
        }
    }

    /// Grid point of `row` rows below the line at `pos` and `column`, while it is still where
    /// it was when `pos` was handed out.
    fn text_point(&self, pos: TextLinePos, row: usize, column: usize) -> Option<Point> {
        let line = match pos {
            TextLinePos::Main {
                line,
                epoch,
                remap_seq,
            } => {
                let view = self.line_view()?;
                if view.epoch != epoch || view.remap_seq != remap_seq || line < view.oldest {
                    return None;
                }
                i32::try_from(line - self.lines.top()).ok()?
            }
            TextLinePos::Alt(line) if self.is_alt_screen() => line,
            TextLinePos::Alt(_) => return None,
        };
        let line = line.checked_add(i32::try_from(row).ok()?)?;
        if line < self.term.topmost_line().0 || line > self.term.bottommost_line().0 {
            return None;
        }
        (column < self.cols).then(|| Point::new(Line(line), Column(column)))
    }

    /// Make the match of `needle` (literal and case-insensitive, like [`Terminal::search`])
    /// that starts at `row`/`column` of the line at `pos` the focused search match, and scroll
    /// it into view. When that line is gone or no longer holds `needle` there, the newest
    /// match of `needle` is focused instead. Returns false when nothing matches at all.
    pub fn focus_search_at(
        &mut self,
        needle: &str,
        pos: TextLinePos,
        row: usize,
        column: usize,
    ) -> bool {
        let Ok(mut regex) = RegexSearch::new(&literal_pattern(needle)) else {
            self.clear_search();
            return false;
        };
        let found = self.text_point(pos, row, column).and_then(|start| {
            let last = (start.line + MAX_JOINED_ROWS as i32).min(self.term.bottommost_line());
            let end = Point::new(last, self.term.last_column());
            RegexIter::new(start, end, Direction::Right, &self.term, &mut regex)
                .next()
                .filter(|m| *m.start() == start)
        });
        let Some(found) = found else {
            self.clear_search();
            self.term.scroll_display(Scroll::Bottom);
            return self.search(needle, Direction::Left);
        };
        self.term.scroll_to_point(*found.start());
        self.search = Some(SearchState {
            query: needle.to_owned(),
            regex: RefCell::new(regex),
            focus: Some(found),
            position: (0, 0),
            cached_generation: 0,
            cached_starts: Vec::new(),
        });
        self.count_matches();
        true
    }

    /// Per viewport row, the inclusive column spans of visible matches and their level
    /// (1 = match, 2 = focused). Empty without a search.
    pub(super) fn search_spans(&self, display_offset: usize) -> Vec<Vec<(usize, usize, u8)>> {
        let Some(state) = self.search.as_ref() else {
            return Vec::new();
        };
        let mut spans = vec![Vec::new(); self.rows];
        let top = Line(-(display_offset as i32));
        let start = Point::new(top, Column(0));
        let end = Point::new(top + (self.rows as i32 - 1), self.term.last_column());
        let mut regex = state.regex.borrow_mut();
        let last_col = self.cols.saturating_sub(1);
        for m in RegexIter::new(start, end, Direction::Right, &self.term, &mut regex) {
            let level = if state.focus.as_ref().is_some_and(|f| f.start() == m.start()) {
                2
            } else {
                1
            };
            for line in m.start().line.0..=m.end().line.0 {
                let Some(row) =
                    point_to_viewport(display_offset, Point::new(Line(line), Column(0)))
                else {
                    continue;
                };
                let row = row.line;
                if row >= self.rows {
                    continue;
                }
                let from = if Line(line) == m.start().line {
                    m.start().column.0
                } else {
                    0
                };
                let to = if Line(line) == m.end().line {
                    m.end().column.0
                } else {
                    last_col
                };
                spans[row].push((from.min(last_col), to.min(last_col), level));
            }
        }
        spans
    }
}
