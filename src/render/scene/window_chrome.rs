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
    pub zoomed: bool,
    /// Outline flash intensity in `0.0..=1.0` (`pane.flash` / after notification jump).
    pub flash: f32,
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
            SearchPlaceholder::Hidden => {}
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
            self.label(
                text,
                &tab.title,
                UiSize::Body,
                UiWeight::Semibold,
                UiAlign::Start,
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
            let (mut left, mut right) = if rtl {
                (title.x + pad, icon.x - pad)
            } else {
                (icon.right() + pad, title.right() - pad)
            };
            if pane.zoomed && right - left > s.px(26.0) {
                let pill_h = s.px(18.0).min((title.h - s.px(4.0)).max(0.0));
                let pill_y = title.y + (title.h - pill_h) * 0.5;
                let pill_pad = s.px(5.0);
                let icon_side = s.px(12.0).min(pill_h);
                let full_text = "Zoom · مكبّرة";
                let short_text = "Zoom";
                let full_buf = self.font.shape(
                    self.ctx.font_system,
                    UiSize::Small,
                    UiWeight::Semibold,
                    s.scale,
                    full_text,
                );
                let short_buf = self.font.shape(
                    self.ctx.font_system,
                    UiSize::Small,
                    UiWeight::Semibold,
                    s.scale,
                    short_text,
                );
                let avail = (right - left - pad).max(0.0);
                let full_w = pill_pad * 2.0 + icon_side + s.px(4.0) + label_width(&full_buf).ceil();
                let short_w =
                    pill_pad * 2.0 + icon_side + s.px(4.0) + label_width(&short_buf).ceil();
                let icon_only_w = pill_pad * 2.0 + icon_side;
                let (pill_w, text_buf) = if full_w <= avail * 0.55 && full_w <= avail {
                    (full_w, Some(full_buf))
                } else if short_w <= avail * 0.55 && short_w <= avail {
                    (short_w, Some(short_buf))
                } else {
                    (icon_only_w.min(avail), None)
                };
                if pill_w >= icon_only_w * 0.8 && pill_h > 0.0 {
                    let pill_x = if rtl { left } else { right - pill_w };
                    let pill = Rect::new(pill_x, pill_y, pill_w, pill_h);
                    self.shapes.push(Shape::RoundedRect {
                        rect: pill,
                        radii: [s.px(RADIUS_6); 4],
                        fill: self.ctx.color(s.accent_tint(s.bg2)),
                        border: Some(Border {
                            width: s.px(1.0),
                            color: self.ctx.color(s.accent),
                        }),
                        shadow: None,
                    });
                    let z_icon = Rect::new(
                        pill.x + pill_pad,
                        pill.y + (pill.h - icon_side) * 0.5,
                        icon_side,
                        icon_side,
                    );
                    Icon::Zoom.draw(z_icon, self.ctx.color(s.text), self.shapes);
                    if let Some(buf) = text_buf {
                        let lx = z_icon.right() + s.px(4.0);
                        let label_rect =
                            Rect::new(lx, pill.y, (pill.right() - pill_pad - lx).max(0.0), pill.h);
                        self.labels
                            .push(UiLabel::new(buf, label_rect, UiAlign::Left, s.text));
                    }
                    if rtl {
                        left = pill.right() + pad;
                    } else {
                        right = pill.x - pad;
                    }
                }
            }
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
        let flash = pane.flash.clamp(0.0, 1.0);
        if (rect.h > 0.0 && pane.focused) || pane.attention || flash > 0.0 {
            let outer = Rect::new(rect.x, rect.y, rect.w, pane.layout.grid.bottom() - rect.y);
            if (rect.h > 0.0 && pane.focused) || pane.attention {
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
            if flash > 0.0 {
                let mut glow = s.accent.mix(s.text, 0.35 * flash);
                glow.a = (0.35 + 0.65 * flash).min(1.0);
                self.shapes.push(Shape::RoundedRect {
                    rect: outer,
                    radii: [0.0; 4],
                    fill: [0.0; 4],
                    border: Some(Border {
                        width: s.px(2.0 + 2.0 * flash),
                        color: self.ctx.color(glow),
                    }),
                    shadow: None,
                });
            }
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
