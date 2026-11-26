//! Round-robin scheduling for PTY drains on the UI thread.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::layout::PaneId;

/// Maximum amount parsed before giving the next ready pane a turn.
pub const CHUNK_BYTES: usize = 64 * 1024;
const TIME_BUDGET: Duration = Duration::from_millis(10);

#[derive(Default)]
pub struct OutputSchedule {
    ready: VecDeque<PaneId>,
    exits: HashMap<PaneId, Option<u32>>,
    deadline: Option<Instant>,
}

impl OutputSchedule {
    pub fn queue(&mut self, id: PaneId) {
        if !self.ready.contains(&id) {
            self.ready.push_back(id);
        }
    }

    /// Hold an exit until its pane's queued bytes have been parsed.
    pub fn defer_exit(&mut self, id: PaneId, status: Option<u32>) -> bool {
        if !self.ready.contains(&id) {
            return false;
        }
        self.exits.insert(id, status);
        true
    }

    /// Take one ready pane while this event-loop turn still has time to parse output.
    pub fn next_pane(&mut self, now: impl FnOnce() -> Instant) -> Option<PaneId> {
        self.ready.front()?;
        let now = now();
        let deadline = self.deadline.get_or_insert(now + TIME_BUDGET);
        if now >= *deadline {
            return None;
        }
        self.ready.pop_front()
    }

    /// Rotate a busy pane to the back, or return its deferred exit once it is empty.
    pub fn chunk_drained(&mut self, id: PaneId, more: bool) -> Option<Option<u32>> {
        if more {
            self.ready.push_back(id);
            None
        } else {
            self.exits.remove(&id)
        }
    }

    /// Yield to other events, then start a new budget immediately if output remains.
    pub fn end_turn(&mut self, now: Instant) -> Option<Instant> {
        self.deadline = None;
        (!self.ready.is_empty()).then_some(now)
    }
}
