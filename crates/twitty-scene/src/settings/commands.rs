//! Drawing the Commands section: saved commands and their form.

use super::*;

impl SettingsDrawer<'_, '_> {
    /// Settings → Commands: the saved-command list, or the add/edit form.
    pub(super) fn draw_commands(&mut self) {
        let s = self.style;
        let view = self.view;
        let layout = self.layout;
        let Some(list) = layout.commands.as_ref() else {
            return;
        };
        let lang = self.lang();
        let clip = rect_to_bounds(layout.viewport);
        let editor = &view.state.commands_editor;
        let hover = view.state.hover;

        if let Some(form) = &editor.form {
            for &(field, label, input) in &list.fields {
                self.push_label(
                    label,
                    field.label(lang),
                    UiSize::Caption,
                    UiWeight::Semibold,
                    self.start_align(),
                    s.text_dim,
                    Some(clip),
                );
                let focused = form.field == field;
                if field == FormField::Mode {
                    if focused {
                        self.shapes.push(Shape::RoundedRect {
                            rect: input,
                            radii: [s.px(RADIUS_8); 4],
                            fill: self.ctx.color(s.bg0),
                            border: Some(Border {
                                width: s.px(1.5),
                                color: self.ctx.color(s.accent),
                            }),
                            shadow: None,
                        });
                    }
                    for &(mode, rect) in &list.mode_buttons {
                        let hovered = hover == Some(SettingsHit::CommandModeButton(mode));
                        self.draw_small_button(
                            rect,
                            mode_label(mode, lang),
                            hovered,
                            form.mode == mode,
                            clip,
                        );
                    }
                    continue;
                }
                let placeholder = match field {
                    FormField::Name => lang.text("e.g. Run tests", "مثلاً: شغّل الاختبارات"),
                    FormField::NameAr => lang.text("e.g. شغّل الاختبارات", "مثلاً: شغّل الاختبارات"),
                    _ => lang.text("e.g. cargo test", "مثلاً: cargo test"),
                };
                self.draw_text_input(input, form.text(field), placeholder, focused, clip);
            }
            if let (Some(rect), Some((en, ar))) = (list.error, &form.error) {
                self.push_label(
                    rect,
                    if lang == UiLanguage::Arabic { ar } else { en },
                    UiSize::Caption,
                    UiWeight::Semibold,
                    self.start_align(),
                    s.danger,
                    Some(clip),
                );
            }
            if let Some(save) = list.save {
                let text = if form.editing.is_some() {
                    lang.text("Save", "احفظ")
                } else {
                    lang.text("Add", "أضف")
                };
                self.draw_small_button(save, text, false, true, clip);
            }
            if let Some(cancel) = list.cancel {
                let hovered = hover == Some(SettingsHit::CommandCancel);
                self.draw_small_button(cancel, lang.text("Cancel", "إلغاء"), hovered, false, clip);
            }
            return;
        }

        if let Some(add) = list.add_button {
            let hovered = hover == Some(SettingsHit::CommandsAdd);
            self.draw_small_button(
                add,
                lang.text("+ Add command", "+ أضف أمر"),
                hovered,
                true,
                clip,
            );
        }
        let commands = &view.config.commands;
        if let Some(intro) = list.intro {
            let text = if commands.is_empty() {
                lang.text(
                    "No saved commands yet. Saved commands appear in the palette and the composer.",
                    "مفيش أوامر محفوظة لسه. الأوامر المحفوظة بتظهر في لوحة الأوامر وفي خانة الكتابة.",
                )
            } else {
                lang.text(
                    "Enter edits · Delete removes · Ctrl+N adds",
                    "Enter للتعديل · Delete للحذف · Ctrl+N للإضافة",
                )
            };
            self.push_label(
                intro,
                text,
                UiSize::Caption,
                UiWeight::Regular,
                self.start_align(),
                s.text_dim,
                Some(clip),
            );
        }
        for row in &list.rows {
            let Some(spec) = commands.get(row.index) else {
                continue;
            };
            let selected = view.state.focus == FocusZone::Content && editor.selected == row.index;
            let row_hovered = matches!(
                hover,
                Some(
                    SettingsHit::CommandRow(i)
                        | SettingsHit::CommandEdit(i)
                        | SettingsHit::CommandDelete(i)
                ) if i == row.index
            );
            let fill = if selected {
                s.accent_tint(s.bg2)
            } else if row_hovered {
                s.bg2
            } else {
                s.bg1
            };
            self.shapes.push(Shape::RoundedRect {
                rect: row.rect,
                radii: [s.px(RADIUS_6); 4],
                fill: self.ctx.color(fill),
                border: Some(Border {
                    width: s.px(1.0),
                    color: self.ctx.color(s.border),
                }),
                shadow: None,
            });
            let pad = s.px(10.0);
            let buttons_w = row.edit.w + row.delete.w + s.px(6.0 + 8.0 + 10.0);
            let text_w = (row.rect.w - pad - buttons_w).max(0.0);
            let text_x = if self.mirrored() {
                row.rect.right() - pad - text_w
            } else {
                row.rect.x + pad
            };
            let half = row.rect.h / 2.0;
            let title = format!(
                "{}  ·  {}",
                display_name(spec, lang),
                mode_label(spec.mode, lang)
            );
            self.push_label(
                Rect::new(text_x, row.rect.y + s.px(2.0), text_w, half),
                &title,
                UiSize::Body,
                UiWeight::Semibold,
                self.start_align(),
                s.text,
                Some(clip),
            );
            let command: String = spec
                .command
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(200)
                .collect();
            self.push_label(
                Rect::new(text_x, row.rect.y + half - s.px(2.0), text_w, half),
                &command,
                UiSize::Caption,
                UiWeight::Regular,
                self.start_align(),
                s.text_dim,
                Some(clip),
            );
            let edit_hover = hover == Some(SettingsHit::CommandEdit(row.index));
            self.draw_small_button(row.edit, lang.text("Edit", "عدّل"), edit_hover, false, clip);
            let delete_hover = hover == Some(SettingsHit::CommandDelete(row.index));
            self.draw_small_button(
                row.delete,
                lang.text("Delete", "احذف"),
                delete_hover,
                false,
                clip,
            );
        }
    }
}
