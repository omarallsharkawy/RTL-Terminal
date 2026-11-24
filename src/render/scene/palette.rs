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

/// Result of hit-testing a pointer press against an open command palette panel.
///
/// Clicking outside the panel closes the palette and consumes the click so nothing reaches
/// the panes or sidebar behind it; clicking an item row activates that item; any other click
/// inside the panel is consumed without closing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteHit {
    Outside,
    Inside,
    Item(usize),
}

/// Panel geometry in a `width` x `height` window, clamped inside the surface.
pub fn panel_rect(
    char_width: f32,
    line_height: f32,
    (width, height): (f32, f32),
    rows: usize,
) -> Rect {
    let max_w = width.max(0.0);
    let max_h = height.max(0.0);
    let w = (72.0 * char_width)
        .min((max_w - 2.0 * char_width).max(0.0))
        .max(char_width * 10.0)
        .min(max_w);
    let x = ((max_w - w) / 2.0).max(0.0);
    let y = (line_height * 2.0).min(max_h / 4.0).max(0.0);
    let h = line_height * (rows as f32 + 1.0) + line_height * 0.5;
    Rect::new(x, y, w, h.min((max_h - y).max(0.0)))
}

/// Hit-test physical pixel `(x, y)` against the command palette panel.
pub fn hit_test(
    panel: Rect,
    line_height: f32,
    edge: f32,
    item_count: usize,
    x: f64,
    y: f64,
) -> PaletteHit {
    let outer = Rect::new(
        (panel.x - edge).max(0.0),
        (panel.y - edge).max(0.0),
        panel.w + 2.0 * edge,
        panel.h + 2.0 * edge,
    );
    if !outer.contains(x, y) {
        return PaletteHit::Outside;
    }
    let rule_y = panel.y + line_height * 1.25;
    let first_row = rule_y + edge;
    if y >= first_row as f64 && y < panel.bottom() as f64 && line_height > 0.0 {
        let index = ((y as f32 - first_row) / line_height).floor() as usize;
        let top = first_row + index as f32 * line_height;
        if index < item_count && top + line_height <= panel.bottom() + 0.5 {
            return PaletteHit::Item(index);
        }
    }
    PaletteHit::Inside
}

/// Quads (appended to `quads`), clamped outer border rect (for clipping underlying text),
/// and labels of the palette.
pub(crate) fn palette_overlay(
    ctx: &mut DrawContext,
    view: &PaletteView,
    surface: (f32, f32),
    quads: &mut Vec<Quad>,
) -> (Option<Rect>, Vec<OverlayText>) {
    let (sw, sh) = (surface.0.max(0.0), surface.1.max(0.0));
    let m = ctx.metrics;
    let (cw, lh) = (m.char_width, m.line_height);
    let panel = panel_rect(cw, lh, (sw, sh), view.items.len().max(1));
    if sw <= 0.0 || sh <= 0.0 || panel.w <= 0.0 || panel.h <= 0.0 {
        return (None, Vec::new());
    }
    let palette = ctx.palette;
    let panel_bg = palette.background.mix(palette.foreground, 0.12);
    let border = palette.background.mix(palette.foreground, 0.35);
    let edge = (m.scale).round().max(1.0);
    let bx = (panel.x - edge).max(0.0);
    let by = (panel.y - edge).max(0.0);
    let br = (panel.right() + edge).min(sw);
    let bb = (panel.bottom() + edge).min(sh);
    let outer = Rect::new(bx, by, (br - bx).max(0.0), (bb - by).max(0.0));
    quads.push((outer.x, outer.y, outer.w, outer.h, ctx.color(border)));
    quads.push((panel.x, panel.y, panel.w, panel.h, ctx.color(panel_bg)));

    let fg = palette.foreground;
    let dim = palette.foreground.mix(palette.background, 0.45);
    let bounds = TextBounds {
        left: panel.x.ceil() as i32,
        top: panel.y.ceil() as i32,
        right: panel.right().floor() as i32,
        bottom: panel.bottom().floor() as i32,
    };
    let pad = cw;
    let mut texts = Vec::new();

    // Query line, with a prompt and a hint when empty.
    let query_y = panel.y + lh * 0.25;
    let (query, query_color) = if view.query.is_empty() {
        ("› Type a command… / اكتب أمر…".to_string(), dim)
    } else {
        (format!("› {}", view.query), fg)
    };
    if bounds.left < bounds.right && bounds.top < bounds.bottom {
        texts.push(OverlayText {
            buffer: ctx.label(&query),
            left: panel.x + pad,
            top: query_y,
            bounds,
            color: query_color.to_glyphon(),
        });
    }
    let rule_y = query_y + lh;
    if rule_y < panel.bottom() {
        let rule_h = edge.min(panel.bottom() - rule_y);
        quads.push((panel.x, rule_y, panel.w, rule_h, ctx.color(border)));
    }

    for (i, item) in view.items.iter().enumerate() {
        let top = rule_y + edge + i as f32 * lh;
        if top + lh > panel.bottom() + 0.5 {
            break;
        }
        if i == view.selected {
            let sel = panel_bg.mix(palette.selection, 0.7);
            let sel_h = lh.min((panel.bottom() - top).max(0.0));
            if sel_h > 0.0 {
                quads.push((panel.x, top, panel.w, sel_h, ctx.color(sel)));
            }
        }
        if bounds.left >= bounds.right || bounds.top >= bounds.bottom {
            continue;
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
        if !item.title_ar.is_empty() {
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
    }
    (Some(outer), texts)
}

/// Width of a shaped one-line label.
pub fn label_width(buffer: &Buffer) -> f32 {
    buffer
        .layout_runs()
        .map(|run| run.line_w)
        .fold(0.0, f32::max)
}
