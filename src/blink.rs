//! Cursor blink phase. Pure timing logic so it can be tested without a window.
use std::time::{Duration, Instant};

/// Half period: the cursor is on for this long, then off for this long.
pub const BLINK_INTERVAL: Duration = Duration::from_millis(530);

#[derive(Debug)]
pub struct BlinkState {
    interval: Duration,
    /// Current phase: true while the cursor is drawn.
    on: bool,
    /// The app and config want a blinking cursor, and the cursor is shown.
    enabled: bool,
    focused: bool,
    /// When the phase flips next. `None` whenever the cursor is not blinking.
    next_toggle: Option<Instant>,
}

impl BlinkState {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            on: true,
            enabled: false,
            focused: true,
            next_toggle: None,
        }
    }

    /// Blinking only happens while enabled and focused. Otherwise the cursor stays solid
    /// (or hollow, when unfocused, which the renderer handles).
    fn active(&self) -> bool {
        self.enabled && self.focused
    }

    /// Restart the cycle: visible now, next flip one interval later.
    pub fn reset(&mut self, now: Instant) {
        self.on = true;
        self.next_toggle = self.active().then(|| now + self.interval);
    }

    /// Flip the phase if its deadline has passed. Returns true when the phase changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        match self.next_toggle {
            Some(deadline) if self.active() && now >= deadline => {
                self.on = !self.on;
                self.next_toggle = Some(now + self.interval);
                true
            }
            _ => false,
        }
    }

    /// The next moment `tick` needs to run, if any.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.next_toggle.filter(|_| self.active())
    }

    /// Whether the cursor should be drawn right now.
    pub fn visible(&self) -> bool {
        !self.active() || self.on
    }

    pub fn set_enabled(&mut self, now: Instant, enabled: bool) {
        if self.enabled != enabled {
            self.enabled = enabled;
            self.reset(now);
        }
    }

    pub fn set_focused(&mut self, now: Instant, focused: bool) {
        if self.focused != focused {
            self.focused = focused;
            self.reset(now);
        }
    }
}
