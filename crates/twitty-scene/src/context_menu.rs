//! Drawing an open right-click menu (`ui::context_menu`): a raised card with one row per item,
//! the highlighted row tinted, shortcuts dimmed at the trailing edge, separators as hairlines.

use super::context::DrawContext;
use twitty_core::layout::Rect;
use twitty_render::shapes::{Border, Shadow, Shape};
use twitty_render::text::ui::{label_width, UiAlign, UiFont, UiLabel, UiSize, UiWeight};
use twitty_render::ui_style::{
    UiStyle, OVERLAY_BLUR, OVERLAY_OFFSET, OVERLAY_SHADOW, RADIUS_6, RADIUS_8, SPACE_12, SPACE_8,
};
use twitty_ui::ui::chrome::UiLanguage;
use twitty_ui::ui::context_menu::{ContextMenu, MenuEntry, MenuLayout};

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

/// Draw a one-line tooltip just below and after pointer `at`, kept inside `surface`. Returns its
/// label and rect.
pub fn draw_tooltip(
    ctx: &mut DrawContext,
    style: UiStyle,
    font: &UiFont,
    text: &str,
    at: (f32, f32),
    surface: (f32, f32),
    shapes: &mut Vec<Shape>,
) -> (UiLabel, Rect) {
    let buffer = font.shape(
        ctx.font_system,
        UiSize::Small,
        UiWeight::Regular,
        style.scale,
        text,
    );
    let pad = style.px(SPACE_8);
    let w = (label_width(&buffer).ceil() + 2.0 * pad).min(surface.0);
    let h = style.px(24.0);
    let margin = style.px(4.0);
    let below = at.1 + style.px(18.0);
    let y = if below + h + margin <= surface.1 {
        below
    } else {
        at.1 - h - style.px(8.0)
    };
    let x = (at.0 + style.px(4.0))
        .min(surface.0 - w - margin)
        .max(margin);
    let rect = Rect::new(x.round(), y.max(margin).round(), w, h);
    shapes.push(Shape::RoundedRect {
        rect,
        radii: [style.px(RADIUS_6); 4],
        fill: ctx.color(style.bg3),
        border: Some(Border {
            width: style.px(1.0),
            color: ctx.color(style.border),
        }),
        shadow: Some(Shadow {
            blur: style.px(8.0),
            offset: [0.0, style.px(2.0)],
            color: ctx.color(OVERLAY_SHADOW),
        }),
    });
    (
        UiLabel::new(buffer, rect, UiAlign::Center, style.text),
        rect,
    )
}

/// Draw notices stacked down from just under the top bar, centered, newest on top. Returns their
/// labels and rects.
pub fn draw_toasts(
    ctx: &mut DrawContext,
    style: UiStyle,
    font: &UiFont,
    toasts: &[twitty_ui::ui::toast::Toast],
    surface: (f32, f32),
    language: UiLanguage,
    shapes: &mut Vec<Shape>,
) -> (Vec<UiLabel>, Vec<Rect>) {
    use twitty_render::icons::Icon;
    use twitty_ui::ui::toast::ToastKind;
    let mut labels = Vec::new();
    let mut rects = Vec::new();
    let h = style.px(34.0);
    let gap = style.px(SPACE_8);
    let max_w = (surface.0 - 2.0 * style.px(16.0)).max(0.0);
    // Below the 36 px top bar, clear of the composer at the bottom.
    let mut top = style.px(36.0 + 12.0);
    for toast in toasts.iter().rev() {
        let icon_side = style.px(14.0);
        let pad = style.px(SPACE_12);
        let text_room = (max_w - 2.0 * pad - icon_side - gap).max(0.0);
        let buffer = font.fit(
            ctx.font_system,
            UiSize::Caption,
            UiWeight::Regular,
            style.scale,
            &toast.text,
            text_room,
        );
        let text_w = label_width(&buffer).ceil().min(text_room);
        let w = (2.0 * pad + icon_side + gap + text_w).min(max_w);
        let rect = Rect::new(((surface.0 - w) / 2.0).round(), top.round(), w, h);
        let (icon, color) = match toast.kind {
            ToastKind::Success => (Icon::Check, style.ok),
            ToastKind::Error => (Icon::Warning, style.danger),
            ToastKind::Info => (Icon::Check, style.accent),
        };
        shapes.push(Shape::RoundedRect {
            rect,
            radii: [style.px(RADIUS_8); 4],
            fill: ctx.color(style.bg2),
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
        let icon_rect = language.mirror(
            Rect::new(
                rect.x + pad,
                rect.y + (h - icon_side) / 2.0,
                icon_side,
                icon_side,
            ),
            rect,
        );
        icon.draw(icon_rect, ctx.color(color), shapes);
        let text_rect = language.mirror(
            Rect::new(rect.x + pad + icon_side + gap, rect.y, text_w + 1.0, h),
            rect,
        );
        labels.push(UiLabel::new(buffer, text_rect, UiAlign::Start, style.text));
        rects.push(rect);
        top = rect.bottom() + gap;
        if top + h > surface.1 {
            break;
        }
    }
    (labels, rects)
}
