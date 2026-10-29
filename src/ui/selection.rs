//! Mouse selection lifecycle: dragging, then a brief highlight of the copied text that is
//! cleared later unless a newer selection has started.

use std::time::Duration;

/// How long a copied selection stays highlighted before it is cleared.
pub(crate) const COPIED_HIGHLIGHT: Duration = Duration::from_millis(350);

#[derive(Debug, Default)]
pub(crate) struct SelectionState {
    /// A mouse selection is being dragged.
    pub selecting: bool,
    /// Bumped on every new selection, so a delayed clear never removes a newer one.
    generation: u64,
}

impl SelectionState {
    /// A new selection starts.
    pub(crate) fn begin(&mut self) {
        self.generation += 1;
        self.selecting = true;
    }

    /// The drag ended (or was abandoned).
    pub fn end(&mut self) {
        self.selecting = false;
    }

    /// Identifies the current selection for a delayed clear.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// A delayed clear for `generation` may run: no newer selection started and none is
    /// being dragged.
    pub(crate) fn may_clear(&self, generation: u64) -> bool {
        self.generation == generation && !self.selecting
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delayed_clear_only_for_the_finished_selection() {
        let mut s = SelectionState::default();
        s.begin();
        let first = s.generation();
        assert!(!s.may_clear(first), "still dragging");
        s.end();
        assert!(s.may_clear(first));
        s.begin();
        s.end();
        assert!(!s.may_clear(first), "a newer selection started");
        assert!(s.may_clear(s.generation()));
    }
}
