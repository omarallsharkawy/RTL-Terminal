//! Viewport snapshotting, row hashing, and logical recent-line extraction.

use std::collections::HashMap;
use std::ops::Range;

use alacritty_terminal::grid::{Dimensions, Row};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::selection::SelectionRange;
use alacritty_terminal::term::cell::{Cell, Flags as CellFlags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::point_to_viewport;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape};
use unicode_bidi::{bidi_class, BidiClass};

use super::{
    drawn_marks, is_bidi_control, program_color, CellData, CursorState, LineData, Terminal,
};

impl Terminal {
    /// Up to the last `max_lines` lines from scrollback and screen in logical grid order,
    /// omitting trailing blank lines below the last non-empty row.
    pub fn recent_lines(&self, max_lines: usize) -> String {
        if max_lines == 0 {
            return String::new();
        }
        let grid = self.term.grid();
        let top = self.term.topmost_line().0;
        let bottom = self.term.bottommost_line().0;
        let row_is_blank = |line: i32| {
            let row = &grid[Line(line)];
            (0..self.cols).all(|c| {
                let cell = &row[Column(c)];
                cell.c == ' ' && drawn_marks(cell).is_empty()
            })
        };
        let mut end = bottom;
        while end >= top && row_is_blank(end) {
            end -= 1;
        }
        if end < top {
            return String::new();
        }
        let available = (end - top + 1) as usize;
        let take = available.min(max_lines);
        let start = top + (available - take) as i32;
        let mut out = String::new();
        for (i, line) in (start..=end).enumerate() {
            if i > 0 {
                out.push('\n');
            }
            let row = &grid[Line(line)];
            let mut text = String::new();
            for c in 0..self.cols {
                let cell = &row[Column(c)];
                if cell
                    .flags
                    .intersects(CellFlags::WIDE_CHAR_SPACER | CellFlags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                if !cell.c.is_control()
                    && !is_bidi_control(cell.c)
                    && !matches!(cell.c, '\u{2028}' | '\u{2029}')
                {
                    text.push(cell.c);
                }
                for &z in drawn_marks(cell) {
                    if !z.is_control()
                        && !is_bidi_control(z)
                        && !matches!(z, '\u{2028}' | '\u{2029}')
                    {
                        text.push(z);
                    }
                }
            }
            out.push_str(text.trim_end_matches(' '));
        }
        out
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
///
/// Rows with equal keys are drawn from the cache, so a collision shows one row's content in
/// place of another's. The seed is random per process: with a fixed one, output could be crafted
/// offline so that a line on screen looks like a different line seen earlier.
struct KeyHasher(u64);

impl KeyHasher {
    const MUL: u64 = 0x9E37_79B9_7F4A_7C15;

    fn seed() -> u64 {
        use std::hash::BuildHasher;
        static SEED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        *SEED.get_or_init(|| {
            std::collections::hash_map::RandomState::new().hash_one(0x7477_6974_7479_u64)
        })
    }

    fn new() -> Self {
        Self(Self::seed())
    }

    #[inline]
    fn write(&mut self, word: u64) {
        let product = u128::from(self.0 ^ word) * u128::from(Self::MUL);
        self.0 = (product as u64) ^ ((product >> 64) as u64);
    }

    fn finish(mut self) -> u64 {
        self.write(Self::seed());
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
        let marks = drawn_marks(cell);
        if !marks.is_empty() {
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
                zerowidth: drawn_marks(cell).to_vec(),
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
        if from <= to && to < out.len() {
            for cell in &mut out[from..=to] {
                cell.search_match = cell.search_match.max(level);
            }
        }
    }
    LineData {
        cells: out.into(),
        has_rtl,
        hash,
    }
}
