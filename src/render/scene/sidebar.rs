//! Workspace cards, with proportional headings and messages and a monospace footer.

use std::{collections::HashMap, path::Path, rc::Rc};

use cosmic_text::{Buffer, FontSystem};
use glyphon::TextBounds;
use unicode_segmentation::UnicodeSegmentation;

use super::context::DrawContext;
use crate::color::Rgba;
use crate::layout::Rect;
use crate::render::icons::Icon;
use crate::render::shapes::{Border, Shape};
use crate::render::style::Quad;
use crate::render::text::shaper::shape_label;
use crate::render::text::ui::{label_width, UiAlign, UiFont, UiSize, UiWeight};
use crate::render::ui_style::{UiStyle, RADIUS_12, RADIUS_6, RADIUS_8};
use crate::render::CellMetrics;
use crate::sidebar::cards::{
    wrap_notice, Card, CardLayout, Item, View, STATE_ICON_WIDTH, STATE_LABEL_PAD,
};
use crate::sidebar::{display_path, AgentState, Entry};
use crate::ui::chrome::{count_badge, UiLanguage};
use crate::ui::text::fit_text;

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
    wrap_width: (f32, f32),
    messages: HashMap<String, Vec<String>>,
    state_widths: HashMap<&'static str, f32>,
    title_ellipsis_width: f32,
}

impl LabelCache {
    pub fn cards(
        &mut self,
        font: &UiFont,
        fonts: &mut FontSystem,
        entries: Vec<Entry>,
        area: Rect,
        scale: f32,
    ) -> Vec<Card> {
        let width = CardLayout::message_width(area, scale);
        if self.wrap_width.1 != scale {
            self.state_widths.clear();
            let ellipsis = font.shape(fonts, UiSize::Body, UiWeight::Semibold, scale, "…");
            self.title_ellipsis_width = label_width(&ellipsis).ceil() + 1.0;
        }
        let mut previous = if self.wrap_width == (width, scale) {
            std::mem::take(&mut self.messages)
        } else {
            self.messages.clear();
            HashMap::new()
        };
        self.wrap_width = (width, scale);
        entries
            .into_iter()
            .map(|entry| {
                let message = entry.display_notice().map_or_else(Vec::new, |text| {
                    self.messages
                        .entry(text)
                        .or_insert_with_key(|text| {
                            previous.remove(text).unwrap_or_else(|| {
                                wrap_notice(text, |line| {
                                    let buffer = font.shape(
                                        fonts,
                                        UiSize::Caption,
                                        UiWeight::Regular,
                                        scale,
                                        line,
                                    );
                                    label_width(&buffer).ceil() + 1.0 <= width
                                })
                            })
                        })
                        .clone()
                });
                let mut card = Card::new(entry, message);
                if let Some(state) = card.entry.display_state() {
                    card.title_ellipsis_width = self.title_ellipsis_width;
                    card.state_label_widths =
                        [UiLanguage::English, UiLanguage::Arabic].map(|language| {
                            let text = state.label(language);
                            *self.state_widths.entry(text).or_insert_with(|| {
                                let label = font.shape(
                                    fonts,
                                    UiSize::Small,
                                    UiWeight::Semibold,
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
        self.messages.clear();
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
        Kind::Title => font.fit_words(
            ctx.font_system,
            UiSize::Body,
            UiWeight::Semibold,
            scale,
            text,
            (width - 1.0).floor(),
        ),
        Kind::State => font.shape(
            ctx.font_system,
            UiSize::Small,
            UiWeight::Semibold,
            scale,
            text,
        ),
        Kind::Message => font.shape(
            ctx.font_system,
            UiSize::Caption,
            UiWeight::Regular,
            scale,
            text,
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
            // Preserve the working-directory tail, eliding whole components when possible.
            for cuts in [&components, &graphemes] {
                for keep_home in [true, false]
                    .into_iter()
                    .filter(|&keep| !keep || text.starts_with("~/"))
                {
                    let candidate = |cut| path_tail(text, cut, keep_home);
                    let n = cuts.partition_point(|&cut| fits(&mono(&candidate(cut))));
                    if n > 0 {
                        return mono(&candidate(cuts[n - 1]));
                    }
                }
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
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let mut draw = Cards {
        scene: Scene {
            quads: vec![(area.x, area.y, area.w, area.h, ctx.color(style.bg1))],
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
        let hovered = self.view.hover == Some(item.index);
        let bg = if item.active {
            s.accent
        } else if hovered {
            s.bg3
        } else {
            s.bg2
        };
        let fg = if item.active { s.accent_text() } else { s.text };
        let dim = if item.active {
            fg.mix(bg, 0.17)
        } else {
            s.text_dim
        };
        let faint = if item.active {
            fg.mix(bg, 0.24)
        } else {
            s.text_dim.mix(bg, 0.12)
        };
        self.panel(item.rect, RADIUS_8, bg);
        if self.view.drop_target == Some(item.index) {
            if let Some(Shape::RoundedRect { border, .. }) = self.scene.shapes.last_mut() {
                *border = Some(Border {
                    width: s.px(1.0),
                    color: self.ctx.color(s.accent),
                });
            }
        }
        let entry = &item.card.entry;
        let parts = item.layout(self.view);
        let align = if self.view.mirrored {
            UiAlign::Right
        } else {
            UiAlign::Start
        };
        self.label(parts.title, Kind::Title, &entry.title, align, fg);
        if let Some(count) = count_badge(entry.attention_count) {
            let badge = center_height(parts.unread, s.px(16.0));
            let (fill, text) = if item.active {
                (fg, bg)
            } else {
                (s.accent, s.accent_text())
            };
            self.panel(badge, RADIUS_12, fill);
            self.label(badge, Kind::Small, &count, UiAlign::Center, text);
        }
        if let Some(state) = entry.display_state() {
            self.state(parts.state, parts.state_has_label, state, bg, item.active);
        }
        if hovered
            && parts.close.y >= self.view.area.y
            && parts.close.bottom() <= self.view.area.bottom()
        {
            if self.view.close_hover {
                self.panel(
                    center_height(parts.close, parts.close.w),
                    RADIUS_6,
                    bg.mix(fg, 0.14),
                );
            }
            self.icon(parts.close, Icon::Close, s.px(13.0), fg);
        }
        let lines = &item.card.message;
        for (i, line) in lines.iter().enumerate() {
            let h = parts.message.h / lines.len() as f32;
            let row = Rect::new(
                parts.message.x,
                parts.message.y + i as f32 * h,
                parts.message.w,
                h,
            );
            self.label(row, Kind::Message, line, align, dim);
        }
        self.footer(parts.footer, entry, home, faint);
    }

    fn state(&mut self, rect: Rect, with_text: bool, state: AgentState, bg: Rgba, selected: bool) {
        let s = self.style;
        let (icon, semantic) = match state {
            AgentState::Working => (None, s.text),
            AgentState::NeedsInput => (Some(Icon::Warning), s.warn),
            AgentState::Done => (Some(Icon::Check), s.ok),
            AgentState::Error => (Some(Icon::Close), s.danger),
        };
        let rect = center_height(rect, s.px(16.0));
        let surface = if selected { s.bg1 } else { bg };
        let fill = surface.mix(semantic, 0.17);
        let ink = semantic.mix(s.text, 0.28);
        self.panel(rect, RADIUS_12, fill);
        let glyph_width = s.px(STATE_ICON_WIDTH);
        if rect.w < glyph_width {
            return;
        }
        let mirror = self.view.mirrored;
        let pad = if with_text {
            s.px(STATE_LABEL_PAD)
        } else {
            (rect.w - glyph_width) / 2.0
        };
        let (glyph, _) = take_side(rect, pad, mirror);
        let (_, rest) = take_side(rect, glyph.w, mirror);
        let (glyph, caption) = take_side(rest, glyph_width, mirror);
        if let Some(icon) = icon {
            self.icon(glyph, icon, s.px(12.0), ink);
        } else if glyph.y >= self.view.area.y && glyph.bottom() <= self.view.area.bottom() {
            for i in 0..3 {
                let diameter = s.px(if i == self.view.dot_phase { 3.4 } else { 2.4 });
                let x = glyph.x + s.px(3.0 + 5.0 * i as f32);
                let y = glyph.y + glyph.h / 2.0;
                let dot = Rect::new(x - diameter / 2.0, y - diameter / 2.0, diameter, diameter);
                let color = if i == self.view.dot_phase {
                    ink
                } else {
                    ink.mix(fill, 0.50)
                };
                self.panel(dot, 4.0, color);
            }
        }
        if with_text {
            let (_, caption) = take_side(caption, s.px(STATE_LABEL_PAD), !mirror);
            self.label(
                caption,
                Kind::State,
                state.label(self.view.language),
                UiAlign::Center,
                ink,
            );
        }
    }

    fn footer(&mut self, rect: Rect, entry: &Entry, home: Option<&Path>, color: Rgba) {
        if rect.w <= 0.0
            || rect.h <= 0.0
            || rect.bottom() <= self.view.area.y
            || rect.y >= self.view.area.bottom()
        {
            return;
        }
        let s = self.style;
        let mirror = self.view.mirrored;
        let gap = s.px(4.0);
        let remote = entry.remote_state.map(|state| {
            use crate::remote::ConnectionState::*;
            match state {
                Connecting => self.view.language.text("Connecting", "جاري الاتصال"),
                Connected => self.view.language.text("Connected", "متّصل"),
                Disconnected => self.view.language.text("Disconnected", "الاتصال مقطوع"),
            }
        });
        let detail = remote
            .map(str::to_owned)
            .or_else(|| entry.cwd.as_deref().map(|p| display_path(p, home)));
        let mut rest = rect;
        let mut port_rect = None;
        if !entry.ports.is_empty() {
            let ports = crate::ports::format_ports(&entry.ports);
            let w = if entry.branch.is_none() && detail.is_none() {
                rect.w
            } else {
                s.px(7.0 * ports.len() as f32).min(rect.w * 0.38)
            };
            let (port, remainder) = take_side(rest, w, !mirror);
            rest = take_side(remainder, gap, !mirror).1;
            port_rect = Some(port);
        }
        if let Some(branch) = &entry.branch {
            let (icon, room) = take_side(rest, s.px(15.0), mirror);
            self.icon(icon, Icon::GitBranch, s.px(12.0), color);
            let width = if detail.is_some() {
                s.px(7.0 * branch.chars().count() as f32).min(room.w * 0.47)
            } else {
                room.w
            };
            let (label, remainder) = take_side(room, width, mirror);
            let align = if mirror {
                UiAlign::Right
            } else {
                UiAlign::Start
            };
            self.label(label, Kind::Footer, branch, align, color);
            rest = remainder;
            if detail.is_some() {
                let (dot, remainder) = take_side(rest, s.px(12.0), mirror);
                self.label(dot, Kind::Footer, "•", UiAlign::Center, color);
                rest = remainder;
            }
        }
        if let Some(detail) = detail {
            let align = if mirror {
                UiAlign::Right
            } else {
                UiAlign::Start
            };
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
        if let Some(port_rect) = port_rect {
            let align = if mirror {
                UiAlign::Left
            } else {
                UiAlign::Right
            };
            for limit in (1..=entry.ports.len().min(crate::ports::MAX_SHOWN)).rev() {
                let text = crate::ports::format_ports_limit(&entry.ports, limit);
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
        let fill = if self.view.new_tab_hover {
            s.bg3
        } else {
            s.bg1
        };
        self.panel(rect, RADIUS_8, fill);
        let mirror = self.view.mirrored;
        let (_, inner) = take_side(rect, s.px(8.0), mirror);
        let (icon, rest) = take_side(inner, s.px(16.0), mirror);
        self.icon(icon, Icon::Plus, s.px(14.0), s.text_dim);
        let (_, title) = take_side(rest, s.px(4.0), mirror);
        let (_, title) = take_side(title, s.px(8.0), !mirror);
        let align = if mirror {
            UiAlign::Right
        } else {
            UiAlign::Start
        };
        self.label(
            title,
            Kind::Title,
            self.view.language.text("New tab", "تاب جديد"),
            align,
            s.text_dim,
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
        let clip = intersect(row, self.view.area);
        if clip.w <= 0.0 || clip.h <= 0.0 || text.is_empty() {
            return;
        }
        let buffer = self.cache.get(self.ctx, self.font, kind, text, row.w);
        let room = (row.w - label_width(&buffer)).max(0.0);
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
