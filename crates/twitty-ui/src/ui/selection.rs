//! Mouse selection lifecycle: dragging, then a brief highlight of the copied text that is
//! cleared later unless a newer selection has started.

use std::time::{Duration, Instant};

/// How long a copied selection stays highlighted before it is cleared.
pub const COPIED_HIGHLIGHT: Duration = Duration::from_millis(350);

#[derive(Debug, Default)]
pub struct SelectionState {
    /// A mouse selection is being dragged.
    pub selecting: bool,
    /// Bumped on every new selection, so a delayed clear never removes a newer one.
    generation: u64,
    /// Deadline at which the copied highlight should clear, tagged with its selection generation.
    clear_at: Option<(Instant, u64)>,
}

impl SelectionState {
    /// A new selection starts.
    pub fn begin(&mut self) {
        self.generation += 1;
        self.selecting = true;
        self.clear_at = None;
    }

    /// The drag ended (or was abandoned).
    pub fn end(&mut self) {
        self.selecting = false;
    }

    /// Schedule the copied highlight of the current selection to clear after [`COPIED_HIGHLIGHT`].
    pub fn schedule_clear(&mut self, now: Instant) {
        self.clear_at = Some((now + COPIED_HIGHLIGHT, self.generation));
    }

    /// Cancel any pending delayed clear.
    pub fn cancel_clear(&mut self) {
        self.clear_at = None;
    }

    /// Identifies the current selection for a delayed clear.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// A delayed clear for `generation` may run: no newer selection started and none is
    /// being dragged.
    pub fn may_clear(&self, generation: u64) -> bool {
        self.generation == generation && !self.selecting
    }

    /// Returns true once the scheduled copied highlight is due at `now` (and still matches the
    /// current selection), clearing the pending deadline.
    pub fn take_due_clear(&mut self, now: Instant) -> bool {
        let Some((at, generation)) = self.clear_at else {
            return false;
        };
        if !self.may_clear(generation) {
            self.clear_at = None;
            return false;
        }
        if now >= at {
            self.clear_at = None;
            true
        } else {
            false
        }
    }

    /// The pending clear deadline for `ControlFlow::WaitUntil`, if still active.
    pub fn next_clear_deadline(&self) -> Option<Instant> {
        let (at, generation) = self.clear_at?;
        self.may_clear(generation).then_some(at)
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
