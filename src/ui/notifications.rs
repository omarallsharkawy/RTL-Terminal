//! Notification history: the last notifications of a window (newest kept, oldest dropped),
//! listed by the palette's notifications mode (`notifications.list`, Ctrl+Shift+I).
//! Window-free; the palette rows are built here, drawing is `render::scene::palette`.

use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

use crate::layout::PaneId;
use crate::render::scene::palette::PaletteItem;
use crate::shell_integration::{clean_text, MAX_NOTIFY_CHARS};
use crate::ui::palette::Searchable;

/// Most notifications kept per window.
pub const MAX_HISTORY: usize = 50;

/// Longest tab title kept with a notification, in characters.
const MAX_TAB_CHARS: usize = 64;

/// Longest tab title shown in a palette row, in characters.
const SHOWN_TAB_CHARS: usize = 24;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    pub title: Option<String>,
    pub body: String,
    /// The pane that sent it; it may have been closed since.
    pub pane: PaneId,
    /// Title of the pane's tab when the notification arrived.
    pub tab_title: String,
    pub at: SystemTime,
    /// The user has looked at the pane since (or was looking when it arrived).
    pub seen: bool,
}

impl HistoryEntry {
    /// `title: body`, or whichever of them there is.
    pub fn text(&self) -> String {
        match &self.title {
            Some(title) if !self.body.is_empty() => format!("{title}: {}", self.body),
            Some(title) => title.clone(),
            None => self.body.clone(),
        }
    }
}

impl Searchable for HistoryEntry {
    fn search_fields(&self) -> Vec<&str> {
        let mut fields = vec![self.body.as_str(), self.tab_title.as_str()];
        if let Some(title) = &self.title {
            fields.insert(0, title);
        }
        fields
    }
}

/// The last [`MAX_HISTORY`] notifications, oldest first in storage.
#[derive(Debug, Default)]
pub struct NotificationHistory {
    entries: VecDeque<HistoryEntry>,
}

impl NotificationHistory {
    /// Record a notification. Its text is cleaned and bounded again here, so the history never
    /// holds more than `MAX_HISTORY` bounded entries. Empty notifications are not recorded.
    pub fn push(
        &mut self,
        pane: PaneId,
        title: Option<&str>,
        body: &str,
        tab_title: &str,
        at: SystemTime,
        seen: bool,
    ) -> bool {
        let title = title
            .map(|t| clean_text(t, MAX_NOTIFY_CHARS))
            .filter(|t| !t.is_empty());
        let body = clean_text(body, MAX_NOTIFY_CHARS);
        if title.is_none() && body.is_empty() {
            return false;
        }
        if self.entries.len() >= MAX_HISTORY {
            self.entries.pop_front();
        }
        self.entries.push_back(HistoryEntry {
            title,
            body,
            pane,
            tab_title: clean_text(tab_title, MAX_TAB_CHARS),
            at,
            seen,
        });
        true
    }

    /// Every entry, newest first.
    pub fn newest_first(&self) -> Vec<&HistoryEntry> {
        self.entries.iter().rev().collect()
    }

    /// The notifications of pane `id` have been seen.
    pub fn mark_seen(&mut self, id: PaneId) {
        for entry in self.entries.iter_mut().filter(|e| e.pane == id) {
            entry.seen = true;
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn unseen(&self) -> usize {
        self.entries.iter().filter(|e| !e.seen).count()
    }
}

/// The pane to jump to for `entry`: none once that pane is closed.
pub fn jump_target(entry: &HistoryEntry, is_open: impl Fn(PaneId) -> bool) -> Option<PaneId> {
    is_open(entry.pane).then_some(entry.pane)
}

/// How long ago `at` was, shortly: `now`, `42s`, `5m`, `3h`, `2d`.
pub fn age_label(at: SystemTime, now: SystemTime) -> String {
    let secs = now.duration_since(at).unwrap_or(Duration::ZERO).as_secs();
    match secs {
        0..=4 => "now".into(),
        5..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

/// A notification as a palette row. The notification text goes on the side its direction
/// reads from (Arabic on the right, like the commands' Arabic titles) and the tab and age on
/// the other. Unseen ones start with `●`, and those whose pane is closed are drawn dimmed.
pub fn palette_item(entry: &HistoryEntry, open: bool, now: SystemTime) -> PaletteItem {
    let text = if entry.seen {
        entry.text()
    } else {
        format!("● {}", entry.text())
    };
    let tab: String = if entry.tab_title.chars().count() > SHOWN_TAB_CHARS {
        let cut: String = entry.tab_title.chars().take(SHOWN_TAB_CHARS - 1).collect();
        format!("{cut}…")
    } else {
        entry.tab_title.clone()
    };
    let age = age_label(entry.at, now);
    let meta = if tab.is_empty() {
        age
    } else {
        format!("{tab} · {age}")
    };
    let (title_en, title_ar) = if is_rtl(&entry.text()) {
        (meta, text)
    } else {
        (text, meta)
    };
    PaletteItem {
        title_en,
        title_ar,
        shortcut: None,
        dimmed: !open,
    }
}

/// The first strong character of `text` is right to left.
pub fn is_rtl(text: &str) -> bool {
    use unicode_bidi::{bidi_class, BidiClass};
    text.chars()
        .find_map(|c| match bidi_class(c) {
            BidiClass::L => Some(false),
            BidiClass::R | BidiClass::AL => Some(true),
            _ => None,
        })
        .unwrap_or(false)
}
