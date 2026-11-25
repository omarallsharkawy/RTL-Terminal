//! The workspace card list and its pointer geometry, in physical pixels.

use std::time::Duration;

use super::Entry;
use crate::layout::{Rect, SidebarPosition};
use crate::ui::chrome::{ChromeAction, UiLanguage};
use crate::ui::hit::Hit;

pub const DOT_INTERVAL: Duration = Duration::from_millis(240);
const GAP: f32 = 4.0;
const MARGIN: f32 = 6.0;
const PAD: f32 = 8.0;
const TITLE_HEIGHT: f32 = 20.0;
const MESSAGE_HEIGHT: f32 = 18.0;
const FOOTER_HEIGHT: f32 = 17.0;
const NEW_TAB_HEIGHT: f32 = 30.0;

#[derive(Clone, Debug)]
pub struct Card {
    pub entry: Entry,
    pub message: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct CardLayout {
    pub title: Rect,
    pub unread: Rect,
    pub state: Rect,
    pub close: Rect,
    pub message: Rect,
    pub footer: Rect,
}

impl CardLayout {
    pub fn card_width(area: Rect, scale: f32) -> f32 {
        (area.w - 2.0 * MARGIN * scale).max(0.0)
    }

    pub fn message_width(area: Rect, scale: f32) -> f32 {
        (Self::card_width(area, scale) - 2.0 * PAD * scale).max(0.0)
    }

    pub fn height(card: &Card, scale: f32) -> f32 {
        let footer = if has_footer(&card.entry) {
            FOOTER_HEIGHT
        } else {
            0.0
        };
        (2.0 * PAD + TITLE_HEIGHT + card.message.len() as f32 * MESSAGE_HEIGHT + footer) * scale
    }

    pub fn new(rect: Rect, card: &Card, scale: f32, mirrored: bool, language: UiLanguage) -> Self {
        let pad = (PAD * scale).min(rect.w / 2.0);
        let inner = Rect::new(
            rect.x + pad,
            rect.y + PAD * scale,
            rect.w - 2.0 * pad,
            TITLE_HEIGHT * scale,
        );
        let gap = (GAP * scale).min(inner.w / 8.0);
        let start = |offset: f32, width: f32| {
            let width = width.max(0.0);
            let x = if mirrored {
                inner.right() - offset - width
            } else {
                inner.x + offset
            };
            Rect::new(x, inner.y, width, inner.h)
        };
        let close_w = (18.0 * scale).min(inner.w * 0.20);
        let close = start(inner.w - close_w, close_w);
        let badge_w = if card.entry.attention_count == 0 {
            0.0
        } else {
            let w = if card.entry.attention_count < 10 {
                16.0
            } else {
                24.0
            };
            (w * scale).min(inner.w * 0.22)
        };
        let unread = start(0.0, badge_w);
        let leading = if badge_w > 0.0 { badge_w + gap } else { 0.0 };
        let remaining = (inner.w - leading - close_w - gap).max(0.0);
        let state_w = card.entry.display_state().map_or(0.0, |state| {
            let label_w = match (language, state) {
                (UiLanguage::English, crate::sidebar::AgentState::NeedsInput) => 82.0,
                _ => 67.0,
            } * scale;
            if remaining >= label_w + 38.0 * scale {
                label_w
            } else {
                (25.0 * scale).min(remaining * 0.36)
            }
        });
        let state = start(leading + remaining - state_w, state_w);
        let title = start(
            leading,
            remaining - state_w - if state_w > 0.0 { gap } else { 0.0 },
        );
        let message = Rect::new(
            inner.x,
            inner.bottom(),
            inner.w,
            card.message.len() as f32 * MESSAGE_HEIGHT * scale,
        );
        let footer = Rect::new(
            inner.x,
            message.bottom(),
            inner.w,
            if has_footer(&card.entry) {
                FOOTER_HEIGHT * scale
            } else {
                0.0
            },
        );
        Self {
            title,
            unread,
            state,
            close,
            message,
            footer,
        }
    }
}

fn has_footer(entry: &Entry) -> bool {
    entry.cwd.is_some()
        || entry.branch.is_some()
        || entry.remote_state.is_some()
        || !entry.ports.is_empty()
}

#[derive(Clone, Debug)]
pub struct Item {
    pub index: usize,
    pub rect: Rect,
    pub active: bool,
    pub card: Card,
}

impl Item {
    pub fn layout(&self, view: &View) -> CardLayout {
        CardLayout::new(
            self.rect,
            &self.card,
            view.scale,
            view.mirrored,
            view.language,
        )
    }
}

#[derive(Clone, Debug)]
pub struct View {
    pub area: Rect,
    pub scale: f32,
    pub mirrored: bool,
    pub items: Vec<Item>,
    pub new_tab: Rect,
    pub language: UiLanguage,
    pub hover: Option<usize>,
    pub close_hover: bool,
    pub new_tab_hover: bool,
    pub dot_phase: usize,
    pub drop_target: Option<usize>,
    card_count: usize,
    content_height: f32,
}

impl View {
    pub fn matches(&self, area: Rect, card_count: usize) -> bool {
        self.area == area && self.card_count == card_count
    }

    pub fn hit_test(&self, x: f64, y: f64) -> Option<usize> {
        if !self.area.contains(x, y) {
            return None;
        }
        self.items
            .iter()
            .find(|i| i.rect.contains(x, y))
            .map(|i| i.index)
    }

    pub fn pointer_hit(&self, x: f64, y: f64) -> Option<Hit> {
        if !self.area.contains(x, y) {
            return None;
        }
        if self.new_tab.contains(x, y) {
            return Some(Hit::Chrome(ChromeAction::NewTab));
        }
        let Some(item) = self.items.iter().find(|item| item.rect.contains(x, y)) else {
            return Some(Hit::TabBar);
        };
        let close = item.layout(self).close;
        if close.y >= self.area.y && close.bottom() <= self.area.bottom() && close.contains(x, y) {
            Some(Hit::Chrome(ChromeAction::CloseTab(item.index)))
        } else {
            Some(Hit::Tab(item.index))
        }
    }

    pub fn animates(&self) -> bool {
        self.items.iter().any(|item| {
            let state = item.layout(self).state;
            state.w >= 16.0 * self.scale
                && state.y >= self.area.y
                && state.bottom() <= self.area.bottom()
                && item.card.entry.display_state() == Some(super::AgentState::Working)
        })
    }

    pub fn update_hover(&mut self, (x, y): (f64, f64)) {
        self.hover = self.hit_test(x, y);
        self.close_hover = matches!(
            self.pointer_hit(x, y),
            Some(Hit::Chrome(ChromeAction::CloseTab(_)))
        );
        self.new_tab_hover = self.area.contains(x, y) && self.new_tab.contains(x, y);
    }
}

/// A press on the card body selects it immediately. Moving vertically before release
/// can then move that workspace to the slot under the pointer.
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    pub tab_id: u64,
    start_y: f64,
}

impl Drag {
    pub fn new(tab_id: u64, start_y: f64) -> Self {
        Self { tab_id, start_y }
    }

    pub fn target(self, view: &View, active: usize, (x, y): (f64, f64)) -> Option<usize> {
        if (y - self.start_y).abs() < f64::from(5.0 * view.scale) {
            return None;
        }
        view.hit_test(x, y).filter(|&index| index != active)
    }
}

#[derive(Default)]
pub struct ScrollState {
    offset: f32,
    active: Option<usize>,
}

impl ScrollState {
    pub fn scroll(&mut self, pixels: f32, view: &View) {
        self.offset = (self.offset + pixels).clamp(0.0, max_scroll(view.content_height, view.area));
    }

    pub fn reveal_active(&mut self) {
        self.active = None;
    }

    pub fn view(
        &mut self,
        cards: &[Card],
        active: usize,
        area: Rect,
        scale: f32,
        position: SidebarPosition,
        language: UiLanguage,
    ) -> View {
        let advance = |card| CardLayout::height(card, scale) + GAP * scale;
        let margin = MARGIN * scale;
        let top = margin + cards.iter().take(active).map(advance).sum::<f32>();
        let bottom = top
            + cards
                .get(active)
                .map_or(0.0, |card| CardLayout::height(card, scale));
        let content_height =
            2.0 * margin + cards.iter().map(advance).sum::<f32>() + NEW_TAB_HEIGHT * scale;
        if self.active != Some(active) {
            if top < self.offset {
                self.offset = (top - margin).max(0.0);
            } else if bottom > self.offset + area.h {
                self.offset = if bottom - top > area.h {
                    top
                } else {
                    bottom - area.h
                };
            }
            self.active = Some(active);
        }
        self.offset = self.offset.clamp(0.0, max_scroll(content_height, area));

        let x = area.x + margin.min(area.w / 2.0);
        let w = CardLayout::card_width(area, scale);
        let mut y = area.y + margin - self.offset;
        let mut items = Vec::new();
        for (index, card) in cards.iter().enumerate() {
            let rect = Rect::new(x, y, w, CardLayout::height(card, scale));
            y += advance(card);
            if rect.bottom() > area.y && rect.y < area.bottom() {
                items.push(Item {
                    index,
                    rect,
                    active: index == active,
                    card: card.clone(),
                });
            }
        }
        View {
            area,
            scale,
            mirrored: position == SidebarPosition::Right,
            items,
            new_tab: Rect::new(x, y, w, NEW_TAB_HEIGHT * scale),
            language,
            hover: None,
            close_hover: false,
            new_tab_hover: false,
            dot_phase: 0,
            drop_target: None,
            card_count: cards.len(),
            content_height,
        }
    }
}

fn max_scroll(content_height: f32, area: Rect) -> f32 {
    (content_height - area.h).max(0.0)
}

/// Keep the longest prefix made of whole words. A single overlong word becomes an ellipsis.
pub fn fit_title(text: &str, mut fits: impl FnMut(&str) -> bool) -> String {
    if fits(text) {
        return text.to_owned();
    }
    if !fits("…") {
        return String::new();
    }
    let cuts: Vec<_> = text
        .match_indices(char::is_whitespace)
        .filter(|(i, _)| *i > 0 && !text[..*i].ends_with(char::is_whitespace))
        .map(|(i, _)| i)
        .collect();
    let n = cuts.partition_point(|&cut| fits(&format!("{}…", &text[..cut])));
    if n == 0 {
        "…".to_owned()
    } else {
        format!("{}…", &text[..cuts[n - 1]])
    }
}

/// Wrap at spaces with the actual UI font's widths. The last line indicates remaining text.
pub fn wrap_notice(text: &str, mut fits: impl FnMut(&str) -> bool) -> Vec<String> {
    if text.is_empty() || !fits("…") {
        return Vec::new();
    }
    if fits(text) {
        return vec![text.to_owned()];
    }
    let cuts: Vec<_> = text
        .match_indices(char::is_whitespace)
        .map(|(i, _)| i)
        .filter(|&i| i > 0)
        .collect();
    let n = cuts.partition_point(|&cut| fits(text[..cut].trim_end()));
    if n == 0 {
        // An unbroken URL or path can still use the first line and leave room for following words.
        use unicode_segmentation::UnicodeSegmentation;
        let cuts: Vec<_> = text
            .grapheme_indices(true)
            .skip(1)
            .map(|(i, _)| i)
            .collect();
        let n = cuts.partition_point(|&cut| fits(&text[..cut]));
        if n == 0 {
            return vec!["…".to_owned()];
        }
        let cut = cuts[n - 1];
        return vec![
            text[..cut].to_owned(),
            fit_title(text[cut..].trim_start(), fits),
        ];
    }
    let cut = cuts[n - 1];
    vec![
        text[..cut].trim_end().to_owned(),
        fit_title(text[cut..].trim_start(), fits),
    ]
}
