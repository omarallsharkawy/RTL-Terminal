//! Agent side panel rendering (`docs/design/AGENT.md` §1).
//!
//! Draws the 36 px header, scrolling step cards (Plan, Read, Command, Patch reusing the review
//! diff renderer, CommandResult, PatchApplied, FactSuggestion, Message, Denied, LimitReached),
//! empty state with three bilingual chips, "Jump to latest" pill, footer composer editor + context
//! strip, and the Context Inspector overlay sheet.
//!
//! Mirrors chrome in Arabic UI while keeping commands, paths, and diffs in terminal LTR/bidi row
//! shaping and giving model text its own paragraph direction (first strong character).

mod cards;
mod inspector;

use std::rc::Rc;

use glyphon::TextBounds;
use twitty_ai::engine::model::StepStatus;

use super::context::DrawContext;
use super::review::text_line;
use crate::helpers::{inset, intersect_bounds, rect_to_bounds, rgb8};
use twitty_core::color::Rgba;
use twitty_core::layout::Rect;
use twitty_render::icons::Icon;
use twitty_render::row::CachedRow;
use twitty_render::shapes::{Border, Shadow, Shape};
use twitty_render::style::Quad;
use twitty_render::text::ui::{label_width, UiAlign, UiFont, UiLabel, UiSize, UiWeight};
use twitty_render::ui_style::{UiStyle, OVERLAY_SHADOW, RADIUS_12, RADIUS_6, RADIUS_8};
use twitty_ui::composer::editor::TextEditor;
use twitty_ui::ui::agent_panel::{
    format_context_strip, format_usage_pill, AgentFocus, AgentPanelHit, AgentPanelLayout,
    AgentPanelView, COMPOSER_MAX_LINES, EMPTY_INTRO_LINE_H, EMPTY_INTRO_TOP, EXAMPLE_CHIPS,
};
use twitty_ui::ui::chrome::UiLanguage;

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

pub(super) struct AgentDrawer<'a, 'b> {
    ctx: &'a mut DrawContext<'b>,
    style: UiStyle,
    font: &'a UiFont,
    view: &'a AgentPanelView,
    layout: &'a AgentPanelLayout,
    scene: AgentScene,
}

impl AgentDrawer<'_, '_> {
    pub(super) fn lang(&self) -> UiLanguage {
        self.view.language
    }

    pub(super) fn mirrored(&self) -> bool {
        self.layout.mirrored
    }

    pub(super) fn start_align(&self) -> UiAlign {
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
            .unwrap_or_else(|| self.lang().t("Agent"));

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
                self.lang().t("Stop"),
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
                self.lang().t("Jump to latest"),
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
            let ph = self.lang().t("Ask the agent…");
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
            self.lang().t("Send"),
            UiSize::Small,
            UiWeight::Semibold,
            UiAlign::Center,
            fg,
            Some(footer_clip),
        );
    }

    pub(super) fn draw_editor_box(
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

    pub(super) fn rect_fill(&mut self, rect: Rect, radius: f32, fill: Rgba, border: Option<Rgba>) {
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
    pub(super) fn push_label(
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

    pub(super) fn push_mono_line(&mut self, rect: Rect, text: &str, color: Rgba, clip: TextBounds) {
        if rect.w <= 0.0 || rect.h <= 0.0 || text.is_empty() {
            return;
        }
        let bounds = intersect_bounds(rect_to_bounds(rect), clip);
        if bounds.left >= bounds.right || bounds.top >= bounds.bottom {
            return;
        }
        let cleaned = twitty_core::shell_integration::clean_text(text, 512);
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

pub(super) fn clamp_rect_y(rect: Rect, clip: Rect) -> Rect {
    let top = rect.y.max(clip.y);
    let bottom = rect.bottom().min(clip.bottom());
    Rect::new(rect.x, top, rect.w, (bottom - top).max(0.0))
}

pub(super) fn wrap_mono_lines(
    text: &str,
    max_w: f32,
    char_w: f32,
    max_lines: usize,
) -> Vec<String> {
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
