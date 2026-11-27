//! CPU-side layout and rendering for built-in AI overlays:
//! - Failed command block explanation / fix suggestion popover card
//! - One-time bilingual privacy consent modal
//!
//! Built strictly with `UiStyle`, `Shape`, `Icon`, `UiFont`, and `UiLabel` so everything scales
//! with DPI, respects light and dark themes, mirrors cleanly in Arabic UI, and clips terminal text
//! behind every surface via `Frame::text_runs`.

use super::context::DrawContext;
use crate::ai::{BlockAiCardStatus, BlockAiMode, ConsentNotice};
use crate::layout::Rect;
use crate::render::icons::Icon;
use crate::render::shapes::{Border, Shadow, Shape};
use crate::render::text::ui::{label_width, UiAlign, UiFont, UiLabel, UiSize, UiWeight};
use crate::render::ui_style::{
    UiStyle, OVERLAY_BLUR, OVERLAY_OFFSET, OVERLAY_SHADOW, RADIUS_12, RADIUS_6, RADIUS_8, SPACE_12,
    SPACE_16, SPACE_4, SPACE_8,
};
use crate::ui::chrome::UiLanguage;

// ---- Failed-block Explain / Fix popover card ----

/// Everything the renderer needs to draw the block explanation/fix popover.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockAiView {
    /// Physical pixel rect of the failed block (or pane fallback) to position next to.
    pub anchor: Rect,
    pub mode: BlockAiMode,
    pub command: Option<String>,
    pub exit_code: i32,
    pub status: BlockAiCardStatus,
    pub spinner_phase: usize,
    pub scroll_lines: usize,
    pub language: UiLanguage,
}

/// Pointer hit target on the failed-block AI card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockAiHit {
    Outside,
    Inside,
    Close,
    InsertFix,
    SuggestFix,
}

/// Shared geometry for drawing and hit-testing the failed-block AI card (physical pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockAiLayout {
    pub card: Rect,
    pub header: Rect,
    pub close_button: Rect,
    pub command_bar: Option<Rect>,
    pub body: Rect,
    pub code_box: Option<Rect>,
    pub footer: Rect,
    pub insert_fix_button: Option<Rect>,
    pub suggest_fix_button: Option<Rect>,
    pub body_line_height: f32,
    pub visible_body_lines: usize,
}

impl BlockAiLayout {
    pub fn new(
        scale: f32,
        surface: (f32, f32),
        anchor: Rect,
        has_command: bool,
        has_fix: bool,
        show_suggest_fix: bool,
        language: UiLanguage,
    ) -> Self {
        let (sw, sh) = surface;
        let margin = (SPACE_12 * scale).round();
        let pad = (SPACE_12 * scale).round();
        let gap = (SPACE_8 * scale).round();

        let card_w = (420.0 * scale).min((sw - 2.0 * margin).max(0.0)).round();
        let header_h = (28.0 * scale).round();
        let cmd_h = if has_command {
            (22.0 * scale).round()
        } else {
            0.0
        };
        let code_h = if has_fix { (32.0 * scale).round() } else { 0.0 };
        let footer_h = if has_fix || show_suggest_fix {
            (30.0 * scale).round()
        } else {
            0.0
        };
        let body_line_height = (UiSize::Caption.px(scale) * 1.55).ceil().max(1.0);
        let max_card_h = (360.0 * scale).min((sh - 2.0 * margin).max(0.0)).round();

        let fixed_h = 2.0 * pad
            + header_h
            + gap
            + if cmd_h > 0.0 { cmd_h + gap } else { 0.0 }
            + if code_h > 0.0 { code_h + gap } else { 0.0 }
            + if footer_h > 0.0 { footer_h + gap } else { 0.0 };
        let avail_body_h = (max_card_h - fixed_h).max(body_line_height * 3.0);
        let body_h = avail_body_h.min((sh - 2.0 * margin - fixed_h).max(0.0));
        let visible_body_lines = ((body_h / body_line_height).floor() as usize).max(1);
        let card_h = (fixed_h + body_h).min((sh - 2.0 * margin).max(0.0)).round();

        // Place next to the block on its trailing edge (left in Arabic UI, right in English UI),
        // falling back to overlapping the trailing side of the block inside the window.
        let ideal_x = if language == UiLanguage::Arabic {
            if anchor.x - margin - card_w >= margin {
                anchor.x - margin - card_w
            } else {
                anchor.x + margin
            }
        } else if anchor.right() + margin + card_w <= sw - margin {
            anchor.right() + margin
        } else {
            anchor.right() - card_w - margin
        };
        let x = ideal_x
            .clamp(margin, (sw - margin - card_w).max(margin))
            .round();
        let y = anchor
            .y
            .clamp(margin, (sh - margin - card_h).max(margin))
            .round();
        let card = Rect::new(x, y, card_w, card_h);

        let inner_x = card.x + pad;
        let inner_w = (card.w - 2.0 * pad).max(0.0);
        let mut cursor_y = card.y + pad;

        let header = Rect::new(inner_x, cursor_y, inner_w, header_h);
        let close_side = (22.0 * scale).round().min(header_h);
        let close_x = if language == UiLanguage::Arabic {
            header.x
        } else {
            header.right() - close_side
        };
        let close_button = Rect::new(
            close_x,
            header.y + (header_h - close_side) * 0.5,
            close_side,
            close_side,
        );
        cursor_y += header_h + gap;

        let command_bar = if cmd_h > 0.0 {
            let r = Rect::new(inner_x, cursor_y, inner_w, cmd_h);
            cursor_y += cmd_h + gap;
            Some(r)
        } else {
            None
        };

        let body = Rect::new(inner_x, cursor_y, inner_w, body_h);
        cursor_y += body_h + gap;

        let code_box = if code_h > 0.0 {
            let r = Rect::new(inner_x, cursor_y, inner_w, code_h);
            cursor_y += code_h + gap;
            Some(r)
        } else {
            None
        };

        let footer = Rect::new(inner_x, cursor_y, inner_w, footer_h);
        let (insert_fix_button, suggest_fix_button) = if footer_h > 0.0 && inner_w > 0.0 {
            match (has_fix, show_suggest_fix) {
                (true, true) => {
                    let suggest_w = (128.0 * scale).min(inner_w * 0.38).round();
                    let insert_w = (inner_w - suggest_w - gap).max(0.0);
                    if language == UiLanguage::Arabic {
                        (
                            Some(Rect::new(
                                footer.right() - insert_w,
                                footer.y,
                                insert_w,
                                footer_h,
                            )),
                            Some(Rect::new(footer.x, footer.y, suggest_w, footer_h)),
                        )
                    } else {
                        (
                            Some(Rect::new(footer.x, footer.y, insert_w, footer_h)),
                            Some(Rect::new(
                                footer.right() - suggest_w,
                                footer.y,
                                suggest_w,
                                footer_h,
                            )),
                        )
                    }
                }
                (true, false) => (Some(footer), None),
                (false, true) => (None, Some(footer)),
                (false, false) => (None, None),
            }
        } else {
            (None, None)
        };

        Self {
            card,
            header,
            close_button,
            command_bar,
            body,
            code_box,
            footer,
            insert_fix_button,
            suggest_fix_button,
            body_line_height,
            visible_body_lines,
        }
    }

    pub fn hit(&self, x: f64, y: f64) -> BlockAiHit {
        if !self.card.contains(x, y) {
            return BlockAiHit::Outside;
        }
        if self.close_button.contains(x, y) {
            BlockAiHit::Close
        } else if self.insert_fix_button.is_some_and(|r| r.contains(x, y)) {
            BlockAiHit::InsertFix
        } else if self.suggest_fix_button.is_some_and(|r| r.contains(x, y)) {
            BlockAiHit::SuggestFix
        } else {
            BlockAiHit::Inside
        }
    }
}

/// Draw the failed-block AI card into `shapes`, returning its clip rect and UI labels.
pub fn block_ai_overlay(
    ctx: &mut DrawContext,
    style: UiStyle,
    font: &UiFont,
    view: &BlockAiView,
    surface: (f32, f32),
    shapes: &mut Vec<Shape>,
) -> (Option<Rect>, Vec<UiLabel>) {
    let has_fix = matches!(
        &view.status,
        BlockAiCardStatus::Ready(reply) if reply.suggested_command.is_some()
    );
    let show_suggest_fix = matches!(&view.status, BlockAiCardStatus::Ready(_))
        && view.mode == BlockAiMode::Explain
        && !has_fix;
    let layout = BlockAiLayout::new(
        style.scale,
        surface,
        view.anchor,
        view.command.is_some(),
        has_fix,
        show_suggest_fix,
        view.language,
    );
    if layout.card.w <= 0.0 || layout.card.h <= 0.0 {
        return (None, Vec::new());
    }

    shapes.push(Shape::RoundedRect {
        rect: layout.card,
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

    let mut labels = Vec::new();
    let pad = style.px(SPACE_8);

    // Header: icon, title, exit badge, close button.
    let close_icon_side = style.px(12.0).min(layout.close_button.w);
    let close_icon_rect = Rect::new(
        layout.close_button.x + (layout.close_button.w - close_icon_side) * 0.5,
        layout.close_button.y + (layout.close_button.h - close_icon_side) * 0.5,
        close_icon_side,
        close_icon_side,
    );
    Icon::Close.draw(close_icon_rect, ctx.color(style.text_dim), shapes);

    let exit_text = match view.language {
        UiLanguage::Arabic => format!("خروج {}", view.exit_code),
        UiLanguage::English => format!("exit {}", view.exit_code),
    };
    let exit_buf = font.shape(
        ctx.font_system,
        UiSize::Small,
        UiWeight::Semibold,
        style.scale,
        &exit_text,
    );
    let exit_w = label_width(&exit_buf).ceil() + style.px(12.0);
    let exit_h = style.px(18.0).min(layout.header.h);
    let exit_y = layout.header.y + (layout.header.h - exit_h) * 0.5;

    let icon_side = style.px(14.0).min(layout.header.h);
    let icon_y = layout.header.y + (layout.header.h - icon_side) * 0.5;
    let (icon_x, exit_x, title_x, title_w, title_align) = if view.language == UiLanguage::Arabic {
        let icon_x = layout.header.right() - icon_side;
        let exit_x = layout.close_button.right() + style.px(SPACE_4);
        let title_left = exit_x + exit_w + style.px(SPACE_8);
        let title_w = (icon_x - style.px(6.0) - title_left).max(0.0);
        (icon_x, exit_x, title_left, title_w, UiAlign::Right)
    } else {
        let icon_x = layout.header.x;
        let exit_x = layout.close_button.x - style.px(SPACE_4) - exit_w;
        let title_left = icon_x + icon_side + style.px(6.0);
        let title_w = (exit_x - style.px(SPACE_8) - title_left).max(0.0);
        (icon_x, exit_x, title_left, title_w, UiAlign::Left)
    };
    Icon::Explain.draw(
        Rect::new(icon_x, icon_y, icon_side, icon_side),
        ctx.color(style.accent),
        shapes,
    );

    if view.exit_code != 0 && exit_w + style.px(60.0) < layout.header.w {
        let exit_rect = Rect::new(exit_x, exit_y, exit_w, exit_h);
        shapes.push(Shape::rounded(
            exit_rect,
            style.px(RADIUS_6),
            ctx.color(style.bg2.mix(style.danger, 0.22)),
        ));
        labels.push(UiLabel::new(
            exit_buf,
            exit_rect,
            UiAlign::Center,
            style.danger,
        ));
    }

    let title_str = if title_w >= style.px(180.0) {
        view.mode.title_localized()
    } else if view.language == UiLanguage::Arabic {
        view.mode.title_ar()
    } else {
        view.mode.title_en()
    };
    let title_buf = font.fit(
        ctx.font_system,
        UiSize::Body,
        UiWeight::Semibold,
        style.scale,
        title_str,
        title_w,
    );
    labels.push(UiLabel::new(
        title_buf,
        Rect::new(title_x, layout.header.y, title_w, layout.header.h),
        title_align,
        style.text,
    ));

    // Command preview bar.
    if let (Some(cmd_rect), Some(cmd)) = (layout.command_bar, view.command.as_deref()) {
        shapes.push(Shape::rounded(
            cmd_rect,
            style.px(RADIUS_6),
            ctx.color(style.bg2),
        ));
        let text_rect = Rect::new(
            cmd_rect.x + pad,
            cmd_rect.y,
            (cmd_rect.w - 2.0 * pad).max(0.0),
            cmd_rect.h,
        );
        let preview = format!("$ {cmd}");
        let buf = font.fit(
            ctx.font_system,
            UiSize::Small,
            UiWeight::Regular,
            style.scale,
            &preview,
            text_rect.w,
        );
        labels.push(UiLabel::new(buf, text_rect, UiAlign::Left, style.text_dim));
    }

    // Body: loading spinner, error message, or wrapped explanation paragraphs.
    match &view.status {
        BlockAiCardStatus::Loading { .. } => {
            let dot_r = style.px(3.0).max(1.5);
            let dot_step = style.px(8.0);
            let dot_y = layout.body.y + layout.body_line_height * 0.5;
            let active = view.spinner_phase % 3;
            for i in 0..3 {
                let dot_x = layout.body.x + dot_r + i as f32 * dot_step;
                let color = if i == active {
                    style.accent
                } else {
                    style.text_faint
                };
                shapes.push(Shape::rounded(
                    Rect::new(dot_x - dot_r, dot_y - dot_r, dot_r * 2.0, dot_r * 2.0),
                    dot_r,
                    ctx.color(color),
                ));
            }
            let text_x = layout.body.x + 3.0 * dot_step + style.px(6.0);
            let text_rect = Rect::new(
                text_x,
                layout.body.y,
                (layout.body.right() - text_x).max(0.0),
                layout.body_line_height,
            );
            let msg = match view.language {
                UiLanguage::Arabic => "جاري تحليل الخطأ… (Esc للإلغاء)",
                UiLanguage::English => "Analyzing error… (Esc to cancel)",
            };
            let buf = font.fit(
                ctx.font_system,
                UiSize::Caption,
                UiWeight::Regular,
                style.scale,
                msg,
                text_rect.w,
            );
            labels.push(UiLabel::new(buf, text_rect, UiAlign::Start, style.text_dim));
        }
        BlockAiCardStatus::Error(err) => {
            let wrapped = wrap_lines(
                font,
                ctx.font_system,
                UiSize::Caption,
                UiWeight::Regular,
                style.scale,
                err,
                layout.body.w,
            );
            for (idx, line) in wrapped.iter().take(layout.visible_body_lines).enumerate() {
                let r = Rect::new(
                    layout.body.x,
                    layout.body.y + idx as f32 * layout.body_line_height,
                    layout.body.w,
                    layout.body_line_height,
                );
                let buf = font.fit(
                    ctx.font_system,
                    UiSize::Caption,
                    UiWeight::Regular,
                    style.scale,
                    line,
                    r.w,
                );
                labels.push(UiLabel::new(buf, r, UiAlign::Start, style.danger));
            }
        }
        BlockAiCardStatus::Ready(reply) => {
            let wrapped = wrap_lines(
                font,
                ctx.font_system,
                UiSize::Caption,
                UiWeight::Regular,
                style.scale,
                &reply.summary,
                layout.body.w,
            );
            let max_scroll = wrapped.len().saturating_sub(layout.visible_body_lines);
            let start = view.scroll_lines.min(max_scroll);
            for (vis, line) in wrapped
                .iter()
                .skip(start)
                .take(layout.visible_body_lines)
                .enumerate()
            {
                if line.is_empty() {
                    continue;
                }
                let r = Rect::new(
                    layout.body.x,
                    layout.body.y + vis as f32 * layout.body_line_height,
                    layout.body.w,
                    layout.body_line_height,
                );
                let buf = font.fit(
                    ctx.font_system,
                    UiSize::Caption,
                    UiWeight::Regular,
                    style.scale,
                    line,
                    r.w,
                );
                labels.push(UiLabel::new(buf, r, UiAlign::Start, style.text));
            }

            if let (Some(code_rect), Some(fix_cmd)) = (layout.code_box, &reply.suggested_command) {
                shapes.push(Shape::RoundedRect {
                    rect: code_rect,
                    radii: [style.px(RADIUS_8); 4],
                    fill: ctx.color(style.bg2),
                    border: Some(Border {
                        width: style.hairline(),
                        color: ctx.color(style.accent),
                    }),
                    shadow: None,
                });
                let inner = Rect::new(
                    code_rect.x + pad,
                    code_rect.y,
                    (code_rect.w - 2.0 * pad).max(0.0),
                    code_rect.h,
                );
                let single_line = fix_cmd.replace('\n', " && ");
                let buf = font.fit(
                    ctx.font_system,
                    UiSize::Caption,
                    UiWeight::Semibold,
                    style.scale,
                    &single_line,
                    inner.w,
                );
                labels.push(UiLabel::new(buf, inner, UiAlign::Left, style.text));
            }
        }
    }

    // Footer buttons.
    if let Some(btn) = layout.insert_fix_button {
        shapes.push(Shape::RoundedRect {
            rect: btn,
            radii: [style.px(RADIUS_8); 4],
            fill: ctx.color(style.accent),
            border: None,
            shadow: None,
        });
        let text = if btn.w >= style.px(290.0) {
            crate::ui::chrome::tr("Insert fix into composer", "حط الحل في خانة الأوامر")
        } else if view.language == UiLanguage::Arabic {
            "حط الحل في خانة الأوامر"
        } else {
            "Insert fix into composer"
        };
        let inner = Rect::new(
            btn.x + style.px(SPACE_8),
            btn.y,
            (btn.w - 2.0 * style.px(SPACE_8)).max(0.0),
            btn.h,
        );
        let buf = font.fit(
            ctx.font_system,
            UiSize::Caption,
            UiWeight::Semibold,
            style.scale,
            text,
            inner.w,
        );
        labels.push(UiLabel::new(
            buf,
            inner,
            UiAlign::Center,
            style.accent_text(),
        ));
    }

    if let Some(btn) = layout.suggest_fix_button {
        shapes.push(Shape::RoundedRect {
            rect: btn,
            radii: [style.px(RADIUS_8); 4],
            fill: ctx.color(style.bg2),
            border: Some(Border {
                width: style.hairline(),
                color: ctx.color(style.border),
            }),
            shadow: None,
        });
        let text = if btn.w >= style.px(180.0) {
            crate::ui::chrome::tr("Suggest a fix", "اقترح حل")
        } else if view.language == UiLanguage::Arabic {
            "اقترح حل"
        } else {
            "Suggest a fix"
        };
        let inner = Rect::new(
            btn.x + style.px(SPACE_8),
            btn.y,
            (btn.w - 2.0 * style.px(SPACE_8)).max(0.0),
            btn.h,
        );
        let buf = font.fit(
            ctx.font_system,
            UiSize::Caption,
            UiWeight::Semibold,
            style.scale,
            text,
            inner.w,
        );
        labels.push(UiLabel::new(buf, inner, UiAlign::Center, style.text));
    }

    (Some(layout.card), labels)
}

// ---- One-time Privacy Consent modal ----

/// Everything the renderer needs to draw the one-time AI privacy consent modal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentOverlayView {
    pub agent_name: String,
    pub language: UiLanguage,
}

/// Pointer hit target on the privacy consent modal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentHit {
    Outside,
    Inside,
    Accept,
    Cancel,
}

/// Shared geometry for drawing and hit-testing the privacy consent modal (physical pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConsentLayout {
    pub modal: Rect,
    pub title: Rect,
    pub body_en: Rect,
    pub divider_y: f32,
    pub body_ar: Rect,
    pub accept_button: Rect,
    pub cancel_button: Rect,
    pub line_height: f32,
}

impl ConsentLayout {
    pub fn new(scale: f32, (sw, sh): (f32, f32), language: UiLanguage) -> Self {
        let margin = (SPACE_16 * scale).round();
        let pad = (SPACE_16 * scale).round();
        let gap = (SPACE_12 * scale).round();
        let w = (520.0 * scale).min((sw - 2.0 * margin).max(0.0)).round();
        let title_h = (26.0 * scale).round();
        let line_height = (UiSize::Caption.px(scale) * 1.55).ceil().max(1.0);
        let para_h = (line_height * 3.0).ceil();
        let footer_h = (32.0 * scale).round();

        let ideal_h = 2.0 * pad + title_h + gap + para_h + gap + para_h + gap + footer_h;
        let h = ideal_h.min((sh - 2.0 * margin).max(0.0)).round();
        let x = ((sw - w) * 0.5).round();
        let y = ((sh - h) * 0.42).max(margin).round();
        let modal = Rect::new(x, y, w, h);

        let inner_x = modal.x + pad;
        let inner_w = (modal.w - 2.0 * pad).max(0.0);
        let mut cy = modal.y + pad;

        let title = Rect::new(inner_x, cy, inner_w, title_h);
        cy += title_h + gap;

        let avail_bodies = (modal.bottom() - pad - footer_h - gap - cy).max(0.0);
        let single_body_h = ((avail_bodies - gap).max(0.0) * 0.5).floor();
        let body_en = Rect::new(inner_x, cy, inner_w, single_body_h);
        let divider_y = (body_en.bottom() + gap * 0.5).round();
        let body_ar = Rect::new(inner_x, body_en.bottom() + gap, inner_w, single_body_h);

        let footer_y = (modal.bottom() - pad - footer_h).max(body_ar.bottom());
        let btn_gap = (SPACE_8 * scale).round();
        let cancel_w = (128.0 * scale).min(inner_w * 0.38).round();
        let accept_w = (inner_w - cancel_w - btn_gap).max(0.0);

        let (accept_button, cancel_button) = if language == UiLanguage::Arabic {
            (
                Rect::new(inner_x, footer_y, accept_w, footer_h),
                Rect::new(inner_x + inner_w - cancel_w, footer_y, cancel_w, footer_h),
            )
        } else {
            (
                Rect::new(inner_x + inner_w - accept_w, footer_y, accept_w, footer_h),
                Rect::new(inner_x, footer_y, cancel_w, footer_h),
            )
        };

        Self {
            modal,
            title,
            body_en,
            divider_y,
            body_ar,
            accept_button,
            cancel_button,
            line_height,
        }
    }

    pub fn hit(&self, x: f64, y: f64) -> ConsentHit {
        if !self.modal.contains(x, y) {
            ConsentHit::Outside
        } else if self.accept_button.contains(x, y) {
            ConsentHit::Accept
        } else if self.cancel_button.contains(x, y) {
            ConsentHit::Cancel
        } else {
            ConsentHit::Inside
        }
    }
}

/// Draw the one-time bilingual privacy consent modal into `shapes`, returning its clip rect and
/// shaped UI labels.
pub fn consent_overlay(
    ctx: &mut DrawContext,
    style: UiStyle,
    font: &UiFont,
    view: &ConsentOverlayView,
    surface: (f32, f32),
    shapes: &mut Vec<Shape>,
) -> (Option<Rect>, Vec<UiLabel>) {
    let layout = ConsentLayout::new(style.scale, surface, view.language);
    if layout.modal.w <= 0.0 || layout.modal.h <= 0.0 {
        return (None, Vec::new());
    }

    let notice = ConsentNotice::for_agent(&view.agent_name);

    shapes.push(Shape::RoundedRect {
        rect: layout.modal,
        radii: [style.px(RADIUS_12); 4],
        fill: ctx.color(style.bg1),
        border: Some(Border {
            width: style.px(1.5),
            color: ctx.color(style.accent),
        }),
        shadow: Some(Shadow {
            blur: style.px(OVERLAY_BLUR),
            offset: OVERLAY_OFFSET.map(|v| style.px(v)),
            color: ctx.color(OVERLAY_SHADOW),
        }),
    });

    let mut labels = Vec::new();

    // Title bar with icon + bilingual title.
    let icon_side = style.px(16.0).min(layout.title.h);
    let icon_rect = Rect::new(
        layout.title.x,
        layout.title.y + (layout.title.h - icon_side) * 0.5,
        icon_side,
        icon_side,
    );
    Icon::Agent.draw(icon_rect, ctx.color(style.accent), shapes);
    let title_x = icon_rect.right() + style.px(SPACE_8);
    let title_rect = Rect::new(
        title_x,
        layout.title.y,
        (layout.title.right() - title_x).max(0.0),
        layout.title.h,
    );
    let title_full = format!("{} / {}", notice.title_en, notice.title_ar);
    let title_buf = font.fit(
        ctx.font_system,
        UiSize::Title,
        UiWeight::Semibold,
        style.scale,
        &title_full,
        title_rect.w,
    );
    labels.push(UiLabel::new(
        title_buf,
        title_rect,
        UiAlign::Left,
        style.text,
    ));

    // English body.
    let max_lines = ((layout.body_en.h / layout.line_height).floor() as usize).max(1);
    let en_lines = wrap_lines(
        font,
        ctx.font_system,
        UiSize::Caption,
        UiWeight::Regular,
        style.scale,
        &notice.body_en,
        layout.body_en.w,
    );
    for (i, line) in en_lines.iter().take(max_lines).enumerate() {
        let r = Rect::new(
            layout.body_en.x,
            layout.body_en.y + i as f32 * layout.line_height,
            layout.body_en.w,
            layout.line_height,
        );
        let buf = font.fit(
            ctx.font_system,
            UiSize::Caption,
            UiWeight::Regular,
            style.scale,
            line,
            r.w,
        );
        labels.push(UiLabel::new(buf, r, UiAlign::Left, style.text_dim));
    }

    // Hairline separator between English and Arabic notice.
    shapes.push(Shape::rounded(
        Rect::new(
            layout.body_en.x,
            layout.divider_y,
            layout.body_en.w,
            style.hairline(),
        ),
        0.0,
        ctx.color(style.border),
    ));

    // Arabic body (right-aligned).
    let ar_lines = wrap_lines(
        font,
        ctx.font_system,
        UiSize::Caption,
        UiWeight::Regular,
        style.scale,
        &notice.body_ar,
        layout.body_ar.w,
    );
    for (i, line) in ar_lines.iter().take(max_lines).enumerate() {
        let r = Rect::new(
            layout.body_ar.x,
            layout.body_ar.y + i as f32 * layout.line_height,
            layout.body_ar.w,
            layout.line_height,
        );
        let buf = font.fit(
            ctx.font_system,
            UiSize::Caption,
            UiWeight::Regular,
            style.scale,
            line,
            r.w,
        );
        labels.push(UiLabel::new(buf, r, UiAlign::Right, style.text_dim));
    }

    // Cancel button.
    shapes.push(Shape::RoundedRect {
        rect: layout.cancel_button,
        radii: [style.px(RADIUS_8); 4],
        fill: ctx.color(style.bg2),
        border: Some(Border {
            width: style.hairline(),
            color: ctx.color(style.border),
        }),
        shadow: None,
    });
    let cancel_text = format!(
        "{} / {} (Esc)",
        notice.cancel_label_en, notice.cancel_label_ar
    );
    let cancel_inner = Rect::new(
        layout.cancel_button.x + style.px(SPACE_8),
        layout.cancel_button.y,
        (layout.cancel_button.w - 2.0 * style.px(SPACE_8)).max(0.0),
        layout.cancel_button.h,
    );
    let cancel_buf = font.fit(
        ctx.font_system,
        UiSize::Caption,
        UiWeight::Regular,
        style.scale,
        &cancel_text,
        cancel_inner.w,
    );
    labels.push(UiLabel::new(
        cancel_buf,
        cancel_inner,
        UiAlign::Center,
        style.text,
    ));

    // Accept button.
    shapes.push(Shape::RoundedRect {
        rect: layout.accept_button,
        radii: [style.px(RADIUS_8); 4],
        fill: ctx.color(style.accent),
        border: None,
        shadow: None,
    });
    let accept_text = format!(
        "{} / {} (Enter)",
        notice.allow_label_en, notice.allow_label_ar
    );
    let accept_inner = Rect::new(
        layout.accept_button.x + style.px(SPACE_8),
        layout.accept_button.y,
        (layout.accept_button.w - 2.0 * style.px(SPACE_8)).max(0.0),
        layout.accept_button.h,
    );
    let accept_buf = font.fit(
        ctx.font_system,
        UiSize::Caption,
        UiWeight::Semibold,
        style.scale,
        &accept_text,
        accept_inner.w,
    );
    labels.push(UiLabel::new(
        accept_buf,
        accept_inner,
        UiAlign::Center,
        style.accent_text(),
    ));

    (Some(layout.modal), labels)
}

/// Greedy word-wrapping helper using `UiFont` measurements, preserving explicit newlines.
pub fn wrap_lines(
    font: &UiFont,
    fonts: &mut cosmic_text::FontSystem,
    size: UiSize,
    weight: UiWeight,
    scale: f32,
    text: &str,
    max_width: f32,
) -> Vec<String> {
    if max_width <= 1.0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for para in text.split('\n') {
        let trimmed = para.trim();
        if trimmed.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in trimmed.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            let buf = font.shape(fonts, size, weight, scale, &candidate);
            if label_width(&buf) <= max_width || current.is_empty() {
                current = candidate;
            } else {
                out.push(current);
                current = word.to_string();
            }
        }
        if !current.is_empty() {
            out.push(current);
        }
    }
    out
}
