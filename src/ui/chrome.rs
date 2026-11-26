//! Shared physical-pixel geometry for painting and hit testing the window chrome.

use std::time::{Duration, Instant};

use crate::layout::{sidebar_layout, PaneId, Rect, SidebarPosition};
use crate::render::ui_style::{SPACE_4, SPACE_8};
use crate::render::CellMetrics;
use crate::ui::git_diff::DiffStat;

pub use twitty_core::language::{UiLanguage, UiLanguagePref};

pub const TOP_BAR_HEIGHT: f32 = 36.0;
pub const PANE_HEADER_HEIGHT: f32 = 24.0;
/// How long `pane.flash` highlights a pane's outline.
pub const PANE_FLASH_DURATION: Duration = Duration::from_millis(450);
use super::motion::{ease_out, progress, FRAME_STEP};

/// Brief outline flash on a pane (`pane.flash`, and after jumping to an unread notification).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PaneFlash {
    active: Option<(PaneId, Instant)>,
}

impl PaneFlash {
    pub fn trigger(&mut self, pane: PaneId, now: Instant) {
        self.active = Some((pane, now));
    }

    pub fn forget(&mut self, pane: PaneId) {
        if self.active.is_some_and(|(id, _)| id == pane) {
            self.active = None;
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Strength in `0.0..=1.0` for `pane` at `now` (0.0 when inactive or expired).
    pub fn intensity(&self, pane: PaneId, now: Instant) -> f32 {
        let Some((id, started)) = self.active else {
            return 0.0;
        };
        if id != pane {
            return 0.0;
        }
        let elapsed = now.saturating_duration_since(started);
        if elapsed >= PANE_FLASH_DURATION {
            return 0.0;
        }
        1.0 - ease_out(progress(elapsed, PANE_FLASH_DURATION))
    }

    /// Expire finished flashes and return the next redraw deadline while active.
    pub fn tick(&mut self, now: Instant) -> Option<Instant> {
        let (_, started) = self.active?;
        let end = started + PANE_FLASH_DURATION;
        if now >= end {
            self.active = None;
            return None;
        }
        Some((now + FRAME_STEP).min(end))
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum ChromeAction {
    Sidebar,
    Search,
    Diff,
    Notifications,
    NewTab,
    CloseTab(usize),
    FocusPane(PaneId),
    SplitRight(PaneId),
    SplitDown(PaneId),
    Close(PaneId),
}

#[derive(Debug)]
pub struct WindowLayout {
    pub content: Rect,
    pub sidebar: Option<Rect>,
    pub tab_bar: Rect,
    pub tabs: Vec<Rect>,
    pub new_tab: Rect,
    scale: f32,
    language: UiLanguage,
    pub top_bar: Rect,
    /// The code review panel, when open (set by the window, not by `new`).
    pub review: Option<Rect>,
}

impl WindowLayout {
    pub fn new(
        metrics: CellMetrics,
        (width, height): (f32, f32),
        tab_count: usize,
        sidebar: (SidebarPosition, f32),
        top_bar: bool,
    ) -> Self {
        let top = if top_bar {
            (TOP_BAR_HEIGHT * metrics.scale).round().min(height)
        } else {
            0.0
        };
        let body = Rect::new(0.0, top, width, (height - top).max(0.0));
        let strip_height = if tab_count > 0 {
            (32.0 * metrics.scale).round().min(body.h)
        } else {
            0.0
        };
        let with_strip = Rect::new(0.0, top + strip_height, width, body.h - strip_height);
        let (content, sidebar) =
            sidebar_layout(body, with_strip, sidebar.0, sidebar.1, metrics.scale);
        let tab_height = if sidebar.is_none() { strip_height } else { 0.0 };
        let tab_bar = Rect::new(0.0, top, width, tab_height);
        let scale = metrics.scale;
        let pad = (SPACE_4 * scale).min(width / 8.0);
        let h = (tab_height - 2.0 * pad).max(0.0);
        let plus_w = (28.0 * scale).min((width - 2.0 * pad).max(0.0) / 3.0);
        let available = (width - 3.0 * pad - plus_w).max(0.0);
        let count = tab_count.max(1) as f32;
        let gap = pad.min(available / count / 4.0);
        let w = ((available - gap * (count - 1.0)) / count).min(200.0 * scale);
        let y = top + (tab_height - h) / 2.0;
        let tabs: Vec<_> = if h > 0.0 {
            (0..tab_count)
                .map(|i| Rect::new(pad + i as f32 * (w + gap), y, w, h))
                .collect()
        } else {
            Vec::new()
        };
        let plus_x = tabs.last().map_or(pad, |tab| tab.right() + pad);
        let new_tab = Rect::new(plus_x, y, plus_w, h);
        Self {
            content,
            sidebar,
            tab_bar,
            tabs,
            new_tab,
            scale,
            language: UiLanguage::English,
            top_bar: Rect::new(0.0, 0.0, width, top),
            review: None,
        }
    }

    pub fn with_language(mut self, language: UiLanguage) -> Self {
        self.language = language;
        for tab in &mut self.tabs {
            *tab = language.mirror(*tab, self.tab_bar);
        }
        self.new_tab = language.mirror(self.new_tab, self.tab_bar);
        self
    }

    pub fn tab_close(&self, rect: Rect) -> Rect {
        let pad = (SPACE_4 * self.scale).min(rect.w / 8.0).min(rect.h / 2.0);
        let side = (16.0 * self.scale)
            .min(rect.h - 2.0 * pad)
            .min(rect.w / 2.0);
        self.language.mirror(
            Rect::new(
                rect.right() - pad - side,
                rect.y + (rect.h - side) / 2.0,
                side,
                side,
            ),
            rect,
        )
    }

    pub fn tab_hit(&self, x: f64, y: f64) -> Option<super::hit::Hit> {
        use super::hit::Hit;
        if !self.tab_bar.contains(x, y) {
            return None;
        }
        if self.new_tab.contains(x, y) {
            return Some(Hit::Chrome(ChromeAction::NewTab));
        }
        Some(
            self.tabs
                .iter()
                .enumerate()
                .find_map(|(i, &tab)| {
                    tab.contains(x, y).then(|| {
                        if self.tab_close(tab).contains(x, y) {
                            Hit::Chrome(ChromeAction::CloseTab(i))
                        } else {
                            Hit::Tab(i)
                        }
                    })
                })
                .unwrap_or(Hit::TabBar),
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TopBarLayout {
    pub area: Rect,
    pub sidebar: Rect,
    pub search: Rect,
    pub diff: Option<Rect>,
    pub notifications: Rect,
}

impl TopBarLayout {
    pub fn can_show_diff(area: Rect, scale: f32) -> bool {
        area.h > 0.0 && area.w >= 304.0 * scale
    }

    pub fn new(area: Rect, scale: f32, stats: Option<DiffStat>) -> Self {
        let pad = (SPACE_8 * scale).min(area.w / 4.0);
        let gap = (SPACE_4 * scale).min(area.w / 8.0);
        let button_h = (28.0 * scale).min(area.h);
        let y = area.y + (area.h - button_h) / 2.0;
        let side = area.w * 0.5;
        let menu_w = (32.0 * scale).min((side - pad).max(0.0));
        let bell_w = (64.0 * scale).min((side - pad).max(0.0));
        let sidebar = Rect::new(area.x + pad, y, menu_w, button_h);
        let notifications = Rect::new(area.right() - pad - bell_w, y, bell_w, button_h);
        let diff = stats
            .filter(|stats| {
                (stats.added > 0 || stats.removed > 0) && Self::can_show_diff(area, scale)
            })
            .map(|stats| {
                let w = (40.0 + 7.0 * stats.label().chars().count() as f32) * scale;
                Rect::new(notifications.x - gap - w, y, w, button_h)
            });
        let search_right = diff.unwrap_or(notifications).x - gap;
        let search_left = sidebar.right() + gap;
        let center = area.x + area.w / 2.0;
        let centered_room = 2.0 * (center - search_left).min(search_right - center);
        let room = if centered_room >= 80.0 * scale {
            centered_room
        } else {
            (search_right - search_left).max(0.0)
        };
        let w = (456.0 * scale).min(room);
        let x = (center - w / 2.0).min(search_right - w).max(search_left);
        let search = Rect::new(x, y, w, button_h);
        Self {
            area,
            sidebar,
            search,
            diff,
            notifications,
        }
    }

    pub fn with_language(mut self, language: UiLanguage) -> Self {
        self.sidebar = language.mirror(self.sidebar, self.area);
        self.search = language.mirror(self.search, self.area);
        self.diff = self.diff.map(|rect| language.mirror(rect, self.area));
        self.notifications = language.mirror(self.notifications, self.area);
        self
    }

    pub fn hit(self, x: f64, y: f64) -> Option<super::hit::Hit> {
        self.area.contains(x, y).then(|| {
            use super::hit::Hit;
            [
                (Some(self.sidebar), ChromeAction::Sidebar),
                (Some(self.search), ChromeAction::Search),
                (self.diff, ChromeAction::Diff),
                (Some(self.notifications), ChromeAction::Notifications),
            ]
            .into_iter()
            .find_map(|(rect, action)| {
                rect.filter(|r| r.contains(x, y))
                    .map(|_| Hit::Chrome(action))
            })
            .unwrap_or(Hit::ChromeBackground)
        })
    }
}

pub const SEARCH_EN: &str = "Search or run a command…";
pub const SEARCH_AR: &str = "دوّر أو شغّل أمر…";
pub const SEARCH_KEYCAP: &str = "Ctrl+Shift+P";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchPlaceholder {
    Bilingual,
    Local,
    LocalShortened,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchContents {
    pub placeholder: SearchPlaceholder,
    pub keycap: bool,
}

impl SearchContents {
    pub fn fit(room: f32, bilingual: f32, local: f32, keycap: f32) -> Self {
        let (placeholder, used) = if room >= bilingual {
            (SearchPlaceholder::Bilingual, bilingual)
        } else if room >= local {
            (SearchPlaceholder::Local, local)
        } else {
            (SearchPlaceholder::LocalShortened, 0.0)
        };
        Self {
            placeholder,
            keycap: room - used >= keycap,
        }
    }
}

pub fn count_badge(count: usize) -> Option<String> {
    match count {
        0 => None,
        1..=99 => Some(count.to_string()),
        _ => Some("99+".to_owned()),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PaneHeaderLayout {
    pub header: Rect,
    pub grid: Rect,
    pub title: Rect,
    pub split_right: Rect,
    pub split_down: Rect,
    pub close: Rect,
}

impl PaneHeaderLayout {
    pub fn new(pane: Rect, scale: f32, enabled: bool) -> Self {
        let h = if enabled {
            (PANE_HEADER_HEIGHT * scale).round().min(pane.h)
        } else {
            0.0
        };
        let header = Rect::new(pane.x, pane.y, pane.w, h);
        let grid = Rect::new(pane.x, pane.y + h, pane.w, pane.h - h);
        let pad = (SPACE_4 * scale).min(pane.w / 8.0);
        let inset_y = (SPACE_4 * scale / 2.0).min(h / 2.0);
        let button_w = (24.0 * scale).min((pane.w - 2.0 * pad) / 3.0);
        let right = pane.right() - pad;
        let button = |n: f32| {
            Rect::new(
                right - n * button_w,
                pane.y + inset_y,
                button_w,
                h - 2.0 * inset_y,
            )
        };
        let split_right = button(3.0);
        let title = Rect::new(
            pane.x + pad,
            pane.y,
            (split_right.x - pane.x - pad).max(0.0),
            h,
        );
        Self {
            header,
            grid,
            title,
            split_right,
            split_down: button(2.0),
            close: button(1.0),
        }
    }

    pub fn with_language(mut self, language: UiLanguage) -> Self {
        self.title = language.mirror(self.title, self.header);
        self.split_right = language.mirror(self.split_right, self.header);
        self.split_down = language.mirror(self.split_down, self.header);
        self.close = language.mirror(self.close, self.header);
        self
    }

    pub fn hit(self, pane: PaneId, x: f64, y: f64) -> Option<ChromeAction> {
        self.header.contains(x, y).then(|| {
            [
                (self.split_right, ChromeAction::SplitRight(pane)),
                (self.split_down, ChromeAction::SplitDown(pane)),
                (self.close, ChromeAction::Close(pane)),
            ]
            .into_iter()
            .find_map(|(rect, action)| rect.contains(x, y).then_some(action))
            .unwrap_or(ChromeAction::FocusPane(pane))
        })
    }
}
