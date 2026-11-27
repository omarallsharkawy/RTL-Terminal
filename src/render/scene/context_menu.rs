//! Drawing an open right-click menu (`ui::context_menu`): a raised card with one row per item,
//! the highlighted row tinted, shortcuts dimmed at the trailing edge, separators as hairlines.

use super::context::DrawContext;
use crate::layout::Rect;
use crate::render::shapes::{Border, Shadow, Shape};
use crate::render::text::ui::{label_width, UiAlign, UiFont, UiLabel, UiSize, UiWeight};
use crate::render::ui_style::{
    UiStyle, OVERLAY_BLUR, OVERLAY_OFFSET, OVERLAY_SHADOW, RADIUS_6, RADIUS_8, SPACE_12, SPACE_8,
};
use crate::ui::chrome::UiLanguage;
use crate::ui::context_menu::{ContextMenu, MenuEntry, MenuLayout};

/// Push the menu's shapes into `shapes` and return its labels.
pub fn draw(
    ctx: &mut DrawContext,
    style: UiStyle,
    font: &UiFont,
    menu: &ContextMenu,
    layout: &MenuLayout,
    language: UiLanguage,
    shapes: &mut Vec<Shape>,
) -> Vec<UiLabel> {
    let mut labels = Vec::new();
    shapes.push(Shape::RoundedRect {
        rect: layout.rect,
        radii: [style.px(RADIUS_8); 4],
        fill: ctx.color(style.bg1),
        border: Some(Border {
            width: style.px(1.0),
            color: ctx.color(style.border),
        }),
        shadow: Some(Shadow {
            blur: style.px(OVERLAY_BLUR),
            offset: OVERLAY_OFFSET.map(|v| style.px(v)),
            color: ctx.color(OVERLAY_SHADOW),
        }),
    });
    for (i, (entry, &row)) in menu.entries.iter().zip(&layout.rows).enumerate() {
        if row.bottom() > layout.rect.bottom() {
            break;
        }
        let item = match entry {
            MenuEntry::Separator => {
                let line = Rect::new(
                    row.x + style.px(SPACE_8),
                    (row.y + row.h / 2.0).round(),
                    (row.w - 2.0 * style.px(SPACE_8)).max(0.0),
                    style.hairline(),
                );
                shapes.push(Shape::rounded(line, 0.0, ctx.color(style.border)));
                continue;
            }
            MenuEntry::Item(item) => item,
        };
        let selected = menu.selected == Some(i);
        if selected {
            shapes.push(Shape::rounded(
                row,
                style.px(RADIUS_6),
                ctx.color(style.accent_tint(style.bg2)),
            ));
        }
        let pad = style.px(SPACE_12);
        let inner = Rect::new(row.x + pad, row.y, (row.w - 2.0 * pad).max(0.0), row.h);
        let mut text_w = inner.w;
        if let Some(keys) = item.shortcut.as_deref() {
            let buffer = font.shape(
                ctx.font_system,
                UiSize::Small,
                UiWeight::Regular,
                style.scale,
                keys,
            );
            let w = label_width(&buffer).ceil() + style.px(2.0);
            if w + style.px(48.0) < inner.w {
                let rect =
                    language.mirror(Rect::new(inner.right() - w, inner.y, w, inner.h), inner);
                labels.push(UiLabel::new(
                    buffer,
                    rect,
                    UiAlign::Center,
                    style.text_faint,
                ));
                text_w = inner.w - w - style.px(SPACE_8);
            }
        }
        let color = match (item.danger, selected) {
            (true, _) => style.danger,
            (false, true) => style.text,
            (false, false) => style.text_dim,
        };
        let buffer = font.fit(
            ctx.font_system,
            UiSize::Caption,
            UiWeight::Regular,
            style.scale,
            &item.label,
            text_w,
        );
        let rect = language.mirror(Rect::new(inner.x, inner.y, text_w, inner.h), inner);
        labels.push(UiLabel::new(buffer, rect, UiAlign::Start, color));
    }
    labels
}
