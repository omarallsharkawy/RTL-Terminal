use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{point_to_viewport, viewport_to_point};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{
    Color as AnsiColor, CursorShape, NamedColor, Processor, StdSyncHandler,
};
use std::cell::RefCell;
use std::sync::Arc;
use unicode_bidi::{bidi_class, BidiClass};

struct ForwardListener {
    on_event: Arc<dyn Fn(Event) + Send + Sync>,
}

impl EventListener for ForwardListener {
    fn send_event(&self, event: Event) {
        (self.on_event)(event);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CellData {
    pub c: char,
    /// Combining marks attached to `c` (Arabic harakat, accents, ZWJ sequences).
    pub zerowidth: Vec<char>,
    pub fg: AnsiColor,
    pub bg: AnsiColor,
    pub flags: CellFlags,
    /// Inside the mouse selection.
    pub selected: bool,
    /// 0 = no search match, 1 = match, 2 = the focused match.
    pub search_match: u8,
}

impl CellData {
    pub fn blank() -> Self {
        Self {
            c: ' ',
            zerowidth: Vec::new(),
            fg: AnsiColor::Named(NamedColor::Foreground),
            bg: AnsiColor::Named(NamedColor::Background),
            flags: CellFlags::empty(),
            selected: false,
            search_match: 0,
        }
    }

    /// Spacer cells sit after (or before) a double-width glyph and carry no text of their own.
    pub fn is_spacer(&self) -> bool {
        self.flags
            .intersects(CellFlags::WIDE_CHAR_SPACER | CellFlags::LEADING_WIDE_CHAR_SPACER)
    }

    /// A plain space with nothing attached, usable as a segment separator.
    pub fn is_blank(&self) -> bool {
        self.c == ' ' && self.zerowidth.is_empty() && !self.is_spacer()
    }

    /// Text this cell contributes to a shaped run (empty for spacers).
    pub fn push_text(&self, out: &mut String) {
        if self.is_spacer() {
            return;
        }
        out.push(self.c);
        out.extend(self.zerowidth.iter());
    }

    pub fn text_len(&self) -> usize {
        if self.is_spacer() {
            return 0;
        }
        self.c.len_utf8() + self.zerowidth.iter().map(|c| c.len_utf8()).sum::<usize>()
    }
}

#[derive(Clone, Debug)]
pub struct LineData {
    pub cells: Vec<CellData>,
    pub has_rtl: bool,
}

#[derive(Clone, Debug)]
pub struct CursorState {
    pub col: usize,
    pub row: usize,
    pub is_visible: bool,
    pub shape: CursorShape,
}

pub struct Terminal {
    term: Term<ForwardListener>,
    parser: Processor<StdSyncHandler>,
    cols: usize,
    rows: usize,
    search: Option<SearchState>,
}

struct SearchState {
    query: String,
    regex: RefCell<RegexSearch>,
    focus: Option<Match>,
    /// 1-based index of the focused match (oldest first) and the total count.
    position: (usize, usize),
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
    /// Terminal that only forwards replies destined for the PTY (used by tests).
    pub fn new<F>(cols: usize, rows: usize, on_pty_write: F) -> Self
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        Self::with_events(cols, rows, move |event| {
            if let Event::PtyWrite(text) = event {
                on_pty_write(text);
            }
        })
    }

    /// Terminal that forwards every event (title, bell, OSC queries, clipboard, PTY replies).
    pub fn with_events<F>(cols: usize, rows: usize, on_event: F) -> Self
    where
        F: Fn(Event) + Send + Sync + 'static,
    {
        let size = TermSize::new(cols, rows);
        let listener = ForwardListener {
            on_event: Arc::new(on_event),
        };
        let term = Term::new(Config::default(), &size, listener);
        let parser = Processor::<StdSyncHandler>::new();
        Self {
            term,
            parser,
            cols,
            rows,
            search: None,
        }
    }

    pub fn process_bytes(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.rows = rows;
        self.term.resize(TermSize::new(cols, rows));
    }

    pub fn size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    pub fn is_app_cursor(&self) -> bool {
        self.term.mode().contains(TermMode::APP_CURSOR)
    }

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    pub fn scroll_display(&mut self, delta: i32) {
        self.term.scroll_display(Scroll::Delta(delta));
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    fn grid_point(&self, col: usize, row: usize) -> Point {
        let col = col.min(self.cols.saturating_sub(1));
        let row = row.min(self.rows.saturating_sub(1));
        viewport_to_point(self.display_offset(), Point::new(row, Column(col)))
    }

    /// Start a selection at a viewport cell: 1 click = characters, 2 = word, 3 = line.
    pub fn start_selection(&mut self, col: usize, row: usize, side: Side, clicks: u8) {
        let ty = match clicks {
            1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            _ => SelectionType::Lines,
        };
        let point = self.grid_point(col, row);
        self.term.selection = Some(Selection::new(ty, point, side));
    }

    pub fn update_selection(&mut self, col: usize, row: usize, side: Side) {
        let point = self.grid_point(col, row);
        if let Some(selection) = self.term.selection.as_mut() {
            selection.update(point, side);
        }
    }

    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    pub fn has_selection(&self) -> bool {
        self.term.selection.as_ref().is_some_and(|s| !s.is_empty())
    }

    /// Selected text, or `None` when nothing (or only an empty range) is selected.
    pub fn selection_text(&self) -> Option<String> {
        if !self.has_selection() {
            return None;
        }
        self.term
            .selection_to_string()
            .filter(|text| !text.is_empty())
    }

    pub fn snapshot(&self) -> (Vec<LineData>, CursorState) {
        let content = self.term.renderable_content();
        let display_offset = content.display_offset;
        let selection = content.selection;
        let mut lines = vec![
            LineData {
                cells: vec![CellData::blank(); self.cols],
                has_rtl: false,
            };
            self.rows
        ];

        for cell in content.display_iter {
            // Grid lines are negative inside scrollback; map them into the viewport.
            let Some(point) = point_to_viewport(display_offset, cell.point) else {
                continue;
            };
            let (row, col) = (point.line, point.column.0);
            if row >= self.rows || col >= self.cols {
                continue;
            }
            let c = cell.c;
            let class = bidi_class(c);
            if class == BidiClass::R || class == BidiClass::AL {
                lines[row].has_rtl = true;
            }
            lines[row].cells[col] = CellData {
                c,
                zerowidth: cell.zerowidth().map(<[char]>::to_vec).unwrap_or_default(),
                fg: cell.fg,
                bg: cell.bg,
                flags: cell.flags,
                selected: selection.is_some_and(|range| range.contains(cell.point)),
                search_match: 0,
            };
        }

        self.mark_search_matches(&mut lines, display_offset);

        let cursor_point = point_to_viewport(display_offset, content.cursor.point);
        let cursor = match cursor_point {
            Some(p) if p.line < self.rows => CursorState {
                row: p.line,
                col: p.column.0.min(self.cols.saturating_sub(1)),
                is_visible: content.cursor.shape != CursorShape::Hidden,
                shape: content.cursor.shape,
            },
            // Cursor scrolled out of the viewport.
            _ => CursorState {
                row: 0,
                col: 0,
                is_visible: false,
                shape: CursorShape::Hidden,
            },
        };

        (lines, cursor)
    }

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

    /// Fill `position` for the focused match by walking every match from the oldest line.
    fn count_matches(&mut self) {
        let Some(state) = self.search.as_mut() else {
            return;
        };
        let Some(focus) = state.focus.clone() else {
            return;
        };
        let start = Point::new(self.term.topmost_line(), Column(0));
        let end = Point::new(self.term.bottommost_line(), self.term.last_column());
        let mut regex = state.regex.borrow_mut();
        let (mut index, mut total) = (0, 0);
        for m in RegexIter::new(start, end, Direction::Right, &self.term, &mut regex)
            .take(MAX_COUNTED_MATCHES)
        {
            total += 1;
            if m.start() == focus.start() {
                index = total;
            }
        }
        drop(regex);
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

    /// Mark cells of visible matches in a snapshot (1 = match, 2 = focused).
    fn mark_search_matches(&self, lines: &mut [LineData], display_offset: usize) {
        let Some(state) = self.search.as_ref() else {
            return;
        };
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
                for cell in &mut lines[row].cells[from.min(last_col)..=to.min(last_col)] {
                    cell.search_match = cell.search_match.max(level);
                }
            }
        }
    }
}
