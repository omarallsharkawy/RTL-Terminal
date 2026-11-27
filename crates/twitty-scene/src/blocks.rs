//! Command blocks drawn over a pane (`blocks::BlockDecor`):
//! - Card mode (`block_cards = true`, default): a faint tint behind the terminal rows, a thin
//!   leading status strip, and a floating action toolbar on hover.
//!   The terminal grid stays fixed and text is clipped around overlays.
//! - Legacy mode (`block_cards = false`): a thin separator above each prompt, a gutter strip at the
//!   pane's leading edge, a tint over the selected block, and a badge at the end of its first line.

use cosmic_text::Buffer;
use std::time::Instant;

use super::context::DrawContext;
use crate::PaneFrame;
use twitty_core::color::Rgba;
use twitty_core::layout::{PaneId, Rect};
use twitty_render::icons::Icon;
use twitty_render::shapes::{Border, Shadow, Shape, ShapeLayers};
use twitty_render::style::Quad;
use twitty_render::text::ui::{label_width, UiAlign, UiFont, UiLabel, UiSize, UiWeight};
use twitty_render::ui_style::{UiStyle, RADIUS_6, SPACE_12, SPACE_16, SPACE_4};
use twitty_ui::blocks::cards::{layout_cards, visible_rows, BlockButton, BlockHover};
use twitty_ui::blocks::BlockStatus;
use twitty_ui::ui::chrome::UiLanguage;
use twitty_ui::ui::motion::Highlights;

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
pub struct CardScene {
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
        BlockButton::ExplainError => Icon::Explain,
        BlockButton::CopyCommand => Icon::CopyCommand,
        BlockButton::CopyOutput => Icon::Copy,
        BlockButton::AskAgent => Icon::Agent,
        BlockButton::Rerun => Icon::Rerun,
    }
}

/// Paint state shared by all visible command blocks in one frame.
pub struct BlockPaint<'a> {
    pub hover: Option<BlockHover>,
    pub fades: Option<(&'a BlockFades, Instant)>,
}

#[derive(Default)]
pub struct BlockFades {
    pub cards: Highlights<BlockHover>,
    pub buttons: Highlights<BlockHover>,
}

impl BlockPaint<'_> {
    fn emphasis(
        &self,
        pane: PaneId,
        block: u64,
        button: Option<BlockButton>,
        hovered: bool,
    ) -> f32 {
        self.fades.map_or(f32::from(hovered), |(fades, now)| {
            let tracks = if button.is_some() {
                &fades.buttons
            } else {
                &fades.cards
            };
            tracks.value(
                BlockHover {
                    pane,
                    block,
                    button,
                },
                now,
            )
        })
    }
}

/// Draw command block cards into `shapes.underlay` (backgrounds and leading status strips) and
/// floating toolbars into `shapes.chrome`, returning the dim labels and clip rects for row text.
#[allow(clippy::too_many_arguments)]
pub fn draw_cards(
    ctx: &mut DrawContext,
    style: UiStyle,
    font: &UiFont,
    pane: &PaneFrame,
    hidden_row: Option<usize>,
    paint: &BlockPaint<'_>,
    shapes: &mut ShapeLayers,
    language: UiLanguage,
) -> CardScene {
    if pane.blocks.is_empty() || pane.lines.is_empty() {
        return CardScene::default();
    }

    let pane_hover = paint.hover.filter(|h| h.pane == pane.id);
    let hovered_block = pane_hover.map(|h| h.block);

    // Only the hovered command needs a measured label for toolbar geometry.
    let measure = |text: &str| {
        let text = localized_meta(text, language);
        let buf = font.shape(
            ctx.font_system,
            UiSize::Small,
            UiWeight::Regular,
            style.scale,
            &text,
        );
        label_width(&buf)
    };
    let cards = layout_cards(
        pane.rect,
        ctx.metrics,
        pane.lines.len(),
        &pane.blocks,
        hovered_block,
        hidden_row,
        measure,
    );

    let mut scene = CardScene::default();
    let pad = style.px(SPACE_4).round();
    for card in cards {
        let decor = pane.blocks.iter().find(|d| d.id == card.id);
        let hover = paint.emphasis(pane.id, card.id, None, card.hovered);
        let fill = style.block_tint(card.selected, hover);
        shapes.underlay.push(Shape::RoundedRect {
            rect: card.card,
            radii: card.radii,
            fill: ctx.color(fill),
            border: None,
            shadow: None,
        });

        let strip_color = if card.bookmarked {
            Some(Rgba::from_rgb8(0xE8, 0xA8, 0x38))
        } else {
            card_strip_color(style, card.status)
        };
        if let Some(strip_color) = strip_color {
            let mut strip = card.strip;
            if card.bookmarked {
                strip.w = (3.0 * style.scale).round().max(strip.w).min(card.card.w);
            }
            let strip = language.mirror(strip, card.card);
            let radii = if language == UiLanguage::Arabic {
                [0.0, card.radii[1], card.radii[2], 0.0]
            } else {
                [card.radii[0], 0.0, 0.0, card.radii[3]]
            };
            shapes.underlay.push(Shape::RoundedRect {
                rect: strip,
                radii,
                fill: ctx.color(strip_color),
                border: None,
                shadow: None,
            });
        }

        let meta_text = decor
            .and_then(|d| d.meta.as_deref())
            .map(|text| localized_meta(text, language));
        let align = if language == UiLanguage::Arabic {
            UiAlign::Left
        } else {
            UiAlign::Right
        };
        if let Some(tb) = card.toolbar {
            let tb = tb.with_language(language, card.card);
            scene.clips.push((pane.id, tb.rect));
            shapes.chrome.push(Shape::RoundedRect {
                rect: tb.rect,
                radii: [style.px(RADIUS_6); 4],
                fill: ctx.color(style.bg3),
                border: Some(Border {
                    width: style.hairline(),
                    color: ctx.color(style.border),
                }),
                shadow: Some(Shadow {
                    blur: style.px(8.0),
                    offset: [0.0, style.px(SPACE_4)],
                    color: ctx.color(Rgba::new(0.0, 0.0, 0.0, 0.18)),
                }),
            });

            if let (Some(meta_rect), Some(text)) = (tb.meta, meta_text.as_deref()) {
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
                    .push(UiLabel::new(buf, meta_rect, align, style.text_dim));
            }

            let hovered_btn = pane_hover
                .filter(|h| h.block == card.id)
                .and_then(|h| h.button);
            for (btn, rect, enabled) in tb.buttons {
                let hover = if enabled {
                    paint.emphasis(pane.id, card.id, Some(btn), hovered_btn == Some(btn))
                } else {
                    0.0
                };
                if hover > 0.0 {
                    shapes.chrome.push(Shape::rounded(
                        rect,
                        style.px(RADIUS_6),
                        ctx.color(style.bg3.mix(style.text, 0.08 * hover)),
                    ));
                }
                let color = if enabled {
                    style.text_dim.mix(style.text, hover)
                } else {
                    style.text_faint
                };
                let side = style.px(SPACE_12).min(rect.w).min(rect.h);
                let icon_rect = Rect::new(
                    rect.x + (rect.w - side) * 0.5,
                    rect.y + (rect.h - side) * 0.5,
                    side,
                    side,
                );
                button_icon(btn).draw(icon_rect, ctx.color(color), &mut shapes.chrome);
            }
        } else if let Some(text) =
            meta_text.filter(|_| !card.clipped_top && hidden_row != Some(card.first_visible_row))
        {
            let buf = font.shape(
                ctx.font_system,
                UiSize::Small,
                UiWeight::Regular,
                style.scale,
                &text,
            );
            let label_w = label_width(&buf).ceil();
            let h = style.px(SPACE_16).min(card.card.h);
            let w = label_w + 2.0 * pad;
            if label_w <= 0.0 || w > card.card.w - card.strip.w - 2.0 * pad {
                continue;
            }
            let first_y = pane.rect.y
                + ctx.metrics.padding_top
                + card.first_visible_row as f32 * ctx.metrics.line_height;
            let y = (first_y + (ctx.metrics.line_height - h) * 0.5)
                .clamp(card.card.y, (card.card.bottom() - h).max(card.card.y));
            let badge = Rect::new(card.card.right() - pad - w, y, w, h);
            let badge = language.mirror(badge, card.card);
            // Leave a cell of breathing room around the terminal text. In a narrow pane the
            // hover toolbar still carries this label, even when the row has no free space.
            let mut occupied = pane.lines[card.first_visible_row]
                .cells
                .iter()
                .enumerate()
                .filter(|(_, cell)| !cell.is_blank() || cell.selected || cell.search_match != 0);
            let origin_x = pane.rect.x + ctx.metrics.padding_left;
            let cell_w = ctx.metrics.char_width;
            let fits = if language == UiLanguage::Arabic {
                occupied
                    .next()
                    .is_none_or(|(col, _)| badge.right() <= origin_x + (col as f32 - 1.0) * cell_w)
            } else {
                occupied
                    .next_back()
                    .is_none_or(|(col, _)| badge.x >= origin_x + (col as f32 + 2.0) * cell_w)
            };
            if fits {
                scene.clips.push((pane.id, badge));
                let text_rect = Rect::new(badge.x + pad, badge.y, label_w, badge.h);
                scene
                    .labels
                    .push(UiLabel::new(buf, text_rect, align, style.text_dim));
            }
        }
    }
    scene
}

fn localized_meta(text: &str, language: UiLanguage) -> std::borrow::Cow<'_, str> {
    if language == UiLanguage::English {
        return text.into();
    }
    text.split(" · ")
        .map(|part| {
            if let Some(exit) = part.strip_prefix("exit ") {
                return twitty_core::language::fill(language.t("exit {}"), &[&exit]);
            }
            let unit = |text: &str| format!(" {}", language.t(text));
            part.replace('h', &unit(twitty_core::en!("h|hours")))
                .replace('m', &unit(twitty_core::en!("m|minutes")))
                .replace('s', &unit(twitty_core::en!("s|seconds")))
        })
        .collect::<Vec<_>>()
        .join(" · ")
        .into()
}

/// The selected block's background tint, drawn under the rows (cells with a background of
/// their own cover it).
pub fn tint_quads(ctx: &DrawContext, pane: &PaneFrame, out: &mut Vec<Quad>) {
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
pub fn mark_quads(ctx: &DrawContext, pane: &PaneFrame, out: &mut Vec<Quad>) {
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
pub fn badges(
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
