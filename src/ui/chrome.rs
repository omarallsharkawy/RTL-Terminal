//! Shared physical-pixel geometry for painting and hit testing the window chrome.

use crate::layout::{sidebar_layout, PaneId, Rect, SidebarPosition};
use crate::render::CellMetrics;
use crate::ui::git_diff::DiffStat;

pub const TOP_BAR_HEIGHT: f32 = 36.0;
pub const PANE_HEADER_HEIGHT: f32 = 28.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromeAction {
    Sidebar,
    Search,
    Diff,
    Notifications,
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
    pub top_bar: Rect,
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
        let legacy = metrics.content_rect(tab_count, (width, body.h));
        let legacy = Rect::new(
            legacy.x,
            legacy.y.min(body.h) + top,
            legacy.w,
            (body.h - legacy.y).max(0.0),
        );
        let (content, sidebar) = sidebar_layout(body, legacy, sidebar.0, sidebar.1, metrics.scale);
        let tab_height = if sidebar.is_none() {
            legacy.y - top
        } else {
            0.0
        };
        let tabs = if tab_height > 0.0 {
            metrics
                .tab_rects(tab_count, width)
                .into_iter()
                .map(|r| Rect::new(r.x, r.y + top, r.w, tab_height))
                .collect()
        } else {
            Vec::new()
        };
        Self {
            content,
            sidebar,
            tab_bar: Rect::new(0.0, top, width, tab_height),
            tabs,
            top_bar: Rect::new(0.0, 0.0, width, top),
        }
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
    pub fn new(area: Rect, scale: f32, stats: Option<DiffStat>) -> Self {
        let pad = (6.0 * scale).min(area.w / 4.0);
        let button_h = (26.0 * scale).min(area.h);
        let y = area.y + (area.h - button_h) / 2.0;
        let side = area.w * 0.5;
        let menu_w = (28.0 * scale).min((side - pad).max(0.0));
        let bell_w = (44.0 * scale).min((side - pad).max(0.0));
        let sidebar = Rect::new(area.x + pad, y, menu_w, button_h);
        let notifications = Rect::new(area.right() - pad - bell_w, y, bell_w, button_h);
        let diff = stats.filter(|_| area.w >= 304.0 * scale).map(|stats| {
            let w = (28.0 + 7.0 * stats.label().chars().count() as f32) * scale;
            Rect::new(notifications.x - pad - w, y, w, button_h)
        });
        let search_right = diff.unwrap_or(notifications).x - pad;
        let search_left = sidebar.right() + pad;
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
        let pad = (3.0 * scale).min(pane.w / 8.0).min(h / 2.0);
        let button_w = (24.0 * scale).min((pane.w - 2.0 * pad) / 3.0);
        let right = pane.right() - pad;
        let button =
            |n: f32| Rect::new(right - n * button_w, pane.y + pad, button_w, h - 2.0 * pad);
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
