//! The workspace card list and its pointer geometry, in physical pixels.

use std::time::{Duration, Instant};

use unicode_bidi::{bidi_class, BidiClass};
use unicode_segmentation::UnicodeSegmentation;

use super::{AgentState, Entry};
use crate::layout::{Rect, SidebarPosition};
use crate::render::ui_style::{SPACE_12, SPACE_16, SPACE_4, SPACE_8};
use crate::ui::chrome::{ChromeAction, UiLanguage};
use crate::ui::hit::Hit;
use crate::ui::motion::Reorder;

pub const DOT_INTERVAL: Duration = Duration::from_millis(240);
const TITLE_HEIGHT: f32 = 20.0;
const DETAIL_HEIGHT: f32 = SPACE_16;
const MESSAGE_HEIGHT: f32 = 20.0;
const NEW_TAB_HEIGHT: f32 = 32.0;
pub(crate) const STATE_ICON_WIDTH: f32 = SPACE_12;
pub(crate) const STATE_LABEL_PAD: f32 = SPACE_4;

#[derive(Clone, Debug)]
pub struct Card {
    pub entry: Entry,
    pub message: Option<String>,
    pub(crate) state_label_widths: [f32; 2],
}

impl Card {
    pub fn new(entry: Entry) -> Self {
        let message = entry
            .display_notice()
            .or_else(|| entry.last_command.clone());
        Self {
            entry,
            message,
            state_label_widths: [0.0; 2],
        }
    }

    fn has_details(&self) -> bool {
        self.entry.display_state().is_some() || self.has_metadata()
    }

    fn has_metadata(&self) -> bool {
        self.entry.cwd.is_some()
            || self.entry.branch.is_some()
            || self.entry.remote_state.is_some()
            || !self.entry.ports.is_empty()
    }
}

impl AgentState {
    pub(crate) fn label(self, language: UiLanguage) -> &'static str {
        match self {
            Self::Working => language.text("Working", "شغّال"),
            Self::NeedsInput => language.text("Needs input", "مستني إدخال"),
            Self::Done => language.text("Done", "تم"),
            Self::Error => language.text("Error", "خطأ"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CardLayout {
    pub title: Rect,
    pub unread: Rect,
    pub state: Rect,
    pub state_has_label: bool,
    pub close: Rect,
    pub message: Rect,
    pub footer: Rect,
}

impl CardLayout {
    pub fn card_width(area: Rect, scale: f32) -> f32 {
        (area.w - 2.0 * SPACE_8 * scale).max(0.0)
    }

    pub fn height(card: &Card, scale: f32) -> f32 {
        let details = if card.has_details() {
            DETAIL_HEIGHT
        } else {
            0.0
        };
        let message = if card.message.is_some() {
            MESSAGE_HEIGHT
        } else {
            0.0
        };
        (2.0 * SPACE_8 + TITLE_HEIGHT + details + message) * scale
    }

    pub fn new(rect: Rect, card: &Card, scale: f32, mirrored: bool, language: UiLanguage) -> Self {
        let pad = (SPACE_12 * scale).min(rect.w / 2.0);
        let inner = Rect::new(
            rect.x + pad,
            rect.y + SPACE_8 * scale,
            rect.w - 2.0 * pad,
            TITLE_HEIGHT * scale,
        );
        let gap = (SPACE_4 * scale).min(inner.w / 8.0);
        let row = |y, h, offset, width: f32| {
            let x = if mirrored {
                inner.right() - offset - width
            } else {
                inner.x + offset
            };
            Rect::new(x, y, width, h)
        };
        let title_row = |offset, width| row(inner.y, inner.h, offset, width);
        let close_w = (SPACE_16 * scale).min(inner.w / 5.0);
        let close = title_row(inner.w - close_w, close_w);
        let badge_w = match card.entry.attention_count {
            0 => 0.0,
            1..=9 => SPACE_16,
            _ => 24.0,
        };
        let badge_w = (badge_w * scale).min(inner.w / 5.0);
        let body_x = if mirrored {
            close.right() + gap
        } else {
            inner.x
        };
        let body_w = (inner.w - close_w - gap).max(0.0);
        let badge_w = badge_w.min(body_w);
        let badge_slot = if badge_w > 0.0 {
            (badge_w + gap).min(body_w)
        } else {
            0.0
        };
        let title = card.entry.title.trim();
        let rtl = if title.is_empty() || matches!(title, "…" | "...") {
            language == UiLanguage::Arabic
        } else {
            title
                .chars()
                .find_map(|ch| match bidi_class(ch) {
                    BidiClass::R | BidiClass::AL => Some(true),
                    BidiClass::L => Some(false),
                    _ => None,
                })
                .unwrap_or(false)
        };
        let unread_x = if rtl {
            body_x
        } else {
            body_x + body_w - badge_w
        };
        let unread = Rect::new(unread_x, inner.y, badge_w, inner.h);
        let title = Rect::new(
            if rtl { body_x + badge_slot } else { body_x },
            inner.y,
            body_w - badge_slot,
            inner.h,
        );

        let details_h = if card.has_details() {
            DETAIL_HEIGHT * scale
        } else {
            0.0
        };
        let caption = card.state_label_widths[usize::from(language == UiLanguage::Arabic)];
        let icon_w = STATE_ICON_WIDTH * scale;
        let state_full = caption + icon_w + STATE_LABEL_PAD * scale;
        let state_room = if card.has_metadata() {
            inner.w * 0.55
        } else {
            inner.w
        };
        let state_has_label = caption > 0.0 && state_full <= state_room;
        let state_w = if state_has_label {
            state_full
        } else if card.entry.display_state().is_some() {
            icon_w.min(inner.w)
        } else {
            0.0
        };
        let state = row(inner.bottom(), details_h, inner.w - state_w, state_w);
        let footer = row(
            inner.bottom(),
            details_h,
            0.0,
            (inner.w - state_w - if state_w > 0.0 { gap } else { 0.0 }).max(0.0),
        );
        let message = row(
            footer.bottom(),
            if card.message.is_some() {
                MESSAGE_HEIGHT * scale
            } else {
                0.0
            },
            0.0,
            inner.w,
        );
        Self {
            title,
            unread,
            state,
            state_has_label,
            close,
            message,
            footer,
        }
    }
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
    pub(crate) position: SidebarPosition,
    pub items: Vec<Item>,
    pub new_tab: Rect,
    pub language: UiLanguage,
    pub hover: Option<usize>,
    pub close_hover: bool,
    pub new_tab_hover: bool,
    pub dot_phase: usize,
    pub(crate) dot_progress: Option<f32>,
    pub(crate) reordering: bool,
    pub(crate) highlights: Option<Vec<(f32, f32)>>,
    pub(crate) close_mixes: Vec<f32>,
    pub(crate) new_tab_mix: Option<f32>,
    first_y: f32,
    pub drop_target: Option<usize>,
    card_count: usize,
    content_height: f32,
}

impl View {
    /// Move every affected card from its previous slot, clipping against the sidebar viewport.
    pub(crate) fn apply_reorder(
        &mut self,
        cards: &[Card],
        ids: &[u64],
        active: usize,
        reorder: &mut Reorder,
        now: Instant,
        reduced: bool,
    ) {
        let mut y = self.first_y;
        let slots = cards
            .iter()
            .zip(ids)
            .map(|(card, &id)| {
                let h = CardLayout::height(card, self.scale);
                let rect = Rect::new(self.new_tab.x, y, self.new_tab.w, h);
                y += h + SPACE_4 * self.scale;
                (id, rect)
            })
            .collect::<Vec<_>>();
        reorder.update(
            ids,
            slots.iter().map(|&(id, rect)| (id, rect.y)).collect(),
            now,
            reduced,
        );
        self.reordering = reorder.next_frame(now).is_some();
        if self.reordering {
            self.items.clear();
            for (index, ((id, mut rect), card)) in slots.into_iter().zip(cards).enumerate() {
                rect.y += reorder.offset(id, now);
                if rect.bottom() > self.area.y && rect.y < self.area.bottom() {
                    self.items.push(Item {
                        index,
                        rect,
                        active: index == active,
                        card: card.clone(),
                    });
                }
            }
            // The moved card travels in front of the cards making room for it.
            self.items.sort_by_key(|item| item.active);
        }
    }

    pub fn matches(&self, area: Rect, card_count: usize) -> bool {
        self.area == area && self.card_count == card_count
    }

    pub fn hit_test(&self, x: f64, y: f64) -> Option<usize> {
        if !self.area.contains(x, y) {
            return None;
        }
        self.items
            .iter()
            .rev()
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
        let Some(item) = self
            .items
            .iter()
            .rev()
            .find(|item| item.rect.contains(x, y))
        else {
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
            state.w >= STATE_ICON_WIDTH * self.scale
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
        let advance = |card| CardLayout::height(card, scale) + SPACE_4 * scale;
        let margin = SPACE_8 * scale;
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
        let first_y = area.y + margin - self.offset;
        let mut y = first_y;
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
            mirrored: position == SidebarPosition::Right || language == UiLanguage::Arabic,
            position,
            items,
            new_tab: Rect::new(x, y, w, NEW_TAB_HEIGHT * scale),
            language,
            hover: None,
            close_hover: false,
            new_tab_hover: false,
            dot_phase: 0,
            dot_progress: None,
            reordering: false,
            highlights: None,
            close_mixes: Vec::new(),
            new_tab_mix: None,
            first_y,
            drop_target: None,
            card_count: cards.len(),
            content_height,
        }
    }
}

fn max_scroll(content_height: f32, area: Rect) -> f32 {
    (content_height - area.h).max(0.0)
}

/// Keep a whole-word prefix when possible and fall back to graphemes in an overlong word.
pub fn fit_title(text: &str, mut fits: impl FnMut(&str) -> bool) -> String {
    let fitted = crate::ui::text::fit_text(text, &mut fits);
    if fitted != "…" {
        return fitted;
    }
    text.graphemes(true)
        .next()
        .filter(|grapheme| *grapheme != "…" && fits(grapheme))
        .unwrap_or("")
        .to_owned()
}
