//! The command palette overlay: a panel near the top of the window with the query and the
//! matching items (commands, or notifications). Each row shows the English title and shortcut
//! on the left and the Arabic title on the right, each shaped on its own so neither language
//! reorders the other. When both do not fit, the wider one is cut with `…`.

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
    /// Shown while the query is empty; `None` for the commands' hint.
    pub placeholder: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PaletteItem {
    pub title_en: String,
    pub title_ar: String,
    pub shortcut: Option<String>,
    /// Listed but cannot be chosen (a notification from a closed pane).
    pub dimmed: bool,
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
) -> (Vec<OverlayText>, Rect) {
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
    let faint = palette.foreground.mix(palette.background, 0.65);
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
        let hint = view
            .placeholder
            .as_deref()
            .unwrap_or("Type a command… / اكتب أمر…");
        (format!("› {hint}"), dim)
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
        let mut english = ctx.label(&left);
        let mut arabic = ctx.label(&item.title_ar);
        // Long items (notification text) are cut so the two sides never overlap.
        let room = panel.w - 2.0 * pad - 2.0 * cw;
        let (en_w, ar_w) = (label_width(&english), label_width(&arabic));
        if en_w + ar_w > room {
            if en_w >= ar_w {
                english = fit(ctx, &left, (room - ar_w).max(room / 2.0));
            } else {
                arabic = fit(ctx, &item.title_ar, (room - en_w).max(room / 2.0));
            }
        }
        let (en_color, ar_color) = if item.dimmed {
            (faint, faint)
        } else if i == view.selected {
            (fg, fg)
        } else {
            (dim, fg)
        };
        texts.push(OverlayText {
            buffer: english,
            left: panel.x + pad,
            top,
            bounds,
            color: en_color.to_glyphon(),
        });
        let width = label_width(&arabic);
        texts.push(OverlayText {
            buffer: arabic,
            left: (panel.right() - pad - width).max(panel.x + pad),
            top,
            bounds,
            color: ar_color.to_glyphon(),
        });
    }
    let edge_rect = Rect::new(
        panel.x - edge,
        panel.y - edge,
        panel.w + 2.0 * edge,
        panel.h + 2.0 * edge,
    );
    (texts, edge_rect)
}

/// `text` shaped and cut at its end (by grapheme, with `…`) to at most `width`.
fn fit(ctx: &mut DrawContext, text: &str, width: f32) -> Buffer {
    use unicode_segmentation::UnicodeSegmentation;
    let mut cuts: Vec<usize> = text.grapheme_indices(true).map(|(i, _)| i).collect();
    cuts.push(text.len());
    let (mut low, mut high) = (0, cuts.len() - 1);
    let mut best = ctx.label("…");
    while low < high {
        let keep = (low + high).div_ceil(2);
        let candidate = ctx.label(&format!("{}…", text[..cuts[keep]].trim_end()));
        if label_width(&candidate) <= width {
            best = candidate;
            low = keep;
        } else {
            high = keep - 1;
        }
    }
    best
}

/// Width of a shaped one-line label.
pub fn label_width(buffer: &Buffer) -> f32 {
    buffer
        .layout_runs()
        .map(|run| run.line_w)
        .fold(0.0, f32::max)
}
