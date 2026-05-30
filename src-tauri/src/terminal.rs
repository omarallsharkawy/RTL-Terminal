use serde::Serialize;

use crate::bidi::{visual_line, TerminalLine};

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CellStyle {
    pub fg: Option<[u8; 3]>,
    pub bg: Option<[u8; 3]>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

#[derive(Debug, Clone)]
pub struct Cell {
    pub ch: char,
    pub style: CellStyle,
}

impl Default for Cell {
    fn default() -> Self {
        Self { ch: ' ', style: CellStyle::default() }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalFrame {
    pub cols: usize,
    pub rows: usize,
    pub cursor_col: usize,
    pub cursor_row: usize,
    pub cursor_visible: bool,
    pub title: String,
    pub lines: Vec<TerminalLine>,
}

#[derive(Debug, Clone)]
pub struct TerminalGrid {
    cols: usize,
    rows: usize,
    cursor_col: usize,
    cursor_row: usize,
    title: String,
    cells: Vec<Vec<Cell>>,
    current_style: CellStyle,
    force_rtl: bool,
}

impl TerminalGrid {
    pub fn new(cols: usize, rows: usize) -> Self {
        let cols = cols.max(20);
        let rows = rows.max(6);
        Self {
            cols,
            rows,
            cursor_col: 0,
            cursor_row: 0,
            title: String::from("PowerShell"),
            cells: vec![vec![Cell::default(); cols]; rows],
            current_style: CellStyle { fg: Some([228, 238, 247]), ..CellStyle::default() },
            force_rtl: false,
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(20);
        let rows = rows.max(6);
        self.cols = cols;
        self.rows = rows;
        self.cells.resize_with(rows, || vec![Cell::default(); cols]);
        for row in &mut self.cells {
            row.resize(cols, Cell::default());
        }
        self.cursor_col = self.cursor_col.min(cols - 1);
        self.cursor_row = self.cursor_row.min(rows - 1);
    }

    pub fn put_char(&mut self, ch: char) {
        match ch {
            '\r' => self.cursor_col = 0,
            '\n' => self.line_feed(),
            '\u{08}' => self.backspace(),
            '\t' => {
                let spaces = 4 - (self.cursor_col % 4);
                for _ in 0..spaces { self.put_char(' '); }
            }
            ch if ch.is_control() => {}
            ch => {
                if self.cursor_col >= self.cols {
                    self.cursor_col = 0;
                    self.line_feed();
                }
                self.cells[self.cursor_row][self.cursor_col] = Cell { ch, style: self.current_style.clone() };
                self.cursor_col += 1;
            }
        }
    }

    pub fn sgr(&mut self, params: &[u16]) {
        if params.is_empty() {
            self.current_style = CellStyle { fg: Some([228, 238, 247]), ..CellStyle::default() };
            return;
        }
        for param in params {
            match *param {
                0 => self.current_style = CellStyle { fg: Some([228, 238, 247]), ..CellStyle::default() },
                1 => self.current_style.bold = true,
                3 => self.current_style.italic = true,
                4 => self.current_style.underline = true,
                7 => self.current_style.inverse = true,
                22 => self.current_style.bold = false,
                23 => self.current_style.italic = false,
                24 => self.current_style.underline = false,
                27 => self.current_style.inverse = false,
                30..=37 => self.current_style.fg = Some(ansi_color((*param - 30) as u8, false)),
                90..=97 => self.current_style.fg = Some(ansi_color((*param - 90) as u8, true)),
                40..=47 => self.current_style.bg = Some(ansi_color((*param - 40) as u8, false)),
                100..=107 => self.current_style.bg = Some(ansi_color((*param - 100) as u8, true)),
                39 => self.current_style.fg = Some([228, 238, 247]),
                49 => self.current_style.bg = None,
                _ => {}
            }
        }
    }

    pub fn clear_screen(&mut self) {
        self.cells = vec![vec![Cell::default(); self.cols]; self.rows];
        self.cursor_col = 0;
        self.cursor_row = 0;
    }

    pub fn clear_line_from_cursor(&mut self) {
        for col in self.cursor_col..self.cols {
            self.cells[self.cursor_row][col] = Cell::default();
        }
    }

    pub fn move_cursor(&mut self, row: usize, col: usize) {
        self.cursor_row = row.saturating_sub(1).min(self.rows - 1);
        self.cursor_col = col.saturating_sub(1).min(self.cols - 1);
    }

    pub fn set_title(&mut self, title: String) {
        self.title = title;
    }

    pub fn frame(&self) -> TerminalFrame {
        let lines = self.cells.iter().enumerate().map(|(row, cells)| {
            let logical: Vec<_> = cells.iter().map(|cell| (cell.ch, cell.style.clone())).collect();
            visual_line(row, &logical, self.force_rtl)
        }).collect();
        TerminalFrame {
            cols: self.cols,
            rows: self.rows,
            cursor_col: self.cursor_col.min(self.cols - 1),
            cursor_row: self.cursor_row.min(self.rows - 1),
            cursor_visible: true,
            title: self.title.clone(),
            lines,
        }
    }

    fn line_feed(&mut self) {
        if self.cursor_row + 1 >= self.rows {
            self.cells.remove(0);
            self.cells.push(vec![Cell::default(); self.cols]);
        } else {
            self.cursor_row += 1;
        }
    }

    fn backspace(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
            self.cells[self.cursor_row][self.cursor_col] = Cell::default();
        }
    }
}

fn ansi_color(index: u8, bright: bool) -> [u8; 3] {
    const NORMAL: [[u8; 3]; 8] = [[26, 31, 38], [239, 68, 68], [34, 197, 94], [234, 179, 8], [59, 130, 246], [168, 85, 247], [6, 182, 212], [228, 238, 247]];
    const BRIGHT: [[u8; 3]; 8] = [[100, 116, 139], [248, 113, 113], [74, 222, 128], [250, 204, 21], [96, 165, 250], [192, 132, 252], [34, 211, 238], [248, 250, 252]];
    if bright { BRIGHT[index as usize] } else { NORMAL[index as usize] }
}

