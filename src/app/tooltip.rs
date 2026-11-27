//! Hover tooltips: after the pointer rests on an icon button for a moment, a small label names
//! it and shows its shortcut.

use std::time::{Duration, Instant};

use super::App;
use crate::blocks::cards::BlockButton;
use crate::ui::chrome::{tr, ChromeAction};

/// How long the pointer rests on a button before its tooltip shows.
const DELAY: Duration = Duration::from_millis(450);

/// What the pointer rests on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TipTarget {
    Chrome(ChromeAction),
    Block(BlockButton),
}

#[derive(Default)]
pub(super) struct TooltipState {
    target: Option<(TipTarget, Instant)>,
}

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

    /// Text for `target`: its name and, when it has one, its shortcut.
    fn tip_text(&self, target: TipTarget) -> Option<String> {
        let (label, command) = match target {
            TipTarget::Chrome(action) => match action {
                ChromeAction::Sidebar => (
                    tr("Show or hide the sidebar", "إظهار أو إخفاء الشريط الجانبي"),
                    Some("sidebar.toggle"),
                ),
                ChromeAction::Diff => (
                    tr("Review changes", "راجع التغييرات"),
                    Some("review.toggle"),
                ),
                ChromeAction::Notifications => {
                    (tr("Notifications", "الإشعارات"), Some("notifications.list"))
                }
                ChromeAction::NewTab => (tr("New tab", "تاب جديد"), Some("tab.new")),
                ChromeAction::CloseTab(_) => (tr("Close tab", "اقفل التاب"), None),
                ChromeAction::SplitRight(_) => (tr("Split right", "قسّم يمين"), Some("split.right")),
                ChromeAction::SplitDown(_) => (tr("Split down", "قسّم تحت"), Some("split.down")),
                ChromeAction::Close(_) => (tr("Close pane", "قفل اللوحة"), Some("pane.close")),
                ChromeAction::Search | ChromeAction::FocusPane(_) => return None,
            },
            TipTarget::Block(button) => match button {
                BlockButton::ExplainError => {
                    (tr("Explain error", "اشرح الخطأ"), Some("ai.explain-error"))
                }
                BlockButton::CopyCommand => (tr("Copy command", "انسخ الأمر"), None),
                BlockButton::CopyOutput => {
                    (tr("Copy output", "انسخ الناتج"), Some("block.copy-output"))
                }
                BlockButton::AskAgent => (
                    tr("Ask an agent about this", "اسأل وكيل عن ده"),
                    Some("agent.ask"),
                ),
                BlockButton::Rerun => (tr("Run again", "شغّله تاني"), None),
            },
        };
        let keys = command.and_then(|c| self.keybindings.display_shortcut(c));
        Some(match keys {
            Some(keys) => format!("{label}  {keys}"),
            None => label.to_owned(),
        })
    }

    /// Track what the pointer rests on; the tooltip to draw once it is due.
    pub(super) fn sync_tooltip(&mut self, now: Instant) -> Option<(String, (f32, f32))> {
        let target = self.tip_target();
        if self.tooltip.target.map(|(t, _)| t) != target {
            self.tooltip.target = target.map(|t| (t, now));
        }
        let shown = self
            .tooltip
            .target
            .filter(|&(_, since)| now.duration_since(since) >= DELAY)
            .and_then(|(t, _)| self.tip_text(t));
        let (x, y) = self.pointer.pos;
        shown.map(|text| (text, (x as f32, y as f32)))
    }

    /// When a resting pointer's tooltip becomes due; redraws once it is.
    pub(super) fn poll_tooltip(&mut self, now: Instant) -> Option<Instant> {
        let (target, since) = self.tooltip.target?;
        if self.tip_target() != Some(target) {
            return None;
        }
        let due = since + DELAY;
        if now >= due {
            if !self.window.tooltip_shown() {
                self.window.request_redraw();
            }
            None
        } else {
            Some(due)
        }
    }
}
