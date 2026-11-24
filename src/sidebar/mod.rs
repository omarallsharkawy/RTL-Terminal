//! Window-free workspace labels, scrolling and repository metadata.

pub mod git;

use std::path::{Path, PathBuf};

use crate::layout::{Rect, SidebarPosition};
use crate::shell_integration::{clean_text, MAX_NOTIFY_CHARS};
use crate::ui::hit::Hit;

pub fn entry_title(title: Option<&str>, program: Option<&str>, index: usize) -> String {
    title
        .into_iter()
        .chain(program)
        .map(|text| clean_text(text, 256))
        .find(|text| !text.is_empty())
        .unwrap_or_else(|| format!("Tab {}", index + 1))
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

/// One tab, with the focused pane's cwd/command state and notifications from *all* panes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub title: String,
    pub cwd: Option<PathBuf>,
    pub branch: Option<String>,
    pub running: bool,
    pub last_exit: Option<i32>,
    pub attention_count: usize,
    pub notification: Option<Notice>,
    /// Listening TCP ports of the tab's processes, sorted.
    pub ports: Vec<u16>,
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
            attention_count,
            notification,
            ports: Vec::new(),
        }
    }

    /// The entry with these listening ports (shown on their own line when there are any).
    pub fn with_ports(mut self, ports: Vec<u16>) -> Self {
        self.ports = ports;
        self
    }

    pub fn height(&self, line_height: f32) -> f32 {
        let lines = 3.5
            + f32::from(u8::from(self.notification.is_some()))
            + f32::from(u8::from(!self.ports.is_empty()));
        line_height * lines
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

/// An entry's full rect; labels and highlights are clipped against `View::area`.
#[derive(Clone, Debug)]
pub struct Item {
    pub index: usize,
    pub rect: Rect,
    pub active: bool,
    pub entry: Entry,
}

#[derive(Clone, Debug)]
pub struct View {
    pub area: Rect,
    pub right_aligned: bool,
    pub items: Vec<Item>,
}

impl View {
    pub fn hit_test(&self, x: f64, y: f64) -> Option<usize> {
        self.area
            .contains(x, y)
            .then(|| {
                self.items
                    .iter()
                    .find(|i| i.rect.contains(x, y))
                    .map(|i| i.index)
            })
            .flatten()
    }

    pub fn pointer_hit(&self, x: f64, y: f64) -> Option<Hit> {
        self.area
            .contains(x, y)
            .then(|| self.hit_test(x, y).map_or(Hit::TabBar, Hit::Tab))
    }
}

#[derive(Default)]
pub struct ScrollState {
    offset: f32,
    active: Option<usize>,
}

impl ScrollState {
    pub fn scroll(&mut self, pixels: f32, entries: &[Entry], area: Rect, line_height: f32) {
        self.offset = (self.offset + pixels).clamp(0.0, max_scroll(entries, area, line_height));
    }

    /// Force the next `update` to scroll the active tab into view even if its index is unchanged
    /// (for example after closing the active tab or focusing a pane via the control socket).
    pub fn reveal_active(&mut self) {
        self.active = None;
    }

    /// Reveal a tab selected by keyboard, while allowing the user to scroll away afterwards.
    pub fn update(&mut self, entries: &[Entry], active: usize, area: Rect, line_height: f32) {
        if self.active != Some(active) {
            let top: f32 = entries
                .iter()
                .take(active)
                .map(|e| e.height(line_height))
                .sum();
            let bottom = top + entries.get(active).map_or(0.0, |e| e.height(line_height));
            if top < self.offset {
                self.offset = top;
            } else if bottom > self.offset + area.h {
                self.offset = bottom - area.h;
            }
            self.active = Some(active);
        }
        self.offset = self
            .offset
            .clamp(0.0, max_scroll(entries, area, line_height));
    }

    pub fn view(
        &self,
        entries: &[Entry],
        active: usize,
        area: Rect,
        line_height: f32,
        position: SidebarPosition,
    ) -> View {
        let mut y = area.y - self.offset.min(max_scroll(entries, area, line_height));
        let mut items = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            let rect = Rect::new(area.x, y, area.w, entry.height(line_height));
            y = rect.bottom();
            if rect.bottom() > area.y && rect.y < area.bottom() {
                items.push(Item {
                    index,
                    rect,
                    active: index == active,
                    entry: entry.clone(),
                });
            }
        }
        View {
            area,
            right_aligned: position == SidebarPosition::Right,
            items,
        }
    }
}

fn max_scroll(entries: &[Entry], area: Rect, line_height: f32) -> f32 {
    (entries.iter().map(|e| e.height(line_height)).sum::<f32>() - area.h).max(0.0)
}
