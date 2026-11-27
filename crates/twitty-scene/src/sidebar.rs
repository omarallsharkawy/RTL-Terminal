//! Workspace cards, with proportional headings and messages and a monospace footer.

use std::{collections::HashMap, path::Path, rc::Rc};

use cosmic_text::{Buffer, FontSystem};
use glyphon::TextBounds;
use unicode_segmentation::UnicodeSegmentation;

use super::context::DrawContext;
use super::frame::bounds_around;
use twitty_core::color::Rgba;
use twitty_core::layout::{Rect, SidebarPosition};
use twitty_render::icons::Icon;
use twitty_render::metrics::CellMetrics;
use twitty_render::shapes::{Border, Shape};
use twitty_render::style::Quad;
use twitty_render::text::fit_text;
use twitty_render::text::shaper::shape_label;
use twitty_render::text::ui::{label_width, UiAlign, UiFont, UiSize, UiWeight};
use twitty_render::ui_style::{UiStyle, RADIUS_6, RADIUS_8, SPACE_12, SPACE_16, SPACE_4};
use twitty_ui::sidebar::cards::{fit_title, Card, Item, View, STATE_ICON_WIDTH, STATE_LABEL_PAD};
use twitty_ui::sidebar::{display_path, AgentState, Entry};
use twitty_ui::ui::chrome::{count_badge, UiLanguage};
use twitty_ui::ui::motion::working_dot;

#[derive(Clone)]
pub struct Label {
    pub buffer: Rc<Buffer>,
    pub left: f32,
    pub top: f32,
    pub bounds: TextBounds,
    pub color: glyphon::Color,
}

pub struct Scene {
    pub quads: Vec<Quad>,
    pub shapes: Vec<Shape>,
    pub labels: Vec<Label>,
}

fn intersect(a: Rect, b: Rect) -> Rect {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    Rect::new(
        x,
        y,
        (a.right().min(b.right()) - x).max(0.0),
        (a.bottom().min(b.bottom()) - y).max(0.0),
    )
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum Kind {
    Title,
    Message,
    Small,
    State,
    Footer,
    Path,
    Ports,
}

#[derive(Hash, PartialEq, Eq)]
struct LabelKey {
    text: String,
    width: u32,
    kind: Kind,
}

/// Reuse shaping as cards move or animate. Only labels drawn in the last frame are retained.
#[derive(Default)]
pub struct LabelCache {
    metrics: Option<(CellMetrics, Option<&'static str>)>,
    labels: HashMap<LabelKey, (Rc<Buffer>, bool)>,
    state_scale: f32,
    state_widths: HashMap<&'static str, f32>,
}

impl LabelCache {
    pub fn cards(
        &mut self,
        font: &UiFont,
        fonts: &mut FontSystem,
        entries: Vec<Entry>,
        _area: Rect,
        scale: f32,
    ) -> Vec<Card> {
        if self.state_scale != scale {
            self.state_widths.clear();
            self.state_scale = scale;
        }
        entries
            .into_iter()
            .map(|entry| {
                let mut card = Card::new(entry);
                if let Some(state) = card.entry.display_state() {
                    card.state_label_widths =
                        [UiLanguage::English, UiLanguage::Arabic].map(|language| {
                            let text = state.label(language);
                            *self.state_widths.entry(text).or_insert_with(|| {
                                let label = font.shape(
                                    fonts,
                                    UiSize::Small,
                                    UiWeight::Regular,
                                    scale,
                                    text,
                                );
                                label_width(&label).ceil() + 1.0
                            })
                        });
                }
                card
            })
            .collect()
    }

    fn begin_frame(&mut self, ctx: &DrawContext) {
        let metrics = Some((ctx.metrics, ctx.family));
        if self.metrics != metrics {
            self.labels.clear();
            self.metrics = metrics;
        }
        for (_, drawn) in self.labels.values_mut() {
            *drawn = false;
        }
    }

    fn get(
        &mut self,
        ctx: &mut DrawContext,
        font: &UiFont,
        kind: Kind,
        text: &str,
        width: f32,
    ) -> Rc<Buffer> {
        let (buffer, drawn) = self
            .labels
            .entry(LabelKey {
                text: text.to_owned(),
                width: width.to_bits(),
                kind,
            })
            .or_insert_with(|| (Rc::new(shape(ctx, font, kind, text, width)), true));
        *drawn = true;
        Rc::clone(buffer)
    }

    pub fn clear(&mut self) {
        self.labels.clear();
        self.state_widths.clear();
    }

    pub fn len(&self) -> usize {
        self.labels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }
}

fn shape(ctx: &mut DrawContext, font: &UiFont, kind: Kind, text: &str, width: f32) -> Buffer {
    let scale = ctx.metrics.scale;
    match kind {
        Kind::Title => {
            let mut shape = |text: &str| {
                font.shape(
                    ctx.font_system,
                    UiSize::Body,
                    UiWeight::Semibold,
                    scale,
                    text,
                )
            };
            let fitted = fit_title(text, |candidate| {
                label_width(&shape(candidate)).ceil() + 1.0 <= width
            });
            shape(&fitted)
        }
        Kind::State => font.shape(
            ctx.font_system,
            UiSize::Small,
            UiWeight::Regular,
            scale,
            text,
        ),
        Kind::Message => font.fit_words(
            ctx.font_system,
            UiSize::Caption,
            UiWeight::Regular,
            scale,
            text,
            (width - 1.0).floor(),
        ),
        Kind::Small => font.fit(
            ctx.font_system,
            UiSize::Small,
            UiWeight::Semibold,
            scale,
            text,
            (width - 1.0).max(0.0),
        ),
        Kind::Footer | Kind::Path | Kind::Ports => {
            let size = UiSize::Small.px(scale);
            let family = ctx.family;
            let mut mono = |text: &str| {
                shape_label(ctx.font_system, family, (size, (size * 1.5).ceil()), text)
            };
            let fits = |buffer: &Buffer| label_width(buffer).ceil() + 1.0 <= width;
            if kind == Kind::Footer {
                let fitted = fit_text(text, |text| fits(&mono(text)));
                return mono(&fitted);
            }
            let full = mono(text);
            if fits(&full) {
                return full;
            }
            if kind == Kind::Ports {
                return mono("");
            }
            let ellipsis = mono("…");
            if !fits(&ellipsis) {
                let tail = mono(text.graphemes(true).next_back().unwrap_or(""));
                return if fits(&tail) { tail } else { mono("") };
            }
            let components: Vec<_> = text
                .rmatch_indices(['/', '\\'])
                .map(|(i, _)| i)
                .filter(|&i| i > 0 && i + 1 < text.len())
                .collect();
            let graphemes: Vec<_> = text
                .grapheme_indices(true)
                .rev()
                .map(|(i, _)| i)
                .take_while(|&i| i > 0)
                .collect();
            // Keep the current directory recognizable as the width shrinks.
            let tail = |cuts: &[usize], mono: &mut dyn FnMut(&str) -> Buffer| {
                for keep_home in [true, false]
                    .into_iter()
                    .filter(|&keep| !keep || text.starts_with("~/"))
                {
                    let candidate = |cut| path_tail(text, cut, keep_home);
                    let n = cuts.partition_point(|&cut| fits(&mono(&candidate(cut))));
                    if n > 0 {
                        return Some(mono(&candidate(cuts[n - 1])));
                    }
                }
                None
            };
            if let Some(path) = tail(&components, &mut mono) {
                return path;
            }
            if let Some((_, name)) = text.rsplit_once(['/', '\\']) {
                let name = mono(name);
                if fits(&name) {
                    return name;
                }
            }
            if let Some(path) = tail(&graphemes, &mut mono) {
                return path;
            }
            ellipsis
        }
    }
}

fn path_tail(text: &str, cut: usize, home_prefix: bool) -> String {
    let prefix = if home_prefix && cut >= 2 {
        "~/…"
    } else {
        "…"
    };
    format!("{prefix}{}", &text[cut..])
}

pub fn build(
    ctx: &mut DrawContext,
    view: &View,
    cache: &mut LabelCache,
    style: UiStyle,
    font: &UiFont,
) -> Scene {
    cache.begin_frame(ctx);
    let area = view.area;
    let rule_w = style.hairline().min(area.w);
    let rule_x = if view.position == SidebarPosition::Left {
        area.right() - rule_w
    } else {
        area.x
    };
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let mut draw = Cards {
        scene: Scene {
            quads: vec![
                (area.x, area.y, area.w, area.h, ctx.color(style.bg1)),
                (rule_x, area.y, rule_w, area.h, ctx.color(style.border)),
            ],
            shapes: Vec::new(),
            labels: Vec::new(),
        },
        view,
        cache,
        ctx,
        style,
        font,
    };
    for item in &view.items {
        draw.card(item, home.as_deref());
    }
    draw.new_tab();
    draw.cache.labels.retain(|_, (_, drawn)| *drawn);
    draw.scene
}

struct Cards<'a, 'b> {
    scene: Scene,
    view: &'a View,
    cache: &'a mut LabelCache,
    ctx: &'a mut DrawContext<'b>,
    style: UiStyle,
    font: &'a UiFont,
}

impl Cards<'_, '_> {
    fn card(&mut self, item: &Item, home: Option<&Path>) {
        let s = self.style;
        let (hovered, selected) = self
            .view
            .highlights
            .as_ref()
            .and_then(|mix| mix.get(item.index))
            .copied()
            .unwrap_or((
                f32::from(self.view.hover == Some(item.index)),
                f32::from(item.active),
            ));
        let drop = self.view.drop_target == Some(item.index);
        if self.view.reordering {
            let clip = intersect(item.rect, self.view.area);
            self.scene.labels = self
                .scene
                .labels
                .drain(..)
                .flat_map(|label| {
                    bounds_around(label.bounds, clip)
                        .into_iter()
                        .map(move |bounds| Label {
                            bounds,
                            ..label.clone()
                        })
                })
                .collect();
        }
        if (selected > 0.0 || hovered > 0.0 || drop || self.view.reordering)
            && intersect(item.rect, self.view.area).w > 0.0
        {
            let fill = s
                .bg1
                .mix(s.bg2, if drop { 1.0 } else { hovered })
                .mix(s.bg3.mix(s.text, 0.025 * hovered), selected);
            self.panel(item.rect, RADIUS_8, fill);
            if drop {
                if let Some(Shape::RoundedRect { border, .. }) = self.scene.shapes.last_mut() {
                    *border = Some(Border {
                        width: s.hairline(),
                        color: self.ctx.color(s.accent),
                    });
                }
            }
        }
        let entry = &item.card.entry;
        let parts = item.layout(self.view);
        if selected > 0.0 {
            let w = s.px(2.0).round().min(item.rect.w);
            let inset = s.px(SPACE_4);
            let x = if self.view.mirrored {
                item.rect.right() - inset - w
            } else {
                item.rect.x + inset
            };
            let marker = Rect::new(x, parts.title.y, w, parts.title.h);
            let mut accent = s.accent;
            accent.a *= selected;
            self.panel(center_height(marker, s.px(SPACE_12)), RADIUS_6, accent);
        }
        let title = entry.title.trim();
        let title = if title.is_empty() || matches!(title, "…" | "...") {
            self.view.language.t("Tab")
        } else {
            title
        };
        self.title(parts.title, parts.unread, title, entry.attention_count);
        self.footer(parts.footer, entry, home, s.text_dim);
        if let Some(state) = entry.display_state() {
            self.state(parts.state, parts.state_has_label, state);
        }
        if hovered > 0.0
            && parts.close.y >= self.view.area.y
            && parts.close.bottom() <= self.view.area.bottom()
        {
            let close = self
                .view
                .close_mixes
                .get(item.index)
                .copied()
                .unwrap_or(f32::from(self.view.close_hover));
            if close > 0.0 {
                let mut fill = s.bg3;
                fill.a *= close * hovered;
                self.panel(center_height(parts.close, parts.close.w), RADIUS_6, fill);
            }
            let mut color = s.text_dim.mix(s.text, close);
            color.a *= hovered;
            self.icon(parts.close, Icon::Close, s.px(SPACE_12), color);
        }
        if let Some(message) = &item.card.message {
            self.label(
                parts.message,
                Kind::Message,
                message,
                self.leading_align(),
                s.text_dim,
            );
        }
    }

    fn title(&mut self, row: Rect, badge: Rect, title: &str, unread: usize) {
        let s = self.style;
        let buffer = self
            .cache
            .get(self.ctx, self.font, Kind::Title, title, row.w);
        let text_width = label_width(&buffer).min(row.w);
        self.placed_label(row, buffer, self.leading_align(), s.text);
        if let Some(count) = count_badge(unread).filter(|_| badge.w > 0.0) {
            let gap = s.px(SPACE_4);
            // Move the reserved badge toward the measured title, on the card's trailing side.
            let x = if self.view.mirrored {
                (row.right() - text_width - gap - badge.w).max(badge.x)
            } else {
                (row.x + text_width + gap).min(badge.x)
            };
            let badge = center_height(Rect { x, ..badge }, s.px(SPACE_16));
            self.panel(badge, RADIUS_6, s.bg1.mix(s.accent, 0.16));
            self.label(badge, Kind::Small, &count, UiAlign::Center, s.text);
        }
    }

    fn state(&mut self, rect: Rect, with_text: bool, state: AgentState) {
        let s = self.style;
        let (icon, ink) = match state {
            AgentState::Working => (None, s.accent),
            AgentState::NeedsInput => (Some(Icon::Warning), s.warn),
            AgentState::Done => (Some(Icon::Check), s.ok),
            AgentState::Error => (Some(Icon::Close), s.danger),
        };
        let glyph_width = s.px(STATE_ICON_WIDTH);
        if rect.w < glyph_width {
            return;
        }
        let mirror = self.view.mirrored;
        let (glyph, rest) = take_side(rect, glyph_width, mirror);
        if let Some(icon) = icon {
            self.icon(glyph, icon, s.px(SPACE_12), ink);
        } else if glyph.y >= self.view.area.y && glyph.bottom() <= self.view.area.bottom() {
            for i in 0..3 {
                let strength = working_dot(
                    self.view.dot_progress.unwrap_or(self.view.dot_phase as f32),
                    i,
                );
                let diameter = s.px(2.0 + strength);
                let offset = s.px(2.0 + SPACE_4 * i as f32);
                let x = if mirror {
                    glyph.right() - offset
                } else {
                    glyph.x + offset
                };
                let y = glyph.y + glyph.h / 2.0;
                let dot = Rect::new(x - diameter / 2.0, y - diameter / 2.0, diameter, diameter);
                let color = s.text_faint.mix(ink, strength);
                self.panel(dot, RADIUS_6, color);
            }
        }
        if with_text {
            let (_, caption) = take_side(rest, s.px(STATE_LABEL_PAD), mirror);
            self.label(
                caption,
                Kind::State,
                state.label(self.view.language),
                if mirror {
                    UiAlign::Left
                } else {
                    UiAlign::Right
                },
                s.text_dim.mix(ink, 0.24),
            );
        }
    }

    fn leading_align(&self) -> UiAlign {
        if self.view.mirrored {
            UiAlign::Right
        } else {
            UiAlign::Left
        }
    }

    fn footer(&mut self, rect: Rect, entry: &Entry, home: Option<&Path>, color: Rgba) {
        let align = self.leading_align();
        if rect.w <= 0.0
            || rect.h <= 0.0
            || rect.bottom() <= self.view.area.y
            || rect.y >= self.view.area.bottom()
        {
            return;
        }
        let s = self.style;
        let mirror = self.view.mirrored;
        let gap = s.px(SPACE_4);
        let remote = entry.remote_state.map(|state| {
            use twitty_ui::remote::ConnectionState::*;
            match state {
                Connecting => self.view.language.t("Connecting"),
                Connected => self.view.language.t("Connected"),
                Disconnected => self.view.language.t("Disconnected"),
            }
        });
        let detail = remote
            .map(str::to_owned)
            .or_else(|| entry.cwd.as_deref().map(|p| display_path(p, home)));
        let mut rest = rect;
        let mut port_rect = None;
        if !entry.ports.is_empty() {
            let ports = twitty_ui::ports::format_ports(&entry.ports);
            let w = if entry.branch.is_none() && detail.is_none() {
                rect.w
            } else {
                s.px(7.0 * ports.len() as f32).min(rect.w * 0.46)
            };
            let (port, remainder) = take_side(rest, w, !mirror);
            rest = take_side(remainder, gap, !mirror).1;
            port_rect = Some(port);
        }
        let mut branch_rect = None;
        let mut dot_rect = None;
        if let Some(branch) = &entry.branch {
            if detail.is_some() {
                // Keep the branch visible while giving the path the larger share. Short paths and
                // branches only take up their measured width, keeping the separator next to them.
                let separator = s.px(SPACE_12);
                let branch_full =
                    self.cache
                        .get(self.ctx, self.font, Kind::Footer, branch, f32::INFINITY);
                let branch_w = (label_width(&branch_full).ceil() + 2.0).min(rest.w * 0.43);
                let path = detail.as_deref().unwrap_or_default();
                let path_full =
                    self.cache
                        .get(self.ctx, self.font, Kind::Path, path, f32::INFINITY);
                let path_w = (label_width(&path_full).ceil() + 2.0)
                    .min((rest.w - branch_w - separator).max(0.0));
                if path_w >= s.px(SPACE_12) && branch_w >= s.px(SPACE_12) {
                    let (_, after_path) = take_side(rest, path_w, mirror);
                    let (dot, after_dot) = take_side(after_path, separator, mirror);
                    branch_rect = Some(take_side(after_dot, branch_w, mirror).0);
                    dot_rect = Some(dot);
                    rest = take_side(rest, path_w, mirror).0;
                }
            } else {
                branch_rect = Some(rest);
            }
        }
        if let Some(detail) = detail {
            self.label(
                rest,
                if remote.is_some() {
                    Kind::Footer
                } else {
                    Kind::Path
                },
                &detail,
                align,
                color,
            );
        }
        if let Some(dot) = dot_rect {
            self.label(dot, Kind::Footer, "·", UiAlign::Center, color);
        }
        if let (Some(branch), Some(label)) = (&entry.branch, branch_rect) {
            self.label(label, Kind::Footer, branch, align, color);
        }
        if let Some(port_rect) = port_rect {
            let align = if mirror {
                UiAlign::Left
            } else {
                UiAlign::Right
            };
            for limit in (1..=entry.ports.len().min(twitty_ui::ports::MAX_SHOWN)).rev() {
                let text = twitty_ui::ports::format_ports_limit(&entry.ports, limit);
                let result = self
                    .cache
                    .get(self.ctx, self.font, Kind::Ports, &text, port_rect.w);
                if label_width(&result) > 0.0 {
                    self.label(port_rect, Kind::Ports, &text, align, color);
                    return;
                }
            }
            if let Some(port) = entry.ports.iter().min() {
                self.label(port_rect, Kind::Ports, &format!(":{port}"), align, color);
            }
        }
    }

    fn new_tab(&mut self) {
        let s = self.style;
        let rect = self.view.new_tab;
        let hover = self
            .view
            .new_tab_mix
            .unwrap_or(f32::from(self.view.new_tab_hover));
        let fill = s.bg2.mix(s.bg3, hover);
        self.panel(rect, RADIUS_8, fill);
        let mirror = self.view.mirrored;
        let (_, inner) = take_side(rect, s.px(SPACE_12), mirror);
        let (icon, rest) = take_side(inner, s.px(SPACE_12), mirror);
        let ink = s.text_dim.mix(s.text, hover);
        self.icon(icon, Icon::Plus, s.px(SPACE_12), ink);
        let (_, title) = take_side(rest, s.px(SPACE_4), mirror);
        let (_, title) = take_side(title, s.px(SPACE_12), !mirror);
        self.label(
            title,
            Kind::Title,
            self.view.language.t("New tab"),
            self.leading_align(),
            ink,
        );
    }

    fn panel(&mut self, rect: Rect, radius: f32, color: Rgba) {
        let area = intersect(rect, self.view.area);
        if area.w <= 0.0 || area.h <= 0.0 {
            return;
        }
        let r = self.style.px(radius);
        let top = if rect.y < area.y { 0.0 } else { r };
        let bottom = if rect.bottom() > area.bottom() {
            0.0
        } else {
            r
        };
        self.scene.shapes.push(Shape::RoundedRect {
            rect: area,
            radii: [top, top, bottom, bottom],
            fill: self.ctx.color(color),
            border: None,
            shadow: None,
        });
    }

    fn icon(&mut self, rect: Rect, icon: Icon, size: f32, color: Rgba) {
        let side = size.min(rect.w).min(rect.h);
        let rect = Rect::new(
            rect.x + (rect.w - side) / 2.0,
            rect.y + (rect.h - side) / 2.0,
            side,
            side,
        );
        if rect.y >= self.view.area.y && rect.bottom() <= self.view.area.bottom() {
            icon.draw(rect, self.ctx.color(color), &mut self.scene.shapes);
        }
    }

    fn label(&mut self, row: Rect, kind: Kind, text: &str, align: UiAlign, color: Rgba) {
        if text.is_empty() {
            return;
        }
        let buffer = self.cache.get(self.ctx, self.font, kind, text, row.w);
        self.placed_label(row, buffer, align, color);
    }

    fn placed_label(&mut self, row: Rect, buffer: Rc<Buffer>, align: UiAlign, color: Rgba) {
        let clip = intersect(row, self.view.area);
        if clip.w <= 0.0 || clip.h <= 0.0 {
            return;
        }
        let width = label_width(&buffer);
        if width <= 0.0 {
            return;
        }
        let room = (row.w - width).max(0.0);
        let rtl = buffer.layout_runs().next().is_some_and(|run| run.rtl);
        let x = match align {
            UiAlign::Start if rtl => room,
            UiAlign::Right => room,
            UiAlign::Center => room / 2.0,
            _ => 0.0,
        };
        let top = row.y + (row.h - buffer.metrics().line_height) / 2.0;
        let bounds = TextBounds {
            left: clip.x.ceil() as i32,
            top: clip.y.ceil() as i32,
            right: clip.right().floor() as i32,
            bottom: clip.bottom().floor() as i32,
        };
        if bounds.left < bounds.right && bounds.top < bounds.bottom {
            self.scene.labels.push(Label {
                buffer,
                left: (row.x + x).round(),
                top: top.round(),
                bounds,
                color: color.to_glyphon(),
            });
        }
    }
}

fn take_side(rect: Rect, width: f32, right: bool) -> (Rect, Rect) {
    let width = width.min(rect.w);
    let rest = rect.w - width;
    if right {
        (
            Rect::new(rect.x + rest, rect.y, width, rect.h),
            Rect::new(rect.x, rect.y, rest, rect.h),
        )
    } else {
        (
            Rect::new(rect.x, rect.y, width, rect.h),
            Rect::new(rect.x + width, rect.y, rest, rect.h),
        )
    }
}

fn center_height(rect: Rect, h: f32) -> Rect {
    Rect::new(rect.x, rect.y + (rect.h - h) / 2.0, rect.w, h)
}

/// Attention is visual only; it does not affect the terminal's logical grid or cell mapping.
pub fn attention_quads(ctx: &DrawContext, rects: &[Rect]) -> Vec<Quad> {
    let color = ctx.color(ctx.palette.ansi[4]);
    let mut out = Vec::new();
    for r in rects {
        let t = ctx.metrics.stroke().min(r.w / 2.0).min(r.h / 2.0).max(0.0);
        if t <= 0.0 {
            continue;
        }
        out.extend([
            (r.x, r.y, r.w, t, color),
            (r.x, r.bottom() - t, r.w, t, color),
            (r.x, r.y, t, r.h, color),
            (r.right() - t, r.y, t, r.h, color),
        ]);
    }
    out
}
