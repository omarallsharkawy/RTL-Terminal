//! Hover tooltips: what the pointer rests on, from the window's hover state. Timing and wording
//! live in `ui::tooltip`.

use std::time::Instant;

use super::App;
use crate::ui::tooltip::{tip_text, TipTarget};

impl App {
    fn tip_target(&self) -> Option<TipTarget> {
        if self.context_menu_open() || self.pointer.capture.is_some() || self.pointer.drag.is_some()
        {
            return None;
        }
        if let Some(action) = self.window.chrome_hover() {
            return Some(TipTarget::Chrome(action));
        }
        self.window
            .block_hover()
            .and_then(|h| h.button)
            .map(TipTarget::Block)
    }

    /// Track what the pointer rests on; the tooltip to draw once it is due.
    pub(super) fn sync_tooltip(&mut self, now: Instant) -> Option<(String, (f32, f32))> {
        let target = self.tip_target();
        let (label, command) = self.tooltip.update(target, now)?.label()?;
        let keys = command.and_then(|c| self.keybindings.display_shortcut(c));
        let (x, y) = self.pointer.pos;
        Some((tip_text(label, keys.as_deref()), (x as f32, y as f32)))
    }

    /// When a resting pointer's tooltip becomes due; redraws once it is.
    pub(super) fn poll_tooltip(&mut self, now: Instant) -> Option<Instant> {
        let target = self.tip_target();
        if !self.tooltip.tracking(target) {
            return None;
        }
        let pending = self.tooltip.pending(target, now);
        if pending.is_none() && !self.window.tooltip_shown() {
            self.window.request_redraw();
        }
        pending
    }
}
