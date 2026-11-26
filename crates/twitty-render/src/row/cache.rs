//! Shaped rows of every pane, so a row that did not change is not reshaped.

use std::collections::HashMap;

use alacritty_terminal::index::Side;

use super::CachedRow;
use crate::metrics::CellMetrics;
use twitty_core::layout::{PaneId, Rect};

/// Rows are keyed by pane and row index; each row remembers the hash it was built from.
#[derive(Default)]
pub struct RowCache {
    panes: HashMap<PaneId, Vec<CachedRow>>,
}

impl RowCache {
    /// Drop every row (font, size or surface changed).
    pub fn clear(&mut self) {
        self.panes.clear();
    }

    /// Forget the rows of a closed pane.
    #[doc(hidden)]
    pub fn forget(&mut self, pane: PaneId) {
        self.panes.remove(&pane);
    }

    /// The cached row `row` of `pane`, as last drawn.
    pub fn row(&self, pane: PaneId, row: usize) -> Option<&CachedRow> {
        self.panes.get(&pane).and_then(|rows| rows.get(row))
    }

    /// The logical cell drawn at a physical pixel. A drag outside the grid keeps the outer
    /// half of its edge cell, and a row from before a width change cannot supply the hit map.
    pub fn cell_at(
        &self,
        pane: PaneId,
        metrics: CellMetrics,
        rect: Rect,
        x: f64,
        y: f64,
    ) -> (usize, usize, Side) {
        let (cols, _) = metrics.grid_size(rect);
        let origin = f64::from(rect.x + metrics.padding_left);
        let span = cols as f64 * f64::from(metrics.char_width);
        let x = x.clamp(origin, origin + span - 0.01);
        let (col, row) = metrics.pixel_to_cell(rect, x, y);
        match self
            .row(pane, row)
            .filter(|cached| cached.visual.len() == cols)
        {
            Some(cached) => {
                let (col, side) = cached.hit_test((x - origin) as f32, metrics.char_width, cols);
                (col, row, side)
            }
            None => (col, row, metrics.pixel_side(rect, x)),
        }
    }

    /// Take a pane's rows out of the cache, to be looked up by hash while the next frame is
    /// built. Put the new rows back with `put` once drawn.
    pub fn take(&mut self, pane: PaneId) -> PreviousRows {
        let rows = self.panes.remove(&pane).unwrap_or_default();
        let by_hash = rows.iter().enumerate().map(|(i, r)| (r.hash, i)).collect();
        PreviousRows { rows, by_hash }
    }

    #[doc(hidden)]
    pub fn put(&mut self, pane: PaneId, rows: Vec<CachedRow>) {
        self.panes.insert(pane, rows);
    }
}

/// A pane's rows from its previous frame.
pub struct PreviousRows {
    rows: Vec<CachedRow>,
    by_hash: HashMap<u64, usize>,
}

impl PreviousRows {
    /// The row drawn from `hash`, preferably the one that was at `index`. Rows are found
    /// wherever they were, so output that scrolls the screen reuses every row it moved. Each
    /// row is handed out once.
    pub fn take(&mut self, index: usize, hash: u64) -> Option<CachedRow> {
        let at = if self.rows.get(index).is_some_and(|r| r.hash == hash) {
            index
        } else {
            *self.by_hash.get(&hash)?
        };
        let row = self.rows.get_mut(at).filter(|r| r.hash == hash)?;
        Some(std::mem::take(row))
    }
}
