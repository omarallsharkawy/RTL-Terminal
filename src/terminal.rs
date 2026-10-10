use crate::links::find_url_at;
use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{point_to_viewport, viewport_to_point};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{
    Color as AnsiColor, CursorShape, NamedColor, Processor, StdSyncHandler,
};
use std::ops::Range;
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
    /// Under the Ctrl-hovered link.
    pub link_hover: bool,
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
            link_hover: false,
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

/// A link under a viewport cell: its target and the cells it covers on that row.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub uri: String,
    pub row: usize,
    pub cols: Range<usize>,
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
    link_hover: Option<(usize, Range<usize>)>,
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
            link_hover: None,
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
        let hover = self.link_hover.clone();
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
                link_hover: hover
                    .as_ref()
                    .is_some_and(|(r, cols)| *r == row && cols.contains(&col)),
            };
        }

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

    /// Underline these cells (a Ctrl-hovered link), or nothing. Returns true if it changed.
    pub fn set_link_hover(&mut self, hover: Option<(usize, Range<usize>)>) -> bool {
        let changed = self.link_hover != hover;
        self.link_hover = hover;
        changed
    }

    /// The link at a viewport cell: an OSC 8 hyperlink, else a plain URL in the row's text.
    pub fn link_at(&self, col: usize, row: usize) -> Option<Link> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        let line = viewport_to_point(self.display_offset(), Point::new(row, Column(0))).line;
        let grid = self.term.grid();
        let at = |c: usize| &grid[line][Column(c)];

        if let Some(h) = at(col).hyperlink() {
            let same = |c: usize| at(c).hyperlink().is_some_and(|o| o == h);
            let (mut a, mut b) = (col, col + 1);
            while a > 0 && same(a - 1) {
                a -= 1;
            }
            while b < self.cols && same(b) {
                b += 1;
            }
            return Some(Link {
                uri: h.uri().to_string(),
                row,
                cols: a..b,
            });
        }

        // Row text with a char -> column map (spacers carry no text).
        let mut text = String::new();
        let mut char_cols = Vec::new();
        let mut hit = None;
        for c in 0..self.cols {
            let cell = at(c);
            if cell
                .flags
                .intersects(CellFlags::WIDE_CHAR_SPACER | CellFlags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            let before = char_cols.len();
            text.push(cell.c);
            char_cols.push(c);
            for z in cell.zerowidth().unwrap_or(&[]) {
                text.push(*z);
                char_cols.push(c);
            }
            let width = if cell.flags.contains(CellFlags::WIDE_CHAR) {
                2
            } else {
                1
            };
            if (c..c + width).contains(&col) {
                hit = Some(before);
            }
        }
        let (start, end, uri) = find_url_at(&text, hit?)?;
        let last = char_cols[end - 1];
        let wide = at(last).flags.contains(CellFlags::WIDE_CHAR);
        Some(Link {
            uri,
            row,
            cols: char_cols[start]..(last + if wide { 2 } else { 1 }).min(self.cols),
        })
    }
}
