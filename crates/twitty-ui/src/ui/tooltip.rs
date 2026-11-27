//! Hover tooltips: after the pointer rests on an icon button for a moment, a small label names
//! it and shows its shortcut. The app says what the pointer rests on (`app::tooltip`) and
//! formats the shortcut; timing and wording live here.

use std::time::{Duration, Instant};

use crate::blocks::cards::BlockButton;
use crate::ui::chrome::{t, ChromeAction};

/// How long the pointer rests on a button before its tooltip shows.
pub const DELAY: Duration = Duration::from_millis(450);

/// What the pointer rests on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TipTarget {
    Chrome(ChromeAction),
    Block(BlockButton),
}

impl TipTarget {
    /// The tooltip's words and the command whose shortcut follows them. None for targets that
    /// need no tooltip (the search field, a pane header).
    pub fn label(self) -> Option<(&'static str, Option<&'static str>)> {
        Some(match self {
            TipTarget::Chrome(action) => match action {
                ChromeAction::Sidebar => (t("Show or hide the sidebar"), Some("sidebar.toggle")),
                ChromeAction::Diff => (t("Review changes"), Some("review.toggle")),
                ChromeAction::Notifications => (t("Notifications"), Some("notifications.list")),
                ChromeAction::NewTab => (t("New tab"), Some("tab.new")),
                ChromeAction::CloseTab(_) => (t("Close tab"), None),
                ChromeAction::SplitRight(_) => (t("Split right"), Some("split.right")),
                ChromeAction::SplitDown(_) => (t("Split down"), Some("split.down")),
                ChromeAction::Close(_) => (t("Close pane"), Some("pane.close")),
                ChromeAction::Search | ChromeAction::FocusPane(_) => return None,
            },
            TipTarget::Block(button) => match button {
                BlockButton::ExplainError => (t("Explain error"), Some("ai.explain-error")),
                BlockButton::CopyCommand => (t("Copy command"), None),
                BlockButton::CopyOutput => (t("Copy output"), Some("block.copy-output")),
                BlockButton::AskAgent => (t("Ask an agent about this"), Some("agent.ask")),
                BlockButton::Rerun => (t("Run again"), None),
            },
        })
    }
}

/// The label with its shortcut, as the tooltip shows it.
pub fn tip_text(label: &str, keys: Option<&str>) -> String {
    match keys {
        Some(keys) => format!("{label}  {keys}"),
        None => label.to_owned(),
    }
}

/// When the pointer came to rest on what it rests on now.
#[derive(Clone, Copy, Debug, Default)]
pub struct TooltipState {
    target: Option<(TipTarget, Instant)>,
}

impl TooltipState {
    /// The pointer rests on `target` at `now`; returns the target once its tooltip is due.
    /// Moving to another target starts the wait again.
    pub fn update(&mut self, target: Option<TipTarget>, now: Instant) -> Option<TipTarget> {
        if self.target.map(|(t, _)| t) != target {
            self.target = target.map(|t| (t, now));
        }
        self.target
            .filter(|&(_, since)| now.duration_since(since) >= DELAY)
            .map(|(t, _)| t)
    }

    /// When the tooltip for `target` becomes due, while it is still on its way; None when it is
    /// already due or the pointer moved on.
    pub fn pending(&self, target: Option<TipTarget>, now: Instant) -> Option<Instant> {
        let (current, since) = self.target?;
        let due = since + DELAY;
        (Some(current) == target && now < due).then_some(due)
    }

    /// Whether the pointer still rests on the target being timed.
    pub fn tracking(&self, target: Option<TipTarget>) -> bool {
        self.target.is_some_and(|(t, _)| Some(t) == target)
    }
}
