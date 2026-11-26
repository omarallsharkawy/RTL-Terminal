//! The command palette overlay: a panel near the top of the window with the query and the
//! matching items (commands, or notifications). Each row shows the English title and shortcut
//! on the left and the Arabic title on the right, each shaped on its own so neither language
//! reorders the other. Titles are shortened at grapheme boundaries when space is tight.

use unicode_segmentation::UnicodeSegmentation;

use super::context::DrawContext;
use crate::layout::Rect;
use crate::render::icons::Icon;
use crate::render::shapes::{Border, Shadow, Shape};
use crate::render::text::ui::{UiAlign, UiFont, UiLabel, UiSize, UiWeight};
use crate::render::ui_style::{
    UiStyle, OVERLAY_BLUR, OVERLAY_OFFSET, OVERLAY_SHADOW, RADIUS_12, RADIUS_6, RADIUS_8,
};

pub use crate::render::text::ui::label_width;

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

/// Optional pictograms for the active mode and the visible command rows.
pub struct PaletteIcons {
    pub leading: Icon,
    pub rows: Vec<Option<Icon>>,
}

impl Default for PaletteIcons {
    fn default() -> Self {
        Self {
            leading: Icon::Search,
            rows: Vec::new(),
        }
    }
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

/// The palette's shared drawing and pointer geometry, in physical pixels.
#[derive(Clone, Copy, Debug)]
pub struct PaletteLayout {
    pub panel: Rect,
    pub search: Rect,
    rows: Rect,
    row_height: f32,
}

impl PaletteLayout {
    pub fn new(scale: f32, (width, height): (f32, f32), rows: usize) -> Self {
        let margin = 8.0 * scale;
        let w = (648.0 * scale).min((width - margin * 2.0).max(0.0));
        let x = ((width - w) / 2.0).round();
        let y = (52.0 * scale).min(height / 5.0).round();
        let pad = margin.min(w / 3.0);
        let search_h = (34.0 * scale).min((height - y - 2.0 * pad).max(0.0));
        let row_height = (28.0 * scale).round().max(1.0);
        let first = y + pad * 2.0 + search_h;
        let capacity = ((height - first - pad - margin).max(0.0) / row_height).floor() as usize;
        let rows_h = row_height * rows.min(capacity) as f32;
        let panel_h = (first - y + rows_h + pad).min((height - y).max(0.0));
        let inner_w = (w - 2.0 * pad).max(0.0);
        Self {
            panel: Rect::new(x, y, w, panel_h),
            search: Rect::new(x + pad, y + pad, inner_w, search_h),
            rows: Rect::new(x + pad, first, inner_w, rows_h),
            row_height,
        }
    }

    pub fn rows(self) -> usize {
        (self.rows.h / self.row_height).round() as usize
    }

    pub fn row(self, index: usize) -> Option<Rect> {
        (index < self.rows()).then(|| {
            Rect::new(
                self.rows.x,
                self.rows.y + index as f32 * self.row_height,
                self.rows.w,
                self.row_height,
            )
        })
    }

    pub fn hit(self, x: f64, y: f64) -> PaletteHit {
        if !self.panel.contains(x, y) {
            PaletteHit::Outside
        } else if self.rows.contains(x, y) && self.rows() > 0 {
            let row = (((y as f32 - self.rows.y) / self.row_height)
                .floor()
                .max(0.0) as usize)
                .min(self.rows() - 1);
            PaletteHit::Item(row)
        } else {
            PaletteHit::Inside
        }
    }
}

/// Fit `query` into `width` physical pixels, keeping the tail prefixed with `…` when it overflows
/// so what the user is typing stays visible.
pub fn fit_query_tail(
    font: &UiFont,
    font_system: &mut cosmic_text::FontSystem,
    scale: f32,
    query: &str,
    width: f32,
) -> (String, cosmic_text::Buffer) {
    let mut shape =
        |text: &str| font.shape(font_system, UiSize::Title, UiWeight::Regular, scale, text);
    let full = shape(query);
    if label_width(&full) <= width {
        return (query.to_owned(), full);
    }
    let cuts: Vec<_> = query.grapheme_indices(true).map(|(i, _)| i).collect();
    let ellipsis = shape("…");
    if label_width(&ellipsis) > width {
        return (String::new(), shape(""));
    }
    let mut best_text = "…".to_owned();
    let mut best = ellipsis;
    let (mut low, mut high) = (0, cuts.len().saturating_sub(1));
    while low < high {
        let keep = (low + high).div_ceil(2);
        let start = cuts[cuts.len() - keep];
        let text = format!("…{}", &query[start..]);
        let candidate = shape(&text);
        if label_width(&candidate) <= width {
            best_text = text;
            best = candidate;
            low = keep;
        } else {
            high = keep - 1;
        }
    }
    (best_text, best)
}

/// Panel shapes and labels, with the area to clip out of previously laid-out text.
pub(crate) fn palette_overlay(
    ctx: &mut DrawContext,
    style: UiStyle,
    font: &UiFont,
    view: &PaletteView,
    icons: &PaletteIcons,
    surface: (f32, f32),
    shapes: &mut Vec<Shape>,
) -> (Option<Rect>, Vec<UiLabel>) {
    let layout = PaletteLayout::new(style.scale, surface, view.items.len());
    let panel = layout.panel;
    if panel.w <= 0.0 || panel.h <= 0.0 {
        return (None, Vec::new());
    }
    shapes.push(Shape::RoundedRect {
        rect: panel,
        radii: [style.px(RADIUS_12); 4],
        fill: ctx.color(style.bg1),
        border: Some(Border {
            width: style.hairline(),
            color: ctx.color(style.border),
        }),
        shadow: Some(Shadow {
            blur: style.px(OVERLAY_BLUR),
            offset: OVERLAY_OFFSET.map(|v| style.px(v)),
            color: ctx.color(OVERLAY_SHADOW),
        }),
    });

    let mut texts = Vec::new();
    let search = layout.search;
    if search.h > 0.0 && search.w > 0.0 {
        shapes.push(Shape::rounded(
            search,
            style.px(RADIUS_8),
            ctx.color(style.bg2),
        ));
        let side = style.px(16.0).min(search.h).min(search.w / 2.0);
        let icon = Rect::new(
            search.x + style.px(8.0).min(search.w / 4.0),
            search.y + (search.h - side) / 2.0,
            side,
            side,
        );
        icons.leading.draw(icon, ctx.color(style.text_dim), shapes);
        let x = icon.right() + style.px(8.0);
        let text = Rect::new(
            x,
            search.y,
            (search.right() - x - style.px(8.0)).max(0.0),
            search.h,
        );
        let (buffer, color) = if view.query.is_empty() {
            let placeholder = view
                .placeholder
                .as_deref()
                .unwrap_or("Type a command… / اكتب أمر…");
            (
                font.fit(
                    ctx.font_system,
                    UiSize::Title,
                    UiWeight::Regular,
                    style.scale,
                    placeholder,
                    text.w,
                ),
                style.text_dim,
            )
        } else {
            let (_, buffer) =
                fit_query_tail(font, ctx.font_system, style.scale, &view.query, text.w);
            (buffer, style.text)
        };
        texts.push(UiLabel::new(buffer, text, UiAlign::Start, color));
    }

    for (i, item) in view.items.iter().take(layout.rows()).enumerate() {
        let Some(row) = layout.row(i) else { break };
        let selected = i == view.selected;
        if selected {
            let highlight = Rect::new(row.x, row.y + style.px(1.0), row.w, row.h - style.px(2.0));
            shapes.push(Shape::rounded(
                highlight,
                style.px(RADIUS_8),
                ctx.color(style.accent_tint(style.bg2)),
            ));
        }
        let title_color = if item.dimmed {
            style.text_faint
        } else {
            style.text
        };
        let key_color = if selected {
            title_color
        } else {
            style.text_dim
        };
        let pad = style.px(8.0);
        let rect = Rect::new(row.x + pad, row.y, (row.w - 2.0 * pad).max(0.0), row.h);
        let keys_w = item.shortcut.as_deref().map_or(0.0, |shortcut| {
            if rect.w < style.px(232.0) {
                return 0.0;
            }
            let keys = shortcut
                .split('+')
                .map(|key| {
                    font.shape(
                        ctx.font_system,
                        UiSize::Small,
                        UiWeight::Regular,
                        style.scale,
                        key,
                    )
                })
                .collect::<Vec<_>>();
            let gap = style.px(3.0);
            let padding = style.px(8.0);
            let total: f32 = keys
                .iter()
                .map(|key| label_width(key) + padding + gap)
                .sum();
            if total > rect.w / 2.0 {
                return 0.0;
            }
            let mut x = rect.x + (rect.w - total) / 2.0;
            for key in keys {
                let width = label_width(&key) + padding;
                let h = style.px(18.0).min(rect.h);
                let cap = Rect::new(x, rect.y + (rect.h - h) / 2.0, width, h);
                shapes.push(Shape::RoundedRect {
                    rect: cap,
                    radii: [style.px(RADIUS_6); 4],
                    fill: ctx.color(style.bg2),
                    border: Some(Border {
                        width: style.hairline(),
                        color: ctx.color(style.border),
                    }),
                    shadow: None,
                });
                texts.push(UiLabel::new(key, cap, UiAlign::Center, key_color));
                x += width + gap;
            }
            total
        });
        let room = (rect.w - keys_w - pad).max(0.0);
        let widths = if keys_w > 0.0 || (!item.title_en.is_empty() && !item.title_ar.is_empty()) {
            [room / 2.0; 2]
        } else {
            [rect.w; 2]
        };
        for (title, width, align, color) in [
            (&item.title_en, widths[0], UiAlign::Left, title_color),
            (&item.title_ar, widths[1], UiAlign::Right, title_color),
        ] {
            if title.is_empty() {
                continue;
            }
            let x = if matches!(align, UiAlign::Right) {
                rect.right() - width
            } else {
                rect.x
            };
            let mut text = Rect::new(x, rect.y, width, rect.h);
            if matches!(align, UiAlign::Left) {
                if let Some(Some(icon)) = icons.rows.get(i).filter(|_| width > style.px(64.0)) {
                    let side = style.px(14.0);
                    icon.draw(
                        Rect::new(x, row.y + (row.h - side) / 2.0, side, side),
                        ctx.color(style.text_dim),
                        shapes,
                    );
                    text.x += side + pad;
                    text.w -= side + pad;
                }
            }
            let buffer = font.fit(
                ctx.font_system,
                UiSize::Body,
                UiWeight::Regular,
                style.scale,
                title,
                text.w,
            );
            texts.push(UiLabel::new(buffer, text, align, color));
        }
    }
    (Some(panel), texts)
}
