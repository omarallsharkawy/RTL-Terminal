//! The toolbar, tab strip and pane headers. All controls share geometry with pointer input.

use crate::layout::{PaneId, Rect};
use crate::render::icons::Icon;
use crate::render::shapes::{Border, Shape};
use crate::render::text::ui::{label_width, UiAlign, UiFont, UiLabel, UiSize, UiWeight};
use crate::render::ui_style::{UiStyle, RADIUS_12, RADIUS_6};
use crate::render::TabLabel;
use crate::ui::chrome::{
    count_badge, ChromeAction, PaneHeaderLayout, SearchContents, SearchPlaceholder, TopBarLayout,
    UiLanguage, WindowLayout, SEARCH_AR, SEARCH_EN, SEARCH_KEYCAP,
};
use crate::ui::git_diff::DiffStat;

use super::context::DrawContext;

pub struct PaneHeaderView {
    pub id: PaneId,
    pub layout: PaneHeaderLayout,
    pub title: String,
    pub agent: bool,
    pub focused: bool,
    pub attention: bool,
}

pub struct ChromeView<'a> {
    pub layout: &'a WindowLayout,
    pub top: TopBarLayout,
    pub diff: Option<DiffStat>,
    pub unread: usize,
    pub sidebar_visible: bool,
    pub hover: Option<ChromeAction>,
    pub hover_tab: Option<usize>,
    pub language: UiLanguage,
    pub panes: &'a [PaneHeaderView],
    pub tabs: &'a [TabLabel],
}

pub(crate) fn draw(
    ctx: &mut DrawContext,
    style: UiStyle,
    font: &UiFont,
    view: &ChromeView,
    shapes: &mut Vec<Shape>,
) -> Vec<UiLabel> {
    let mut chrome = Chrome {
        ctx,
        style,
        font,
        shapes,
        labels: Vec::new(),
    };
    chrome.top_bar(view);
    chrome.tabs(view);
    for pane in view.panes {
        chrome.pane(pane, view.hover);
    }
    chrome.labels
}

struct Chrome<'a, 'b> {
    ctx: &'a mut DrawContext<'b>,
    style: UiStyle,
    font: &'a UiFont,
    shapes: &'a mut Vec<Shape>,
    labels: Vec<UiLabel>,
}

impl Chrome<'_, '_> {
    fn top_bar(&mut self, view: &ChromeView) {
        let top = view.top;
        if top.area.h <= 0.0 {
            return;
        }
        let s = self.style;
        self.panel(top.area, 0.0, s.bg1);
        self.rule(top.area);
        self.button(
            top.sidebar,
            Icon::Menu,
            view.hover == Some(ChromeAction::Sidebar),
            view.sidebar_visible,
            false,
        );
        self.panel(
            top.search,
            RADIUS_6,
            if view.hover == Some(ChromeAction::Search) {
                s.bg3
            } else {
                s.bg2
            },
        );
        let search_icon = self.leading_icon(top.search);
        Icon::Search.draw(search_icon, self.ctx.color(s.text_dim), self.shapes);
        let x = search_icon.right() + s.px(6.0);
        let room = (top.search.right() - s.px(8.0) - x).max(0.0);
        if room > s.px(10.0) {
            self.search_contents(
                Rect::new(x, top.search.y, room, top.search.h),
                view.language,
            );
        }
        if let (Some(rect), Some(diff)) = (top.diff, view.diff) {
            self.panel(
                rect,
                RADIUS_6,
                if view.hover == Some(ChromeAction::Diff) {
                    s.bg3
                } else {
                    s.bg2
                },
            );
            let icon = self.leading_icon(rect);
            Icon::Diff.draw(icon, self.ctx.color(s.text_dim), self.shapes);
            let x = icon.right() + s.px(3.0);
            let content = Rect::new(x, rect.y, (rect.right() - s.px(4.0) - x).max(0.0), rect.h);
            let label = diff.label();
            let counts = label
                .split_whitespace()
                .zip([s.ok, s.danger])
                .map(|(count, color)| {
                    (
                        self.font.shape(
                            self.ctx.font_system,
                            UiSize::Small,
                            UiWeight::Semibold,
                            s.scale,
                            count,
                        ),
                        color,
                    )
                })
                .collect::<Vec<_>>();
            let gap = s.px(4.0);
            let total = counts
                .iter()
                .map(|(buffer, _)| label_width(buffer))
                .sum::<f32>()
                + gap;
            if total <= content.w {
                let mut x = content.x + (content.w - total) / 2.0;
                for (buffer, color) in counts {
                    let width = label_width(&buffer).ceil();
                    let label = Rect::new(x, content.y, width, content.h);
                    self.labels
                        .push(UiLabel::new(buffer, label, UiAlign::Left, color));
                    x += width + gap;
                }
            } else {
                self.label(
                    content,
                    &label,
                    UiSize::Small,
                    UiWeight::Semibold,
                    UiAlign::Center,
                    s.text,
                );
            }
        }
        let rect = top.notifications;
        if let Some(count) = count_badge(view.unread) {
            if view.hover == Some(ChromeAction::Notifications) {
                self.panel(rect, RADIUS_6, s.bg3);
            }
            let icon = self.leading_icon(rect);
            Icon::Bell.draw(icon, self.ctx.color(s.text), self.shapes);
            let x = icon.right();
            let h = s.px(15.0).min(rect.h);
            let badge = Rect::new(
                x,
                rect.y + (rect.h - h) / 2.0,
                (rect.right() - x - s.px(1.0)).max(0.0),
                h,
            );
            let buffer = self.font.shape(
                self.ctx.font_system,
                UiSize::Small,
                UiWeight::Semibold,
                s.scale,
                &count,
            );
            if label_width(&buffer).ceil() <= badge.w {
                self.panel(badge, RADIUS_12, s.accent);
                self.labels.push(UiLabel::new(
                    buffer,
                    badge,
                    UiAlign::Center,
                    s.accent_text(),
                ));
            }
        } else {
            self.button(
                rect,
                Icon::Bell,
                view.hover == Some(ChromeAction::Notifications),
                false,
                false,
            );
        }
    }

    fn search_contents(&mut self, rect: Rect, language: UiLanguage) {
        let s = self.style;
        let en = self.font.shape(
            self.ctx.font_system,
            UiSize::Caption,
            UiWeight::Regular,
            s.scale,
            SEARCH_EN,
        );
        let ar = self.font.shape(
            self.ctx.font_system,
            UiSize::Caption,
            UiWeight::Regular,
            s.scale,
            SEARCH_AR,
        );
        let divider = self.font.shape(
            self.ctx.font_system,
            UiSize::Caption,
            UiWeight::Regular,
            s.scale,
            "/",
        );
        let key = self.font.shape(
            self.ctx.font_system,
            UiSize::Small,
            UiWeight::Regular,
            s.scale,
            SEARCH_KEYCAP,
        );
        let width = |buffer: &cosmic_text::Buffer| label_width(buffer).ceil() + 1.0;
        let gap = s.px(5.0);
        let key_w = width(&key) + 2.0 * gap;
        let bilingual = width(&en) + width(&divider) + width(&ar) + 2.0 * gap;
        let local_w = if language == UiLanguage::Arabic {
            width(&ar)
        } else {
            width(&en)
        };
        let choice = SearchContents::fit(rect.w, bilingual, local_w, key_w + gap);
        let mut remaining = rect;
        if choice.keycap {
            let h = s.px(18.0).min(rect.h);
            let key_rect = Rect::new(rect.right() - key_w, rect.y + (rect.h - h) / 2.0, key_w, h);
            self.panel(key_rect, RADIUS_6, s.bg3);
            self.labels
                .push(UiLabel::new(key, key_rect, UiAlign::Center, s.text_dim));
            remaining.w -= key_w + gap;
        }
        match choice.placeholder {
            SearchPlaceholder::Bilingual => {
                let mut x = rect.x;
                for buffer in [en, divider, ar] {
                    let w = width(&buffer);
                    let label = Rect::new(x, rect.y, w, rect.h);
                    self.labels
                        .push(UiLabel::new(buffer, label, UiAlign::Left, s.text_dim));
                    x += w + gap;
                }
            }
            SearchPlaceholder::Local => {
                let buffer = if language == UiLanguage::Arabic {
                    ar
                } else {
                    en
                };
                self.labels
                    .push(UiLabel::new(buffer, remaining, UiAlign::Start, s.text_dim));
            }
            SearchPlaceholder::LocalShortened => {
                self.word_label(
                    remaining,
                    language.text(SEARCH_EN, SEARCH_AR),
                    UiSize::Caption,
                    UiWeight::Regular,
                    s.text_dim,
                );
            }
        }
    }

    fn tabs(&mut self, view: &ChromeView) {
        let s = self.style;
        let layout = view.layout;
        self.panel(layout.tab_bar, 0.0, s.bg1);
        self.rule(layout.tab_bar);
        for (i, (&rect, tab)) in layout.tabs.iter().zip(view.tabs).enumerate() {
            let hovered = view.hover_tab == Some(i);
            self.panel(
                rect,
                RADIUS_12,
                if tab.active {
                    s.accent_tint(s.bg3)
                } else if hovered {
                    s.bg3
                } else {
                    s.bg2
                },
            );
            let close = layout.tab_close(rect);
            let pad = s.px(8.0).min(rect.w / 4.0);
            let text = Rect::new(
                rect.x + pad,
                rect.y,
                (close.x - rect.x - pad).max(0.0),
                rect.h,
            );
            self.word_label(
                text,
                &tab.title,
                UiSize::Body,
                UiWeight::Semibold,
                if tab.active || hovered {
                    s.text
                } else {
                    s.text_dim
                },
            );
            if hovered {
                let hot = view.hover == Some(ChromeAction::CloseTab(i));
                self.button(close, Icon::Close, hot, false, true);
            }
        }
        self.button(
            layout.new_tab,
            Icon::Plus,
            view.hover == Some(ChromeAction::NewTab),
            false,
            false,
        );
    }

    fn pane(&mut self, pane: &PaneHeaderView, hover: Option<ChromeAction>) {
        let s = self.style;
        let rect = pane.layout.header;
        let title = pane.layout.title;
        if rect.h > 0.0 {
            self.panel(rect, 0.0, if pane.focused { s.bg3 } else { s.bg2 });
            self.rule(rect);
            let icon = self.leading_icon(title);
            let rtl = pane
                .title
                .chars()
                .find_map(|c| match unicode_bidi::bidi_class(c) {
                    unicode_bidi::BidiClass::R | unicode_bidi::BidiClass::AL => Some(true),
                    unicode_bidi::BidiClass::L => Some(false),
                    _ => None,
                })
                .unwrap_or(false);
            let icon = if rtl {
                Rect::new(
                    title.right() - (icon.x - title.x) - icon.w,
                    icon.y,
                    icon.w,
                    icon.h,
                )
            } else {
                icon
            };
            let pictogram = if pane.agent {
                Icon::Agent
            } else {
                Icon::Terminal
            };
            pictogram.draw(icon, self.ctx.color(s.text_dim), self.shapes);
            let pad = s.px(6.0);
            let (left, right) = if rtl {
                (title.x + pad, icon.x - pad)
            } else {
                (icon.right() + pad, title.right() - pad)
            };
            let label = Rect::new(left, title.y, (right - left).max(0.0), title.h);
            self.label(
                label,
                &pane.title,
                UiSize::Body,
                UiWeight::Semibold,
                UiAlign::Start,
                s.text,
            );
            for (button, icon, action) in [
                (
                    pane.layout.split_right,
                    Icon::SplitRight,
                    ChromeAction::SplitRight(pane.id),
                ),
                (
                    pane.layout.split_down,
                    Icon::SplitDown,
                    ChromeAction::SplitDown(pane.id),
                ),
                (pane.layout.close, Icon::Close, ChromeAction::Close(pane.id)),
            ] {
                self.button(
                    button,
                    icon,
                    hover == Some(action),
                    false,
                    icon == Icon::Close,
                );
            }
        }
        if (rect.h > 0.0 && pane.focused) || pane.attention {
            let outer = Rect::new(rect.x, rect.y, rect.w, pane.layout.grid.bottom() - rect.y);
            self.shapes.push(Shape::RoundedRect {
                rect: outer,
                radii: [0.0; 4],
                fill: [0.0; 4],
                border: Some(Border {
                    width: s.px(if pane.attention { 2.0 } else { 1.0 }),
                    color: self.ctx.color(s.accent),
                }),
                shadow: None,
            });
        }
    }

    fn leading_icon(&self, rect: Rect) -> Rect {
        let pad = self.style.px(7.0).min(rect.w / 4.0);
        let side = self
            .style
            .px(16.0)
            .min((rect.w - 2.0 * pad).max(0.0))
            .min(rect.h);
        Rect::new(rect.x + pad, rect.y + (rect.h - side) / 2.0, side, side)
    }

    fn button(&mut self, rect: Rect, icon: Icon, hover: bool, active: bool, danger: bool) {
        let s = self.style;
        if hover || active {
            self.panel(
                rect,
                RADIUS_6,
                if hover { s.bg3 } else { s.accent_tint(s.bg1) },
            );
        }
        let side = s.px(16.0).min(rect.w).min(rect.h);
        let icon_rect = Rect::new(
            rect.x + (rect.w - side) / 2.0,
            rect.y + (rect.h - side) / 2.0,
            side,
            side,
        );
        let color = if hover && danger {
            s.danger
        } else if hover || active {
            s.text
        } else {
            s.text_dim
        };
        icon.draw(icon_rect, self.ctx.color(color), self.shapes);
    }

    fn panel(&mut self, rect: Rect, radius: f32, color: crate::color::Rgba) {
        if rect.w > 0.0 && rect.h > 0.0 {
            self.shapes.push(Shape::rounded(
                rect,
                self.style.px(radius),
                self.ctx.color(color),
            ));
        }
    }

    fn rule(&mut self, rect: Rect) {
        let h = self.style.px(1.0).min(rect.h);
        self.panel(
            Rect::new(rect.x, rect.bottom() - h, rect.w, h),
            0.0,
            self.style.border,
        );
    }

    fn word_label(
        &mut self,
        rect: Rect,
        text: &str,
        size: UiSize,
        weight: UiWeight,
        color: crate::color::Rgba,
    ) {
        if rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }
        let buffer = self.font.fit_words(
            self.ctx.font_system,
            size,
            weight,
            self.style.scale,
            text,
            (rect.w - 1.0).floor(),
        );
        if label_width(&buffer) > 0.0 {
            self.labels
                .push(UiLabel::new(buffer, rect, UiAlign::Start, color));
        }
    }

    fn label(
        &mut self,
        rect: Rect,
        text: &str,
        size: UiSize,
        weight: UiWeight,
        align: UiAlign,
        color: crate::color::Rgba,
    ) {
        if rect.w > 0.0 && rect.h > 0.0 {
            let buffer = self.font.fit(
                self.ctx.font_system,
                size,
                weight,
                self.style.scale,
                text,
                rect.w,
            );
            self.labels.push(UiLabel::new(buffer, rect, align, color));
        }
    }
}
