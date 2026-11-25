//! Command blocks drawn over a pane (`blocks::BlockDecor`):
//! - Card mode (`block_cards = true`, default): subtle rounded `bg2` cards in `ShapeLayers::underlay`
//!   behind each visible block's rows (with 4 px inner padding effect via the background only,
//!   never shifting the terminal grid), a thin left status strip (`ok`/`danger`/`accent`), and on
//!   hover or for the selected block a floating top-right toolbar in `ShapeLayers::chrome` with
//!   icon buttons (copy command, copy output, ask agent, rerun) plus a dim duration/exit label.
//! - Legacy mode (`block_cards = false`): a thin separator above each prompt, a gutter strip at the
//!   pane's leading edge, a tint over the selected block, and a badge at the end of its first line.

use cosmic_text::Buffer;

use super::context::DrawContext;
use crate::blocks::cards::{layout_cards, visible_rows, BlockButton, BlockHover};
use crate::blocks::BlockStatus;
use crate::color::Rgba;
use crate::layout::{PaneId, Rect};
use crate::render::icons::Icon;
use crate::render::shapes::{Border, Shadow, Shape, ShapeLayers};
use crate::render::style::Quad;
use crate::render::text::ui::{label_width, UiAlign, UiFont, UiLabel, UiSize, UiWeight};
use crate::render::ui_style::{UiStyle, RADIUS_6, SPACE_4};
use crate::render::PaneFrame;

/// A badge's shaped text and where it goes (legacy mode).
pub struct Badge {
    pub buffer: Buffer,
    /// Viewport row it is drawn on.
    pub row: usize,
    /// Pixel span of its background (row text under it is clipped).
    pub left: f32,
    pub right: f32,
    /// Where the text starts.
    pub text_left: f32,
    pub top: f32,
    pub color: glyphon::Color,
}

/// Text labels and clip rectangles emitted by command block cards for one pane.
#[derive(Default)]
pub(crate) struct CardScene {
    pub clips: Vec<(PaneId, Rect)>,
    pub labels: Vec<UiLabel>,
}

fn status_color(ctx: &DrawContext, status: BlockStatus) -> Option<Rgba> {
    let p = ctx.palette;
    match status {
        BlockStatus::Prompt => None,
        BlockStatus::Running => Some(p.cursor),
        BlockStatus::Succeeded => Some(p.ansi[2]),
        BlockStatus::Failed(_) => Some(p.ansi[1]),
        BlockStatus::Finished => Some(p.foreground.mix(p.background, 0.5)),
    }
}

fn card_strip_color(style: UiStyle, status: BlockStatus) -> Option<Rgba> {
    match status {
        BlockStatus::Prompt => None,
        BlockStatus::Running => Some(style.accent),
        BlockStatus::Succeeded => Some(style.ok),
        BlockStatus::Failed(_) => Some(style.danger),
        BlockStatus::Finished => Some(style.text_faint),
    }
}

fn button_icon(button: BlockButton) -> Icon {
    match button {
        BlockButton::CopyCommand => Icon::CopyCommand,
        BlockButton::CopyOutput => Icon::Copy,
        BlockButton::AskAgent => Icon::Agent,
        BlockButton::Rerun => Icon::Rerun,
    }
}

/// Draw command block cards into `shapes.underlay` (backgrounds + left status strips) and
/// floating toolbars into `shapes.chrome`, returning the dim labels and clip rects for row text.
pub(crate) fn draw_cards(
    ctx: &mut DrawContext,
    style: UiStyle,
    font: &UiFont,
    pane: &PaneFrame,
    hidden_row: Option<usize>,
    hover: Option<BlockHover>,
    shapes: &mut ShapeLayers,
) -> CardScene {
    if pane.blocks.is_empty() || pane.lines.is_empty() {
        return CardScene::default();
    }

    let pane_hover = hover.filter(|h| h.pane == pane.id);
    let hovered_block = pane_hover.map(|h| h.block);

    // Pre-measure any dim meta labels so `layout_cards` stays pure and we shape each once.
    let mut meta_widths: Vec<(u64, f32)> = Vec::new();
    for decor in &pane.blocks {
        if let Some(meta) = decor.meta.as_deref() {
            let buf = font.shape(
                ctx.font_system,
                UiSize::Small,
                UiWeight::Regular,
                style.scale,
                meta,
            );
            meta_widths.push((decor.id, label_width(&buf)));
        }
    }

    let cards = layout_cards(
        pane.rect,
        ctx.metrics,
        pane.lines.len(),
        &pane.blocks,
        hovered_block,
        hidden_row,
        |text| {
            pane.blocks
                .iter()
                .find(|d| d.meta.as_deref() == Some(text))
                .and_then(|d| {
                    meta_widths
                        .iter()
                        .find_map(|&(id, w)| (id == d.id).then_some(w))
                })
                .unwrap_or(0.0)
        },
    );

    let mut scene = CardScene::default();
    let pad = style.px(SPACE_4).round();
    for card in cards {
        let decor = pane.blocks.iter().find(|d| d.id == card.id);
        let fill = if card.selected {
            style.bg2.mix(style.accent, 0.12)
        } else {
            style.bg2
        };
        let border_color = if card.selected {
            style.accent.mix(style.border, 0.35)
        } else if card.hovered {
            style.border.mix(style.text_dim, 0.25)
        } else {
            style.border
        };
        shapes.underlay.push(Shape::RoundedRect {
            rect: card.card,
            radii: card.radii,
            fill: ctx.color(fill),
            border: Some(Border {
                width: style.px(1.0),
                color: ctx.color(border_color),
            }),
            shadow: None,
        });

        if let Some(strip_color) = card_strip_color(style, card.status) {
            shapes.underlay.push(Shape::RoundedRect {
                rect: card.strip,
                radii: [card.radii[0], 0.0, 0.0, card.radii[3]],
                fill: ctx.color(strip_color),
                border: None,
                shadow: None,
            });
        }

        let meta_text = decor.and_then(|d| d.meta.as_deref());
        if let Some(tb) = card.toolbar {
            scene.clips.push((pane.id, tb.rect));
            shapes.chrome.push(Shape::RoundedRect {
                rect: tb.rect,
                radii: [style.px(RADIUS_6); 4],
                fill: ctx.color(style.bg1),
                border: Some(Border {
                    width: style.px(1.0),
                    color: ctx.color(style.border),
                }),
                shadow: Some(Shadow {
                    blur: style.px(6.0),
                    offset: [0.0, style.px(2.0)],
                    color: ctx.color(Rgba::new(0.0, 0.0, 0.0, 0.18)),
                }),
            });

            if let (Some(meta_rect), Some(text)) = (tb.meta, meta_text) {
                let buf = font.fit(
                    ctx.font_system,
                    UiSize::Small,
                    UiWeight::Regular,
                    style.scale,
                    text,
                    meta_rect.w,
                );
                scene
                    .labels
                    .push(UiLabel::new(buf, meta_rect, UiAlign::Right, style.text_dim));
            }

            let hovered_btn = pane_hover
                .filter(|h| h.block == card.id)
                .and_then(|h| h.button);
            for (btn, rect, enabled) in tb.buttons {
                let is_hover = enabled && hovered_btn == Some(btn);
                if is_hover {
                    shapes
                        .chrome
                        .push(Shape::rounded(rect, style.px(4.0), ctx.color(style.bg3)));
                }
                let color = if !enabled {
                    style.text_faint
                } else if is_hover {
                    style.text
                } else {
                    style.text_dim
                };
                let side = style.px(14.0).min(rect.w).min(rect.h);
                let icon_rect = Rect::new(
                    rect.x + (rect.w - side) * 0.5,
                    rect.y + (rect.h - side) * 0.5,
                    side,
                    side,
                );
                button_icon(btn).draw(icon_rect, ctx.color(color), &mut shapes.chrome);
            }
        } else if let Some(text) = meta_text.filter(|_| hidden_row != Some(card.first_visible_row))
        {
            // Compact dim status pill on finished blocks with duration or non-zero exit when not
            // hovered/selected, so failures and long commands stay glanceable.
            let measured = meta_widths
                .iter()
                .find_map(|&(id, w)| (id == card.id).then_some(w))
                .unwrap_or(0.0);
            let pill_h = style.px(18.0).min(card.card.h);
            let pill_pad = style.px(6.0).round();
            let max_w = (card.card.w - card.strip.w - pad * 3.0).max(0.0);
            let pill_w = (measured.ceil() + pill_pad * 2.0).min(max_w);
            if measured > 0.0 && pill_w >= style.px(24.0) && pill_h >= style.px(10.0) {
                let first_y = pane.rect.y
                    + ctx.metrics.padding_top
                    + card.first_visible_row as f32 * ctx.metrics.line_height;
                let pill_y = (first_y + (ctx.metrics.line_height - pill_h) * 0.5)
                    .max(card.card.y + style.px(1.0).round())
                    .min((card.card.bottom() - pill_h).max(card.card.y));
                let pill = Rect::new(card.card.right() - pad - pill_w, pill_y, pill_w, pill_h);
                scene.clips.push((pane.id, pill));
                shapes.chrome.push(Shape::rounded(
                    pill,
                    style.px(RADIUS_6),
                    ctx.color(style.bg1),
                ));
                let text_rect = Rect::new(
                    pill.x + pill_pad,
                    pill.y,
                    (pill.w - pill_pad * 2.0).max(0.0),
                    pill.h,
                );
                let buf = font.fit(
                    ctx.font_system,
                    UiSize::Small,
                    UiWeight::Regular,
                    style.scale,
                    text,
                    text_rect.w,
                );
                scene
                    .labels
                    .push(UiLabel::new(buf, text_rect, UiAlign::Right, style.text_dim));
            }
        }
    }
    scene
}

/// The selected block's background tint, drawn under the rows (cells with a background of
/// their own cover it).
pub(crate) fn tint_quads(ctx: &DrawContext, pane: &PaneFrame, out: &mut Vec<Quad>) {
    let m = ctx.metrics;
    let rows = pane.lines.len();
    let top = pane.rect.y + m.padding_top;
    let color = ctx.color(ctx.palette.background.mix(ctx.palette.cursor, 0.08));
    for decor in pane.blocks.iter().filter(|d| d.selected) {
        if let Some((first, last)) = visible_rows(decor, rows) {
            let y = top + first as f32 * m.line_height;
            let h = (last - first + 1) as f32 * m.line_height;
            out.push((pane.rect.x, y, pane.rect.w, h, color));
        }
    }
}

/// Separators and gutter strips, drawn over the rows.
pub(crate) fn mark_quads(ctx: &DrawContext, pane: &PaneFrame, out: &mut Vec<Quad>) {
    let m = ctx.metrics;
    let rows = pane.lines.len();
    let top = pane.rect.y + m.padding_top;
    let thin = m.scale.round().max(1.0);
    let separator = ctx.color(ctx.palette.background.mix(ctx.palette.foreground, 0.18));
    // The strip sits in the pane's left padding, clear of the first column.
    let gutter_w = (m.scale * 2.0)
        .round()
        .max(2.0)
        .min(m.padding_left.max(2.0));
    let gutter_x = pane.rect.x + ((m.padding_left - gutter_w) * 0.5).max(0.0).floor();
    for decor in &pane.blocks {
        if (0..rows as i64).contains(&decor.first_row) && decor.first_row > 0 {
            let y = top + decor.first_row as f32 * m.line_height;
            out.push((pane.rect.x, y, pane.rect.w, thin, separator));
        }
        let (Some(color), Some((first, last))) =
            (status_color(ctx, decor.status), visible_rows(decor, rows))
        else {
            continue;
        };
        let y = top + first as f32 * m.line_height;
        let h = (last - first + 1) as f32 * m.line_height;
        // A small gap under the separator keeps blocks apart.
        let gap = if first as i64 == decor.first_row {
            thin
        } else {
            0.0
        };
        out.push((gutter_x, y + gap, gutter_w, h - gap, ctx.color(color)));
    }
}

/// Badges at the end of each visible block's first line, with their backgrounds in `out`.
/// `hidden_row` is a row covered by the pane's status line.
pub(crate) fn badges(
    ctx: &mut DrawContext,
    pane: &PaneFrame,
    hidden_row: Option<usize>,
    out: &mut Vec<Quad>,
) -> Vec<Badge> {
    let m = ctx.metrics;
    let rows = pane.lines.len();
    let cols = pane.lines.first().map_or(0, |l| l.cells.len());
    let mut badges = Vec::new();
    for decor in &pane.blocks {
        let Some(text) = decor.badge.as_deref() else {
            continue;
        };
        let Ok(row) = usize::try_from(decor.first_row) else {
            continue;
        };
        if row >= rows || Some(row) == hidden_row {
            continue;
        }
        let buffer = ctx.label(text);
        let width = buffer
            .layout_runs()
            .map(|run| run.line_w)
            .fold(0.0_f32, f32::max);
        let pad = m.char_width * 0.5;
        let grid_left = pane.rect.x + m.padding_left;
        let right = (grid_left + cols as f32 * m.char_width).min(pane.rect.right());
        let left = (right - width - 2.0 * pad).max(grid_left);
        let top = pane.rect.y + m.padding_top + row as f32 * m.line_height;
        let p = ctx.palette;
        out.push((
            left,
            top,
            right - left,
            m.line_height,
            ctx.color(p.background.mix(p.foreground, 0.1)),
        ));
        let color = match decor.status {
            BlockStatus::Failed(_) => p.ansi[1],
            _ => p.foreground.mix(p.background, 0.3),
        };
        badges.push(Badge {
            buffer,
            row,
            left,
            right,
            text_left: left + pad,
            top,
            color: color.to_glyphon(),
        });
    }
    badges
}
