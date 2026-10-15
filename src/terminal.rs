use crate::links::find_url_at;
use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Row, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionRange, SelectionType};
use alacritty_terminal::term::cell::{Cell, Flags as CellFlags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{point_to_viewport, viewport_to_point};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{
    Color as AnsiColor, CursorShape, CursorStyle, NamedColor, Processor, StdSyncHandler,
};
use std::cell::RefCell;
use std::collections::HashMap;
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
    /// 0 = no search match, 1 = match, 2 = the focused match.
    pub search_match: u8,
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
            search_match: 0,
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
    /// Shared with the terminal's previous snapshot while the row is unchanged.
    pub cells: Arc<[CellData]>,
    pub has_rtl: bool,
    /// Key of everything in `cells` (text, colors, flags, selection, search and link
    /// highlights): rows with equal keys draw the same.
    pub hash: u64,
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
    link_hover: Option<(usize, Range<usize>)>,
    /// The last snapshot's rows by key, so rows that did not change are not rebuilt.
    snapshot_rows: RefCell<HashMap<u64, LineData>>,
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

/// A palette color the program redefined with OSC 4 (pywal, vim themes), as a direct RGB
/// color. Only the 256 indexed colors are taken over; the default foreground and background
/// stay with the theme so the window and the cells keep matching.
fn program_color(color: AnsiColor, colors: &Colors) -> AnsiColor {
    let index = match color {
        AnsiColor::Indexed(i) => i as usize,
        AnsiColor::Named(n) if (n as usize) < 16 => n as usize,
        _ => return color,
    };
    colors[index].map_or(color, AnsiColor::Spec)
}

/// Terminal config whose default cursor blinks as `cursor_blink` says. DECSCUSR (`CSI n SP q`)
/// overrides it, and `CSI 0 SP q` falls back to it.
fn term_config(cursor_blink: bool) -> Config {
    Config {
        default_cursor_style: CursorStyle {
            shape: CursorShape::Block,
            blinking: cursor_blink,
        },
        kitty_keyboard: true,
        ..Config::default()
    }
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
        let (cols, rows) = (cols.max(1), rows.max(1));
        let size = TermSize::new(cols, rows);
        let listener = ForwardListener {
            on_event: Arc::new(on_event),
        };
        let term = Term::new(term_config(true), &size, listener);
        let parser = Processor::<StdSyncHandler>::new();
        Self {
            term,
            parser,
            cols,
            rows,
            search: None,
            link_hover: None,
            snapshot_rows: RefCell::default(),
        }
    }

    pub fn process_bytes(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// The RGB a program gave palette entry `index` (0..256) with OSC 4, if it changed it.
    pub fn program_palette_color(
        &self,
        index: usize,
    ) -> Option<alacritty_terminal::vte::ansi::Rgb> {
        if index >= 256 {
            return None;
        }
        self.term.colors()[index]
    }

    /// Default blinking for when the program has not chosen a cursor style.
    pub fn set_default_cursor_blink(&mut self, cursor_blink: bool) {
        self.term.set_options(term_config(cursor_blink));
    }

    /// Whether the cursor should blink: the program's DECSCUSR choice, else the default.
    pub fn cursor_blinking(&self) -> bool {
        self.term.cursor_style().blinking
    }

    /// Grid position of the cursor, to tell whether output moved it.
    pub fn cursor_position(&self) -> (i32, usize) {
        let point = self.term.grid().cursor.point;
        (point.line.0, point.column.0)
    }

    /// Resize the grid; a zero dimension is treated as one (alacritty panics on an empty grid).
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(1), rows.max(1));
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

    /// The viewport's rows and the cursor. Rows whose content and highlights did not change
    /// since the previous snapshot are shared with it instead of being rebuilt.
    pub fn snapshot(&self) -> (Vec<LineData>, CursorState) {
        let content = self.term.renderable_content();
        let display_offset = content.display_offset;
        let selection = content.selection;
        let colors = content.colors;
        let cursor = content.cursor;

        let search = self.search_spans(display_offset);
        let grid = self.term.grid();
        let palette_key = palette_key(colors);
        let mut previous = self.snapshot_rows.borrow_mut();
        let mut current = HashMap::with_capacity(self.rows);
        let mut lines = Vec::with_capacity(self.rows);
        for row in 0..self.rows {
            let grid_line = Line(row as i32 - display_offset as i32);
            let cells = &grid[grid_line];
            let overlay = RowOverlay {
                selected: selection.and_then(|s| selected_span(&s, grid_line)),
                search: search.get(row).map_or(&[], Vec::as_slice),
                hover: self
                    .link_hover
                    .as_ref()
                    .filter(|(r, _)| *r == row)
                    .map(|(_, cols)| cols.clone()),
            };
            let key = row_key(cells, self.cols, palette_key, &overlay);
            let line = match previous.remove(&key).or_else(|| current.get(&key).cloned()) {
                Some(line) => line,
                None => build_line(cells, self.cols, colors, &overlay, key),
            };
            current.insert(key, line.clone());
            lines.push(line);
        }
        *previous = current;

        let cursor_point = point_to_viewport(display_offset, cursor.point);
        let cursor = match cursor_point {
            Some(p) if p.line < self.rows => CursorState {
                row: p.line,
                col: p.column.0.min(self.cols.saturating_sub(1)),
                is_visible: cursor.shape != CursorShape::Hidden,
                shape: cursor.shape,
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

    /// Per viewport row, the inclusive column spans of visible matches and their level
    /// (1 = match, 2 = focused). Empty without a search.
    fn search_spans(&self, display_offset: usize) -> Vec<Vec<(usize, usize, u8)>> {
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

/// Highlights drawn over a row's cells.
struct RowOverlay<'a> {
    /// Selected columns (inclusive).
    selected: Option<(usize, usize)>,
    /// Search matches (inclusive columns, level).
    search: &'a [(usize, usize, u8)],
    /// Columns under the Ctrl-hovered link.
    hover: Option<Range<usize>>,
}

/// The selected columns of grid line `line` (inclusive): the cells `SelectionRange::contains`
/// accepts on that line.
fn selected_span(range: &SelectionRange, line: Line) -> Option<(usize, usize)> {
    if line < range.start.line || line > range.end.line {
        return None;
    }
    let from = if range.is_block || line == range.start.line {
        range.start.column.0
    } else {
        0
    };
    let to = if range.is_block || line == range.end.line {
        range.end.column.0
    } else {
        usize::MAX
    };
    (from <= to).then_some((from, to))
}

/// Fast 64-bit hash (a folded multiply per word, as in foldhash) for row keys.
struct KeyHasher(u64);

impl KeyHasher {
    const MUL: u64 = 0x9E37_79B9_7F4A_7C15;
    const SEED: u64 = 0xD6E8_FEB8_6659_FD93;

    fn new() -> Self {
        Self(Self::SEED)
    }

    #[inline]
    fn write(&mut self, word: u64) {
        let product = u128::from(self.0 ^ word) * u128::from(Self::MUL);
        self.0 = (product as u64) ^ ((product >> 64) as u64);
    }

    fn finish(mut self) -> u64 {
        self.write(Self::SEED);
        self.0
    }
}

/// A color in 28 bits: a tag and the name, RGB or index.
fn color_bits(color: AnsiColor) -> u64 {
    match color {
        AnsiColor::Named(n) => (1 << 24) | n as u64,
        AnsiColor::Spec(rgb) => {
            (2 << 24) | (rgb.r as u64) << 16 | (rgb.g as u64) << 8 | rgb.b as u64
        }
        AnsiColor::Indexed(i) => (3 << 24) | i as u64,
    }
}

/// Key of the palette entries programs redefined (OSC 4), which change cell colors.
fn palette_key(colors: &Colors) -> u64 {
    let mut hasher = KeyHasher::new();
    for i in 0..256 {
        hasher.write(colors[i].map_or(0, |rgb| color_bits(AnsiColor::Spec(rgb))));
    }
    hasher.finish()
}

/// Key of everything a row's `LineData` is built from, read straight from the grid.
fn row_key(cells: &Row<Cell>, cols: usize, palette: u64, overlay: &RowOverlay) -> u64 {
    let mut hasher = KeyHasher::new();
    hasher.write(palette);
    hasher.write(cols as u64);
    for cell in &cells[..Column(cols)] {
        hasher.write(cell.c as u64 | (cell.flags.bits() as u64) << 32);
        hasher.write(color_bits(cell.fg) << 28 | color_bits(cell.bg));
        if let Some(marks) = cell.zerowidth().filter(|z| !z.is_empty()) {
            hasher.write(u64::MAX - marks.len() as u64);
            for &mark in marks {
                hasher.write(mark as u64);
            }
        }
    }
    match overlay.selected {
        Some((from, to)) => {
            hasher.write(1);
            hasher.write(from as u64);
            hasher.write(to as u64);
        }
        None => hasher.write(0),
    }
    hasher.write(overlay.search.len() as u64);
    for &(from, to, level) in overlay.search {
        hasher.write(from as u64 | (to as u64) << 24 | (level as u64) << 48);
    }
    match &overlay.hover {
        Some(cols) => {
            hasher.write(1);
            hasher.write(cols.start as u64 | (cols.end as u64) << 32);
        }
        None => hasher.write(0),
    }
    hasher.finish()
}

/// Materialize a grid row into `LineData`.
fn build_line(
    cells: &Row<Cell>,
    cols: usize,
    colors: &Colors,
    overlay: &RowOverlay,
    hash: u64,
) -> LineData {
    let mut has_rtl = false;
    let mut out: Vec<CellData> = cells[..Column(cols)]
        .iter()
        .enumerate()
        .map(|(col, cell)| {
            let c = cell.c;
            let class = bidi_class(c);
            if class == BidiClass::R || class == BidiClass::AL {
                has_rtl = true;
            }
            CellData {
                c,
                zerowidth: cell.zerowidth().map(<[char]>::to_vec).unwrap_or_default(),
                fg: program_color(cell.fg, colors),
                bg: program_color(cell.bg, colors),
                flags: cell.flags,
                selected: overlay
                    .selected
                    .is_some_and(|(from, to)| from <= col && col <= to),
                search_match: 0,
                link_hover: overlay.hover.as_ref().is_some_and(|h| h.contains(&col)),
            }
        })
        .collect();
    for &(from, to, level) in overlay.search {
        for cell in &mut out[from..=to] {
            cell.search_match = cell.search_match.max(level);
        }
    }
    LineData {
        cells: out.into(),
        has_rtl,
        hash,
    }
}
