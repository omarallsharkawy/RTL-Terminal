//! Step card, diff, button, and model paragraph rendering for the Agent side panel.

use glyphon::TextBounds;
use twitty_ai::engine::model::{StepKind, StepStatus};

use super::{clamp_rect_y, status_strip_color, wrap_mono_lines, AgentDrawer};
use crate::helpers::{inset, intersect_bounds, rect_to_bounds};
use twitty_core::color::Rgba;
use twitty_core::layout::Rect;
use twitty_render::icons::Icon;
use twitty_render::text::ui::{UiAlign, UiSize, UiWeight};
use twitty_render::ui_style::{RADIUS_6, RADIUS_8};
use twitty_ui::review::diff::{parse as parse_diff, Limits as DiffLimits, LineKind};
use twitty_ui::ui::agent_panel::{
    format_deny_reason, format_duration_ms, paragraph_is_rtl, read_tool_name, wrap_words,
    AgentFocus, AgentPanelHit, CardButton, CardLayout, StepEntry, CARD_EDITOR_MAX_LINES,
    MAX_PATCH_DIFF_ROWS, PLAN_COLLAPSED_LINES,
};

impl AgentDrawer<'_, '_> {
    pub(super) fn draw_card(
        &mut self,
        entry: &StepEntry,
        card: &CardLayout,
        body_clip: TextBounds,
    ) {
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
                self.lang().t("Edit patch")
            } else {
                self.lang().t("Edit command")
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
                    self.lang().t("Plan"),
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
                let heading = format!("{} · {}", self.lang().t("Read"), read_tool_name(*tool));
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
                    self.lang().t("Read needs approval"),
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
                    self.lang().t("Command (user-edited)")
                } else {
                    self.lang().t("Command")
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
                    let note = self
                        .lang()
                        .t("Suggested after reading output — review carefully");
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
                    self.lang().t("Patch (user-edited)")
                } else {
                    self.lang().t("Patch")
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
                    self.draw_patch_diff(diff, entry.parsed_diff.as_deref(), diff_rect, clip);
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
                        self.lang().t("Exit 0"),
                        format_duration_ms(*duration_ms)
                    ),
                    Some(code) => format!(
                        "{} {code} · {}",
                        self.lang().t("Exit"),
                        format_duration_ms(*duration_ms)
                    ),
                    None => format!(
                        "{} · {}",
                        self.lang().t("Finished"),
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
                    self.lang().t("Patch applied")
                } else {
                    self.lang().t("Patch applied (undo unavailable)")
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
                    self.lang().t("Remember for this project?"),
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
                    self.lang().t("Agent"),
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
                    self.lang().t("Denied"),
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
                    self.lang().t("Limit reached"),
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
    fn draw_patch_diff(
        &mut self,
        raw_diff: &str,
        cached_diff: Option<&twitty_ui::review::diff::Diff>,
        rect: Rect,
        parent_clip: TextBounds,
    ) {
        let s = self.style;
        let clamped = clamp_rect_y(rect, self.layout.body);
        if clamped.h <= 0.0 {
            return;
        }
        self.rect_fill(clamped, RADIUS_6, s.bg0, Some(s.border));
        let diff_clip = intersect_bounds(rect_to_bounds(clamped), parent_clip);

        let parsed_owned;
        let parsed = match cached_diff {
            Some(cached) => cached,
            None => {
                parsed_owned = parse_diff(raw_diff.as_bytes(), false, &DiffLimits::default());
                &parsed_owned
            }
        };
        let lh = self.ctx.metrics.line_height.max(s.px(17.0));
        let cw = self.ctx.metrics.char_width.max(s.px(7.5));
        let gutter_w = (cw * 5.0).min(rect.w * 0.25);
        let mut y = rect.y + s.px(4.0);
        let mut drawn = 0usize;

        if parsed.files.is_empty() {
            // Fallback for simple raw unified diffs without `diff --git` header.
            for line in raw_diff.lines().take(MAX_PATCH_DIFF_ROWS) {
                if y + lh > rect.bottom() || y > self.layout.body.bottom() {
                    break;
                }
                if y + lh < self.layout.body.y {
                    y += lh;
                    continue;
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
            if drawn >= MAX_PATCH_DIFF_ROWS
                || y + lh > rect.bottom()
                || y > self.layout.body.bottom()
            {
                break;
            }
            if y + lh >= self.layout.body.y {
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
            }
            y += lh;
            drawn += 1;

            for hunk in &file.hunks {
                if drawn >= MAX_PATCH_DIFF_ROWS
                    || y + lh > rect.bottom()
                    || y > self.layout.body.bottom()
                {
                    break;
                }
                if y + lh >= self.layout.body.y {
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
                }
                y += lh;
                drawn += 1;

                for line in &hunk.lines {
                    if drawn >= MAX_PATCH_DIFF_ROWS
                        || y + lh > rect.bottom()
                        || y > self.layout.body.bottom()
                    {
                        break;
                    }
                    if y + lh >= self.layout.body.y {
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
                            let gutter_r =
                                Rect::new(row_r.x, row_r.y, gutter_w.min(row_r.w), row_r.h);
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
                    }
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
                StepKind::Command { .. } => self.lang().t("Run"),
                StepKind::Patch { .. } => self.lang().t("Apply"),
                StepKind::FactSuggestion { .. } => self.lang().t("Remember"),
                StepKind::LimitReached { .. } => self.lang().t("Continue"),
                StepKind::ReadApproval { .. } => self.lang().t("Allow read"),
                _ => self.lang().t("Approve"),
            },
            CardButton::Edit => self.lang().t("Edit"),
            CardButton::Deny => match kind {
                StepKind::FactSuggestion { .. } => self.lang().t("Dismiss"),
                StepKind::LimitReached { .. } => self.lang().t("Stop"),
                _ => self.lang().t("Deny"),
            },
            CardButton::Expand => {
                if expanded {
                    self.lang().t("Collapse")
                } else {
                    self.lang().t("Expand")
                }
            }
            CardButton::OpenBlock => self.lang().t("Open block"),
            CardButton::Undo => self.lang().t("Undo"),
            CardButton::Copy => self.lang().t("Copy"),
            CardButton::ApplyEdit => self.lang().t("Apply edited"),
            CardButton::CancelEdit => self.lang().t("Cancel"),
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
            if y > clip.bottom as f32 {
                break;
            }
            if y + line_h < clip.top as f32 {
                continue;
            }
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
}
