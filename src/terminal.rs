use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color as AnsiColor, Processor, StdSyncHandler};
use unicode_bidi::{bidi_class, BidiClass};

struct DummyListener;
impl EventListener for DummyListener {
    fn send_event(&self, _event: Event) {}
}

#[derive(Clone, Debug)]
pub struct CellData {
    pub c: char,
    pub fg: AnsiColor,
    pub bg: AnsiColor,
    pub flags: CellFlags,
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
    term: Term<DummyListener>,
    parser: Processor<StdSyncHandler>,
    cols: usize,
    rows: usize,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Self {
        let size = TermSize::new(cols, rows);
        let term = Term::new(Config::default(), &size, DummyListener);
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

    pub fn snapshot(&self) -> (Vec<LineData>, CursorState) {
        let content = self.term.renderable_content();
        let mut lines = vec![
            LineData {
                cells: vec![
                    CellData {
                        c: ' ',
                        fg: AnsiColor::Named(alacritty_terminal::vte::ansi::NamedColor::Foreground),
                        bg: AnsiColor::Named(alacritty_terminal::vte::ansi::NamedColor::Background),
                        flags: CellFlags::empty(),
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
                lines[row].cells[col] = CellData {
                    c,
                    fg: cell.fg,
                    bg: cell.bg,
                    flags: cell.flags,
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
