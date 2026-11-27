//! Context Inspector overlay sheet rendering for the Agent side panel.

use twitty_ai::engine::model::ContextKind;

use super::{AgentDrawer, AgentText};
use crate::frame::bounds_around;
use crate::helpers::{inset, rect_to_bounds};
use twitty_core::color::Rgba;
use twitty_core::layout::Rect;
use twitty_render::icons::Icon;
use twitty_render::shapes::{Border, Shadow, Shape};
use twitty_render::text::ui::{UiAlign, UiLabel, UiSize, UiWeight};
use twitty_render::ui_style::{OVERLAY_BLUR, OVERLAY_OFFSET, OVERLAY_SHADOW, RADIUS_12, RADIUS_6};
use twitty_ui::ui::agent_panel::{arabic_count, included_context_tokens, AgentPanelHit};
use twitty_ui::ui::chrome::UiLanguage;

impl AgentDrawer<'_, '_> {
    pub(super) fn draw_inspector(&mut self) {
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
                self.lang().t("No context items attached to this request."),
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
                ContextKind::Rule => self.lang().t("Rule"),
                ContextKind::Memory => self.lang().t("Memory"),
                ContextKind::Block => self.lang().t("Block"),
                ContextKind::File => self.lang().t("File"),
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
}
