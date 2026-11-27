//! CPU-side rendering for the Project Memory overlay.
//!
//! Reuses `UiStyle` tokens, `Shape`, `Icon`, `UiFont`, and `UiLabel` (matching the Settings panel's
//! chrome, surface elevation, and Arabic RTL mirroring). Returns the outer panel rect so
//! `Frame::text_runs` clips terminal/chrome text behind the overlay.

use glyphon::TextBounds;
use twitty_store::RulesFileState;

use super::context::DrawContext;
use super::palette::fit_query_tail;
use twitty_core::color::Rgba;
use twitty_core::layout::Rect;
use twitty_render::icons::Icon;
use twitty_render::shapes::{Border, Shadow, Shape};
use twitty_render::text::ui::{label_width, UiAlign, UiFont, UiLabel, UiSize, UiWeight};
use twitty_render::ui_style::{
    UiStyle, OVERLAY_BLUR, OVERLAY_OFFSET, OVERLAY_SHADOW, RADIUS_12, RADIUS_6, RADIUS_8,
};
use twitty_ui::ui::chrome::UiLanguage;
use twitty_ui::ui::memory_panel::{
    MemoryEntryRowLayout, MemoryHit, MemoryInputTarget, MemoryLayout, MemoryPanelState, MemoryTab,
    RulesFileRowLayout,
};
use twitty_ui::ui::settings_panel::FocusZone;

/// Snapshot of Project Memory state passed to the renderer.
#[derive(Clone, Debug)]
pub struct MemoryView {
    pub state: MemoryPanelState,
}

/// Draw the Project Memory overlay into `shapes`, returning its outer rect (to clip underlying
/// terminal text) and shaped UI labels.
pub fn memory_overlay(
    ctx: &mut DrawContext,
    style: UiStyle,
    font: &UiFont,
    view: &MemoryView,
    surface: (f32, f32),
    shapes: &mut Vec<Shape>,
) -> (Option<Rect>, Vec<UiLabel>) {
    let layout = MemoryLayout::build(surface, style.scale, &view.state);
    if layout.panel.w <= 0.0 || layout.panel.h <= 0.0 {
        return (None, Vec::new());
    }
    let mut drawer = MemoryDrawer {
        ctx,
        style,
        font,
        state: &view.state,
        layout: &layout,
        shapes,
        labels: Vec::new(),
    };
    drawer.draw();
    (Some(layout.panel), drawer.labels)
}

struct MemoryDrawer<'a, 'b> {
    ctx: &'a mut DrawContext<'b>,
    style: UiStyle,
    font: &'a UiFont,
    state: &'a MemoryPanelState,
    layout: &'a MemoryLayout,
    shapes: &'a mut Vec<Shape>,
    labels: Vec<UiLabel>,
}

impl MemoryDrawer<'_, '_> {
    fn lang(&self) -> UiLanguage {
        self.state.language
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

    fn end_align(&self) -> UiAlign {
        if self.mirrored() {
            UiAlign::Left
        } else {
            UiAlign::Right
        }
    }

    fn draw(&mut self) {
        let s = self.style;
        let l = self.layout;

        // Panel shell with border and shadow (identical to Settings).
        self.shapes.push(Shape::RoundedRect {
            rect: l.panel,
            radii: [s.px(RADIUS_12); 4],
            fill: self.ctx.color(s.bg1),
            border: Some(Border {
                width: s.px(1.0),
                color: self.ctx.color(s.border),
            }),
            shadow: Some(Shadow {
                blur: s.px(OVERLAY_BLUR),
                offset: OVERLAY_OFFSET.map(|v| s.px(v)),
                color: self.ctx.color(OVERLAY_SHADOW),
            }),
        });

        self.draw_header();
        self.draw_sidebar();
        self.draw_top_controls();
        self.draw_body();

        if let Some(thumb) = l.scrollbar {
            self.shapes.push(Shape::rounded(
                thumb,
                thumb.w / 2.0,
                self.ctx.color(s.text_faint),
            ));
        }
    }

    fn draw_header(&mut self) {
        let s = self.style;
        let l = self.layout;
        let header = l.header;
        if header.h <= 0.0 {
            return;
        }
        let rule_h = s.hairline();
        self.shapes.push(Shape::rounded(
            Rect::new(header.x, header.bottom() - rule_h, header.w, rule_h),
            0.0,
            self.ctx.color(s.border),
        ));

        self.push_label(
            l.header_title,
            self.lang().t("Project memory"),
            UiSize::Title,
            UiWeight::Semibold,
            self.start_align(),
            s.text,
            None,
        );

        let sub_text = match self.state.project_root.as_deref() {
            Some(root) => root,
            None => self.lang().t("No active local project"),
        };
        self.push_label(
            l.header_subtitle,
            sub_text,
            UiSize::Small,
            UiWeight::Regular,
            self.start_align(),
            s.text_dim,
            None,
        );

        if let Some(badge) = l.excluded_badge {
            self.draw_pill(
                badge,
                self.lang().t("Excluded"),
                s.bg2.mix(s.warn, 0.22),
                s.warn,
                None,
            );
        }

        if let Some(fb) = l.forget_button {
            let hovered = self.state.hover == Some(MemoryHit::ForgetProject);
            let armed = self.state.confirm_forget;
            let (fill, border_c, fg) = if armed {
                (s.danger, s.danger, contrasting_fg(s.danger))
            } else if hovered {
                (s.bg2.mix(s.danger, 0.18), s.danger, s.danger)
            } else {
                (s.bg2, s.border, s.danger)
            };
            self.shapes.push(Shape::RoundedRect {
                rect: fb,
                radii: [s.px(RADIUS_6); 4],
                fill: self.ctx.color(fill),
                border: Some(Border {
                    width: s.px(1.0),
                    color: self.ctx.color(border_c),
                }),
                shadow: None,
            });
            let label = if armed {
                self.lang().t("Confirm forget?")
            } else {
                self.lang().t("Forget this project")
            };
            self.push_label(
                fb,
                label,
                UiSize::Caption,
                UiWeight::Semibold,
                UiAlign::Center,
                fg,
                None,
            );
        }

        let close_hover = self.state.hover == Some(MemoryHit::CloseButton);
        if close_hover {
            self.shapes.push(Shape::rounded(
                l.close_button,
                s.px(RADIUS_6),
                self.ctx.color(s.bg3),
            ));
        }
        let side = s.px(14.0).min(l.close_button.w).min(l.close_button.h);
        let icon_rect = Rect::new(
            l.close_button.x + (l.close_button.w - side) / 2.0,
            l.close_button.y + (l.close_button.h - side) / 2.0,
            side,
            side,
        );
        Icon::Close.draw(
            icon_rect,
            self.ctx
                .color(if close_hover { s.text } else { s.text_dim }),
            self.shapes,
        );
    }

    fn draw_sidebar(&mut self) {
        let s = self.style;
        let l = self.layout;
        self.shapes
            .push(Shape::rounded(l.divider, 0.0, self.ctx.color(s.border)));

        for &(tab, rect) in &l.tabs {
            if rect.w <= 0.0 || rect.h <= 0.0 {
                continue;
            }
            let active = self.state.tab == tab;
            let hovered = self.state.hover == Some(MemoryHit::Tab(tab));
            let sidebar_focused = self.state.focus == FocusZone::Sidebar;

            if active {
                self.shapes.push(Shape::RoundedRect {
                    rect,
                    radii: [s.px(RADIUS_8); 4],
                    fill: self.ctx.color(s.bg3),
                    border: sidebar_focused.then(|| Border {
                        width: s.px(1.0),
                        color: self.ctx.color(s.accent),
                    }),
                    shadow: None,
                });
                let bar_w = s.px(2.5).max(2.0);
                let bar_h = (rect.h - s.px(12.0)).max(s.px(10.0));
                let bar_x = if self.mirrored() {
                    rect.right() - bar_w - s.px(3.0)
                } else {
                    rect.x + s.px(3.0)
                };
                self.shapes.push(Shape::rounded(
                    Rect::new(bar_x, rect.y + (rect.h - bar_h) / 2.0, bar_w, bar_h),
                    bar_w / 2.0,
                    self.ctx.color(s.accent),
                ));
            } else if hovered {
                self.shapes
                    .push(Shape::rounded(rect, s.px(RADIUS_8), self.ctx.color(s.bg2)));
            }

            let pad = s.px(12.0);
            let count_w = s.px(28.0);
            let (label_rect, count_rect) = if self.mirrored() {
                let cr = Rect::new(rect.x + pad, rect.y, count_w, rect.h);
                let lr = Rect::new(
                    cr.right() + s.px(6.0),
                    rect.y,
                    (rect.right() - pad - (cr.right() + s.px(6.0))).max(0.0),
                    rect.h,
                );
                (lr, cr)
            } else {
                let cr = Rect::new(rect.right() - pad - count_w, rect.y, count_w, rect.h);
                let lr = Rect::new(
                    rect.x + pad,
                    rect.y,
                    (cr.x - s.px(6.0) - (rect.x + pad)).max(0.0),
                    rect.h,
                );
                (lr, cr)
            };

            self.push_label(
                label_rect,
                tab.label(self.lang()),
                UiSize::Body,
                if active {
                    UiWeight::Semibold
                } else {
                    UiWeight::Regular
                },
                self.start_align(),
                if active { s.text } else { s.text_dim },
                None,
            );

            let count = self.state.tab_count(tab);
            let attention = self.state.tab_has_attention(tab);
            let count_str = count.to_string();
            let count_color = if attention {
                s.warn
            } else if active {
                s.text
            } else {
                s.text_dim
            };
            self.push_label(
                count_rect,
                &count_str,
                UiSize::Small,
                if attention {
                    UiWeight::Semibold
                } else {
                    UiWeight::Regular
                },
                self.end_align(),
                count_color,
                None,
            );
        }
    }

    fn draw_top_controls(&mut self) {
        let s = self.style;
        let l = self.layout;

        if let Some(banner) = l.excluded_banner {
            self.shapes.push(Shape::RoundedRect {
                rect: banner,
                radii: [s.px(RADIUS_8); 4],
                fill: self.ctx.color(s.bg2.mix(s.warn, 0.12)),
                border: Some(Border {
                    width: s.px(1.0),
                    color: self.ctx.color(s.warn),
                }),
                shadow: None,
            });
            let pad = s.px(12.0);
            let text_rect = Rect::new(
                banner.x + pad,
                banner.y,
                (banner.w - 2.0 * pad).max(0.0),
                banner.h,
            );
            self.push_label(
                text_rect,
                self.lang()
                    .t("This folder is excluded — nothing is recorded or assembled."),
                UiSize::Caption,
                UiWeight::Regular,
                self.start_align(),
                s.text,
                None,
            );
        }

        if let Some(inp) = l.add_input {
            let active = self.state.input_target == Some(MemoryInputTarget::AddBar);
            let value = if active {
                self.state.input_text.as_str()
            } else {
                ""
            };
            let placeholder = self.state.tab.add_placeholder(self.lang());
            self.draw_text_input(inp, value, placeholder, active, None);
        }

        if let Some(btn) = l.add_submit {
            let hovered = self.state.hover == Some(MemoryHit::AddSubmit);
            self.draw_button(
                btn,
                self.lang().t("+ Add"),
                ButtonTone::Accent,
                hovered,
                None,
            );
        }

        if let Some(btn) = l.refresh_button {
            let hovered = self.state.hover == Some(MemoryHit::RefreshEnv);
            self.draw_button(
                btn,
                self.lang().t("Refresh"),
                ButtonTone::Quiet,
                hovered,
                None,
            );
        }

        if let Some(btn) = l.exclude_current_btn {
            let hovered = self.state.hover == Some(MemoryHit::ExcludeCurrentProject);
            self.draw_button(
                btn,
                self.lang().t("Exclude this project"),
                ButtonTone::Quiet,
                hovered,
                None,
            );
        }
    }

    fn draw_body(&mut self) {
        let s = self.style;
        let l = self.layout;
        let vp = l.viewport;
        if vp.w <= 0.0 || vp.h <= 0.0 {
            return;
        }
        let clip = rect_to_bounds(vp);

        match self.state.tab {
            MemoryTab::Rules => {
                if let Some(hdr) = l.rules_heading {
                    if hdr.bottom() >= vp.y && hdr.y <= vp.bottom() {
                        self.push_label(
                            hdr,
                            self.lang().t("Project rules files"),
                            UiSize::Small,
                            UiWeight::Semibold,
                            self.start_align(),
                            s.text_dim,
                            Some(clip),
                        );
                    }
                }
                if l.rules_rows.is_empty() {
                    if let Some(hdr) = l.rules_heading {
                        let empty_r = Rect::new(vp.x, hdr.bottom() + s.px(4.0), vp.w, s.px(22.0));
                        if empty_r.bottom() >= vp.y && empty_r.y <= vp.bottom() {
                            self.push_label(
                                empty_r,
                                self.lang().t("No AGENTS.md, CLAUDE.md, TWITTY.md, or .twitty/rules.md found."),
                                UiSize::Caption,
                                UiWeight::Regular,
                                self.start_align(),
                                s.text_faint,
                                Some(clip),
                            );
                        }
                    }
                } else {
                    for row in &l.rules_rows {
                        if row.rect.y >= vp.y - 1.0 && row.rect.bottom() <= vp.bottom() + 1.0 {
                            self.draw_rules_file_row(row, clip);
                        }
                    }
                }

                if let Some(hdr) = l.user_rules_heading {
                    if hdr.bottom() >= vp.y && hdr.y <= vp.bottom() {
                        self.push_label(
                            hdr,
                            self.lang().t("User rules"),
                            UiSize::Small,
                            UiWeight::Semibold,
                            self.start_align(),
                            s.text_dim,
                            Some(clip),
                        );
                    }
                }
                if l.entry_rows.is_empty() {
                    if let Some(hdr) = l.user_rules_heading {
                        let empty_r = Rect::new(vp.x, hdr.bottom() + s.px(6.0), vp.w, s.px(24.0));
                        if empty_r.bottom() >= vp.y && empty_r.y <= vp.bottom() {
                            self.push_label(
                                empty_r,
                                self.lang().t("No user rules yet. Add one above."),
                                UiSize::Caption,
                                UiWeight::Regular,
                                self.start_align(),
                                s.text_faint,
                                Some(clip),
                            );
                        }
                    }
                } else {
                    for row in &l.entry_rows {
                        if row.rect.y >= vp.y - 1.0 && row.rect.bottom() <= vp.bottom() + 1.0 {
                            self.draw_entry_row(row, clip);
                        }
                    }
                }
            }
            MemoryTab::Facts | MemoryTab::Commands | MemoryTab::Env => {
                if l.entry_rows.is_empty() {
                    let empty_text = match self.state.tab {
                        MemoryTab::Facts => self.lang().t("No saved facts for this project yet."),
                        MemoryTab::Commands => self
                            .lang()
                            .t("No frequent commands recorded for this project yet."),
                        MemoryTab::Env => self
                            .lang()
                            .t("No environment notes detected. Click Refresh to scan."),
                        _ => "",
                    };
                    self.push_label(
                        Rect::new(vp.x, vp.y + s.px(8.0), vp.w, s.px(24.0)),
                        empty_text,
                        UiSize::Caption,
                        UiWeight::Regular,
                        self.start_align(),
                        s.text_faint,
                        Some(clip),
                    );
                } else {
                    for row in &l.entry_rows {
                        if row.rect.y >= vp.y - 1.0 && row.rect.bottom() <= vp.bottom() + 1.0 {
                            self.draw_entry_row(row, clip);
                        }
                    }
                }
            }
            MemoryTab::Exclusions => {
                if l.exclusion_rows.is_empty() {
                    self.push_label(
                        Rect::new(vp.x, vp.y + s.px(8.0), vp.w, s.px(24.0)),
                        self.lang()
                            .t("No excluded folders. Add a path or glob above."),
                        UiSize::Caption,
                        UiWeight::Regular,
                        self.start_align(),
                        s.text_faint,
                        Some(clip),
                    );
                } else {
                    for row in &l.exclusion_rows {
                        if row.rect.y < vp.y - 1.0 || row.rect.bottom() > vp.bottom() + 1.0 {
                            continue;
                        }
                        let focused = self.state.focus == FocusZone::Content
                            && self.state.selected == row.index;
                        self.shapes.push(Shape::RoundedRect {
                            rect: row.rect,
                            radii: [s.px(RADIUS_8); 4],
                            fill: self.ctx.color(s.bg2),
                            border: Some(Border {
                                width: s.px(1.0),
                                color: self.ctx.color(if focused { s.accent } else { s.border }),
                            }),
                            shadow: None,
                        });
                        if let Some(pat) = self.state.exclusions.get(row.index) {
                            self.push_label(
                                row.text_rect,
                                pat,
                                UiSize::Body,
                                UiWeight::Regular,
                                self.start_align(),
                                s.text,
                                Some(clip),
                            );
                        }
                        let hovered =
                            self.state.hover == Some(MemoryHit::ExclusionRemove(row.index));
                        self.draw_button(
                            row.remove_btn,
                            self.lang().t("Remove"),
                            ButtonTone::DangerQuiet,
                            hovered,
                            Some(clip),
                        );
                    }
                }
            }
        }
    }

    fn draw_rules_file_row(&mut self, row: &RulesFileRowLayout, clip: TextBounds) {
        let s = self.style;
        let Some(file) = self.state.rules_files.get(row.index) else {
            return;
        };
        let focused = self.state.focus == FocusZone::Content && self.state.selected == row.index;

        self.shapes.push(Shape::RoundedRect {
            rect: row.rect,
            radii: [s.px(RADIUS_8); 4],
            fill: self.ctx.color(s.bg2),
            border: Some(Border {
                width: s.px(1.0),
                color: self.ctx.color(if focused {
                    s.accent
                } else if file.state == RulesFileState::Changed {
                    s.warn
                } else {
                    s.border
                }),
            }),
            shadow: None,
        });

        self.push_label(
            row.title_rect,
            &file.rel_path,
            UiSize::Body,
            UiWeight::Semibold,
            self.start_align(),
            s.text,
            Some(clip),
        );

        let short_sha: String = file.current_sha256.chars().take(8).collect();
        let meta = match file.state {
            RulesFileState::Changed => format!(
                "{} · \u{2066}sha256:{short_sha}\u{2069}",
                self.lang().t("Changed — re-approve")
            ),
            RulesFileState::Approved => format!(
                "{} · \u{2066}sha256:{short_sha}\u{2069}",
                self.lang().t("Included in context")
            ),
            RulesFileState::Unapproved => format!(
                "{} · \u{2066}sha256:{short_sha}\u{2069}",
                self.lang().t("Not read until approved")
            ),
        };
        self.push_label(
            row.hash_rect,
            &meta,
            UiSize::Small,
            UiWeight::Regular,
            self.start_align(),
            s.text_dim,
            Some(clip),
        );

        let (badge_text, badge_bg, badge_fg) = match file.state {
            RulesFileState::Approved => (self.lang().t("Approved"), s.bg1.mix(s.ok, 0.22), s.ok),
            RulesFileState::Changed => (self.lang().t("Changed"), s.bg1.mix(s.warn, 0.24), s.warn),
            RulesFileState::Unapproved => (self.lang().t("Unapproved"), s.bg3, s.text_dim),
        };
        self.draw_pill(row.badge_rect, badge_text, badge_bg, badge_fg, Some(clip));

        if let Some(btn) = row.approve_btn {
            let hovered = self.state.hover == Some(MemoryHit::RulesApprove(row.index));
            let label = if file.state == RulesFileState::Changed {
                self.lang().t("Re-approve")
            } else {
                self.lang().t("Approve")
            };
            self.draw_button(btn, label, ButtonTone::Accent, hovered, Some(clip));
        }
        if let Some(btn) = row.revoke_btn {
            let hovered = self.state.hover == Some(MemoryHit::RulesRevoke(row.index));
            self.draw_button(
                btn,
                self.lang().t("Revoke"),
                ButtonTone::Quiet,
                hovered,
                Some(clip),
            );
        }
    }

    fn draw_entry_row(&mut self, row: &MemoryEntryRowLayout, clip: TextBounds) {
        let s = self.style;
        let Some(entry) = self.state.entries.iter().find(|e| e.id == row.id) else {
            return;
        };
        let focused =
            self.state.focus == FocusZone::Content && self.state.selected == row.list_index;

        self.shapes.push(Shape::RoundedRect {
            rect: row.rect,
            radii: [s.px(RADIUS_8); 4],
            fill: self.ctx.color(s.bg2),
            border: Some(Border {
                width: s.px(1.0),
                color: self.ctx.color(if focused {
                    s.accent
                } else if !entry.approved {
                    s.warn
                } else {
                    s.border
                }),
            }),
            shadow: None,
        });

        // Inline editor mode.
        if let Some(inp) = row.edit_input {
            self.draw_text_input(
                inp,
                &self.state.input_text,
                self.lang().t("Edit memory text…"),
                true,
                Some(clip),
            );
            if let Some(btn) = row.save_btn {
                let hovered = self.state.hover == Some(MemoryHit::EditSave(row.id));
                self.draw_button(
                    btn,
                    self.lang().t("Save"),
                    ButtonTone::Accent,
                    hovered,
                    Some(clip),
                );
            }
            if let Some(btn) = row.cancel_btn {
                let hovered = self.state.hover == Some(MemoryHit::EditCancel);
                self.draw_button(
                    btn,
                    self.lang().t("Cancel"),
                    ButtonTone::Quiet,
                    hovered,
                    Some(clip),
                );
            }
            return;
        }

        self.push_label(
            row.text_rect,
            &entry.text,
            UiSize::Body,
            UiWeight::Regular,
            self.start_align(),
            s.text,
            Some(clip),
        );

        let meta = format!(
            "{} · \u{2066}#{}\u{2069}",
            self.state.source_badge(entry),
            entry.id
        );
        self.push_label(
            row.meta_rect,
            &meta,
            UiSize::Small,
            UiWeight::Regular,
            self.start_align(),
            s.text_dim,
            Some(clip),
        );

        if let Some(badge) = row.badge_rect {
            if !entry.approved {
                self.draw_pill(
                    badge,
                    self.lang().t("Pending"),
                    s.bg1.mix(s.warn, 0.24),
                    s.warn,
                    Some(clip),
                );
            } else if let Some(fc) = self.state.frequent_stats_for(entry) {
                let (label, color) = match fc.success_rate_pct {
                    Some(pct) => {
                        let c = if pct >= 80 {
                            s.ok
                        } else if pct >= 50 {
                            s.warn
                        } else {
                            s.danger
                        };
                        (format!("{}× · {pct}%", fc.runs), c)
                    }
                    None => (format!("{}×", fc.runs), s.text_dim),
                };
                self.draw_pill(badge, &label, s.bg1.mix(color, 0.18), color, Some(clip));
            }
        }

        if let Some(btn) = row.approve_btn {
            let hovered = self.state.hover == Some(MemoryHit::EntryApprove(row.id));
            self.draw_button(
                btn,
                self.lang().t("Approve"),
                ButtonTone::Accent,
                hovered,
                Some(clip),
            );
        }
        if let Some(btn) = row.edit_btn {
            let hovered = self.state.hover == Some(MemoryHit::EntryEdit(row.id));
            self.draw_button(
                btn,
                self.lang().t("Edit"),
                ButtonTone::Quiet,
                hovered,
                Some(clip),
            );
        }
        if let Some(btn) = row.delete_btn {
            let hovered = self.state.hover == Some(MemoryHit::EntryDelete(row.id));
            self.draw_button(
                btn,
                self.lang().t("Delete"),
                ButtonTone::DangerQuiet,
                hovered,
                Some(clip),
            );
        }
    }

    fn draw_text_input(
        &mut self,
        rect: Rect,
        value: &str,
        placeholder: &str,
        active: bool,
        clip: Option<TextBounds>,
    ) {
        let s = self.style;
        self.shapes.push(Shape::RoundedRect {
            rect,
            radii: [s.px(RADIUS_6); 4],
            fill: self.ctx.color(s.bg0),
            border: Some(Border {
                width: s.px(1.0),
                color: self.ctx.color(if active { s.accent } else { s.border }),
            }),
            shadow: None,
        });
        let pad = s.px(10.0);
        let text_rect = Rect::new(rect.x + pad, rect.y, (rect.w - 2.0 * pad).max(0.0), rect.h);
        if value.is_empty() {
            self.push_label(
                text_rect,
                placeholder,
                UiSize::Caption,
                UiWeight::Regular,
                self.start_align(),
                s.text_faint,
                clip,
            );
            if active {
                self.draw_caret(text_rect, 0.0);
            }
        } else {
            let (_, buffer) =
                fit_query_tail(self.font, self.ctx.font_system, text_rect.w, value, s.scale);
            let measured = label_width(&buffer);
            let mut label = UiLabel::new(buffer, text_rect, self.start_align(), s.text);
            if let Some(c) = clip {
                label.bounds = intersect_bounds(label.bounds, c);
            }
            self.labels.push(label);
            if active {
                self.draw_caret(text_rect, measured.min(text_rect.w));
            }
        }
    }

    fn draw_caret(&mut self, text_rect: Rect, advance: f32) {
        let s = self.style;
        let caret_w = s.px(1.5).max(1.0);
        let caret_h = (text_rect.h - s.px(12.0)).max(s.px(12.0));
        let caret_x = if self.mirrored() {
            (text_rect.right() - advance - caret_w).max(text_rect.x)
        } else {
            (text_rect.x + advance + s.px(1.0)).min(text_rect.right() - caret_w)
        };
        self.shapes.push(Shape::rounded(
            Rect::new(
                caret_x,
                text_rect.y + (text_rect.h - caret_h) / 2.0,
                caret_w,
                caret_h,
            ),
            0.0,
            self.ctx.color(s.accent),
        ));
    }

    fn draw_button(
        &mut self,
        rect: Rect,
        label: &str,
        tone: ButtonTone,
        hovered: bool,
        clip: Option<TextBounds>,
    ) {
        let s = self.style;
        let (fill, border_c, fg) = match tone {
            ButtonTone::Accent => (
                if hovered {
                    s.accent.mix(s.text, 0.12)
                } else {
                    s.accent
                },
                s.accent,
                s.accent_text(),
            ),
            ButtonTone::Quiet => (if hovered { s.bg3 } else { s.bg1 }, s.border, s.text),
            ButtonTone::DangerQuiet => (
                if hovered {
                    s.bg1.mix(s.danger, 0.20)
                } else {
                    s.bg1
                },
                if hovered { s.danger } else { s.border },
                s.danger,
            ),
        };
        self.shapes.push(Shape::RoundedRect {
            rect,
            radii: [s.px(RADIUS_6); 4],
            fill: self.ctx.color(fill),
            border: Some(Border {
                width: s.px(1.0),
                color: self.ctx.color(border_c),
            }),
            shadow: None,
        });
        self.push_label(
            rect,
            label,
            UiSize::Caption,
            UiWeight::Semibold,
            UiAlign::Center,
            fg,
            clip,
        );
    }

    fn draw_pill(&mut self, rect: Rect, text: &str, bg: Rgba, fg: Rgba, clip: Option<TextBounds>) {
        let s = self.style;
        self.shapes
            .push(Shape::rounded(rect, rect.h / 2.0, self.ctx.color(bg)));
        let pad = s.px(6.0);
        self.push_label(
            Rect::new(rect.x + pad, rect.y, (rect.w - 2.0 * pad).max(0.0), rect.h),
            text,
            UiSize::Small,
            UiWeight::Semibold,
            UiAlign::Center,
            fg,
            clip,
        );
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
        let mut label = UiLabel::new(buffer, rect, align, color);
        if let Some(c) = clip {
            label.bounds = intersect_bounds(label.bounds, c);
        }
        self.labels.push(label);
    }
}

#[derive(Clone, Copy)]
enum ButtonTone {
    Accent,
    Quiet,
    DangerQuiet,
}

use crate::helpers::{
    contrasting_fg, intersect_bounds_clamped as intersect_bounds,
    rect_to_bounds_inward as rect_to_bounds,
};
