//! Shaped rows of every pane, so a row that did not change is not reshaped.

use std::collections::HashMap;

use super::CachedRow;
use crate::layout::PaneId;

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
    pub fn forget(&mut self, pane: PaneId) {
        self.panes.remove(&pane);
    }

    /// The cached row `row` of `pane`, as last drawn.
    pub fn row(&self, pane: PaneId, row: usize) -> Option<&CachedRow> {
        self.panes.get(&pane).and_then(|rows| rows.get(row))
    }

    /// Take a pane's rows out of the cache, sized to `rows` (new rows are empty and will be
    /// rebuilt). Put them back with `put` once drawn.
    pub fn take(&mut self, pane: PaneId, rows: usize) -> Vec<CachedRow> {
        let mut cached = self.panes.remove(&pane).unwrap_or_default();
        if cached.len() != rows {
            cached.resize(rows, CachedRow::default());
        }
        cached
    }

    pub fn put(&mut self, pane: PaneId, rows: Vec<CachedRow>) {
        self.panes.insert(pane, rows);
    }
}
