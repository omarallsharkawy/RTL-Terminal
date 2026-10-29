//! A shaped run of cells and where its glyphs put each cell.

use cosmic_text::Buffer;

use crate::terminal::CellData;

#[derive(Clone)]
pub struct CachedSegment {
    pub seg_start: usize,
    pub seg_end: usize,
    /// X offset relative to the left padding.
    pub seg_x: f32,
    pub buffer: Buffer,
    pub has_rtl: bool,
    /// Byte offset (into the shaped text) where each cell of the segment starts.
    pub cell_bytes: Vec<usize>,
}

impl CachedSegment {
    /// Visual `(left, right)` x of each cell (relative to the left padding), taken from the glyphs
    /// that hold it so backgrounds follow reordered RTL text. A glyph shared by several cells
    /// (ligature, cluster) is split between them; cells without a glyph are `None`, spacers are
    /// empty.
    pub(crate) fn cell_extents(&self, cells: &[CellData]) -> Vec<Option<(f32, f32)>> {
        let mut out: Vec<Option<(f32, f32)>> = vec![None; self.seg_end - self.seg_start];
        let mut real = Vec::new();
        let mut real_bytes = Vec::new();
        for (i, cell) in cells[self.seg_start..self.seg_end].iter().enumerate() {
            if cell.is_spacer() {
                continue;
            }
            real.push(i);
            real_bytes.push(self.cell_bytes[i]);
        }
        if real.is_empty() {
            return out;
        }
        // Zero-width glyphs (marks, isolates) are reordered far from their cell, so they only
        // place cells that have no sized glyph, as an empty extent.
        for sized in [true, false] {
            for run in self.buffer.layout_runs() {
                for g in run.glyphs.iter().filter(|g| (g.w > 0.0) == sized) {
                    let i0 = real_bytes
                        .partition_point(|&b| b <= g.start)
                        .saturating_sub(1);
                    let i1 = real_bytes
                        .partition_point(|&b| b < g.end)
                        .saturating_sub(1)
                        .max(i0);
                    let n = (i1 - i0 + 1) as f32;
                    let part = g.w / n;
                    for k in 0..=(i1 - i0) {
                        let left = if g.level.is_rtl() {
                            g.x + g.w - (k + 1) as f32 * part
                        } else {
                            g.x + k as f32 * part
                        };
                        let (l, r) = (self.seg_x + left, self.seg_x + left + part);
                        let slot = &mut out[real[i0 + k]];
                        match slot {
                            None => *slot = Some((l, r)),
                            Some((a, b)) if sized => *slot = Some((a.min(l), b.max(r))),
                            Some(_) => {}
                        }
                    }
                }
            }
        }
        out
    }
}
