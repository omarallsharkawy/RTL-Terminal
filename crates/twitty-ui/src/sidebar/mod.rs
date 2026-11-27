//! Window-free workspace labels, scrolling and repository metadata.

pub mod cards;
pub mod git;

use std::path::{Path, PathBuf};

use crate::layout::{PaneId, SidebarPosition};
use crate::shell_integration::{clean_text, MAX_NOTIFY_CHARS};
use crate::ui::chrome::UiLanguage;
use crate::ui::notifications::HistoryEntry;
use twitty_core::remote::ConnectionState;

/// A desktop notification a pane asked for (OSC 9/99/777), waiting to be seen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    pub title: Option<String>,
    pub body: String,
    pub at: std::time::SystemTime,
}

pub fn entry_title(title: Option<&str>, program: Option<&str>, index: usize) -> String {
    localized_entry_title(title, program, index, UiLanguage::English)
}

pub fn localized_entry_title(
    title: Option<&str>,
    program: Option<&str>,
    index: usize,
    language: UiLanguage,
) -> String {
    title
        .into_iter()
        .chain(program)
        .map(|text| clean_text(text, 256))
        .find(|text| !text.is_empty())
        .unwrap_or_else(|| format!("{} {}", language.t("Tab"), index + 1))
}

pub struct Placement {
    pub position: SidebarPosition,
    restore: SidebarPosition,
}

impl Placement {
    pub fn new(position: SidebarPosition) -> Self {
        Self {
            position,
            restore: if position == SidebarPosition::Hidden {
                SidebarPosition::Right
            } else {
                position
            },
        }
    }

    pub fn toggle(&mut self) -> SidebarPosition {
        self.position = if self.position == SidebarPosition::Hidden {
            self.restore
        } else {
            SidebarPosition::Hidden
        };
        self.position
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub title: Option<String>,
    pub body: String,
}

impl Notice {
    pub fn new(title: Option<&str>, body: &str) -> Self {
        Self {
            title: title
                .map(|s| clean_text(s, MAX_NOTIFY_CHARS))
                .filter(|s| !s.is_empty()),
            body: clean_text(body, MAX_NOTIFY_CHARS),
        }
    }

    pub fn label(&self) -> String {
        match &self.title {
            Some(title) if !self.body.is_empty() => format!("{title}: {}", self.body),
            Some(title) => title.clone(),
            None => self.body.clone(),
        }
    }
}

/// High-level agent activity state for a pane (from Claude Code hooks or cmux status calls).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentState {
    Working,
    NeedsInput,
    Done,
    Error,
}

impl AgentState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::NeedsInput => "needs_input",
            Self::Done => "done",
            Self::Error => "error",
        }
    }

    /// Higher priority wins when summarizing a tab with multiple split panes.
    pub fn priority(self) -> u8 {
        match self {
            Self::Done => 1,
            Self::Working => 2,
            Self::Error => 3,
            Self::NeedsInput => 4,
        }
    }

    /// Map a status key/value pair (e.g. from `workspace.set_status`) to an [`AgentState`].
    pub fn from_status(key: &str, value: &str) -> Option<Self> {
        let k = key.trim().to_ascii_lowercase();
        let v = value.trim().to_ascii_lowercase();
        if matches!(k.as_str(), "needs-input" | "needs_input") && !v.is_empty() {
            return Some(Self::NeedsInput);
        }
        match v.as_str() {
            "working" | "running" | "busy" | "thinking" | "in_progress" => Some(Self::Working),
            "needs_input" | "needs-input" | "waiting" | "input" | "permission" | "attention" => {
                Some(Self::NeedsInput)
            }
            "done" | "idle" | "finished" | "complete" | "completed" => Some(Self::Done),
            "error" | "failed" | "failure" => Some(Self::Error),
            _ => None,
        }
    }
}

/// Per-pane agent status shown in the sidebar and returned by control queries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentStatus {
    pub state: AgentState,
    /// Optional cleaned message/label (e.g. last notification text).
    pub text: Option<String>,
}

impl AgentStatus {
    pub fn new(state: AgentState, text: Option<&str>) -> Self {
        let text = text
            .map(|s| clean_text(s, MAX_NOTIFY_CHARS))
            .filter(|s| !s.is_empty());
        Self { state, text }
    }
}

/// Choose the highest-priority agent status to display for a tab, preferring `focused` on ties.
pub fn pick_tab_agent_status<'a>(
    focused: Option<&'a AgentStatus>,
    others: impl IntoIterator<Item = &'a AgentStatus>,
) -> Option<AgentStatus> {
    let mut best = focused;
    for candidate in others {
        match best {
            None => best = Some(candidate),
            Some(cur) if candidate.state.priority() > cur.state.priority() => {
                best = Some(candidate);
            }
            _ => {}
        }
    }
    best.cloned()
}

/// One tab, with the focused pane's cwd/command state and notifications from *all* panes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub title: String,
    pub cwd: Option<PathBuf>,
    pub branch: Option<String>,
    pub running: bool,
    pub last_exit: Option<i32>,
    pub last_command: Option<String>,
    pub attention_count: usize,
    pub notification: Option<Notice>,
    /// Listening TCP ports of the tab's processes, sorted.
    pub ports: Vec<u16>,
    /// Agent status (working / needs input / done) reported for a pane in this tab.
    pub agent_status: Option<AgentStatus>,
    pub remote_state: Option<ConnectionState>,
}

impl Entry {
    pub fn new(
        title: &str,
        cwd: Option<&Path>,
        running: bool,
        last_exit: Option<i32>,
        notices: impl IntoIterator<Item = Notice>,
    ) -> Self {
        let mut notices = notices.into_iter();
        let notification = notices.next();
        let attention_count = usize::from(notification.is_some()) + notices.count();
        Self {
            title: clean_text(title, 256),
            cwd: cwd.map(Path::to_path_buf),
            branch: None,
            running,
            last_exit,
            last_command: None,
            attention_count,
            notification,
            ports: Vec::new(),
            agent_status: None,
            remote_state: None,
        }
    }

    /// The entry with these listening ports.
    pub fn with_ports(mut self, ports: Vec<u16>) -> Self {
        self.ports = ports;
        self
    }

    /// The entry with this agent status (working / needs input / done).
    pub fn with_agent_status(mut self, status: Option<AgentStatus>) -> Self {
        self.agent_status = status;
        self
    }

    /// Retain the latest message after it has been read. A pane may still carry attention
    /// for a notification that has fallen out of the window's bounded history.
    pub fn with_notification_history<'a>(
        mut self,
        panes: &[PaneId],
        newest_first: &[&HistoryEntry],
        pending: impl Iterator<Item = (PaneId, &'a Notification)>,
    ) -> Self {
        let notices = newest_first
            .iter()
            .copied()
            .filter(|notice| panes.contains(&notice.pane));
        let latest = notices
            .clone()
            .next()
            .map(|notice| Notice::new(notice.title.as_deref(), &notice.body));
        self.attention_count = notices.clone().filter(|notice| !notice.seen).count();
        let mut fallback = None;
        for (id, notice) in pending {
            if notices.clone().any(|entry| entry.pane == id && !entry.seen) {
                continue;
            }
            self.attention_count += 1;
            if fallback.is_none_or(|previous: &Notification| notice.at > previous.at) {
                fallback = Some(notice);
            }
        }
        self.notification = latest
            .or_else(|| fallback.map(|notice| Notice::new(notice.title.as_deref(), &notice.body)));
        self
    }

    /// The latest notification for the workspace, or the current agent message.
    pub fn display_notice(&self) -> Option<String> {
        let notice = || {
            self.notification
                .as_ref()
                .map(Notice::label)
                .filter(|text| !text.is_empty())
        };
        let agent = || {
            self.agent_status
                .as_ref()
                .and_then(|status| status.text.clone())
        };
        if self.attention_count > 0 {
            notice().or_else(agent)
        } else {
            agent().or_else(notice)
        }
    }

    pub fn display_state(&self) -> Option<AgentState> {
        if let Some(status) = &self.agent_status {
            Some(status.state)
        } else if self.running {
            Some(AgentState::Working)
        } else {
            self.last_exit.map(|exit| {
                if exit == 0 {
                    AgentState::Done
                } else {
                    AgentState::Error
                }
            })
        }
    }
}

/// Abbreviate HOME by path components before fitting the label to its rendered width.
/// Cleaning happens only for display; repository discovery uses the original path.
pub fn display_path(path: &Path, home: Option<&Path>) -> String {
    let path = match home.filter(|p| !p.as_os_str().is_empty()) {
        Some(home) => match path.strip_prefix(home) {
            Ok(tail) if tail.as_os_str().is_empty() => "~".to_string(),
            Ok(tail) => format!("~/{}", tail.to_string_lossy()),
            Err(_) => path.to_string_lossy().into_owned(),
        },
        None => path.to_string_lossy().into_owned(),
    };
    clean_text(&path, 4096)
}
