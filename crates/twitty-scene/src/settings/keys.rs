//! Drawing the Keys section: shortcuts, search and key capture.

use super::*;

impl SettingsDrawer<'_, '_> {
    pub(super) fn draw_keys(&mut self) {
        let s = self.style;
        let clip = rect_to_bounds(self.layout.viewport);
        let bindings = KeyBindings::from_config(&self.view.config.keybindings);
        let has_overrides = !self.view.config.keybindings.is_empty();

        if let Some(search) = self.layout.keys_search {
            let placeholder = self.lang().t("Search shortcuts…");
            self.draw_search_box(search, &self.view.state.keys_query, placeholder, clip);
        }

        if let Some(reset_all) = self.layout.keys_reset_all {
            let hovered = self.view.state.hover == Some(SettingsHit::KeysResetAll);
            let fill = if hovered && has_overrides {
                s.bg3
            } else if has_overrides {
                s.bg2
            } else {
                s.bg0
            };
            self.shapes.push(Shape::RoundedRect {
                rect: reset_all,
                radii: [s.px(RADIUS_8); 4],
                fill: self.ctx.color(fill),
                border: Some(Border {
                    width: s.px(1.0),
                    color: self.ctx.color(s.border),
                }),
                shadow: None,
            });
            let fg = if has_overrides {
                if hovered {
                    s.text
                } else {
                    s.text_dim
                }
            } else {
                s.text_faint
            };
            self.push_label(
                reset_all,
                self.lang().t("Reset all"),
                UiSize::Caption,
                UiWeight::Semibold,
                UiAlign::Center,
                fg,
                Some(clip),
            );
        }

        if let Some(banner) = self.layout.keys_banner {
            let is_notice = self.view.state.key_capture_notice.is_some();
            let border_color = if is_notice { s.warn } else { s.accent };
            self.shapes.push(Shape::RoundedRect {
                rect: banner,
                radii: [s.px(RADIUS_6); 4],
                fill: self.ctx.color(s.bg2),
                border: Some(Border {
                    width: s.px(1.0),
                    color: self.ctx.color(border_color),
                }),
                shadow: None,
            });
            let pad = s.px(10.0);
            let inner = Rect::new(
                banner.x + pad,
                banner.y,
                (banner.w - 2.0 * pad).max(0.0),
                banner.h,
            );
            let text = if let Some(notice) = self.view.state.key_capture_notice {
                notice.message(self.lang())
            } else {
                self.lang()
                    .t("Press a new shortcut…  Esc cancels · Backspace clears")
            };
            self.push_label(
                inner,
                text,
                UiSize::Caption,
                UiWeight::Semibold,
                self.start_align(),
                if is_notice { s.warn } else { s.accent },
                Some(clip),
            );
        }

        let filtered = self.view.state.filtered_commands();
        if filtered.is_empty() {
            if let Some(search) = self.layout.keys_search {
                let empty = Rect::new(
                    self.layout.viewport.x,
                    search.bottom() + s.px(16.0),
                    self.layout.viewport.w,
                    s.px(28.0),
                );
                self.push_label(
                    empty,
                    self.lang().t("No matching shortcuts"),
                    UiSize::Body,
                    UiWeight::Regular,
                    self.start_align(),
                    s.text_dim,
                    Some(clip),
                );
            }
            return;
        }

        for row_layout in &self.layout.keys_row_layouts {
            let cmd_idx = row_layout.cmd_idx;
            let row = row_layout.rect;
            let Some(cmd) = filtered.get(cmd_idx) else {
                continue;
            };
            let capturing_this = self.view.state.capturing_key == Some(cmd.name);
            let selected =
                self.view.state.focus == FocusZone::Content && self.view.state.selected == cmd_idx;
            let row_hovered = matches!(
                self.view.state.hover,
                Some(
                    SettingsHit::KeyRow(i) | SettingsHit::KeyChange(i) | SettingsHit::KeyReset(i)
                ) if i == cmd_idx
            );
            if capturing_this {
                self.shapes.push(Shape::RoundedRect {
                    rect: row,
                    radii: [s.px(RADIUS_6); 4],
                    fill: self.ctx.color(s.accent_tint(s.bg2)),
                    border: Some(Border {
                        width: s.px(1.0),
                        color: self.ctx.color(s.accent),
                    }),
                    shadow: None,
                });
            } else if selected {
                self.shapes.push(Shape::rounded(
                    row,
                    s.px(RADIUS_6),
                    self.ctx.color(s.accent_tint(s.bg2)),
                ));
            } else if row_hovered {
                self.shapes
                    .push(Shape::rounded(row, s.px(RADIUS_6), self.ctx.color(s.bg2)));
            }

            // Per-row "Change / غيّر" button
            let change_hover = self.view.state.hover == Some(SettingsHit::KeyChange(cmd_idx));
            let change_rect = row_layout.change_button;
            self.shapes.push(Shape::RoundedRect {
                rect: change_rect,
                radii: [s.px(RADIUS_6); 4],
                fill: self.ctx.color(if capturing_this {
                    s.accent
                } else if change_hover {
                    s.bg3
                } else {
                    s.bg0
                }),
                border: Some(Border {
                    width: s.px(1.0),
                    color: self
                        .ctx
                        .color(if capturing_this { s.accent } else { s.border }),
                }),
                shadow: None,
            });
            let change_label = if capturing_this {
                self.lang().t("Cancel")
            } else {
                self.lang().t("Change")
            };
            self.push_label(
                change_rect,
                change_label,
                UiSize::Small,
                UiWeight::Semibold,
                UiAlign::Center,
                if capturing_this {
                    s.accent_text()
                } else if change_hover {
                    s.text
                } else {
                    s.text_dim
                },
                Some(clip),
            );

            // Per-row "Reset / رجّع الافتراضي" button (shown when customized)
            if let Some(reset_rect) = row_layout.reset_button {
                let reset_hover = self.view.state.hover == Some(SettingsHit::KeyReset(cmd_idx));
                self.shapes.push(Shape::RoundedRect {
                    rect: reset_rect,
                    radii: [s.px(RADIUS_6); 4],
                    fill: self.ctx.color(if reset_hover { s.bg3 } else { s.bg0 }),
                    border: Some(Border {
                        width: s.px(1.0),
                        color: self.ctx.color(s.border),
                    }),
                    shadow: None,
                });
                self.push_label(
                    reset_rect,
                    self.lang().t("Reset"),
                    UiSize::Small,
                    UiWeight::Regular,
                    UiAlign::Center,
                    if reset_hover { s.text } else { s.text_dim },
                    Some(clip),
                );
            }

            let pad = s.px(8.0);
            let inner = if self.mirrored() {
                let left = change_rect.right() + s.px(8.0);
                Rect::new(left, row.y, (row.right() - pad - left).max(0.0), row.h)
            } else {
                let right = change_rect.x - s.px(8.0);
                Rect::new(row.x + pad, row.y, (right - (row.x + pad)).max(0.0), row.h)
            };

            let conflict = bindings.conflict_for(cmd.name);
            let is_custom = self.view.config.keybindings.contains(cmd.name);
            let keys_w = if capturing_this {
                self.draw_key_status_pill(
                    inner,
                    self.lang().t("Press keys…"),
                    s.accent,
                    s.accent,
                    clip,
                )
            } else if let Some(shortcut) = bindings.settings_display_shortcut(cmd.name) {
                let cap_border = if conflict.is_some() { s.warn } else { s.border };
                let cap_text = if conflict.as_ref().is_some_and(|c| !c.is_winner) {
                    s.warn
                } else {
                    s.text_dim
                };
                self.draw_keycaps(inner, &shortcut, cap_border, cap_text, clip)
            } else if is_custom {
                self.draw_key_status_pill(
                    inner,
                    self.lang().t("Unbound"),
                    s.border,
                    s.text_faint,
                    clip,
                )
            } else {
                0.0
            };

            let title_room = (inner.w - keys_w - s.px(8.0)).max(0.0);
            let title_rect = if self.mirrored() {
                Rect::new(inner.right() - title_room, inner.y, title_room, inner.h)
            } else {
                Rect::new(inner.x, inner.y, title_room, inner.h)
            };
            let primary = self.lang().text(cmd.title_en, cmd.title_ar);
            if let Some(info) = conflict {
                let (other_en, other_ar) = command_titles(info.other);
                let note = match (self.lang(), info.is_winner) {
                    (UiLanguage::English, true) => format!("⚠ Conflicts with {other_en}"),
                    (UiLanguage::English, false) => format!("⚠ Overridden by {other_en}"),
                    (UiLanguage::Arabic, true) => format!("⚠ يتعارض مع {other_ar}"),
                    (UiLanguage::Arabic, false) => format!("⚠ مُستبدل بواسطة {other_ar}"),
                };
                let top_h = (title_rect.h * 0.56).round();
                let top = Rect::new(title_rect.x, title_rect.y + s.px(1.0), title_rect.w, top_h);
                let bot = Rect::new(
                    title_rect.x,
                    title_rect.y + top_h - s.px(2.0),
                    title_rect.w,
                    (title_rect.h - top_h).max(0.0),
                );
                self.push_label(
                    top,
                    primary,
                    UiSize::Body,
                    UiWeight::Regular,
                    self.start_align(),
                    s.text,
                    Some(clip),
                );
                self.push_label(
                    bot,
                    &note,
                    UiSize::Small,
                    UiWeight::Regular,
                    self.start_align(),
                    s.warn,
                    Some(clip),
                );
            } else {
                self.push_label(
                    title_rect,
                    primary,
                    UiSize::Body,
                    UiWeight::Regular,
                    self.start_align(),
                    s.text,
                    Some(clip),
                );
            }
        }
    }

    fn draw_key_status_pill(
        &mut self,
        row: Rect,
        text: &str,
        border: Rgba,
        fg: Rgba,
        clip: TextBounds,
    ) -> f32 {
        let s = self.style;
        let buf = self.font.shape(
            self.ctx.font_system,
            UiSize::Small,
            UiWeight::Semibold,
            s.scale,
            text,
        );
        let pad = s.px(10.0);
        let w = (label_width(&buf) + pad).min(row.w * 0.55);
        if w <= s.px(16.0) {
            return 0.0;
        }
        let h = s.px(20.0).min(row.h - s.px(4.0));
        let y = row.y + (row.h - h) / 2.0;
        let x = if self.mirrored() {
            row.x
        } else {
            row.right() - w
        };
        let pill = Rect::new(x, y, w, h);
        self.shapes.push(Shape::RoundedRect {
            rect: pill,
            radii: [s.px(RADIUS_6); 4],
            fill: self.ctx.color(s.bg0),
            border: Some(Border {
                width: s.px(1.0),
                color: self.ctx.color(border),
            }),
            shadow: None,
        });
        let mut label = UiLabel::new(buf, pill, UiAlign::Center, fg);
        label.bounds = intersect_bounds(label.bounds, clip);
        self.labels.push(label);
        w
    }

    fn draw_keycaps(
        &mut self,
        row: Rect,
        shortcut: &str,
        border_color: Rgba,
        text_color: Rgba,
        clip: TextBounds,
    ) -> f32 {
        let s = self.style;
        let parts: Vec<_> = shortcut
            .split('+')
            .map(|k| {
                self.font.shape(
                    self.ctx.font_system,
                    UiSize::Small,
                    UiWeight::Regular,
                    s.scale,
                    k,
                )
            })
            .collect();
        let gap = s.px(3.0);
        let pad = s.px(8.0);
        let total: f32 = parts.iter().map(|b| label_width(b) + pad).sum::<f32>()
            + gap * parts.len().saturating_sub(1) as f32;
        if total > row.w * 0.62 {
            return 0.0;
        }
        let h = s.px(20.0).min(row.h - s.px(4.0));
        let y = row.y + (row.h - h) / 2.0;
        let mut x = if self.mirrored() {
            row.x
        } else {
            row.right() - total
        };
        for buf in parts {
            let w = label_width(&buf) + pad;
            let cap = Rect::new(x, y, w, h);
            self.shapes.push(Shape::RoundedRect {
                rect: cap,
                radii: [s.px(RADIUS_6); 4],
                fill: self.ctx.color(s.bg2),
                border: Some(Border {
                    width: s.px(1.0),
                    color: self.ctx.color(border_color),
                }),
                shadow: None,
            });
            let mut label = UiLabel::new(buf, cap, UiAlign::Center, text_color);
            label.bounds = intersect_bounds(label.bounds, clip);
            self.labels.push(label);
            x += w + gap;
        }
        total
    }
}
