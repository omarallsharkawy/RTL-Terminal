//! Workspace sidebar chrome. All program text goes through the same shaper as tab titles.

use std::{collections::HashMap, rc::Rc};

use cosmic_text::Buffer;
use glyphon::TextBounds;
use unicode_bidi::{bidi_class, BidiClass};
use unicode_segmentation::UnicodeSegmentation;

use super::context::DrawContext;
use crate::color::Rgba;
use crate::layout::Rect;
use crate::render::style::Quad;
use crate::render::CellMetrics;
use crate::sidebar::{display_path, AgentState, View};

pub struct Label {
    pub buffer: Rc<Buffer>,
    pub left: f32,
    pub top: f32,
    pub bounds: TextBounds,
    pub color: glyphon::Color,
}

pub struct Scene {
    pub quads: Vec<Quad>,
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

fn bounds(rect: Rect) -> TextBounds {
    TextBounds {
        left: rect.x.ceil() as i32,
        top: rect.y.ceil() as i32,
        right: rect.right().floor() as i32,
        bottom: rect.bottom().floor() as i32,
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Trim {
    Start,
    End,
}

#[derive(Clone)]
struct ShapedLabel {
    buffer: Rc<Buffer>,
    width: f32,
}

impl ShapedLabel {
    fn shape(ctx: &mut DrawContext, text: &str) -> Self {
        let buffer = ctx.label(text);
        let width = buffer.layout_runs().map(|r| r.line_w).fold(0.0, f32::max);
        Self {
            buffer: Rc::new(buffer),
            width,
        }
    }

    fn place(self, row: Rect, clip: Rect, right: bool, color: Rgba) -> Label {
        Label {
            buffer: self.buffer,
            left: if right {
                row.right() - self.width
            } else {
                row.x
            },
            top: row.y,
            bounds: bounds(intersect(row, clip)),
            color: color.to_glyphon(),
        }
    }
}

#[derive(Hash, PartialEq, Eq)]
struct LabelKey {
    text: String,
    width: u32,
    trim: Trim,
}

/// Keep the shaped labels used by the previous frame. Scroll offsets and colors do not
/// affect shaping, and labels that leave the viewport are dropped at the end of the frame.
#[derive(Default)]
pub struct LabelCache {
    font: Option<(CellMetrics, Option<&'static str>)>,
    labels: HashMap<LabelKey, (ShapedLabel, bool)>,
}

impl LabelCache {
    fn begin_frame(&mut self, ctx: &DrawContext) {
        let font = Some((ctx.metrics, ctx.family));
        if self.font != font {
            self.labels.clear();
            self.font = font;
        }
        for (_, used) in self.labels.values_mut() {
            *used = false;
        }
    }

    fn get(&mut self, ctx: &mut DrawContext, text: String, width: f32, trim: Trim) -> ShapedLabel {
        let (label, used) = self
            .labels
            .entry(LabelKey {
                text,
                width: width.to_bits(),
                trim,
            })
            .or_insert_with_key(|key| (fit_label(ctx, &key.text, width, trim), true));
        *used = true;
        label.clone()
    }

    pub fn clear(&mut self) {
        self.labels.clear();
    }

    pub fn len(&self) -> usize {
        self.labels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }
}

fn rtl(text: &str) -> bool {
    text.chars()
        .find_map(|c| match bidi_class(c) {
            BidiClass::L => Some(false),
            BidiClass::R | BidiClass::AL => Some(true),
            _ => None,
        })
        .unwrap_or(false)
}

fn fit_label(ctx: &mut DrawContext, text: &str, width: f32, trim: Trim) -> ShapedLabel {
    let full = ShapedLabel::shape(ctx, text);
    if full.width <= width {
        return full;
    }
    let mut best = ShapedLabel::shape(ctx, "…");
    if best.width > width {
        return ShapedLabel::shape(ctx, "");
    }
    let mut cuts: Vec<_> = text.grapheme_indices(true).map(|(i, _)| i).collect();
    cuts.push(text.len());
    let count = cuts.len() - 1;
    let rtl_base = rtl(text);
    let (mut low, mut high) = (1, count);
    while low < high {
        let keep = (low + high) / 2;
        let kept = match trim {
            Trim::Start => &text[cuts[count - keep]..],
            Trim::End => &text[..cuts[keep]],
        };
        // Preserve the original paragraph direction if removing the first strong character
        // changes it. The mark is renderer-generated and needs no matching control character.
        let mark = match (rtl_base, rtl(kept)) {
            (true, false) => "\u{200f}",
            (false, true) => "\u{200e}",
            _ => "",
        };
        let candidate = match trim {
            Trim::Start => format!("{mark}…{kept}"),
            Trim::End => format!("{mark}{kept}…"),
        };
        let candidate = ShapedLabel::shape(ctx, &candidate);
        if candidate.width <= width {
            best = candidate;
            low = keep + 1;
        } else {
            high = keep;
        }
    }
    best
}

pub fn build(ctx: &mut DrawContext, view: &View, cache: &mut LabelCache) -> Scene {
    cache.begin_frame(ctx);
    let m = ctx.metrics;
    let pad = (8.0 * m.scale).round();
    let fg = ctx.palette.foreground;
    let bg = ctx.palette.background;
    let muted = fg.mix(bg, 0.3);
    let accent = ctx.palette.ansi[4];
    let area = view.area;
    let mut scene = Scene {
        quads: vec![(area.x, area.y, area.w, area.h, ctx.color(bg.mix(fg, 0.05)))],
        labels: Vec::new(),
    };
    let home = std::env::var_os("HOME")
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from);
    for item in &view.items {
        let rect = item.rect;
        let clip = intersect(rect, area);
        if item.active {
            scene
                .quads
                .push((clip.x, clip.y, clip.w, clip.h, ctx.color(bg.mix(fg, 0.12))));
            let stroke = m.stroke().min(rect.w);
            let x = if view.right_aligned {
                rect.x
            } else {
                rect.right() - stroke
            };
            scene
                .quads
                .push((x, clip.y, stroke, clip.h, ctx.color(accent)));
        }
        let inner = Rect::new(
            rect.x + pad,
            rect.y + m.line_height * 0.25,
            (rect.w - pad * 2.0).max(0.0),
            m.line_height,
        );
        let entry = &item.entry;
        let title = if entry.attention_count > 0 {
            format!("● {} {}", entry.attention_count, entry.title)
        } else {
            entry.title.clone()
        };
        scene
            .labels
            .push(cache.get(ctx, title, inner.w, Trim::End).place(
                inner,
                clip,
                view.right_aligned,
                if entry.attention_count > 0 {
                    accent
                } else {
                    fg
                },
            ));
        let mut row = inner;
        row.y += m.line_height;
        if let Some(notice_text) = entry.display_notice() {
            let notice_color = if entry.notification.is_some() {
                accent
            } else {
                match entry.agent_status.as_ref().map(|s| s.state) {
                    Some(AgentState::NeedsInput) => ctx.palette.ansi[3],
                    Some(AgentState::Done) => ctx.palette.ansi[2],
                    _ => accent,
                }
            };
            scene
                .labels
                .push(cache.get(ctx, notice_text, row.w, Trim::End).place(
                    row,
                    clip,
                    view.right_aligned,
                    notice_color,
                ));
            row.y += m.line_height;
        }
        let cwd = entry
            .cwd
            .as_deref()
            .map(|p| display_path(p, home.as_deref()))
            .unwrap_or_else(|| "—".into());
        scene
            .labels
            .push(cache.get(ctx, cwd, row.w, Trim::Start).place(
                row,
                clip,
                view.right_aligned,
                muted,
            ));
        row.y += m.line_height;

        // Reserve state space first so a long branch cannot hide running/exit indicators.
        let mut states = Vec::new();
        if let Some(agent) = &entry.agent_status {
            let color = match agent.state {
                AgentState::Working => accent,
                AgentState::NeedsInput => ctx.palette.ansi[3],
                AgentState::Done => ctx.palette.ansi[2],
            };
            states.push((agent.state.badge().to_string(), color));
        } else if entry.running {
            states.push(("●".to_string(), accent));
        }
        if let Some(exit) = entry.last_exit {
            states.push(if exit == 0 {
                ("✓".to_string(), ctx.palette.ansi[2])
            } else {
                (format!("✗ {exit}"), ctx.palette.ansi[1])
            });
        }
        let mut remaining = row.w;
        let mut states: Vec<_> = states
            .into_iter()
            .rev()
            .filter_map(|(text, color)| {
                let label = cache.get(ctx, text, remaining, Trim::End);
                if label.width == 0.0 {
                    return None;
                }
                remaining = (remaining - label.width - m.char_width).max(0.0);
                Some((label, color))
            })
            .collect();
        if let Some(branch) = &entry.branch {
            let branch = cache.get(ctx, format!("⎇ {branch}"), remaining, Trim::End);
            if branch.width > 0.0 {
                states.push((branch, muted));
            }
        }
        let total_width = states.iter().map(|(label, _)| label.width).sum::<f32>()
            + states.len().saturating_sub(1) as f32 * m.char_width;
        let mut x = if view.right_aligned {
            row.right() - total_width
        } else {
            row.x
        };
        for (label, color) in states.into_iter().rev() {
            let left = x;
            x += label.width + m.char_width;
            let mut placed = label.place(row, clip, false, color);
            placed.left = left;
            scene.labels.push(placed);
        }
        if !entry.ports.is_empty() {
            row.y += m.line_height;
            let ports = crate::ports::format_ports(&entry.ports);
            scene
                .labels
                .push(cache.get(ctx, ports, row.w, Trim::End).place(
                    row,
                    clip,
                    view.right_aligned,
                    ctx.palette.ansi[6],
                ));
        }
    }
    cache.labels.retain(|_, (_, used)| *used);
    scene
        .labels
        .retain(|l| l.bounds.left < l.bounds.right && l.bounds.top < l.bounds.bottom);
    scene
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
