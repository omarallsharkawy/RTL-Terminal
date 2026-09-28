use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{viewport_to_point, Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color as AnsiColor, Processor, StdSyncHandler};
use std::sync::Arc;
use unicode_bidi::{bidi_class, BidiClass};

struct ForwardListener {
    on_pty_write: Arc<dyn Fn(String) + Send + Sync>,
}

impl EventListener for ForwardListener {
    fn send_event(&self, event: Event) {
        if let Event::PtyWrite(text) = event {
            (self.on_pty_write)(text);
        }
    }
}

#[derive(Clone, Debug)]
pub struct CellData {
    pub c: char,
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
}

pub struct Terminal {
    term: Term<ForwardListener>,
    parser: Processor<StdSyncHandler>,
    cols: usize,
    rows: usize,
}

impl Terminal {
    pub fn new<F>(cols: usize, rows: usize, on_pty_write: F) -> Self
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        let size = TermSize::new(cols, rows);
        let listener = ForwardListener {
            on_pty_write: Arc::new(on_pty_write),
        };
        let term = Term::new(Config::default(), &size, listener);
        let parser = Processor::<StdSyncHandler>::new();
        Self {
            term,
            parser,
            cols,
            rows,
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

    pub fn is_app_cursor(&self) -> bool {
        self.term.mode().contains(TermMode::APP_CURSOR)
    }

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    pub fn scroll_display(&mut self, delta: i32) {
        self.term.scroll_display(Scroll::Delta(delta));
    }

    pub fn start_selection(&mut self, col: usize, row: usize) {
        let point = viewport_to_point(
            self.term.grid().display_offset(),
            Point::new(row, Column(col)),
        );
        self.term.selection = Some(Selection::new(SelectionType::Simple, point, Side::Left));
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
            let row = cell.point.line.0 as usize;
            let col = cell.point.column.0;
            if row < self.rows && col < self.cols {
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
                lines[row].cells[col] = CellData {
                    c,
                    fg: cell.fg,
                    bg: cell.bg,
                    flags: cell.flags,
                    is_selected,
                };
            }
        }

        let cursor_row = content.cursor.point.line.0 as usize;
        let cursor_col = content.cursor.point.column.0;
        let cursor = CursorState {
            row: cursor_row.min(self.rows.saturating_sub(1)),
            col: cursor_col.min(self.cols.saturating_sub(1)),
            is_visible: content.cursor.shape != alacritty_terminal::vte::ansi::CursorShape::Hidden,
        };

        (lines, cursor)
    }
}
