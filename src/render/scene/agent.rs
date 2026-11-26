//! Agent side panel rendering (`docs/design/AGENT.md` §1).
//!
//! Draws the 36 px header, scrolling step cards (Plan, Read, Command, Patch reusing the review
//! diff renderer, CommandResult, PatchApplied, FactSuggestion, Message, Denied, LimitReached),
//! empty state with three bilingual chips, "Jump to latest" pill, footer composer editor + context
//! strip, and the Context Inspector overlay sheet.
//!
//! Mirrors chrome in Arabic UI while keeping commands, paths, and diffs in terminal LTR/bidi row
//! shaping and giving model text its own paragraph direction (first strong character).

use std::rc::Rc;

use alacritty_terminal::vte::ansi::Rgb;
use glyphon::TextBounds;
use twitty_ai::engine::model::{ContextKind, StepKind, StepStatus};

use super::context::DrawContext;
use super::frame::bounds_around;
use super::review::text_line;
use crate::color::Rgba;
use crate::composer::editor::TextEditor;
use crate::layout::Rect;
use crate::render::icons::Icon;
use crate::render::row::CachedRow;
use crate::render::shapes::{Border, Shadow, Shape};
use crate::render::style::Quad;
use crate::render::text::ui::{label_width, UiAlign, UiFont, UiLabel, UiSize, UiWeight};
use crate::render::ui_style::{
    UiStyle, OVERLAY_BLUR, OVERLAY_OFFSET, OVERLAY_SHADOW, RADIUS_12, RADIUS_6, RADIUS_8, SPACE_4,
    SPACE_8,
};
use crate::review::diff::{parse as parse_diff, Limits as DiffLimits, LineKind};
use crate::ui::agent_panel::{
    arabic_count, format_context_strip, format_deny_reason, format_duration_ms, format_usage_pill,
    included_context_tokens, paragraph_is_rtl, read_tool_name, wrap_words, AgentFocus,
    AgentPanelHit, AgentPanelLayout, AgentPanelView, CardButton, CardLayout, StepEntry,
    CARD_EDITOR_MAX_LINES, COMPOSER_MAX_LINES, EMPTY_INTRO_LINE_H, EMPTY_INTRO_TOP, EXAMPLE_CHIPS,
    MAX_PATCH_DIFF_ROWS, PLAN_COLLAPSED_LINES,
};
use crate::ui::chrome::UiLanguage;

/// A terminal-shaped monospace line drawn inside the Agent panel (commands, paths, diffs, editors).
pub struct AgentText {
    pub row: Rc<CachedRow>,
    pub left: f32,
    pub top: f32,
    pub bounds: TextBounds,
}

/// Complete CPU-side drawing output of the Agent panel for one frame.
pub struct AgentScene {
    pub quads: Vec<Quad>,
    pub shapes: Vec<Shape>,
    pub labels: Vec<UiLabel>,
    pub texts: Vec<AgentText>,
}

/// Build the Agent panel scene for `view`.
pub fn build(
    ctx: &mut DrawContext,
    view: &AgentPanelView,
    style: UiStyle,
    font: &UiFont,
) -> AgentScene {
    let layout = AgentPanelLayout::build(view, ctx.metrics);
    let mut drawer = AgentDrawer {
        ctx,
        style,
        font,
        view,
        layout: &layout,
        scene: AgentScene {
            quads: Vec::new(),
            shapes: Vec::new(),
            labels: Vec::new(),
            texts: Vec::new(),
        },
    };
    drawer.draw();
    drawer.scene
}

struct AgentDrawer<'a, 'b> {
    ctx: &'a mut DrawContext<'b>,
    style: UiStyle,
    font: &'a UiFont,
    view: &'a AgentPanelView,
    layout: &'a AgentPanelLayout,
    scene: AgentScene,
}

impl AgentDrawer<'_, '_> {
    fn lang(&self) -> UiLanguage {
        self.view.language
    }

    fn mirrored(&self) -> bool {
        self.layout.mirrored
    }

    fn start_align(&self) -> UiAlign {
        if self.mirrored() {
            UiAlign::Right
        } else {
            UiAlign::Left
        }
    }

    fn draw(&mut self) {
        let s = self.style;
        let l = self.layout;
        if l.panel.w <= 0.0 || l.panel.h <= 0.0 {
            return;
        }

        // Panel background + inner divider hairline.
        self.rect_fill(l.panel, 0.0, s.bg0, None);
        self.rect_fill(l.border_line, 0.0, s.border, None);

        self.draw_header();
        self.draw_body();
        self.draw_footer();

        if l.inspector.is_some() {
            self.draw_inspector();
        }
    }

    fn draw_header(&mut self) {
        let s = self.style;
        let l = self.layout;
        let h = l.header;
        if h.h <= 0.0 {
            return;
        }
        self.rect_fill(h, 0.0, s.bg1, None);
        // Bottom hairline under header.
        let rule = s.hairline();
        self.rect_fill(
            Rect::new(h.x, h.bottom() - rule, h.w, rule),
            0.0,
            s.border,
            None,
        );

        // Leading icon + provider name + project name.
        let icon_side = s.px(16.0).min(h.h - s.px(8.0)).max(0.0);
        let icon_y = h.y + (h.h - icon_side) * 0.5;
        let title_area = l.title_area;
        let (icon_rect, text_area) = if self.mirrored() {
            let ir = Rect::new(
                (title_area.right() - icon_side).max(title_area.x),
                icon_y,
                icon_side,
                icon_side,
            );
            let tr = Rect::new(
                title_area.x,
                title_area.y,
                (ir.x - s.px(6.0) - title_area.x).max(0.0),
                title_area.h,
            );
            (ir, tr)
        } else {
            let ir = Rect::new(title_area.x, icon_y, icon_side, icon_side);
            let tx = ir.right() + s.px(6.0);
            let tr = Rect::new(
                tx,
                title_area.y,
                (title_area.right() - tx).max(0.0),
                title_area.h,
            );
            (ir, tr)
        };

        let icon_color = if self.view.is_running() {
            s.accent
        } else {
            s.text_dim
        };
        Icon::Agent.draw(
            icon_rect,
            self.ctx.color(icon_color),
            &mut self.scene.shapes,
        );

        let provider_label = self
            .view
            .session
            .as_ref()
            .and_then(|sess| sess.provider.as_ref().map(|p| p.display.as_str()))
            .unwrap_or_else(|| self.lang().text("Agent", "الوكيل"));

        let header_title = if self.view.project_name.is_empty() {
            provider_label.to_owned()
        } else {
            format!("{provider_label} · {}", self.view.project_name)
        };
        self.push_label(
            text_area,
            &header_title,
            UiSize::Body,
            UiWeight::Semibold,
            self.start_align(),
            s.text,
            None,
        );

        // Usage / cost pill.
        if let (Some(pill), Some(sess)) = (l.usage_pill, &self.view.session) {
            if let Some(txt) = format_usage_pill(sess.usage, self.lang()) {
                self.rect_fill(pill, RADIUS_6, s.bg2, Some(s.border));
                self.push_label(
                    inset(pill, s.px(4.0), 0.0),
                    &txt,
                    UiSize::Small,
                    UiWeight::Regular,
                    UiAlign::Center,
                    s.text_dim,
                    None,
                );
            }
        }

        // Stop button (only while running).
        if let Some(stop) = l.stop_button {
            let hovered = self.view.hover == AgentPanelHit::Stop;
            let bg = if hovered {
                s.bg1.mix(s.danger, 0.28)
            } else {
                s.bg1.mix(s.danger, 0.16)
            };
            self.rect_fill(stop, RADIUS_6, bg, Some(s.danger));
            self.push_label(
                stop,
                self.lang().text("Stop", "إيقاف"),
                UiSize::Small,
                UiWeight::Semibold,
                UiAlign::Center,
                s.danger,
                None,
            );
        }

        // Close button.
        let close_hover = self.view.hover == AgentPanelHit::Close;
        if close_hover {
            self.rect_fill(l.close_button, RADIUS_6, s.bg3, None);
        }
        let cside = s.px(14.0).min(l.close_button.w).min(l.close_button.h);
        let crect = Rect::new(
            l.close_button.x + (l.close_button.w - cside) * 0.5,
            l.close_button.y + (l.close_button.h - cside) * 0.5,
            cside,
            cside,
        );
        Icon::Close.draw(
            crect,
            self.ctx
                .color(if close_hover { s.text } else { s.text_dim }),
            &mut self.scene.shapes,
        );
    }

    fn draw_body(&mut self) {
        let l = self.layout;
        if l.body.h <= 0.0 || l.body.w <= 0.0 {
            return;
        }
        let body_clip = rect_to_bounds(l.body);

        let steps = self.view.steps();
        if steps.is_empty() {
            self.draw_empty_state(body_clip);
            return;
        }

        for (entry, card) in steps.iter().zip(&l.cards) {
            if card.rect.bottom() < l.body.y || card.rect.y > l.body.bottom() {
                continue;
            }
            self.draw_card(entry, card, body_clip);
        }

        // Floating "Jump to latest" pill when scrolled up.
        if let Some(pill) = l.jump_to_latest {
            let s = self.style;
            let hovered = self.view.hover == AgentPanelHit::JumpToLatest;
            let fill = if hovered { s.bg3 } else { s.bg2 };
            self.scene.shapes.push(Shape::RoundedRect {
                rect: pill,
                radii: [s.px(RADIUS_12); 4],
                fill: self.ctx.color(fill),
                border: Some(Border {
                    width: s.hairline(),
                    color: self.ctx.color(s.accent),
                }),
                shadow: Some(Shadow {
                    blur: s.px(8.0),
                    offset: [0.0, s.px(2.0)],
                    color: self.ctx.color(OVERLAY_SHADOW),
                }),
            });
            self.push_label(
                pill,
                self.lang().text("Jump to latest", "الانتقال للأحدث"),
                UiSize::Small,
                UiWeight::Semibold,
                UiAlign::Center,
                s.text,
                Some(body_clip),
            );
        }
    }

    fn draw_empty_state(&mut self, body_clip: TextBounds) {
        let s = self.style;
        let b = self.layout.body;
        let pad = s.px(12.0);
        let icon_side = s.px(24.0);
        let icon_x = if self.mirrored() {
            b.right() - pad - icon_side
        } else {
            b.x + pad
        };
        Icon::Agent.draw(
            Rect::new(icon_x, b.y + s.px(16.0), icon_side, icon_side),
            self.ctx.color(s.accent),
            &mut self.scene.shapes,
        );

        let intro_w = (b.w - 2.0 * pad).max(0.0);
        let mut y = b.y + s.px(EMPTY_INTRO_TOP);
        for line in self.layout.empty_intro.clone() {
            self.push_label(
                Rect::new(b.x + pad, y, intro_w, s.px(18.0)),
                &line,
                UiSize::Caption,
                UiWeight::Regular,
                self.start_align(),
                s.text_dim,
                Some(body_clip),
            );
            y += s.px(EMPTY_INTRO_LINE_H);
        }

        for (idx, &chip_rect) in self.layout.empty_chips.iter().enumerate() {
            let Some(chip) = EXAMPLE_CHIPS.get(idx) else {
                continue;
            };
            let hovered = self.view.hover == AgentPanelHit::ExampleChip(idx);
            let fill = if hovered { s.bg2 } else { s.bg1 };
            let border = if hovered { s.accent } else { s.border };
            self.rect_fill(chip_rect, RADIUS_8, fill, Some(border));
            self.push_label(
                inset(chip_rect, s.px(10.0), 0.0),
                chip.label(self.lang()),
                UiSize::Caption,
                UiWeight::Regular,
                self.start_align(),
                s.text,
                Some(body_clip),
            );
        }
    }

    fn draw_card(&mut self, entry: &StepEntry, card: &CardLayout, body_clip: TextBounds) {
        let s = self.style;
        let selected = self.view.selected_step == Some(entry.step.id);
        let card_focused = self.view.focused && self.view.focus == AgentFocus::Card && selected;
        let is_denied = matches!(entry.step.kind, StepKind::Denied { .. })
            || entry.step.status == StepStatus::Denied;
        let fill = if is_denied {
            s.bg1.mix(s.danger, 0.10)
        } else if selected {
            s.bg2
        } else {
            s.bg1
        };
        let border = if card_focused {
            Some(s.accent)
        } else if is_denied {
            Some(s.danger.mix(s.border, 0.45))
        } else {
            Some(s.border)
        };

        // Clamp card shape cleanly inside body viewport vertically so cards never bleed over header/footer.
        let clamped_rect = clamp_rect_y(card.rect, self.layout.body);
        if clamped_rect.h <= 0.0 {
            return;
        }
        self.rect_fill(clamped_rect, RADIUS_8, fill, border);

        // 2 px leading status strip (AGENT.md §1.2).
        let strip_color = status_strip_color(&entry.step.status, s);
        let clamped_strip = clamp_rect_y(card.strip, self.layout.body);
        if clamped_strip.h > 0.0 {
            self.rect_fill(clamped_strip, 1.0, strip_color, None);
        }

        let card_clip = intersect_bounds(rect_to_bounds(clamped_rect), body_clip);
        let inner = card.inner;
        let title_rect = Rect::new(inner.x, inner.y, inner.w, s.px(18.0));

        let editing = self
            .view
            .edit_mode
            .as_ref()
            .filter(|e| e.step_id() == entry.step.id);

        if let Some(edit) = editing {
            let heading = if edit.is_patch() {
                self.lang().text("Edit patch", "تعديل الرقعة")
            } else {
                self.lang().text("Edit command", "تعديل الأمر")
            };
            self.push_label(
                title_rect,
                heading,
                UiSize::Caption,
                UiWeight::Semibold,
                self.start_align(),
                s.accent,
                Some(card_clip),
            );
            if let Some(ed_rect) = card.editor_rect {
                self.draw_editor_box(
                    ed_rect,
                    edit.editor(),
                    true,
                    CARD_EDITOR_MAX_LINES,
                    card_clip,
                );
            }
        } else {
            self.draw_card_content(entry, card, title_rect, card_clip);
        }

        // Action buttons row.
        let waiting_buttons = entry.waiting_buttons();
        for &(btn, btn_rect) in &card.buttons {
            if btn_rect.bottom() < self.layout.body.y || btn_rect.y > self.layout.body.bottom() {
                continue;
            }
            let hovered = self.view.hover
                == AgentPanelHit::Card {
                    step: entry.step.id,
                    button: Some(btn),
                };
            let key_focused =
                card_focused && waiting_buttons.get(self.view.focused_button).copied() == Some(btn);
            self.draw_card_button(
                btn,
                btn_rect,
                &entry.step.kind,
                entry.expanded,
                hovered,
                key_focused,
                card_clip,
            );
        }
    }

    fn draw_card_content(
        &mut self,
        entry: &StepEntry,
        card: &CardLayout,
        title_rect: Rect,
        clip: TextBounds,
    ) {
        let s = self.style;
        let inner = card.inner;
        let mono_lh = self.ctx.metrics.line_height.max(s.px(17.0));

        match &entry.step.kind {
            StepKind::Plan { text } => {
                self.push_label(
                    title_rect,
                    self.lang().text("Plan", "الخطة"),
                    UiSize::Small,
                    UiWeight::Semibold,
                    self.start_align(),
                    s.text_dim,
                    Some(clip),
                );
                let max_lines = if entry.expanded {
                    24
                } else {
                    PLAN_COLLAPSED_LINES
                };
                self.draw_model_paragraph(
                    text,
                    inner.x,
                    title_rect.bottom() + s.px(4.0),
                    inner.w,
                    max_lines,
                    s.text,
                    clip,
                );
            }
            StepKind::Read {
                tool,
                target,
                preview,
            } => {
                let heading = format!(
                    "{} · {}",
                    self.lang().text("Read", "قراءة"),
                    read_tool_name(*tool)
                );
                self.push_label(
                    title_rect,
                    &heading,
                    UiSize::Small,
                    UiWeight::Semibold,
                    self.start_align(),
                    s.text_dim,
                    Some(clip),
                );
                // Path keeps terminal LTR/bidi shaping.
                let path_y = title_rect.bottom() + s.px(2.0);
                self.push_mono_line(
                    Rect::new(inner.x, path_y, inner.w, mono_lh),
                    target,
                    s.text,
                    clip,
                );
                if entry.expanded && !preview.is_empty() {
                    let lines: Vec<&str> = preview.lines().take(8).collect();
                    let box_y = path_y + mono_lh + s.px(4.0);
                    let box_h = lines.len() as f32 * mono_lh + s.px(8.0);
                    let box_rect =
                        clamp_rect_y(Rect::new(inner.x, box_y, inner.w, box_h), self.layout.body);
                    if box_rect.h > 0.0 {
                        self.rect_fill(box_rect, RADIUS_6, s.bg2, None);
                    }
                    for (i, line) in lines.iter().enumerate() {
                        let ly = box_y + s.px(4.0) + i as f32 * mono_lh;
                        self.push_mono_line(
                            Rect::new(
                                inner.x + s.px(6.0),
                                ly,
                                (inner.w - s.px(12.0)).max(0.0),
                                mono_lh,
                            ),
                            line,
                            s.text_dim,
                            clip,
                        );
                    }
                }
            }
            StepKind::ReadApproval { tool, target } => {
                let heading = format!(
                    "{} · {}",
                    self.lang()
                        .text("Read needs approval", "قراءة تحتاج موافقة"),
                    read_tool_name(*tool)
                );
                self.push_label(
                    title_rect,
                    &heading,
                    UiSize::Small,
                    UiWeight::Semibold,
                    self.start_align(),
                    s.warn,
                    Some(clip),
                );
                // Path keeps terminal LTR/bidi shaping.
                let path_y = title_rect.bottom() + s.px(2.0);
                self.push_mono_line(
                    Rect::new(inner.x, path_y, inner.w, mono_lh),
                    target,
                    s.text,
                    clip,
                );
            }
            StepKind::Command {
                command,
                cwd,
                reason,
                after_untrusted_data,
            } => {
                let title = if entry.user_edited {
                    self.lang()
                        .text("Command (user-edited)", "أمر (عدّله المستخدم)")
                } else {
                    self.lang().text("Command", "أمر")
                };
                self.push_label(
                    title_rect,
                    title,
                    UiSize::Small,
                    UiWeight::Semibold,
                    self.start_align(),
                    if entry.step.status == StepStatus::WaitingApproval {
                        s.warn
                    } else {
                        s.text_dim
                    },
                    Some(clip),
                );
                // Exact command in mono on bg2 (AGENT.md §1.2).
                let cmd_lines = wrap_mono_lines(
                    command,
                    (inner.w - s.px(12.0)).max(s.px(60.0)),
                    self.ctx.metrics.char_width.max(s.px(7.5)),
                    4,
                );
                let box_y = title_rect.bottom() + s.px(4.0);
                let box_h = cmd_lines.len() as f32 * mono_lh + s.px(10.0);
                let cmd_box =
                    clamp_rect_y(Rect::new(inner.x, box_y, inner.w, box_h), self.layout.body);
                if cmd_box.h > 0.0 {
                    self.rect_fill(cmd_box, RADIUS_6, s.bg2, Some(s.border));
                }
                for (i, line) in cmd_lines.iter().enumerate() {
                    let ly = box_y + s.px(5.0) + i as f32 * mono_lh;
                    self.push_mono_line(
                        Rect::new(
                            inner.x + s.px(6.0),
                            ly,
                            (inner.w - s.px(12.0)).max(0.0),
                            mono_lh,
                        ),
                        line,
                        s.text,
                        clip,
                    );
                }
                let mut cur_y = box_y + box_h + s.px(4.0);
                // cwd in dim mono with full line height so descenders never clip.
                let cwd_str = cwd.display().to_string();
                self.push_mono_line(
                    Rect::new(inner.x, cur_y, inner.w, mono_lh),
                    &cwd_str,
                    s.text_faint,
                    clip,
                );
                cur_y += mono_lh + s.px(2.0);
                if !reason.is_empty() {
                    let align = if paragraph_is_rtl(reason) {
                        UiAlign::Right
                    } else {
                        UiAlign::Left
                    };
                    self.push_label(
                        Rect::new(inner.x, cur_y, inner.w, s.px(18.0)),
                        reason,
                        UiSize::Caption,
                        UiWeight::Regular,
                        align,
                        s.text_dim,
                        Some(clip),
                    );
                    cur_y += s.px(19.0);
                }
                // AGENT.md §1.2 & §2.4: warning note with vector Icon::Warning when proposed after data.
                if *after_untrusted_data {
                    let note = self.lang().text(
                        "Suggested after reading output — review carefully",
                        "مقترح بعد قراءة المخرجات — راجعه بعناية",
                    );
                    let row_r = Rect::new(inner.x, cur_y, inner.w, s.px(17.0));
                    let icon_side = s.px(12.0);
                    let icon_logical = Rect::new(
                        row_r.x,
                        row_r.y + (row_r.h - icon_side) * 0.5,
                        icon_side,
                        icon_side,
                    );
                    let text_logical = Rect::new(
                        icon_logical.right() + s.px(4.0),
                        row_r.y,
                        (row_r.right() - (icon_logical.right() + s.px(4.0))).max(0.0),
                        row_r.h,
                    );
                    Icon::Warning.draw(
                        self.lang().mirror(icon_logical, row_r),
                        self.ctx.color(s.warn),
                        &mut self.scene.shapes,
                    );
                    self.push_label(
                        self.lang().mirror(text_logical, row_r),
                        note,
                        UiSize::Small,
                        UiWeight::Semibold,
                        self.start_align(),
                        s.warn,
                        Some(clip),
                    );
                }
            }
            StepKind::Patch {
                files,
                diff,
                reason,
            } => {
                let title = if entry.user_edited {
                    self.lang()
                        .text("Patch (user-edited)", "تعديل (عدّله المستخدم)")
                } else {
                    self.lang().text("Patch", "تعديل")
                };
                self.push_label(
                    title_rect,
                    title,
                    UiSize::Small,
                    UiWeight::Semibold,
                    self.start_align(),
                    if entry.step.status == StepStatus::WaitingApproval {
                        s.warn
                    } else {
                        s.text_dim
                    },
                    Some(clip),
                );
                let mut cur_y = title_rect.bottom() + s.px(4.0);
                let cw = self.ctx.metrics.char_width.max(s.px(7.5));
                for file in files.iter().take(6) {
                    // File paths and `+N -M` counts are code: always LTR in `+N -M` order.
                    let row_r = Rect::new(inner.x, cur_y, inner.w, mono_lh);
                    let add_s = format!("+{}", file.added);
                    let del_s = format!("-{}", file.removed);
                    let add_w = add_s.chars().count() as f32 * cw + s.px(2.0);
                    let del_w = del_s.chars().count() as f32 * cw + s.px(2.0);
                    let gap = s.px(6.0);
                    let total_cnt_w = add_w + gap + del_w;
                    let count_w = total_cnt_w.min(inner.w * 0.45);
                    let path_r = Rect::new(
                        row_r.x,
                        row_r.y,
                        (row_r.w - count_w - s.px(6.0)).max(0.0),
                        row_r.h,
                    );
                    let cnt_r = Rect::new(path_r.right() + s.px(6.0), row_r.y, count_w, row_r.h);
                    let path_str = file.path.display().to_string();
                    self.push_mono_line(path_r, &path_str, s.text, clip);
                    let start_x = (cnt_r.right() - total_cnt_w).max(cnt_r.x);
                    self.push_mono_line(
                        Rect::new(start_x, cnt_r.y, add_w.min(cnt_r.w), cnt_r.h),
                        &add_s,
                        s.ok,
                        clip,
                    );
                    let del_x = start_x + add_w + gap;
                    if del_x < cnt_r.right() {
                        self.push_mono_line(
                            Rect::new(del_x, cnt_r.y, (cnt_r.right() - del_x).min(del_w), cnt_r.h),
                            &del_s,
                            s.danger,
                            clip,
                        );
                    }
                    cur_y += mono_lh;
                }
                if !reason.is_empty() {
                    let align = if paragraph_is_rtl(reason) {
                        UiAlign::Right
                    } else {
                        UiAlign::Left
                    };
                    self.push_label(
                        Rect::new(inner.x, cur_y, inner.w, s.px(17.0)),
                        reason,
                        UiSize::Caption,
                        UiWeight::Regular,
                        align,
                        s.text_dim,
                        Some(clip),
                    );
                }
                if let Some(diff_rect) = card.diff_rect {
                    self.draw_patch_diff(diff, diff_rect, clip);
                }
            }
            StepKind::CommandResult {
                exit,
                duration_ms,
                tail,
            } => {
                let exit_ok = *exit == Some(0);
                let status_txt = match exit {
                    Some(0) => format!(
                        "{} · {}",
                        self.lang().text("Exit 0", "نجاح 0"),
                        format_duration_ms(*duration_ms)
                    ),
                    Some(code) => format!(
                        "{} {code} · {}",
                        self.lang().text("Exit", "خروج"),
                        format_duration_ms(*duration_ms)
                    ),
                    None => format!(
                        "{} · {}",
                        self.lang().text("Finished", "انتهى"),
                        format_duration_ms(*duration_ms)
                    ),
                };
                self.push_label(
                    title_rect,
                    &status_txt,
                    UiSize::Small,
                    UiWeight::Semibold,
                    self.start_align(),
                    if exit_ok {
                        s.ok
                    } else if exit.is_some() {
                        s.danger
                    } else {
                        s.text_dim
                    },
                    Some(clip),
                );
                if !tail.trim().is_empty() {
                    let lines: Vec<&str> = tail.lines().take(6).collect();
                    let box_y = title_rect.bottom() + s.px(4.0);
                    let box_h = lines.len() as f32 * mono_lh + s.px(8.0);
                    let box_r =
                        clamp_rect_y(Rect::new(inner.x, box_y, inner.w, box_h), self.layout.body);
                    if box_r.h > 0.0 {
                        self.rect_fill(box_r, RADIUS_6, s.bg2, None);
                    }
                    for (i, line) in lines.iter().enumerate() {
                        let ly = box_y + s.px(4.0) + i as f32 * mono_lh;
                        self.push_mono_line(
                            Rect::new(
                                inner.x + s.px(6.0),
                                ly,
                                (inner.w - s.px(12.0)).max(0.0),
                                mono_lh,
                            ),
                            line,
                            s.text_dim,
                            clip,
                        );
                    }
                }
            }
            StepKind::PatchApplied {
                files,
                undo_available,
            } => {
                let heading = if *undo_available {
                    self.lang().text("Patch applied", "تم تطبيق التعديل")
                } else {
                    self.lang()
                        .text("Patch applied (undo unavailable)", "تم تطبيق التعديل")
                };
                self.push_label(
                    title_rect,
                    heading,
                    UiSize::Small,
                    UiWeight::Semibold,
                    self.start_align(),
                    s.ok,
                    Some(clip),
                );
                let mut cur_y = title_rect.bottom() + s.px(4.0);
                for path in files.iter().take(5) {
                    let p = path.display().to_string();
                    self.push_mono_line(
                        Rect::new(inner.x, cur_y, inner.w, s.px(17.0)),
                        &p,
                        s.text_dim,
                        clip,
                    );
                    cur_y += s.px(18.0);
                }
            }
            StepKind::FactSuggestion { text } => {
                self.push_label(
                    title_rect,
                    self.lang()
                        .text("Remember for this project?", "حفظ لهذا المشروع؟"),
                    UiSize::Small,
                    UiWeight::Semibold,
                    self.start_align(),
                    s.accent,
                    Some(clip),
                );
                self.draw_model_paragraph(
                    text,
                    inner.x,
                    title_rect.bottom() + s.px(4.0),
                    inner.w,
                    4,
                    s.text,
                    clip,
                );
            }
            StepKind::Message { text } => {
                self.push_label(
                    title_rect,
                    self.lang().text("Agent", "الوكيل"),
                    UiSize::Small,
                    UiWeight::Semibold,
                    self.start_align(),
                    s.text_dim,
                    Some(clip),
                );
                self.draw_model_paragraph(
                    text,
                    inner.x,
                    title_rect.bottom() + s.px(4.0),
                    inner.w,
                    18,
                    s.text,
                    clip,
                );
            }
            StepKind::Denied { what, reason } => {
                self.push_label(
                    title_rect,
                    self.lang().text("Denied", "مرفوض"),
                    UiSize::Small,
                    UiWeight::Semibold,
                    self.start_align(),
                    s.danger,
                    Some(clip),
                );
                let mut cur_y = title_rect.bottom() + s.px(4.0);
                let wrapped =
                    wrap_mono_lines(what, inner.w, self.ctx.metrics.char_width.max(s.px(7.5)), 3);
                for line in wrapped {
                    self.push_mono_line(
                        Rect::new(inner.x, cur_y, inner.w, mono_lh),
                        &line,
                        s.text,
                        clip,
                    );
                    cur_y += mono_lh;
                }
                let why = format_deny_reason(reason, self.lang());
                self.push_label(
                    Rect::new(inner.x, cur_y + s.px(2.0), inner.w, s.px(18.0)),
                    &why,
                    UiSize::Caption,
                    UiWeight::Regular,
                    self.start_align(),
                    s.danger,
                    Some(clip),
                );
            }
            StepKind::LimitReached { what } => {
                self.push_label(
                    title_rect,
                    self.lang().text("Limit reached", "تم بلوغ الحد"),
                    UiSize::Small,
                    UiWeight::Semibold,
                    self.start_align(),
                    s.warn,
                    Some(clip),
                );
                self.draw_model_paragraph(
                    what,
                    inner.x,
                    title_rect.bottom() + s.px(4.0),
                    inner.w,
                    3,
                    s.text,
                    clip,
                );
            }
        }
    }

    /// Render an expanded Patch card's unified diff reusing the review panel's diff parser,
    /// dim gutter, green/red row tints, and terminal row shaping (`review::text_line`).
    fn draw_patch_diff(&mut self, raw_diff: &str, rect: Rect, parent_clip: TextBounds) {
        let s = self.style;
        let clamped = clamp_rect_y(rect, self.layout.body);
        if clamped.h <= 0.0 {
            return;
        }
        self.rect_fill(clamped, RADIUS_6, s.bg0, Some(s.border));
        let diff_clip = intersect_bounds(rect_to_bounds(clamped), parent_clip);

        let parsed = parse_diff(raw_diff.as_bytes(), false, &DiffLimits::default());
        let lh = self.ctx.metrics.line_height.max(s.px(17.0));
        let cw = self.ctx.metrics.char_width.max(s.px(7.5));
        let gutter_w = (cw * 5.0).min(rect.w * 0.25);
        let mut y = rect.y + s.px(4.0);
        let mut drawn = 0usize;

        if parsed.files.is_empty() {
            // Fallback for simple raw unified diffs without `diff --git` header.
            for line in raw_diff.lines().take(MAX_PATCH_DIFF_ROWS) {
                if y + lh > rect.bottom() {
                    break;
                }
                let row_r = Rect::new(rect.x + s.px(1.0), y, (rect.w - s.px(2.0)).max(0.0), lh);
                let tint = if line.starts_with('+') && !line.starts_with("+++") {
                    Some(s.ok)
                } else if line.starts_with('-') && !line.starts_with("---") {
                    Some(s.danger)
                } else {
                    None
                };
                if let Some(color) = tint {
                    let r_clamped = clamp_rect_y(row_r, self.layout.body);
                    if r_clamped.h > 0.0 {
                        self.rect_fill(r_clamped, 0.0, s.bg1.mix(color, 0.16), None);
                    }
                }
                self.push_mono_line(
                    Rect::new(row_r.x + s.px(6.0), y, (row_r.w - s.px(10.0)).max(0.0), lh),
                    line,
                    s.text,
                    diff_clip,
                );
                y += lh;
            }
            return;
        }

        for file in &parsed.files {
            if drawn >= MAX_PATCH_DIFF_ROWS || y + lh > rect.bottom() {
                break;
            }
            // File row header (matches `review::file` styling).
            let file_r = clamp_rect_y(
                Rect::new(rect.x + s.px(2.0), y, (rect.w - s.px(4.0)).max(0.0), lh),
                self.layout.body,
            );
            if file_r.h > 0.0 {
                self.rect_fill(file_r, RADIUS_6, s.bg3, None);
            }
            self.push_mono_line(
                Rect::new(rect.x + s.px(8.0), y, (rect.w - s.px(16.0)).max(0.0), lh),
                file.path(),
                s.text,
                diff_clip,
            );
            y += lh;
            drawn += 1;

            for hunk in &file.hunks {
                if drawn >= MAX_PATCH_DIFF_ROWS || y + lh > rect.bottom() {
                    break;
                }
                let hunk_r = clamp_rect_y(
                    Rect::new(rect.x + s.px(2.0), y, (rect.w - s.px(4.0)).max(0.0), lh),
                    self.layout.body,
                );
                if hunk_r.h > 0.0 {
                    self.rect_fill(hunk_r, RADIUS_6, s.bg2, None);
                }
                let hdr = hunk.header();
                // Hunk headers (`@@ -1,3 +1,4 @@`) are code: always LTR monospace.
                self.push_mono_line(
                    Rect::new(rect.x + s.px(8.0), y, (rect.w - s.px(16.0)).max(0.0), lh),
                    &hdr,
                    s.text_dim,
                    diff_clip,
                );
                y += lh;
                drawn += 1;

                for line in &hunk.lines {
                    if drawn >= MAX_PATCH_DIFF_ROWS || y + lh > rect.bottom() {
                        break;
                    }
                    let tint = match line.kind {
                        LineKind::Add => Some(s.ok),
                        LineKind::Del => Some(s.danger),
                        LineKind::Context => None,
                    };
                    let row_r = clamp_rect_y(
                        Rect::new(rect.x + s.px(1.0), y, (rect.w - s.px(2.0)).max(0.0), lh),
                        self.layout.body,
                    );
                    if row_r.h > 0.0 {
                        if let Some(color) = tint {
                            self.rect_fill(row_r, 0.0, s.bg1.mix(color, 0.16), None);
                        }
                        let gutter_r = Rect::new(row_r.x, row_r.y, gutter_w.min(row_r.w), row_r.h);
                        self.rect_fill(
                            gutter_r,
                            0.0,
                            tint.map_or(s.bg2, |c| s.bg2.mix(c, 0.19)),
                            None,
                        );
                    }
                    let num = line
                        .new
                        .or(line.old)
                        .map_or_else(|| "   ".to_owned(), |n| format!("{n:>3}"));
                    let prefix = match line.kind {
                        LineKind::Add => '+',
                        LineKind::Del => '-',
                        LineKind::Context => ' ',
                    };
                    self.push_mono_line(
                        Rect::new(rect.x + s.px(4.0), y, (gutter_w - s.px(4.0)).max(0.0), lh),
                        &num,
                        s.text_faint,
                        diff_clip,
                    );
                    let code_x = rect.x + gutter_w + s.px(4.0);
                    let code_text = format!("{prefix}{}", line.text);
                    self.push_mono_line(
                        Rect::new(code_x, y, (rect.right() - code_x - s.px(4.0)).max(0.0), lh),
                        &code_text,
                        s.text,
                        diff_clip,
                    );
                    y += lh;
                    drawn += 1;
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_card_button(
        &mut self,
        btn: CardButton,
        rect: Rect,
        kind: &StepKind,
        expanded: bool,
        hovered: bool,
        key_focused: bool,
        clip: TextBounds,
    ) {
        let s = self.style;
        let is_primary = matches!(btn, CardButton::Primary | CardButton::ApplyEdit);
        let is_deny = matches!(btn, CardButton::Deny | CardButton::CancelEdit);

        let (fill, border, fg) = if is_primary {
            let base = if hovered {
                s.accent.mix(s.text, 0.12)
            } else {
                s.accent
            };
            (base, key_focused.then_some(s.text), s.accent_text())
        } else if is_deny {
            // AGENT.md §1.2: Deny is a quiet outline with readable foreground contrast.
            let bg = if hovered {
                s.bg3
            } else {
                Rgba::new(0.0, 0.0, 0.0, 0.0)
            };
            let brd = if key_focused {
                s.accent
            } else if hovered {
                s.text_dim
            } else {
                s.border
            };
            (bg, Some(brd), s.text)
        } else {
            let bg = if hovered { s.bg3 } else { s.bg2 };
            let brd = if key_focused { s.accent } else { s.border };
            (bg, Some(brd), s.text)
        };

        self.rect_fill(rect, RADIUS_6, fill, border);

        let label = match btn {
            CardButton::Primary => match kind {
                StepKind::Command { .. } => self.lang().text("Run", "تشغيل"),
                StepKind::Patch { .. } => self.lang().text("Apply", "تطبيق"),
                StepKind::FactSuggestion { .. } => self.lang().text("Remember", "حفظ"),
                StepKind::LimitReached { .. } => self.lang().text("Continue", "متابعة"),
                StepKind::ReadApproval { .. } => self.lang().text("Allow read", "اسمح بالقراءة"),
                _ => self.lang().text("Approve", "موافقة"),
            },
            CardButton::Edit => self.lang().text("Edit", "تعديل"),
            CardButton::Deny => match kind {
                StepKind::FactSuggestion { .. } => self.lang().text("Dismiss", "تجاهل"),
                StepKind::LimitReached { .. } => self.lang().text("Stop", "إيقاف"),
                _ => self.lang().text("Deny", "رفض"),
            },
            CardButton::Expand => {
                if expanded {
                    self.lang().text("Collapse", "طيّ")
                } else {
                    self.lang().text("Expand", "توسيع")
                }
            }
            CardButton::OpenBlock => self.lang().text("Open block", "افتح الكتلة"),
            CardButton::Undo => self.lang().text("Undo", "تراجع"),
            CardButton::Copy => self.lang().text("Copy", "نسخ"),
            CardButton::ApplyEdit => self.lang().text("Apply edited", "تطبيق المعدّل"),
            CardButton::CancelEdit => self.lang().text("Cancel", "إلغاء"),
        };

        self.push_label(
            inset(rect, s.px(4.0), 0.0),
            label,
            UiSize::Small,
            if is_primary {
                UiWeight::Semibold
            } else {
                UiWeight::Regular
            },
            UiAlign::Center,
            fg,
            Some(clip),
        );
    }

    fn draw_footer(&mut self) {
        let s = self.style;
        let l = self.layout;
        let f = l.footer;
        if f.h <= 0.0 {
            return;
        }
        self.rect_fill(f, 0.0, s.bg1, None);
        let rule = s.hairline();
        self.rect_fill(Rect::new(f.x, f.y, f.w, rule), 0.0, s.border, None);

        // One-line context strip (`Rules · 3 memories · 2 blocks`).
        let strip_hovered = self.view.hover == AgentPanelHit::ContextStrip;
        if strip_hovered || self.view.inspector_open {
            self.rect_fill(l.context_strip, RADIUS_6, s.bg2, None);
        }
        let strip_txt = format_context_strip(&self.view.context, self.lang());
        self.push_label(
            inset(l.context_strip, s.px(6.0), 0.0),
            &strip_txt,
            UiSize::Small,
            UiWeight::Regular,
            self.start_align(),
            if self.view.inspector_open {
                s.accent
            } else {
                s.text_dim
            },
            None,
        );

        // Multi-line composer editor box.
        let composer_focused = self.view.focused
            && !self.view.inspector_open
            && self.view.edit_mode.is_none()
            && self.view.focus == AgentFocus::Composer;
        let footer_clip = rect_to_bounds(f);
        self.draw_editor_box(
            l.composer_box,
            &self.view.composer,
            composer_focused,
            COMPOSER_MAX_LINES,
            footer_clip,
        );

        // Placeholder when composer is empty.
        if self.view.composer.is_empty() && self.view.composer.preedit().is_none() {
            let ph = self.lang().text("Ask the agent…", "اسأل الوكيل…");
            self.push_label(
                inset(l.composer_box, s.px(8.0), 0.0),
                ph,
                UiSize::Caption,
                UiWeight::Regular,
                self.start_align(),
                s.text_faint,
                Some(rect_to_bounds(l.composer_box)),
            );
        }

        // Send button.
        let has_text = !self.view.composer.text().trim().is_empty();
        let send_hover = self.view.hover == AgentPanelHit::Send;
        let (fill, fg) = if has_text {
            let bg = if send_hover {
                s.accent.mix(s.text, 0.12)
            } else {
                s.accent
            };
            (bg, s.accent_text())
        } else {
            (if send_hover { s.bg3 } else { s.bg2 }, s.text_faint)
        };
        self.rect_fill(l.send_button, RADIUS_6, fill, None);
        self.push_label(
            l.send_button,
            self.lang().text("Send", "إرسال"),
            UiSize::Small,
            UiWeight::Semibold,
            UiAlign::Center,
            fg,
            Some(footer_clip),
        );
    }

    fn draw_editor_box(
        &mut self,
        rect: Rect,
        editor: &TextEditor,
        focused: bool,
        max_lines: usize,
        parent_clip: TextBounds,
    ) {
        let s = self.style;
        let clamped = clamp_rect_y(rect, self.layout.panel);
        if clamped.h <= 0.0 {
            return;
        }
        self.rect_fill(
            clamped,
            RADIUS_8,
            s.bg2,
            Some(if focused { s.accent } else { s.border }),
        );
        let clip = intersect_bounds(
            rect_to_bounds(inset(clamped, s.px(4.0), s.px(2.0))),
            parent_clip,
        );
        let lh = self.ctx.metrics.line_height.max(s.px(18.0));
        let lines = editor.lines();
        let (caret_line, caret_byte) = editor.caret_line_col();
        let start_line = caret_line.saturating_sub(max_lines.saturating_sub(1));
        let pad_x = s.px(8.0);
        let pad_y = s.px(6.0);
        let text_w = (rect.w - 2.0 * pad_x).max(0.0);

        for (vis_idx, line_idx) in (start_line..lines.len()).take(max_lines).enumerate() {
            let y = rect.y + pad_y + vis_idx as f32 * lh;
            let raw = lines[line_idx];
            let display = if line_idx == caret_line {
                if let Some(pre) = editor.preedit() {
                    let mut s_buf = String::with_capacity(raw.len() + pre.text.len());
                    let split = caret_byte.min(raw.len());
                    s_buf.push_str(&raw[..split]);
                    s_buf.push_str(&pre.text);
                    s_buf.push_str(&raw[split..]);
                    s_buf
                } else {
                    raw.to_owned()
                }
            } else {
                raw.to_owned()
            };
            let line_data = text_line(&display, rgb8(s.text));
            let row = Rc::new(self.ctx.build_row(line_data.hash, &line_data, None));

            if focused && self.view.cursor_blink_on && line_idx == caret_line {
                let col = display[..caret_byte.min(display.len())].chars().count();
                let (anchor_x, _) = row.cursor_anchor(col, self.ctx.metrics.char_width);
                let cx = rect.x + pad_x + anchor_x;
                let caret_r = Rect::new(
                    cx,
                    y + s.px(1.0),
                    s.px(1.5).max(1.0),
                    (lh - s.px(2.0)).max(2.0),
                );
                if caret_r.x >= rect.x && caret_r.right() <= rect.right() {
                    self.rect_fill(caret_r, 0.0, s.accent, None);
                }
            }

            if !display.is_empty() && text_w > 0.0 {
                self.scene.texts.push(AgentText {
                    row,
                    left: rect.x + pad_x,
                    top: y,
                    bounds: clip,
                });
            }
        }
    }

    fn draw_inspector(&mut self) {
        let Some(insp) = self.layout.inspector.clone() else {
            return;
        };
        let s = self.style;

        // Clip any underlying card text runs behind the inspector sheet so nothing bleeds through.
        let sheet = insp.sheet;
        self.scene.labels = std::mem::take(&mut self.scene.labels)
            .into_iter()
            .filter_map(|label| {
                let remaining = bounds_around(label.bounds, sheet);
                remaining
                    .into_iter()
                    .next()
                    .map(|bounds| UiLabel { bounds, ..label })
            })
            .collect();
        self.scene.texts = std::mem::take(&mut self.scene.texts)
            .into_iter()
            .filter_map(|text| {
                let remaining = bounds_around(text.bounds, sheet);
                remaining
                    .into_iter()
                    .next()
                    .map(|bounds| AgentText { bounds, ..text })
            })
            .collect();

        // Dim backdrop over body behind the sheet.
        self.rect_fill(self.layout.body, 0.0, Rgba::new(0.0, 0.0, 0.0, 0.25), None);

        // Inspector sheet container with shadow and border.
        self.scene.shapes.push(Shape::RoundedRect {
            rect: sheet,
            radii: [s.px(RADIUS_12); 4],
            fill: self.ctx.color(s.bg1),
            border: Some(Border {
                width: s.hairline(),
                color: self.ctx.color(s.border),
            }),
            shadow: Some(Shadow {
                blur: s.px(OVERLAY_BLUR),
                offset: OVERLAY_OFFSET.map(|v| s.px(v)),
                color: self.ctx.color(OVERLAY_SHADOW),
            }),
        });

        let sheet_clip = rect_to_bounds(sheet);
        let hdr = insp.header;
        let rule = s.hairline();
        self.rect_fill(
            Rect::new(hdr.x, hdr.bottom() - rule, hdr.w, rule),
            0.0,
            s.border,
            None,
        );

        let used_tok = included_context_tokens(&self.view.context);
        let budget = self.view.context.budget_tokens;
        let hdr_text = if budget > 0 {
            match self.lang() {
                UiLanguage::English => format!("Context · {used_tok} / {budget} tok"),
                UiLanguage::Arabic => format!(
                    "السياق · {used_tok} / {}",
                    arabic_count(budget as usize, "رمز واحد", "رمزان", "رموز", "رمز")
                ),
            }
        } else {
            match self.lang() {
                UiLanguage::English => format!("Context · {used_tok} tok"),
                UiLanguage::Arabic => format!(
                    "السياق · {}",
                    arabic_count(used_tok as usize, "رمز واحد", "رمزان", "رموز", "رمز")
                ),
            }
        };
        let title_r = Rect::new(
            hdr.x + s.px(10.0),
            hdr.y,
            (hdr.w - s.px(44.0)).max(0.0),
            hdr.h,
        );
        self.push_label(
            self.lang().mirror(title_r, hdr),
            &hdr_text,
            UiSize::Caption,
            UiWeight::Semibold,
            self.start_align(),
            s.text,
            Some(sheet_clip),
        );

        // Close icon.
        let close_hover = self.view.hover == AgentPanelHit::InspectorClose;
        if close_hover {
            self.rect_fill(insp.close_button, RADIUS_6, s.bg3, None);
        }
        let cside = s.px(13.0).min(insp.close_button.w).min(insp.close_button.h);
        Icon::Close.draw(
            Rect::new(
                insp.close_button.x + (insp.close_button.w - cside) * 0.5,
                insp.close_button.y + (insp.close_button.h - cside) * 0.5,
                cside,
                cside,
            ),
            self.ctx
                .color(if close_hover { s.text } else { s.text_dim }),
            &mut self.scene.shapes,
        );

        if self.view.context.items.is_empty() {
            let empty_r = Rect::new(
                sheet.x + s.px(12.0),
                hdr.bottom() + s.px(16.0),
                (sheet.w - s.px(24.0)).max(0.0),
                s.px(20.0),
            );
            self.push_label(
                empty_r,
                self.lang().text(
                    "No context items attached to this request.",
                    "لا توجد عناصر سياق مرفقة بهذا الطلب.",
                ),
                UiSize::Caption,
                UiWeight::Regular,
                self.start_align(),
                s.text_dim,
                Some(sheet_clip),
            );
            return;
        }

        for &(idx, row_r, cb_r) in &insp.items {
            let Some(item) = self.view.context.items.get(idx) else {
                continue;
            };
            let selected = self.view.inspector_selected == idx;
            let hovered = self.view.hover == AgentPanelHit::InspectorToggle(idx);
            if selected || hovered {
                self.rect_fill(row_r, RADIUS_6, s.bg2, selected.then_some(s.accent));
            }
            // Checkbox.
            self.rect_fill(
                cb_r,
                4.0,
                if item.included { s.accent } else { s.bg0 },
                Some(if item.included { s.accent } else { s.border }),
            );
            if item.included {
                Icon::Check.draw(
                    inset(cb_r, s.px(2.0), s.px(2.0)),
                    self.ctx.color(s.accent_text()),
                    &mut self.scene.shapes,
                );
            }

            let kind_tag = match item.kind {
                ContextKind::Rule => self.lang().text("Rule", "قاعدة"),
                ContextKind::Memory => self.lang().text("Memory", "ذكرى"),
                ContextKind::Block => self.lang().text("Block", "كتلة"),
                ContextKind::File => self.lang().text("File", "ملف"),
            };
            let tok_str = match self.lang() {
                UiLanguage::English => format!("{} tok", item.est_tokens),
                UiLanguage::Arabic => {
                    arabic_count(item.est_tokens as usize, "رمز واحد", "رمزان", "رموز", "رمز")
                }
            };
            let tok_w = s.px(56.0).min(row_r.w * 0.28);
            let label_str = format!("[{kind_tag}] {}", item.label);

            let (label_r, tok_r) = if self.mirrored() {
                let tr = Rect::new(row_r.x + s.px(6.0), row_r.y, tok_w, row_r.h);
                let lr = Rect::new(
                    tr.right() + s.px(6.0),
                    row_r.y,
                    (cb_r.x - s.px(8.0) - (tr.right() + s.px(6.0))).max(0.0),
                    row_r.h,
                );
                (lr, tr)
            } else {
                let lx = cb_r.right() + s.px(8.0);
                let tr = Rect::new(row_r.right() - s.px(6.0) - tok_w, row_r.y, tok_w, row_r.h);
                let lr = Rect::new(lx, row_r.y, (tr.x - s.px(6.0) - lx).max(0.0), row_r.h);
                (lr, tr)
            };

            self.push_label(
                label_r,
                &label_str,
                UiSize::Caption,
                UiWeight::Regular,
                self.start_align(),
                if item.included { s.text } else { s.text_faint },
                Some(sheet_clip),
            );
            self.push_label(
                tok_r,
                &tok_str,
                UiSize::Small,
                UiWeight::Regular,
                if self.mirrored() {
                    UiAlign::Left
                } else {
                    UiAlign::Right
                },
                s.text_faint,
                Some(sheet_clip),
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_model_paragraph(
        &mut self,
        text: &str,
        x: f32,
        start_y: f32,
        width: f32,
        max_lines: usize,
        color: Rgba,
        clip: TextBounds,
    ) {
        let s = self.style;
        let rtl = paragraph_is_rtl(text);
        let align = if rtl { UiAlign::Right } else { UiAlign::Left };
        let line_h = s.px(18.0);
        let lines = wrap_words(text, width, s.px(7.2), max_lines);
        for (i, line) in lines.iter().enumerate() {
            let y = start_y + i as f32 * line_h;
            self.push_label(
                Rect::new(x, y, width, line_h),
                line,
                UiSize::Caption,
                UiWeight::Regular,
                align,
                color,
                Some(clip),
            );
        }
    }

    fn rect_fill(&mut self, rect: Rect, radius: f32, fill: Rgba, border: Option<Rgba>) {
        if rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }
        self.scene.shapes.push(Shape::RoundedRect {
            rect,
            radii: [self.style.px(radius); 4],
            fill: self.ctx.color(fill),
            border: border.map(|color| Border {
                width: self.style.hairline(),
                color: self.ctx.color(color),
            }),
            shadow: None,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn push_label(
        &mut self,
        rect: Rect,
        text: &str,
        size: UiSize,
        weight: UiWeight,
        align: UiAlign,
        color: Rgba,
        clip: Option<TextBounds>,
    ) {
        if rect.w <= 0.0 || rect.h <= 0.0 || text.is_empty() {
            return;
        }
        let buffer = self.font.fit(
            self.ctx.font_system,
            size,
            weight,
            self.style.scale,
            text,
            rect.w,
        );
        if label_width(&buffer) <= 0.0 {
            return;
        }
        let mut label = UiLabel::new(buffer, rect, align, color);
        if let Some(c) = clip {
            label.bounds = intersect_bounds(label.bounds, c);
            if label.bounds.left >= label.bounds.right || label.bounds.top >= label.bounds.bottom {
                return;
            }
        }
        self.scene.labels.push(label);
    }

    fn push_mono_line(&mut self, rect: Rect, text: &str, color: Rgba, clip: TextBounds) {
        if rect.w <= 0.0 || rect.h <= 0.0 || text.is_empty() {
            return;
        }
        let bounds = intersect_bounds(rect_to_bounds(rect), clip);
        if bounds.left >= bounds.right || bounds.top >= bounds.bottom {
            return;
        }
        let cleaned = crate::shell_integration::clean_text(text, 512);
        let line = text_line(&cleaned, rgb8(color));
        let row = Rc::new(self.ctx.build_row(line.hash, &line, None));
        self.scene.texts.push(AgentText {
            row,
            left: rect.x,
            top: rect.y,
            bounds,
        });
    }
}

/// Leading status strip color per AGENT.md §1.2.
pub fn status_strip_color(status: &StepStatus, style: UiStyle) -> Rgba {
    match status {
        StepStatus::Running => style.accent,
        StepStatus::WaitingApproval => style.warn,
        StepStatus::Done => style.ok,
        StepStatus::Denied | StepStatus::Failed(_) => style.danger,
        StepStatus::Cancelled => style.text_faint,
    }
}

fn rgb8(c: Rgba) -> Rgb {
    let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgb {
        r: to8(c.r),
        g: to8(c.g),
        b: to8(c.b),
    }
}

fn inset(rect: Rect, dx: f32, dy: f32) -> Rect {
    Rect::new(
        rect.x + dx,
        rect.y + dy,
        (rect.w - 2.0 * dx).max(0.0),
        (rect.h - 2.0 * dy).max(0.0),
    )
}

fn clamp_rect_y(rect: Rect, clip: Rect) -> Rect {
    let top = rect.y.max(clip.y);
    let bottom = rect.bottom().min(clip.bottom());
    Rect::new(rect.x, top, rect.w, (bottom - top).max(0.0))
}

fn rect_to_bounds(r: Rect) -> TextBounds {
    TextBounds {
        left: r.x.floor() as i32,
        top: r.y.floor() as i32,
        right: r.right().ceil() as i32,
        bottom: r.bottom().ceil() as i32,
    }
}

fn intersect_bounds(a: TextBounds, b: TextBounds) -> TextBounds {
    TextBounds {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    }
}

fn wrap_mono_lines(text: &str, max_w: f32, char_w: f32, max_lines: usize) -> Vec<String> {
    let cols = (max_w / char_w.max(6.0)).floor().max(12.0) as usize;
    let mut out = Vec::new();
    for raw in text.lines() {
        let chars: Vec<char> = raw.chars().collect();
        if chars.is_empty() {
            out.push(String::new());
            if out.len() >= max_lines {
                return out;
            }
            continue;
        }
        for chunk in chars.chunks(cols) {
            out.push(chunk.iter().collect());
            if out.len() >= max_lines {
                return out;
            }
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

#[allow(dead_code)]
const _UNUSED_SPACE: (f32, f32) = (SPACE_4, SPACE_8);
