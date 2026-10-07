use crate::color::Palette;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
pub use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{viewport_to_point, Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape, Processor, StdSyncHandler};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use unicode_bidi::{bidi_class, BidiClass};

pub type ClipboardFormatter = Arc<dyn Fn(&str) -> String + Sync + Send + 'static>;
pub type ClipboardLoader = Arc<dyn Fn(ClipboardFormatter) + Send + Sync>;

struct ForwardListener {
    on_pty_write: Arc<dyn Fn(String) + Send + Sync>,
    on_clipboard_store: Arc<dyn Fn(String) + Send + Sync>,
    on_clipboard_load: ClipboardLoader,
    on_title_change: Arc<dyn Fn(String) + Send + Sync>,
    on_bell: Arc<dyn Fn() + Send + Sync>,
    palette: Arc<Palette>,
    cell_width: Arc<AtomicU16>,
    cell_height: Arc<AtomicU16>,
    num_cols: Arc<AtomicU16>,
    num_lines: Arc<AtomicU16>,
}

impl EventListener for ForwardListener {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => (self.on_pty_write)(text),
            Event::ClipboardStore(_, text) => (self.on_clipboard_store)(text),
            Event::ClipboardLoad(_, formatter) => (self.on_clipboard_load)(formatter),
            Event::ColorRequest(index, formatter) => {
                let rgb = self.palette.resolve_rgb(index);
                let response = formatter(rgb);
                (self.on_pty_write)(response);
            }
            Event::TextAreaSizeRequest(formatter) => {
                let size = WindowSize {
                    num_lines: self.num_lines.load(Ordering::Relaxed),
                    num_cols: self.num_cols.load(Ordering::Relaxed),
                    cell_width: self.cell_width.load(Ordering::Relaxed),
                    cell_height: self.cell_height.load(Ordering::Relaxed),
                };
                let response = formatter(size);
                (self.on_pty_write)(response);
            }
            Event::Title(title) => (self.on_title_change)(title),
            Event::ResetTitle => (self.on_title_change)("Twitty · RTL Terminal".to_string()),
            Event::Bell => (self.on_bell)(),
            _ => {}
        }
    }
}

#[derive(Clone, Debug)]
pub struct CellData {
    pub c: char,
    pub zerowidth: Vec<char>,
    pub fg: AnsiColor,
    pub bg: AnsiColor,
    pub flags: CellFlags,
    pub is_selected: bool,
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
    cell_width: Arc<AtomicU16>,
    cell_height: Arc<AtomicU16>,
    num_cols: Arc<AtomicU16>,
    num_lines: Arc<AtomicU16>,
}

impl Terminal {
    #[allow(dead_code)]
    pub fn new<F>(cols: usize, rows: usize, on_pty_write: F) -> Self
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        Self::new_full(
            cols,
            rows,
            10000,
            on_pty_write,
            |_| {},
            |_| {},
            |_| {},
            || {},
        )
    }

    #[allow(dead_code)]
    pub fn new_with_clipboard<F, C>(
        cols: usize,
        rows: usize,
        on_pty_write: F,
        on_clipboard_store: C,
    ) -> Self
    where
        F: Fn(String) + Send + Sync + 'static,
        C: Fn(String) + Send + Sync + 'static,
    {
        Self::new_full(
            cols,
            rows,
            10000,
            on_pty_write,
            on_clipboard_store,
            |_| {},
            |_| {},
            || {},
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_full<F, C, L, T, B>(
        cols: usize,
        rows: usize,
        scrollback_lines: usize,
        on_pty_write: F,
        on_clipboard_store: C,
        on_clipboard_load: L,
        on_title_change: T,
        on_bell: B,
    ) -> Self
    where
        F: Fn(String) + Send + Sync + 'static,
        C: Fn(String) + Send + Sync + 'static,
        L: Fn(ClipboardFormatter) + Send + Sync + 'static,
        T: Fn(String) + Send + Sync + 'static,
        B: Fn() + Send + Sync + 'static,
    {
        let size = TermSize::new(cols, rows);
        let cell_width = Arc::new(AtomicU16::new(10));
        let cell_height = Arc::new(AtomicU16::new(20));
        let num_cols = Arc::new(AtomicU16::new(cols as u16));
        let num_lines = Arc::new(AtomicU16::new(rows as u16));
        let palette = Arc::new(Palette::default());
        let listener = ForwardListener {
            on_pty_write: Arc::new(on_pty_write),
            on_clipboard_store: Arc::new(on_clipboard_store),
            on_clipboard_load: Arc::new(on_clipboard_load),
            on_title_change: Arc::new(on_title_change),
            on_bell: Arc::new(on_bell),
            palette,
            cell_width: cell_width.clone(),
            cell_height: cell_height.clone(),
            num_cols: num_cols.clone(),
            num_lines: num_lines.clone(),
        };
        let term_config = Config {
            scrolling_history: scrollback_lines.max(100),
            osc52: alacritty_terminal::term::Osc52::CopyPaste,
            ..Default::default()
        };
        let term = Term::new(term_config, &size, listener);
        let parser = Processor::<StdSyncHandler>::new();
        Self {
            term,
            parser,
            cols,
            rows,
            cell_width,
            cell_height,
            num_cols,
            num_lines,
        }
    }

    pub fn process_bytes(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.rows = rows;
        self.num_cols.store(cols as u16, Ordering::Relaxed);
        self.num_lines.store(rows as u16, Ordering::Relaxed);
        self.term.resize(TermSize::new(cols, rows));
    }

    pub fn update_cell_size(&self, cell_width: f32, cell_height: f32) {
        self.cell_width
            .store(cell_width.round() as u16, Ordering::Relaxed);
        self.cell_height
            .store(cell_height.round() as u16, Ordering::Relaxed);
    }

    pub fn sync_timeout(&self) -> Option<std::time::Instant> {
        self.parser.sync_timeout().sync_timeout()
    }

    pub fn stop_sync(&mut self) {
        self.parser.stop_sync(&mut self.term);
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

    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    pub fn scroll_to_line(&mut self, line_idx: i32) {
        if line_idx < 0 {
            let target_offset = (-line_idx) as usize;
            let cur_offset = self.term.grid().display_offset();
            let delta = target_offset as i32 - cur_offset as i32;
            self.term.scroll_display(Scroll::Delta(delta));
        } else {
            self.scroll_to_bottom();
        }
    }

    pub fn search(&self, query: &str) -> Vec<(i32, usize, usize)> {
        if query.is_empty() {
            return Vec::new();
        }
        let query_chars: Vec<char> = query.to_lowercase().chars().collect();
        let query_len = query_chars.len();
        if query_len == 0 {
            return Vec::new();
        }

        let topmost = self.term.topmost_line().0;
        let bottommost = self.term.bottommost_line().0;
        let mut matches = Vec::new();

        for line_idx in topmost..=bottommost {
            let line = Line(line_idx);
            let row = &self.term.grid()[line];
            let line_chars: Vec<char> = (0..self.cols)
                .map(|c| {
                    row[Column(c)]
                        .c
                        .to_lowercase()
                        .next()
                        .unwrap_or(row[Column(c)].c)
                })
                .collect();

            if self.cols >= query_len {
                let mut col = 0;
                while col <= self.cols - query_len {
                    if line_chars[col..col + query_len] == query_chars[..] {
                        matches.push((line_idx, col, query_len));
                        col += query_len.max(1);
                    } else {
                        col += 1;
                    }
                }
            }
        }
        matches
    }

    pub fn start_selection(&mut self, col: usize, row: usize) {
        self.start_selection_type(col, row, SelectionType::Simple);
    }

    pub fn start_selection_type(&mut self, col: usize, row: usize, ty: SelectionType) {
        let point = viewport_to_point(
            self.term.grid().display_offset(),
            Point::new(row, Column(col)),
        );
        self.term.selection = Some(Selection::new(ty, point, Side::Left));
    }

    pub fn update_selection(&mut self, col: usize, row: usize) {
        let point = viewport_to_point(
            self.term.grid().display_offset(),
            Point::new(row, Column(col)),
        );
        if let Some(ref mut sel) = self.term.selection {
            sel.update(point, Side::Right);
        }
    }

    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.selection_to_string()
    }

    pub fn select_all(&mut self) {
        let topmost = self.term.topmost_line();
        let bottommost = self.term.bottommost_line();
        let cols = self.cols.saturating_sub(1);
        let mut sel = Selection::new(
            SelectionType::Simple,
            Point::new(topmost, Column(0)),
            Side::Left,
        );
        sel.update(Point::new(bottommost, Column(cols)), Side::Right);
        self.term.selection = Some(sel);
    }

    pub fn snapshot(&self) -> (Vec<LineData>, CursorState) {
        let content = self.term.renderable_content();
        let selection = content.selection;
        let mut lines = vec![
            LineData {
                cells: vec![
                    CellData {
                        c: ' ',
                        zerowidth: Vec::new(),
                        fg: AnsiColor::Named(alacritty_terminal::vte::ansi::NamedColor::Foreground),
                        bg: AnsiColor::Named(alacritty_terminal::vte::ansi::NamedColor::Background),
                        flags: CellFlags::empty(),
                        is_selected: false,
                    };
                    self.cols
                ],
                has_rtl: false,
            };
            self.rows
        ];

        for cell in content.display_iter {
            let viewport_row = cell.point.line.0 + content.display_offset as i32;
            let col = cell.point.column.0;
            if viewport_row >= 0 && (viewport_row as usize) < self.rows && col < self.cols {
                let row = viewport_row as usize;
                let c = cell.c;
                let class = bidi_class(c);
                let is_rtl = class == BidiClass::R || class == BidiClass::AL;
                if is_rtl {
                    lines[row].has_rtl = true;
                }
                let is_selected = selection
                    .as_ref()
                    .map(|s| s.contains(cell.point))
                    .unwrap_or(false);
                let zerowidth = cell.zerowidth().map(|z| z.to_vec()).unwrap_or_default();
                lines[row].cells[col] = CellData {
                    c,
                    zerowidth,
                    fg: cell.fg,
                    bg: cell.bg,
                    flags: cell.flags,
                    is_selected,
                };
            }
        }

        let cursor_viewport_line = content.cursor.point.line.0 + content.display_offset as i32;
        let cursor_col = content.cursor.point.column.0;
        let is_cursor_in_viewport =
            cursor_viewport_line >= 0 && (cursor_viewport_line as usize) < self.rows;
        let cursor = CursorState {
            row: if is_cursor_in_viewport {
                cursor_viewport_line as usize
            } else {
                0
            },
            col: cursor_col.min(self.cols.saturating_sub(1)),
            is_visible: is_cursor_in_viewport
                && content.cursor.shape != alacritty_terminal::vte::ansi::CursorShape::Hidden,
            shape: content.cursor.shape,
        };

        (lines, cursor)
    }
}
