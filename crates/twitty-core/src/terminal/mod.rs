mod osc;
mod search;
mod snapshot;

pub use osc::MAX_OSC_LEN;

use osc::{OscGuard, OscState, OscStep};
use search::SearchState;

use crate::blocks::anchor::{AbsLine, LineRemap, LineTracker};
use crate::links::find_url_at;
use crate::shell_integration::{
    clean_command_line, parse_osc, MarkKind, ShellEvent, Signal, MAX_COMMAND_CHARS,
    MAX_PENDING_EVENTS,
};
use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::{Cell, Flags as CellFlags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::viewport_to_point;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{
    Color as AnsiColor, CursorShape, CursorStyle, NamedColor, Processor, StdSyncHandler,
};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::sync::Arc;

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

    /// A plain space (or tab placeholder / no-break space) with no visible marks attached,
    /// usable as a segment separator.
    pub fn is_blank(&self) -> bool {
        !self.is_spacer() && is_blank_char(self.c) && !self.has_shaped_marks()
    }

    /// True when this cell carries at least one combining mark or joiner that reaches shaping.
    pub(crate) fn has_shaped_marks(&self) -> bool {
        !self.is_spacer() && self.zerowidth.iter().any(|&c| !is_ignored_mark(c))
    }

    /// The characters this cell contributes to a shaped run: none for spacers, spaces for
    /// control characters (such as `alacritty_terminal`'s `\t` tab placeholders), and never
    /// bidi controls or non-shaping invisible marks (see `is_ignored_mark`).
    fn shaped_chars(&self) -> impl Iterator<Item = char> + '_ {
        let cell = (!self.is_spacer()).then_some(if self.c.is_control() { ' ' } else { self.c });
        cell.into_iter()
            .filter(|&c| !is_ignored_mark(c))
            .chain(
                self.zerowidth
                    .iter()
                    .copied()
                    .filter(|&c| !self.is_spacer() && !is_ignored_mark(c)),
            )
            .flat_map(base_letters)
    }

    /// Separates words for bidi: any space, including the no-break space programs put after a
    /// prompt (Claude Code writes `❯` + U+00A0) and tab placeholders.
    pub fn is_word_break(&self) -> bool {
        !self.is_spacer()
            && !self.has_shaped_marks()
            && (self.c.is_whitespace() || self.c.is_control())
    }

    /// Text this cell contributes to a shaped run (empty for spacers).
    pub fn push_text(&self, out: &mut String) {
        out.extend(self.shaped_chars());
    }
}

/// Arabic presentation forms (`U+FB50–FDFF`, `U+FE70–FEFC`) as the plain letters they stand
/// for; any other character as itself.
///
/// vim and nvim shape Arabic themselves by default ('arabicshape'): they send each letter
/// already in its joined form, in logical order. Twitty shapes and reorders Arabic itself, so
/// pre-shaped letters came out unjoined and reversed. Turned back into plain letters they are
/// joined and laid out like any other Arabic text (lam-alef ligatures become lam + alef).
fn base_letters(c: char) -> impl Iterator<Item = char> {
    use unicode_normalization::char::decompose_compatible;
    let mut out = smallvec_like::Chars::default();
    if matches!(c, '\u{FB50}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFC}') {
        decompose_compatible(c, |d| out.push(d));
    }
    if out.is_empty() {
        out.push(c);
    }
    out.into_iter()
}

/// A few chars without allocating (a presentation form decomposes to at most a handful).
mod smallvec_like {
    #[derive(Default)]
    pub struct Chars {
        buf: [char; 8],
        len: usize,
    }

    impl Chars {
        pub fn push(&mut self, c: char) {
            if self.len < self.buf.len() {
                self.buf[self.len] = c;
                self.len += 1;
            }
        }

        pub fn is_empty(&self) -> bool {
            self.len == 0
        }
    }

    impl IntoIterator for Chars {
        type Item = char;
        type IntoIter = std::iter::Take<std::array::IntoIter<char, 8>>;

        fn into_iter(self) -> Self::IntoIter {
            self.buf.into_iter().take(self.len)
        }
    }
}

/// Bidi embeddings, overrides and isolates (LRE, RLE, PDF, LRO, RLO, LRI, RLI, FSI, PDI).
///
/// Twitty lays out right-to-left text itself and wraps it in its own isolates; these controls
/// from a program's output would reorder a line arbitrarily (`cat` of a file could make text
/// read differently from what it says, as in "Trojan Source") or unbalance Twitty's isolates.
/// They are invisible, so they are simply not shaped. Direction marks (LRM, RLM, ALM) only
/// settle neutrals and stay.
#[doc(hidden)]
pub fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// A single-cell blank character in the terminal grid: plain space, `alacritty_terminal`'s
/// `\t` tab placeholder (which it keeps in the grid so copy preserves literal tabs),
/// no-break space, or any control character.
fn is_blank_char(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\u{00A0}') || c.is_control()
}

/// Zero-width characters that carry no visible mark and play no role in shaping: bidi
/// controls, control characters, and invisible break/join formatters (ZWSP, word joiner, BOM,
/// combining grapheme joiner, invisible math operators). ZWNJ/ZWJ, variation selectors and
/// LRM/RLM/ALM stay because they affect Arabic joining, emoji sequences and neutral runs.
pub(crate) fn is_ignored_mark(c: char) -> bool {
    is_bidi_control(c)
        || c.is_control()
        || matches!(
            c,
            '\u{034F}' | '\u{200B}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}'
        )
}

/// Narrowest grid: a double-width character needs two columns.
pub(crate) const MIN_COLS: usize = 2;

/// Combining marks of one cell that are drawn. Real text needs a few (Arabic harakat, accents,
/// emoji sequences); a program can attach millions to one cell, which would make every frame
/// hash and shape them all.
pub const MAX_DRAWN_MARKS: usize = 16;

/// The combining marks of a grid cell that are drawn and hashed.
fn drawn_marks(cell: &Cell) -> &[char] {
    let marks = cell.zerowidth().unwrap_or(&[]);
    &marks[..marks.len().min(MAX_DRAWN_MARKS)]
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

/// Where the main screen is in absolute lines (see `blocks::anchor`), for drawing and
/// navigating command blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineView {
    /// Absolute line of the top viewport row.
    pub viewport_top: AbsLine,
    pub rows: usize,
    pub cols: usize,
    /// Oldest line still in the scrollback.
    pub oldest: AbsLine,
    /// The cursor: absolute line and column.
    pub cursor: (AbsLine, usize),
    /// Changes when earlier absolute lines lost their meaning (cleared scrollback, a flood).
    pub epoch: u64,
    /// Sequence number of the newest reflow remap (`Terminal::line_remaps_since`).
    pub remap_seq: u64,
}

/// Where a line read by [`Terminal::for_each_text_line`] is, to find it again later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextLinePos {
    /// A main-screen line by absolute number (see `blocks::anchor`), valid while the tracker's
    /// `epoch` and `remap_seq` are unchanged.
    Main {
        line: AbsLine,
        epoch: u64,
        remap_seq: u64,
    },
    /// A row of the alternate screen (0 = top), which has no scrollback.
    Alt(i32),
}

/// One character of a text line: a cell's own character (`mark` false) or one of the combining
/// marks drawn on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextChar {
    /// As stored in the cell (Arabic presentation forms are not decomposed).
    pub c: char,
    pub mark: bool,
    /// Row within the logical line (0 = its first row) and column of the cell.
    pub row: u16,
    pub col: u16,
    /// The cell is a double-width character.
    pub wide: bool,
}

/// How far a [`Terminal::for_each_text_line`] call got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextScan {
    /// Rows read.
    pub rows: usize,
    /// The row to go on from when it stopped early; `None` once the rows ran out.
    pub next: Option<TextLinePos>,
}

/// Most rows joined into one logical line by [`Terminal::for_each_text_line`].
pub const MAX_JOINED_ROWS: usize = 16;

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
    osc_guard: OscGuard,
    cols: usize,
    rows: usize,
    search: Option<SearchState>,
    link_hover: Option<(usize, Range<usize>)>,
    /// The last snapshot's rows by key, so rows that did not change are not rebuilt.
    snapshot_rows: RefCell<HashMap<u64, LineData>>,
    /// Shell signals (OSC 7, 9, 99, 133, 633, 777) not yet taken by the app.
    shell_events: VecDeque<ShellEvent>,
    /// Absolute line numbers for command blocks (see `blocks::anchor`).
    lines: LineTracker,
    /// Where the last OSC 133 B mark was, until its C mark: the command line is read from there
    /// when the shell sent no OSC 633 E.
    command_start: Option<(AbsLine, usize)>,
    scrollback: usize,
    /// Monotonically incremented whenever terminal grid content or dimensions change.
    content_generation: u64,
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
fn term_config(cursor_blink: bool, scrollback: usize) -> Config {
    Config {
        default_cursor_style: CursorStyle {
            shape: CursorShape::Block,
            blinking: cursor_blink,
        },
        kitty_keyboard: true,
        scrolling_history: scrollback,
        ..Config::default()
    }
}

/// Lines of scrollback a terminal keeps.
pub const SCROLLBACK_LINES: usize = 10_000;

/// Most rows a typed command line is read from (OSC 133 B to C) when no OSC 633 E came.
const MAX_TYPED_COMMAND_LINES: AbsLine = 64;

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
        Self::build(cols, rows, SCROLLBACK_LINES, on_event)
    }

    /// Terminal with `scrollback` lines of history that ignores its events (tests of what happens
    /// when the scrollback is full).
    pub fn with_scrollback(cols: usize, rows: usize, scrollback: usize) -> Self {
        Self::build(cols, rows, scrollback, |_| {})
    }

    fn build<F>(cols: usize, rows: usize, scrollback: usize, on_event: F) -> Self
    where
        F: Fn(Event) + Send + Sync + 'static,
    {
        let (cols, rows) = (cols.max(MIN_COLS), rows.max(1));
        let size = TermSize::new(cols, rows);
        let listener = ForwardListener {
            on_event: Arc::new(on_event),
        };
        let term = Term::new(term_config(true, scrollback), &size, listener);
        let parser = Processor::<StdSyncHandler>::new();
        Self {
            term,
            parser,
            osc_guard: OscGuard::default(),
            cols,
            rows,
            search: None,
            link_hover: None,
            snapshot_rows: RefCell::default(),
            shell_events: VecDeque::new(),
            lines: LineTracker::new(scrollback),
            command_start: None,
            scrollback,
            content_generation: 1,
        }
    }

    pub fn process_bytes(&mut self, bytes: &[u8]) {
        let mut from = 0;
        let mut i = 0;
        while i < bytes.len() {
            // Outside an OSC only its opener matters: skip to the next `ESC ]`, or to a
            // trailing ESC whose `]` may come in the next read.
            if self.osc_guard.state == OscState::Other {
                static OSC_START: std::sync::LazyLock<memchr::memmem::Finder<'static>> =
                    std::sync::LazyLock::new(|| memchr::memmem::Finder::new(b"\x1b]"));
                match OSC_START.find(&bytes[i..]) {
                    Some(at) => i += at,
                    None if bytes.last() == Some(&0x1B) => i = bytes.len() - 1,
                    None => break,
                }
            }
            let step = self.osc_guard.step(bytes[i]);
            if step == OscStep::End {
                let signal = (!self.osc_guard.payload_cut)
                    .then(|| parse_osc(&self.osc_guard.payload))
                    .flatten();
                if let Some(signal) = signal {
                    // Parse what came before the terminator first, so the event records
                    // where the cursor was when the shell sent it.
                    self.parser.advance(&mut self.term, &bytes[from..=i]);
                    from = i + 1;
                    self.push_shell_event(signal);
                }
            } else if step != OscStep::Keep {
                self.parser.advance(&mut self.term, &bytes[from..i]);
                if step == OscStep::Spoil {
                    self.parser.advance(&mut self.term, b"!");
                }
                from = i + 1;
            }
            i += 1;
        }
        self.parser.advance(&mut self.term, &bytes[from..]);
        if !bytes.is_empty() {
            self.content_generation = self.content_generation.wrapping_add(1).max(1);
        }
        if self.lines.enabled() {
            self.sync_lines();
        }
    }

    fn is_alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    fn sync_lines(&mut self) {
        let alt = self.is_alt_screen();
        self.lines.sync(self.term.grid(), alt);
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
        self.term
            .set_options(term_config(cursor_blink, self.scrollback));
    }

    /// Whether the cursor should blink: the program's DECSCUSR choice, else the default.
    pub fn cursor_blinking(&self) -> bool {
        self.term.cursor_style().blinking
    }

    /// Shell signals received since the last call, oldest first.
    pub fn take_shell_events(&mut self) -> Vec<ShellEvent> {
        self.shell_events.drain(..).collect()
    }

    fn push_shell_event(&mut self, signal: Signal) {
        let point = self.term.grid().cursor.point;
        let alt = self.is_alt_screen();
        let mut abs_line = None;
        if let Signal::Mark(kind) = signal {
            self.lines.enable(self.term.grid(), alt);
            self.lines.sync(self.term.grid(), alt);
            abs_line = (!alt).then(|| self.lines.top() + AbsLine::from(point.line.0));
            match kind {
                MarkKind::CommandStart => {
                    self.command_start = abs_line.map(|line| (line, point.column.0));
                }
                MarkKind::OutputStart => {
                    // No OSC 633 E: the command line is what was typed after the prompt.
                    // Bounded: a program can send B, a flood, then C.
                    let typed = self
                        .command_start
                        .take()
                        .zip(abs_line)
                        .filter(|((b, _), c)| c - b <= MAX_TYPED_COMMAND_LINES)
                        .and_then(|(b, c)| self.text_between(b, (c, point.column.0)));
                    let typed =
                        typed.map(|t| clean_command_line(&t.replace('\n', " "), MAX_COMMAND_CHARS));
                    if let Some(text) = typed.filter(|t| !t.is_empty()) {
                        self.push_event(ShellEvent {
                            signal: Signal::CommandLine(text),
                            line: point.line.0,
                            column: point.column.0,
                            history: self.term.grid().history_size(),
                            abs_line: None,
                        });
                    }
                }
                _ => {}
            }
        } else if let Signal::CommandLine(_) = signal {
            self.command_start = None;
        }
        self.push_event(ShellEvent {
            signal,
            line: point.line.0,
            column: point.column.0,
            history: self.term.grid().history_size(),
            abs_line,
        });
    }

    fn push_event(&mut self, event: ShellEvent) {
        if self.shell_events.len() >= MAX_PENDING_EVENTS {
            self.shell_events.pop_front();
        }
        self.shell_events.push_back(event);
    }

    /// Absolute-line bookkeeping of the main screen, for command blocks. `None` until the shell
    /// sent its first OSC 133 mark, and while the alternate screen is shown.
    pub fn line_view(&self) -> Option<LineView> {
        if !self.lines.enabled() || self.is_alt_screen() {
            return None;
        }
        let grid = self.term.grid();
        let top = self.lines.top();
        let cursor = grid.cursor.point;
        Some(LineView {
            viewport_top: top - grid.display_offset() as AbsLine,
            rows: self.rows,
            cols: self.cols,
            oldest: top - grid.history_size() as AbsLine,
            cursor: (top + AbsLine::from(cursor.line.0), cursor.column.0),
            epoch: self.lines.epoch(),
            remap_seq: self.lines.remap_seq(),
        })
    }

    /// Oldest absolute line of the main screen's scrollback, also while the alternate screen is
    /// shown. `None` until the first OSC 133 mark.
    pub fn oldest_line(&self) -> Option<AbsLine> {
        self.lines.enabled().then(|| self.lines.oldest())
    }

    /// Reflow remaps after sequence number `seen` (see `LineView::remap_seq`), or `None` when
    /// some of them are no longer kept.
    pub fn line_remaps_since(&self, seen: u64) -> Option<&[LineRemap]> {
        self.lines.remaps_since(seen)
    }

    /// Grid point of an absolute line and column, if the line is still kept.
    fn abs_point(&self, (line, column): (AbsLine, usize)) -> Option<Point> {
        let view = self.line_view()?;
        let grid_line = line.checked_sub(self.lines.top())?;
        if line < view.oldest || grid_line >= self.rows as AbsLine {
            return None;
        }
        let grid_line = i32::try_from(grid_line).ok()?;
        Some(Point::new(
            Line(grid_line),
            Column(column.min(self.cols - 1)),
        ))
    }

    /// Text from `start` up to (not including) `end`, both absolute (line, column): logical
    /// order, soft-wrapped rows joined, hard line ends as `\n`, trailing blanks of each line
    /// dropped. `None` when a line left the scrollback or the alternate screen is shown.
    pub fn text_between(&self, start: (AbsLine, usize), end: (AbsLine, usize)) -> Option<String> {
        let from = self.abs_point(start)?;
        let (end_line, end_col) = end;
        // The cell before `end`.
        let to = if end_col == 0 {
            let to = self.abs_point((end_line.checked_sub(1)?, 0))?;
            Point::new(to.line, Column(self.cols - 1))
        } else {
            self.abs_point((end_line, end_col - 1))?
        };
        if to < from {
            return Some(String::new());
        }
        Some(self.term.bounds_to_string(from, to))
    }

    /// Scroll so absolute line `line` is the top row (as far as the scrollback allows).
    pub fn scroll_to_line(&mut self, line: AbsLine) {
        if self.line_view().is_none() {
            return;
        }
        let grid_line = line.saturating_sub(self.lines.top());
        let offset = grid_line
            .saturating_neg()
            .clamp(0, self.term.grid().history_size() as AbsLine);
        let delta = offset
            .saturating_sub(self.display_offset() as AbsLine)
            .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        self.term.scroll_display(Scroll::Delta(delta));
    }

    /// Grid position of the cursor, to tell whether output moved it.
    pub fn cursor_position(&self) -> (i32, usize) {
        let point = self.term.grid().cursor.point;
        (point.line.0, point.column.0)
    }

    /// Resize the grid; it never gets smaller than `MIN_COLS` x 1 (alacritty panics on an empty
    /// grid, and on a wide character in a one-column one).
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(MIN_COLS), rows.max(1));
        self.cols = cols;
        self.rows = rows;
        let alt = self.is_alt_screen();
        let prep = self.lines.before_resize(self.term.grid(), alt, cols, rows);
        self.term.resize(TermSize::new(cols, rows));
        self.content_generation = self.content_generation.wrapping_add(1).max(1);
        self.lines.after_resize(self.term.grid(), alt, prep);
    }

    pub fn size(&self) -> (usize, usize) {
        (self.cols, self.rows)
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
            for z in drawn_marks(cell) {
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
