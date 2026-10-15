//! A terminal row after shaping: its segments, quads and where each cell is drawn, plus the
//! hit testing and caret placement that follow reordered right-to-left text.

use alacritty_terminal::index::Side;

use super::text::bidi::is_rtl_char;
use super::text::segment::CachedSegment;
use super::Quad;
use crate::terminal::{CellData, LineData};

pub mod cache;

#[derive(Clone, Default)]
pub struct CachedRow {
    pub hash: u64,
    /// Visual extents of cells in RTL segments (see `visual_cell_extents`), for hit testing.
    pub visual: Vec<Option<(f32, f32)>>,
    /// Cells whose character is right-to-left (their left and right halves are swapped).
    pub rtl_cells: Vec<bool>,
    /// Cells holding a plain space (see `CellData::is_blank`).
    pub blank_cells: Vec<bool>,
    pub segments: Vec<CachedSegment>,
    /// Backgrounds, box drawing and text decorations, relative to the row origin.
    pub quads: Vec<Quad>,
}

/// Cursor details that change how a row is drawn (the glyph under a block cursor is recolored).
#[derive(Clone, Copy, PartialEq, Hash)]
pub struct RowCursor {
    pub col: usize,
    pub block: bool,
}

impl CachedRow {
    /// A row built from its shaped `segments` and `quads`, with the per-cell data hit testing
    /// needs.
    pub fn new(
        hash: u64,
        line: &LineData,
        segments: Vec<CachedSegment>,
        quads: Vec<Quad>,
        char_width: f32,
    ) -> Self {
        CachedRow {
            hash,
            visual: visual_cell_extents(line, &segments, char_width),
            rtl_cells: line.cells.iter().map(|c| is_rtl_char(c.c)).collect(),
            blank_cells: line.cells.iter().map(CellData::is_blank).collect(),
            segments,
            quads,
        }
    }

    /// The cell drawn under `x` (relative to the row origin) and which side of it, so mouse
    /// selection and mouse reports follow what is on screen even where Arabic is reordered.
    pub fn hit_test(&self, x: f32, char_width: f32, cols: usize) -> (usize, Side) {
        let is_rtl = |c: usize| self.rtl_cells.get(c).copied().unwrap_or(false);
        let sized = || {
            self.visual
                .iter()
                .enumerate()
                .filter_map(|(c, e)| e.filter(|(l, r)| r > l).map(|e| (c, e)))
        };
        let in_rtl_segment = |x: f32| {
            self.segments.iter().any(|s| {
                s.has_rtl
                    && x >= s.seg_start as f32 * char_width
                    && x < s.seg_end as f32 * char_width
            })
        };
        let hit = sized().find(|(_, (l, r))| x >= *l && x < *r).or_else(|| {
            // Inside an RTL segment but between glyphs: take the nearest drawn cell.
            in_rtl_segment(x).then(|| {
                sized().min_by(|(_, a), (_, b)| {
                    let d = |(l, r): (f32, f32)| (l - x).abs().min((r - x).abs());
                    d(*a).total_cmp(&d(*b))
                })
            })?
        });
        if let Some((col, (l, r))) = hit {
            let left_half = x < (l + r) / 2.0;
            let side = if left_half != is_rtl(col) {
                Side::Left
            } else {
                Side::Right
            };
            return (col, side);
        }
        let rel = (x / char_width).max(0.0);
        let col = (rel as usize).min(cols.saturating_sub(1));
        let side = if rel.fract() < 0.5 {
            Side::Left
        } else {
            Side::Right
        };
        (col, side)
    }

    /// Where the cursor goes (relative to the left padding), and whether it should extend to the
    /// left of that point. After Arabic text the caret sits on the text's left, where the next
    /// Arabic letter will appear; after Latin text it sits on the right.
    pub fn cursor_anchor(&self, col: usize, char_width: f32) -> (f32, bool) {
        let row = self;
        let grid_x = col as f32 * char_width;
        let glyph_of = |seg: &CachedSegment, c: usize| {
            let target = seg.cell_bytes[c - seg.seg_start];
            seg.buffer.layout_runs().find_map(|run| {
                run.glyphs
                    .iter()
                    .find(|g| target >= g.start && target < g.end)
                    .map(|g| (seg.seg_x + g.x, seg.seg_x + g.x + g.w, g.level.is_rtl()))
            })
        };
        for (i, seg) in row.segments.iter().enumerate() {
            if !seg.has_rtl {
                continue;
            }
            if col >= seg.seg_start && col < seg.seg_end {
                // Inside the text: the insertion point is the cell's leading edge.
                return match glyph_of(seg, col) {
                    Some((_, right, true)) => (right, true),
                    Some((left, _, false)) => (left, false),
                    None => (grid_x, false),
                };
            }
            let next_start = row.segments.get(i + 1).map_or(usize::MAX, |s| s.seg_start);
            // Only spaces may separate the caret from the text it follows; past a border or
            // other content it stays on its own cell.
            let only_spaces = || (seg.seg_end..col).all(|c| row.blank_cells.get(c) == Some(&true));
            if col >= seg.seg_end && col < next_start && only_spaces() {
                let gap = (col - seg.seg_end) as f32 * char_width;
                return match glyph_of(seg, seg.seg_end - 1) {
                    Some((left, _, true)) => (left - gap, true),
                    Some((_, right, false)) => (right + gap, false),
                    None => (grid_x, false),
                };
            }
        }
        (grid_x, false)
    }
}

/// Where each cell of a row is drawn, relative to the row origin: `Some((left, right))` for
/// cells inside RTL segments (they follow their glyphs), `None` for cells on the grid.
pub fn visual_cell_extents(
    line: &LineData,
    segments: &[CachedSegment],
    cw: f32,
) -> Vec<Option<(f32, f32)>> {
    let mut visual: Vec<Option<(f32, f32)>> = vec![None; line.cells.len()];
    for seg in segments.iter().filter(|s| s.has_rtl) {
        let mut extents = seg.cell_extents(&line.cells);
        // Arabic glyphs are usually narrower than the cells they occupy: stretch the visually
        // outermost cells to the segment's grid edges, so colored backgrounds (selections, TUI
        // highlights) don't leave unpainted holes at the end of the text.
        let (grid_l, grid_r) = (seg.seg_start as f32 * cw, seg.seg_end as f32 * cw);
        let sized = |e: &Option<(f32, f32)>| e.filter(|(l, r)| r > l);
        if let Some(i) = (0..extents.len())
            .filter(|&i| sized(&extents[i]).is_some())
            .max_by(|&a, &b| extents[a].unwrap().1.total_cmp(&extents[b].unwrap().1))
        {
            if let Some((_, r)) = extents[i].as_mut() {
                *r = r.max(grid_r);
            }
        }
        if let Some(i) = (0..extents.len())
            .filter(|&i| sized(&extents[i]).is_some())
            .min_by(|&a, &b| extents[a].unwrap().0.total_cmp(&extents[b].unwrap().0))
        {
            if let Some((l, _)) = extents[i].as_mut() {
                *l = l.min(grid_l);
            }
        }
        for (i, ext) in extents.into_iter().enumerate() {
            let c = seg.seg_start + i;
            visual[c] = if line.cells[c].is_spacer() {
                let x = c as f32 * cw;
                Some((x, x))
            } else {
                ext
            };
        }
    }
    visual
}

/// Hash of everything that changes how a row is shaped and drawn; the row cache key.
pub(crate) fn hash_row(cells: &[CellData], cursor: Option<RowCursor>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for cell in cells {
        cell.c.hash(&mut hasher);
        cell.zerowidth.hash(&mut hasher);
        hash_color(&cell.fg, &mut hasher);
        hash_color(&cell.bg, &mut hasher);
        cell.flags.bits().hash(&mut hasher);
        cell.selected.hash(&mut hasher);
        cell.search_match.hash(&mut hasher);
        cell.link_hover.hash(&mut hasher);
    }
    cursor.hash(&mut hasher);
    hasher.finish()
}

fn hash_color<H: std::hash::Hasher>(color: &alacritty_terminal::vte::ansi::Color, hasher: &mut H) {
    use alacritty_terminal::vte::ansi::Color;
    use std::hash::Hash;
    match color {
        Color::Named(n) => (0u8, *n as u8).hash(hasher),
        Color::Spec(rgb) => (1u8, rgb.r, rgb.g, rgb.b).hash(hasher),
        Color::Indexed(idx) => (2u8, *idx).hash(hasher),
    }
}
