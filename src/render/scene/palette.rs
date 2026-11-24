//! The command palette overlay: a panel near the top of the window with the query and the
//! matching commands. Each row shows the English title and shortcut on the left and the
//! Arabic title on the right, each shaped on its own so neither language reorders the other.

use cosmic_text::Buffer;
use glyphon::TextBounds;

use super::context::DrawContext;
use crate::layout::Rect;
use crate::render::style::Quad;

/// What the palette shows; built by the app from `ui::palette`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PaletteView {
    pub query: String,
    /// Title (English), Arabic title, shortcut.
    pub items: Vec<PaletteItem>,
    /// Index into `items`.
    pub selected: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaletteItem {
    pub title_en: String,
    pub title_ar: String,
    pub shortcut: Option<String>,
}

/// One shaped label of the overlay and where it goes.
pub(crate) struct OverlayText {
    pub buffer: Buffer,
    pub left: f32,
    pub top: f32,
    pub bounds: TextBounds,
    pub color: glyphon::Color,
}

/// Panel geometry in a `width` x `height` window: the panel and its first row's top.
pub fn panel_rect(
    char_width: f32,
    line_height: f32,
    (width, height): (f32, f32),
    rows: usize,
) -> Rect {
    let w = (72.0 * char_width)
        .min(width - 2.0 * char_width)
        .max(char_width * 10.0);
    let x = ((width - w) / 2.0).max(0.0);
    let y = (line_height * 2.0).min((height / 4.0).max(0.0));
    let h = line_height * (rows as f32 + 1.0) + line_height * 0.5;
    Rect::new(x, y, w, h.min((height - y).max(line_height)))
}

/// Quads (appended to `quads`) and labels of the palette.
pub(crate) fn palette_overlay(
    ctx: &mut DrawContext,
    view: &PaletteView,
    surface: (f32, f32),
    quads: &mut Vec<Quad>,
) -> Vec<OverlayText> {
    let m = ctx.metrics;
    let (cw, lh) = (m.char_width, m.line_height);
    let panel = panel_rect(cw, lh, surface, view.items.len().max(1));
    let palette = ctx.palette;
    let panel_bg = palette.background.mix(palette.foreground, 0.12);
    let border = palette.background.mix(palette.foreground, 0.35);
    let edge = (m.scale).round().max(1.0);
    quads.push((
        panel.x - edge,
        panel.y - edge,
        panel.w + 2.0 * edge,
        panel.h + 2.0 * edge,
        ctx.color(border),
    ));
    quads.push((panel.x, panel.y, panel.w, panel.h, ctx.color(panel_bg)));

    let fg = palette.foreground;
    let dim = palette.foreground.mix(palette.background, 0.45);
    let bounds = TextBounds {
        left: panel.x as i32,
        top: panel.y as i32,
        right: panel.right() as i32,
        bottom: panel.bottom() as i32,
    };
    let pad = cw;
    let mut texts = Vec::new();

    // Query line, with a prompt and a hint when empty.
    let query_y = panel.y + lh * 0.25;
    let (query, query_color) = if view.query.is_empty() {
        ("› اكتب أمر… / Type a command…".to_string(), dim)
    } else {
        (format!("› {}", view.query), fg)
    };
    texts.push(OverlayText {
        buffer: ctx.label(&query),
        left: panel.x + pad,
        top: query_y,
        bounds,
        color: query_color.to_glyphon(),
    });
    let rule_y = query_y + lh;
    quads.push((panel.x, rule_y, panel.w, edge, ctx.color(border)));

    for (i, item) in view.items.iter().enumerate() {
        let top = rule_y + edge + i as f32 * lh;
        if top + lh > panel.bottom() + 0.5 {
            break;
        }
        if i == view.selected {
            let sel = panel_bg.mix(palette.selection, 0.7);
            quads.push((panel.x, top, panel.w, lh, ctx.color(sel)));
        }
        let left = match &item.shortcut {
            Some(keys) => format!("{}   {}", item.title_en, keys),
            None => item.title_en.clone(),
        };
        texts.push(OverlayText {
            buffer: ctx.label(&left),
            left: panel.x + pad,
            top,
            bounds,
            color: if i == view.selected { fg } else { dim }.to_glyphon(),
        });
        let arabic = ctx.label(&item.title_ar);
        let width = label_width(&arabic);
        texts.push(OverlayText {
            buffer: arabic,
            left: (panel.right() - pad - width).max(panel.x + pad),
            top,
            bounds,
            color: fg.to_glyphon(),
        });
    }
    texts
}

/// Width of a shaped one-line label.
pub fn label_width(buffer: &Buffer) -> f32 {
    buffer
        .layout_runs()
        .map(|run| run.line_w)
        .fold(0.0, f32::max)
}
