//! The toolbar, tab strip and pane headers. All controls share geometry with pointer input.

use crate::layout::{PaneId, Rect};
use crate::render::icons::Icon;
use crate::render::shapes::{Border, Shape};
use crate::render::text::labels::UiLabels;
use crate::render::text::ui::{label_width, UiAlign, UiLabel, UiSize, UiWeight};
use crate::render::ui_style::{UiStyle, RADIUS_6, RADIUS_8, SPACE_12, SPACE_16, SPACE_4, SPACE_8};
use crate::render::TabLabel;
use crate::ui::chrome::{
    count_badge, ChromeAction, PaneHeaderLayout, SearchContents, SearchPlaceholder, TopBarLayout,
    UiLanguage, WindowLayout, SEARCH_AR, SEARCH_EN, SEARCH_KEYCAP,
};
use crate::ui::git_diff::DiffStat;
use crate::ui::motion::ChromeMotion;

use super::context::DrawContext;

pub struct PaneHeaderView {
    pub id: PaneId,
    pub layout: PaneHeaderLayout,
    pub title: String,
    pub agent: bool,
    pub focused: bool,
    pub attention: bool,
    /// True when an agent card or hook on this pane is waiting for user approval ("Needs input").
    pub needs_input: bool,
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
    font: UiLabels<'_>,
    view: &ChromeView,
    shapes: &mut Vec<Shape>,
    motion: Option<&ChromeMotion<'_>>,
) -> Vec<UiLabel> {
    let mut chrome = Chrome {
        ctx,
        style,
        font,
        shapes,
        labels: Vec::new(),
        motion,
    };
    chrome.top_bar(view);
    chrome.tabs(view);
    for pane in view.panes {
        chrome.pane(pane, view.hover, view.language);
    }
    chrome.labels
}

struct Chrome<'a, 'b> {
    ctx: &'a mut DrawContext<'b>,
    style: UiStyle,
    font: UiLabels<'a>,
    shapes: &'a mut Vec<Shape>,
    labels: Vec<UiLabel>,
    motion: Option<&'a ChromeMotion<'a>>,
}

impl Chrome<'_, '_> {
    fn hover(&self, current: Option<ChromeAction>, action: ChromeAction) -> f32 {
        self.motion
            .map_or(f32::from(current == Some(action)), |motion| {
                motion.actions.value(action, motion.now)
            })
    }

    fn top_bar(&mut self, view: &ChromeView) {
        let top = view.top;
        if top.area.h <= 0.0 {
            return;
        }
        let s = self.style;
        self.panel(top.area, 0.0, s.bg1);
        self.rule(top.area, s.border);
        self.button(
            top.sidebar,
            Icon::Menu,
            self.hover(view.hover, ChromeAction::Sidebar),
            view.sidebar_visible,
            false,
        );
        self.panel(
            top.search,
            RADIUS_6,
            s.bg2
                .mix(s.bg3, self.hover(view.hover, ChromeAction::Search)),
        );
        let search_icon = self.leading_icon(top.search);
        let search_text = self.after_icon(top.search, search_icon);
        Icon::Search.draw(
            view.language.mirror(search_icon, top.search),
            self.ctx.color(s.text_dim),
            self.shapes,
        );
        if search_text.w >= s.px(SPACE_12) {
            self.search_contents(view.language.mirror(search_text, top.search), view.language);
        }
        if let (Some(rect), Some(diff)) = (top.diff, view.diff) {
            self.panel(
                rect,
                RADIUS_6,
                s.bg2.mix(s.bg3, self.hover(view.hover, ChromeAction::Diff)),
            );
            let icon = self.leading_icon(rect);
            Icon::Diff.draw(
                view.language.mirror(icon, rect),
                self.ctx.color(s.text_dim),
                self.shapes,
            );
            let content = view.language.mirror(self.after_icon(rect, icon), rect);
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
                .map(|(buffer, _)| label_width(buffer).ceil())
                .sum::<f32>()
                + gap;
            if total <= content.w {
                let mut x = content.x + (content.w - total) / 2.0;
                for (buffer, color) in counts {
                    let width = label_width(&buffer).ceil();
                    let label = Rect::new(x, content.y, width, content.h);
                    self.labels.push(UiLabel::new(
                        buffer,
                        view.language.mirror(label, content),
                        UiAlign::Left,
                        color,
                    ));
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
        let hot = self.hover(view.hover, ChromeAction::Notifications);
        if let Some(count) = count_badge(view.unread) {
            let buffer = self.font.shape(
                self.ctx.font_system,
                UiSize::Small,
                UiWeight::Semibold,
                s.scale,
                &count,
            );
            let gap = s.px(SPACE_4);
            let side = s.px(SPACE_16).min(rect.h);
            let text_width = label_width(&buffer).ceil();
            let width = (text_width + gap).max(s.px(SPACE_16));
            if side + width + gap <= rect.w {
                if hot > 0.0 {
                    self.panel(rect, RADIUS_6, s.bg3.mix(s.bg1, 1.0 - hot));
                }
                let x = rect.right() - width - side - gap;
                let icon = Rect::new(x, rect.y + (rect.h - side) / 2.0, side, side);
                Icon::Bell.draw(
                    view.language.mirror(icon, rect),
                    self.ctx.color(s.text),
                    self.shapes,
                );
                let h = s.px(SPACE_16).min(rect.h);
                let badge = Rect::new(icon.right() + gap, rect.y + (rect.h - h) / 2.0, width, h);
                let badge = view.language.mirror(badge, rect);
                self.panel(badge, RADIUS_6, s.bg1.mix(s.accent, 0.2));
                self.labels
                    .push(UiLabel::new(buffer, badge, UiAlign::Center, s.text));
                return;
            }
        }
        let w = s.px(32.0).min(rect.w);
        let icon_button = Rect::new(rect.right() - w, rect.y, w, rect.h);
        self.button(
            view.language.mirror(icon_button, rect),
            Icon::Bell,
            hot,
            view.unread > 0,
            false,
        );
    }

    fn search_contents(&mut self, rect: Rect, language: UiLanguage) {
        let s = self.style;
        let local = self.font.shape(
            self.ctx.font_system,
            UiSize::Caption,
            UiWeight::Regular,
            s.scale,
            language.text(SEARCH_EN, SEARCH_AR),
        );
        let key = self.font.shape(
            self.ctx.font_system,
            UiSize::Small,
            UiWeight::Regular,
            s.scale,
            SEARCH_KEYCAP,
        );
        let width = |buffer: &cosmic_text::Buffer| label_width(buffer).ceil() + 1.0;
        let gap = s.px(SPACE_4);
        let key_w = width(&key) + 2.0 * gap;
        let choice = SearchContents::fit(rect.w, width(&local), key_w + gap);
        let mut remaining = rect;
        if choice.keycap {
            let h = s.px(SPACE_16).min(rect.h);
            let key_rect = Rect::new(rect.right() - key_w, rect.y + (rect.h - h) / 2.0, key_w, h);
            let key_rect = language.mirror(key_rect, rect);
            self.panel(key_rect, RADIUS_6, s.bg3);
            self.labels
                .push(UiLabel::new(key, key_rect, UiAlign::Center, s.text_dim));
            remaining.w -= key_w + gap;
        }
        match choice.placeholder {
            SearchPlaceholder::Local => self.labels.push(UiLabel::new(
                local,
                language.mirror(remaining, rect),
                UiAlign::Start,
                s.text_dim,
            )),
            SearchPlaceholder::LocalShortened => self.word_label(
                language.mirror(remaining, rect),
                language.text(SEARCH_EN, SEARCH_AR),
                UiSize::Caption,
                UiWeight::Regular,
                UiAlign::Start,
                s.text_dim,
            ),
        }
    }

    fn tabs(&mut self, view: &ChromeView) {
        let s = self.style;
        let layout = view.layout;
        self.panel(layout.tab_bar, 0.0, s.bg1);
        self.rule(layout.tab_bar, s.border);
        for (i, (&rect, tab)) in layout.tabs.iter().zip(view.tabs).enumerate() {
            let (hovered, selected) = self
                .motion
                .and_then(|motion| {
                    let &id = motion.tab_ids.get(i)?;
                    Some((
                        motion.tab_hover.value(id, motion.now),
                        motion.selection.value(id, motion.now),
                    ))
                })
                .unwrap_or((f32::from(view.hover_tab == Some(i)), f32::from(tab.active)));
            let emphasis = selected + (1.0 - selected) * 0.65 * hovered;
            self.panel(rect, RADIUS_8, s.bg2.mix(s.bg3, emphasis));
            let close = layout.tab_close(rect);
            let pad = s.px(SPACE_8).min(rect.w / 4.0);
            let canonical_close = view.language.mirror(close, rect);
            let text = view.language.mirror(
                Rect::new(
                    rect.x + pad,
                    rect.y,
                    (canonical_close.x - rect.x - pad).max(0.0),
                    rect.h,
                ),
                rect,
            );
            self.word_label(
                text,
                &tab.title,
                UiSize::Body,
                UiWeight::Semibold,
                if view.language == UiLanguage::Arabic {
                    UiAlign::Right
                } else {
                    UiAlign::Start
                },
                s.text_dim.mix(s.text, emphasis),
            );
            if hovered > 0.0 {
                let hot = self.hover(view.hover, ChromeAction::CloseTab(i));
                self.button_with_opacity(close, Icon::Close, hot, false, true, hovered);
            }
        }
        self.button(
            layout.new_tab,
            Icon::Plus,
            self.hover(view.hover, ChromeAction::NewTab),
            false,
            false,
        );
    }

    fn pane(&mut self, pane: &PaneHeaderView, hover: Option<ChromeAction>, language: UiLanguage) {
        let s = self.style;
        let rect = pane.layout.header;
        let title = pane.layout.title;
        if rect.h > 0.0 {
            self.panel(rect, 0.0, if pane.focused { s.bg2 } else { s.bg1 });
            self.rule(
                rect,
                if pane.focused {
                    s.bg2.mix(s.accent, 0.64)
                } else {
                    s.border
                },
            );
            let icon = self.leading_icon(title);
            let rtl = language == UiLanguage::Arabic
                || pane
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
            let pictogram = if self.motion.is_some_and(|motion| motion.broadcast) {
                Icon::Broadcast
            } else if pane.agent {
                Icon::Agent
            } else {
                Icon::Terminal
            };
            pictogram.draw(
                icon,
                self.ctx.color(if pane.focused {
                    s.text_dim
                } else {
                    s.text_faint
                }),
                self.shapes,
            );
            let pad = s.px(SPACE_4);
            let (mut left, mut right) = if rtl {
                (title.x + pad, icon.x - pad)
            } else {
                (icon.right() + pad, title.right() - pad)
            };
            if pane.needs_input && right - left > s.px(24.0) {
                let pill_h = s.px(SPACE_16).min((title.h - s.px(SPACE_4)).max(0.0));
                let pill_y = title.y + (title.h - pill_h) * 0.5;
                let pill_pad = s.px(SPACE_4);
                let icon_side = s.px(SPACE_12).min(pill_h);
                let text = language.text("Needs input", "بانتظار ردك");
                let text_buf = self.font.shape(
                    self.ctx.font_system,
                    UiSize::Small,
                    UiWeight::Semibold,
                    s.scale,
                    text,
                );
                let avail = (right - left - pad).max(0.0);
                let full_w = pill_pad * 2.0 + icon_side + s.px(4.0) + label_width(&text_buf).ceil();
                let icon_only_w = pill_pad * 2.0 + icon_side;
                let (pill_w, show_buf) = if full_w <= avail * 0.60 && full_w <= avail {
                    (full_w, Some(text_buf))
                } else {
                    (icon_only_w.min(avail), None)
                };
                if pill_w >= icon_only_w * 0.8 && pill_h > 0.0 {
                    let pill_x = if rtl { left } else { right - pill_w };
                    let pill = Rect::new(pill_x, pill_y, pill_w, pill_h);
                    self.shapes.push(Shape::RoundedRect {
                        rect: pill,
                        radii: [s.px(RADIUS_6); 4],
                        fill: self.ctx.color(s.bg2.mix(s.warn, 0.22)),
                        border: Some(Border {
                            width: s.hairline(),
                            color: self.ctx.color(s.warn),
                        }),
                        shadow: None,
                    });
                    let w_icon = Rect::new(
                        pill.x + pill_pad,
                        pill.y + (pill.h - icon_side) * 0.5,
                        icon_side,
                        icon_side,
                    );
                    let direction = if rtl {
                        UiLanguage::Arabic
                    } else {
                        UiLanguage::English
                    };
                    Icon::Warning.draw(
                        direction.mirror(w_icon, pill),
                        self.ctx.color(s.warn),
                        self.shapes,
                    );
                    if let Some(buf) = show_buf {
                        let lx = w_icon.right() + s.px(SPACE_4);
                        let label_rect =
                            Rect::new(lx, pill.y, (pill.right() - pill_pad - lx).max(0.0), pill.h);
                        self.labels.push(UiLabel::new(
                            buf,
                            direction.mirror(label_rect, pill),
                            if rtl { UiAlign::Right } else { UiAlign::Left },
                            s.warn.mix(s.text, 0.30),
                        ));
                    }
                    if rtl {
                        left = pill.right() + pad;
                    } else {
                        right = pill.x - pad;
                    }
                }
            }
            if pane.zoomed && right - left > s.px(24.0) {
                let pill_h = s.px(SPACE_16).min((title.h - s.px(SPACE_4)).max(0.0));
                let pill_y = title.y + (title.h - pill_h) * 0.5;
                let pill_pad = s.px(SPACE_4);
                let icon_side = s.px(SPACE_12).min(pill_h);
                let full_text = language.text("Zoom · مكبّرة", "مكبّرة · Zoom");
                let short_text = language.text("Zoom", "مكبّرة");
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
                            width: s.hairline(),
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
                    let direction = if rtl {
                        UiLanguage::Arabic
                    } else {
                        UiLanguage::English
                    };
                    Icon::Zoom.draw(
                        direction.mirror(z_icon, pill),
                        self.ctx.color(s.text),
                        self.shapes,
                    );
                    if let Some(buf) = text_buf {
                        let lx = z_icon.right() + s.px(SPACE_4);
                        let label_rect =
                            Rect::new(lx, pill.y, (pill.right() - pill_pad - lx).max(0.0), pill.h);
                        self.labels.push(UiLabel::new(
                            buf,
                            direction.mirror(label_rect, pill),
                            if rtl { UiAlign::Right } else { UiAlign::Left },
                            s.text,
                        ));
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
                UiSize::Caption,
                UiWeight::Semibold,
                if rtl { UiAlign::Right } else { UiAlign::Left },
                if pane.focused { s.text } else { s.text_dim },
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
                    self.hover(hover, action),
                    false,
                    icon == Icon::Close,
                );
            }
        }
        let flash = pane.flash.clamp(0.0, 1.0);
        if pane.attention || flash > 0.0 {
            let outer = Rect::new(rect.x, rect.y, rect.w, pane.layout.grid.bottom() - rect.y);
            if pane.attention {
                self.shapes.push(Shape::RoundedRect {
                    rect: outer,
                    radii: [0.0; 4],
                    fill: [0.0; 4],
                    border: Some(Border {
                        width: s.hairline(),
                        color: self.ctx.color(s.accent),
                    }),
                    shadow: None,
                });
            }
            if flash > 0.0 {
                let mut glow = s.accent.mix(s.text, 0.35 * flash);
                glow.a = flash;
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
        let pad = self.style.px(SPACE_8).min(rect.w / 4.0);
        let side = self
            .style
            .px(16.0)
            .min((rect.w - 2.0 * pad).max(0.0))
            .min(rect.h);
        Rect::new(rect.x + pad, rect.y + (rect.h - side) / 2.0, side, side)
    }

    fn after_icon(&self, rect: Rect, icon: Rect) -> Rect {
        let x = icon.right() + self.style.px(SPACE_4);
        Rect::new(
            x,
            rect.y,
            (rect.right() - self.style.px(SPACE_8) - x).max(0.0),
            rect.h,
        )
    }

    fn button(&mut self, rect: Rect, icon: Icon, hover: f32, active: bool, danger: bool) {
        self.button_with_opacity(rect, icon, hover, active, danger, 1.0);
    }

    fn button_with_opacity(
        &mut self,
        rect: Rect,
        icon: Icon,
        hover: f32,
        active: bool,
        danger: bool,
        opacity: f32,
    ) {
        let s = self.style;
        if hover > 0.0 {
            let mut fill = s.bg3;
            fill.a *= hover * opacity;
            self.panel(rect, RADIUS_6, fill);
        }
        let side = s.px(16.0).min(rect.w).min(rect.h);
        let icon_rect = Rect::new(
            rect.x + (rect.w - side) / 2.0,
            rect.y + (rect.h - side) / 2.0,
            side,
            side,
        );
        let base = if active { s.text } else { s.text_faint };
        let mut color = base.mix(if danger { s.danger } else { s.text }, hover);
        color.a *= opacity;
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

    fn rule(&mut self, rect: Rect, color: crate::color::Rgba) {
        let h = self.style.hairline().min(rect.h);
        self.panel(Rect::new(rect.x, rect.bottom() - h, rect.w, h), 0.0, color);
    }

    fn word_label(
        &mut self,
        rect: Rect,
        text: &str,
        size: UiSize,
        weight: UiWeight,
        align: UiAlign,
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
            self.labels.push(UiLabel::new(buffer, rect, align, color));
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
