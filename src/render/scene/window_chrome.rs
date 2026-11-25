//! The toolbar, tab strip and pane headers. All controls share geometry with pointer input.

use crate::layout::{PaneId, Rect};
use crate::render::icons::Icon;
use crate::render::shapes::{Border, Shape};
use crate::render::text::ui::{label_width, UiAlign, UiFont, UiLabel, UiSize, UiWeight};
use crate::render::ui_style::{UiStyle, RADIUS_6};
use crate::render::TabLabel;
use crate::ui::chrome::{ChromeAction, PaneHeaderLayout, TopBarLayout, WindowLayout};
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
            let ar = "دوّر في الجلسات والوكلاء والملفات…";
            let en_room = (room - s.px(8.0)).max(0.0) / 2.0;
            let en = Rect::new(x, top.search.y, en_room, top.search.h);
            let ar_rect = Rect::new(
                top.search.right() - s.px(8.0) - en_room,
                top.search.y,
                en_room,
                top.search.h,
            );
            self.label(
                en,
                "Search sessions, agents, files…",
                UiSize::Caption,
                UiWeight::Regular,
                UiAlign::Left,
                s.text_dim,
            );
            self.label(
                ar_rect,
                ar,
                UiSize::Caption,
                UiWeight::Regular,
                UiAlign::Right,
                s.text_dim,
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
        if view.unread == 0 {
            self.button(
                top.notifications,
                Icon::Bell,
                view.hover == Some(ChromeAction::Notifications),
                false,
                false,
            );
        } else {
            let rect = top.notifications;
            self.panel(
                rect,
                RADIUS_6,
                if view.hover == Some(ChromeAction::Notifications) {
                    s.bg3
                } else {
                    s.accent_tint(s.bg1)
                },
            );
            let icon = self.leading_icon(rect);
            Icon::Bell.draw(icon, self.ctx.color(s.text), self.shapes);
            let x = icon.right() + s.px(1.0);
            let count = Rect::new(x, rect.y, (rect.right() - s.px(3.0) - x).max(0.0), rect.h);
            self.label(
                count,
                &view.unread.to_string(),
                UiSize::Small,
                UiWeight::Semibold,
                UiAlign::Center,
                s.text,
            );
        }
    }

    fn tabs(&mut self, view: &ChromeView) {
        let s = self.style;
        self.panel(view.layout.tab_bar, 0.0, s.bg1);
        for (&rect, tab) in view.layout.tabs.iter().zip(view.tabs) {
            if tab.active {
                self.panel(rect, 0.0, s.bg2);
                self.panel(
                    Rect::new(rect.x, rect.y, rect.w, s.px(1.0).min(rect.h)),
                    0.0,
                    s.accent,
                );
            }
            let pad = s.px(8.0);
            let text = Rect::new(rect.x + pad, rect.y, (rect.w - 2.0 * pad).max(0.0), rect.h);
            self.label(
                text,
                &tab.title,
                UiSize::Body,
                UiWeight::Semibold,
                UiAlign::Start,
                if tab.active { s.text } else { s.text_dim },
            );
        }
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
