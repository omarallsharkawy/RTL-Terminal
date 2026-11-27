//! Drawing the AI section: providers, keys, limits and memory.

use super::*;

impl SettingsDrawer<'_, '_> {
    pub(super) fn draw_ai(&mut self) {
        use twitty_ui::ui::settings_ai::{
            AiTextField, API_PROVIDERS, DEFAULT_PROVIDER_CHOICES, HARD_DENY_SUMMARY,
        };
        // Draw the top Project Memory action row (n21) first.
        self.draw_controls();
        let Some(ai) = self.layout.ai.clone() else {
            return;
        };
        let s = self.style;
        let vp = self.layout.viewport;
        let clip = rect_to_bounds(vp);
        let state = &self.view.state.ai;
        let cfg = &self.view.config;
        let in_vp = |r: Rect| r.bottom() >= vp.y && r.y <= vp.bottom();

        // Notice banner.
        if let (Some(nr), Some((is_err, msg))) = (ai.notice_rect, &state.notice) {
            if in_vp(nr) {
                let tone = if *is_err { s.danger } else { s.ok };
                self.shapes.push(Shape::RoundedRect {
                    rect: nr,
                    radii: [s.px(RADIUS_6); 4],
                    fill: self.ctx.color(s.bg2.mix(tone, 0.16)),
                    border: Some(Border {
                        width: s.hairline(),
                        color: self.ctx.color(tone),
                    }),
                    shadow: None,
                });
                self.push_label(
                    Rect::new(nr.x + s.px(8.0), nr.y, (nr.w - s.px(16.0)).max(0.0), nr.h),
                    msg,
                    UiSize::Small,
                    UiWeight::Regular,
                    self.start_align(),
                    tone.mix(s.text, 0.35),
                    Some(clip),
                );
            }
        }

        // Default provider heading + selector pills.
        if let Some(&(_, first)) = ai.default_provider_buttons.first() {
            let hdr = Rect::new(vp.x, first.y - s.px(22.0), vp.w, s.px(18.0));
            if in_vp(hdr) {
                self.push_label(
                    hdr,
                    self.lang().t("Default provider"),
                    UiSize::Caption,
                    UiWeight::Semibold,
                    self.start_align(),
                    s.text,
                    Some(clip),
                );
            }
        }
        let active_provider = cfg.ai_default_provider.as_deref().unwrap_or("");
        for &(idx, rect) in &ai.default_provider_buttons {
            if !in_vp(rect) {
                continue;
            }
            let (id, en, ar) = DEFAULT_PROVIDER_CHOICES[idx];
            let selected = active_provider.eq_ignore_ascii_case(id);
            let hovered = self.view.state.hover == Some(SettingsHit::AiDefaultProvider(idx));
            self.shapes.push(Shape::RoundedRect {
                rect,
                radii: [s.px(RADIUS_6); 4],
                fill: self.ctx.color(if selected {
                    s.accent
                } else if hovered {
                    s.bg3
                } else {
                    s.bg2
                }),
                border: Some(Border {
                    width: s.hairline(),
                    color: self.ctx.color(if selected { s.accent } else { s.border }),
                }),
                shadow: None,
            });
            self.push_label(
                rect,
                self.lang().text(en, ar),
                UiSize::Small,
                if selected {
                    UiWeight::Semibold
                } else {
                    UiWeight::Regular
                },
                UiAlign::Center,
                if selected { s.accent_text() } else { s.text },
                Some(clip),
            );
        }

        // Detected CLI agents card.
        if in_vp(ai.cli_section) {
            self.shapes.push(Shape::RoundedRect {
                rect: ai.cli_section,
                radii: [s.px(RADIUS_8); 4],
                fill: self.ctx.color(s.bg2),
                border: Some(Border {
                    width: s.hairline(),
                    color: self.ctx.color(s.border),
                }),
                shadow: None,
            });
            let pad = s.px(10.0);
            let top = Rect::new(
                ai.cli_section.x + pad,
                ai.cli_section.y + s.px(4.0),
                (ai.cli_section.w - 2.0 * pad).max(0.0),
                s.px(18.0),
            );
            self.push_label(
                top,
                self.lang().t("CLI agents on PATH"),
                UiSize::Small,
                UiWeight::Semibold,
                self.start_align(),
                s.text_dim,
                Some(clip),
            );
            let cli_list = if state.detected_cli_agents.is_empty() {
                self.lang()
                    .t("None detected (claude, codex, gemini, opencode)")
                    .to_owned()
            } else {
                state.detected_cli_agents.join(" · ")
            };
            self.push_label(
                Rect::new(top.x, top.bottom(), top.w, s.px(18.0)),
                &cli_list,
                UiSize::Caption,
                UiWeight::Regular,
                self.start_align(),
                s.text,
                Some(clip),
            );
        }

        // API providers with keyring "Set key…".
        for row in &ai.api_rows {
            let spec = API_PROVIDERS[row.index];
            let has_key = state.saved_keys[row.index];
            if in_vp(row.row) {
                self.shapes.push(Shape::RoundedRect {
                    rect: row.row,
                    radii: [s.px(RADIUS_8); 4],
                    fill: self.ctx.color(s.bg2),
                    border: None,
                    shadow: None,
                });
                let status = if has_key {
                    self.lang().t("Keyring: set")
                } else {
                    self.lang().t("Keyring: not set")
                };
                let label = format!("{} · {}", spec.label, status);
                let text_r = if self.mirrored() {
                    let left =
                        row.clear_key_button.unwrap_or(row.set_key_button).right() + s.px(8.0);
                    Rect::new(
                        left,
                        row.row.y,
                        (row.row.right() - s.px(10.0) - left).max(0.0),
                        row.row.h,
                    )
                } else {
                    let right = row.clear_key_button.unwrap_or(row.set_key_button).x - s.px(8.0);
                    Rect::new(
                        row.row.x + s.px(10.0),
                        row.row.y,
                        (right - (row.row.x + s.px(10.0))).max(0.0),
                        row.row.h,
                    )
                };
                self.push_label(
                    text_r,
                    &label,
                    UiSize::Caption,
                    UiWeight::Regular,
                    self.start_align(),
                    s.text,
                    Some(clip),
                );
                let set_hover = self.view.state.hover == Some(SettingsHit::AiSetKey(row.index));
                self.shapes.push(Shape::RoundedRect {
                    rect: row.set_key_button,
                    radii: [s.px(RADIUS_6); 4],
                    fill: self.ctx.color(if set_hover { s.bg3 } else { s.bg0 }),
                    border: Some(Border {
                        width: s.hairline(),
                        color: self.ctx.color(s.border),
                    }),
                    shadow: None,
                });
                self.push_label(
                    row.set_key_button,
                    self.lang().t("Set key…"),
                    UiSize::Small,
                    UiWeight::Semibold,
                    UiAlign::Center,
                    s.text,
                    Some(clip),
                );
                if let Some(clr) = row.clear_key_button {
                    let clr_hover =
                        self.view.state.hover == Some(SettingsHit::AiClearKey(row.index));
                    self.shapes.push(Shape::RoundedRect {
                        rect: clr,
                        radii: [s.px(RADIUS_6); 4],
                        fill: self.ctx.color(if clr_hover { s.bg3 } else { s.bg0 }),
                        border: Some(Border {
                            width: s.hairline(),
                            color: self.ctx.color(s.border),
                        }),
                        shadow: None,
                    });
                    self.push_label(
                        clr,
                        self.lang().t("Clear"),
                        UiSize::Small,
                        UiWeight::Regular,
                        UiAlign::Center,
                        s.danger,
                        Some(clip),
                    );
                }
            }
            if let (Some(inp), Some(sv)) = (row.key_input, row.key_save_button) {
                if in_vp(inp) {
                    self.shapes.push(Shape::RoundedRect {
                        rect: inp,
                        radii: [s.px(RADIUS_6); 4],
                        fill: self.ctx.color(s.bg0),
                        border: Some(Border {
                            width: s.hairline(),
                            color: self.ctx.color(s.accent),
                        }),
                        shadow: None,
                    });
                    let masked = if state.key_draft.is_empty() {
                        self.lang()
                            .t("Paste API key (stored in keyring only)…")
                            .to_owned()
                    } else {
                        "•".repeat(state.key_draft.chars().count().min(32))
                    };
                    self.push_label(
                        Rect::new(
                            inp.x + s.px(8.0),
                            inp.y,
                            (inp.w - s.px(16.0)).max(0.0),
                            inp.h,
                        ),
                        &masked,
                        UiSize::Caption,
                        UiWeight::Regular,
                        UiAlign::Left,
                        if state.key_draft.is_empty() {
                            s.text_faint
                        } else {
                            s.text
                        },
                        Some(clip),
                    );
                    self.shapes.push(Shape::RoundedRect {
                        rect: sv,
                        radii: [s.px(RADIUS_6); 4],
                        fill: self.ctx.color(s.accent),
                        border: None,
                        shadow: None,
                    });
                    self.push_label(
                        sv,
                        self.lang().t("Save"),
                        UiSize::Small,
                        UiWeight::Semibold,
                        UiAlign::Center,
                        s.accent_text(),
                        Some(clip),
                    );
                }
            }
        }

        // Ollama URL field.
        if in_vp(ai.ollama_input) {
            let lbl_r = if self.mirrored() {
                let lx = ai.ollama_input.right() + s.px(8.0);
                Rect::new(
                    lx,
                    ai.ollama_input.y,
                    (vp.right() - lx).max(0.0),
                    ai.ollama_input.h,
                )
            } else {
                Rect::new(
                    vp.x,
                    ai.ollama_input.y,
                    (ai.ollama_input.x - s.px(8.0) - vp.x).max(0.0),
                    ai.ollama_input.h,
                )
            };
            self.push_label(
                lbl_r,
                self.lang().t("Ollama URL"),
                UiSize::Caption,
                UiWeight::Regular,
                self.start_align(),
                s.text,
                Some(clip),
            );
            let active = state.active_field == Some(AiTextField::OllamaUrl);
            self.shapes.push(Shape::RoundedRect {
                rect: ai.ollama_input,
                radii: [s.px(RADIUS_6); 4],
                fill: self.ctx.color(s.bg0),
                border: Some(Border {
                    width: s.hairline(),
                    color: self.ctx.color(if active { s.accent } else { s.border }),
                }),
                shadow: None,
            });
            self.push_label(
                Rect::new(
                    ai.ollama_input.x + s.px(8.0),
                    ai.ollama_input.y,
                    (ai.ollama_input.w - s.px(16.0)).max(0.0),
                    ai.ollama_input.h,
                ),
                &cfg.ai_ollama_url,
                UiSize::Caption,
                UiWeight::Regular,
                UiAlign::Left,
                s.text,
                Some(clip),
            );
        }

        // Monthly cost cap field.
        if in_vp(ai.cost_cap_input) {
            let lbl_r = if self.mirrored() {
                let lx = ai.cost_cap_input.right() + s.px(8.0);
                Rect::new(
                    lx,
                    ai.cost_cap_input.y,
                    (vp.right() - lx).max(0.0),
                    ai.cost_cap_input.h,
                )
            } else {
                Rect::new(
                    vp.x,
                    ai.cost_cap_input.y,
                    (ai.cost_cap_input.x - s.px(8.0) - vp.x).max(0.0),
                    ai.cost_cap_input.h,
                )
            };
            self.push_label(
                lbl_r,
                self.lang().t("Monthly cost cap (USD)"),
                UiSize::Caption,
                UiWeight::Regular,
                self.start_align(),
                s.text,
                Some(clip),
            );
            let active = state.active_field == Some(AiTextField::CostCap);
            self.shapes.push(Shape::RoundedRect {
                rect: ai.cost_cap_input,
                radii: [s.px(RADIUS_6); 4],
                fill: self.ctx.color(s.bg0),
                border: Some(Border {
                    width: s.hairline(),
                    color: self.ctx.color(if active { s.accent } else { s.border }),
                }),
                shadow: None,
            });
            let val_txt = if state.cost_cap_draft.is_empty() {
                self.lang().t("Off").to_owned()
            } else {
                format!("${}", state.cost_cap_draft)
            };
            self.push_label(
                Rect::new(
                    ai.cost_cap_input.x + s.px(8.0),
                    ai.cost_cap_input.y,
                    (ai.cost_cap_input.w - s.px(16.0)).max(0.0),
                    ai.cost_cap_input.h,
                ),
                &val_txt,
                UiSize::Caption,
                UiWeight::Regular,
                UiAlign::Left,
                if state.cost_cap_draft.is_empty() {
                    s.text_faint
                } else {
                    s.text
                },
                Some(clip),
            );
        }

        // Per-project allow-list editor.
        let allow_hdr = Rect::new(vp.x, ai.allow_input.y - s.px(22.0), vp.w, s.px(18.0));
        if in_vp(allow_hdr) {
            self.push_label(
                allow_hdr,
                self.lang()
                    .t("Project allow-list (off by default — exact argv prefixes)"),
                UiSize::Caption,
                UiWeight::Semibold,
                self.start_align(),
                s.text,
                Some(clip),
            );
        }
        if in_vp(ai.allow_input) {
            let active = state.active_field == Some(AiTextField::AllowListAdd);
            self.shapes.push(Shape::RoundedRect {
                rect: ai.allow_input,
                radii: [s.px(RADIUS_6); 4],
                fill: self.ctx.color(s.bg0),
                border: Some(Border {
                    width: s.hairline(),
                    color: self.ctx.color(if active { s.accent } else { s.border }),
                }),
                shadow: None,
            });
            let txt = if state.allow_list_draft.is_empty() {
                self.lang().t("e.g. cargo test, git status")
            } else {
                state.allow_list_draft.as_str()
            };
            self.push_label(
                Rect::new(
                    ai.allow_input.x + s.px(8.0),
                    ai.allow_input.y,
                    (ai.allow_input.w - s.px(16.0)).max(0.0),
                    ai.allow_input.h,
                ),
                txt,
                UiSize::Caption,
                UiWeight::Regular,
                UiAlign::Left,
                if state.allow_list_draft.is_empty() {
                    s.text_faint
                } else {
                    s.text
                },
                Some(clip),
            );
            let add_hover = self.view.state.hover == Some(SettingsHit::AiAllowAdd);
            self.shapes.push(Shape::RoundedRect {
                rect: ai.allow_add_button,
                radii: [s.px(RADIUS_6); 4],
                fill: self.ctx.color(if add_hover { s.bg3 } else { s.bg2 }),
                border: Some(Border {
                    width: s.hairline(),
                    color: self.ctx.color(s.border),
                }),
                shadow: None,
            });
            self.push_label(
                ai.allow_add_button,
                self.lang().t("Add"),
                UiSize::Small,
                UiWeight::Semibold,
                UiAlign::Center,
                s.text,
                Some(clip),
            );
        }
        if let Some(entries) = cfg.ai_project_allow_list.get(state.project_key()) {
            for &(idx, row, rm) in &ai.allow_entries {
                if !in_vp(row) {
                    continue;
                }
                let Some(entry) = entries.get(idx) else {
                    continue;
                };
                self.shapes.push(Shape::RoundedRect {
                    rect: row,
                    radii: [s.px(RADIUS_6); 4],
                    fill: self.ctx.color(s.bg2),
                    border: None,
                    shadow: None,
                });
                let lr = Rect::new(
                    row.x + s.px(8.0),
                    row.y,
                    (row.w - s.px(36.0)).max(0.0),
                    row.h,
                );
                self.push_label(
                    lr,
                    entry,
                    UiSize::Caption,
                    UiWeight::Regular,
                    UiAlign::Left,
                    s.text,
                    Some(clip),
                );
                Icon::Close.draw(
                    Rect::new(rm.x + s.px(5.0), rm.y + s.px(5.0), s.px(14.0), s.px(14.0)),
                    self.ctx.color(s.text_dim),
                    self.shapes,
                );
            }
        }

        // Read-only deny-list view.
        if in_vp(ai.deny_box) {
            self.shapes.push(Shape::RoundedRect {
                rect: ai.deny_box,
                radii: [s.px(RADIUS_8); 4],
                fill: self.ctx.color(s.bg2),
                border: Some(Border {
                    width: s.hairline(),
                    color: self.ctx.color(s.danger.mix(s.border, 0.55)),
                }),
                shadow: None,
            });
            let pad = s.px(10.0);
            let mut ry = ai.deny_box.y + s.px(6.0);
            self.push_label(
                Rect::new(
                    ai.deny_box.x + pad,
                    ry,
                    (ai.deny_box.w - 2.0 * pad).max(0.0),
                    s.px(18.0),
                ),
                self.lang()
                    .t("Hard deny-list (read-only — never allow-listed)"),
                UiSize::Small,
                UiWeight::Semibold,
                self.start_align(),
                s.danger,
                Some(clip),
            );
            ry += s.px(20.0);
            for &(en, ar) in &HARD_DENY_SUMMARY {
                let row = Rect::new(
                    ai.deny_box.x + pad,
                    ry,
                    (ai.deny_box.w - 2.0 * pad).max(0.0),
                    s.px(17.0),
                );
                if in_vp(row) {
                    let bullet = format!("• {}", self.lang().text(en, ar));
                    self.push_label(
                        row,
                        &bullet,
                        UiSize::Small,
                        UiWeight::Regular,
                        self.start_align(),
                        s.text_dim,
                        Some(clip),
                    );
                }
                ry += s.px(18.0);
            }
        }

        // Clear agent history button.
        if in_vp(ai.clear_history_button) {
            let hovered = self.view.state.hover == Some(SettingsHit::AiClearHistory);
            self.shapes.push(Shape::RoundedRect {
                rect: ai.clear_history_button,
                radii: [s.px(RADIUS_6); 4],
                fill: self.ctx.color(if hovered { s.bg3 } else { s.bg2 }),
                border: Some(Border {
                    width: s.hairline(),
                    color: self.ctx.color(s.border),
                }),
                shadow: None,
            });
            let lbl = if state.history_cleared {
                self.lang().t("History cleared")
            } else {
                self.lang().t("Clear agent history")
            };
            self.push_label(
                ai.clear_history_button,
                lbl,
                UiSize::Caption,
                UiWeight::Regular,
                UiAlign::Center,
                if state.history_cleared { s.ok } else { s.text },
                Some(clip),
            );
        }
    }
}
