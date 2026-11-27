//! Drawing one setting's control: toggles, segments, sliders, swatches, fields and pickers.

use super::*;

impl SettingsDrawer<'_, '_> {
    pub(super) fn draw_toggle(&mut self, id: SettingId, track: Rect) {
        let s = self.style;
        let on = read_bool(id, &self.view.config);
        let radius = track.h / 2.0;
        self.shapes.push(Shape::RoundedRect {
            rect: track,
            radii: [radius; 4],
            fill: self.ctx.color(if on { s.accent } else { s.bg3 }),
            border: (!on).then(|| Border {
                width: s.px(1.0),
                color: self.ctx.color(s.border),
            }),
            shadow: None,
        });
        let pad = s.px(3.0);
        let knob_side = (track.h - 2.0 * pad).max(4.0);
        let knob_on_right = if self.mirrored() { !on } else { on };
        let knob_x = if knob_on_right {
            track.right() - pad - knob_side
        } else {
            track.x + pad
        };
        let knob = Rect::new(knob_x, track.y + pad, knob_side, knob_side);
        self.shapes.push(Shape::rounded(
            knob,
            knob_side / 2.0,
            self.ctx
                .color(if on { s.accent_text() } else { s.text_dim }),
        ));
    }

    pub(super) fn draw_segmented(
        &mut self,
        id: SettingId,
        options: &[twitty_ui::ui::settings_panel::SegmentedOption],
        container: Rect,
        segments: &[Rect],
        clip: TextBounds,
    ) {
        let s = self.style;
        let active = read_segmented_index(id, &self.view.config);
        self.shapes.push(Shape::RoundedRect {
            rect: container,
            radii: [s.px(RADIUS_8); 4],
            fill: self.ctx.color(s.bg0),
            border: Some(Border {
                width: s.px(1.0),
                color: self.ctx.color(s.border),
            }),
            shadow: None,
        });
        for (i, (opt, &rect)) in options.iter().zip(segments).enumerate() {
            let selected = i == active;
            let hovered = self.view.state.hover == Some(SettingsHit::Segment(id, i));
            if selected {
                self.shapes.push(Shape::rounded(
                    rect,
                    s.px(RADIUS_6),
                    self.ctx.color(s.accent),
                ));
            } else if hovered {
                self.shapes
                    .push(Shape::rounded(rect, s.px(RADIUS_6), self.ctx.color(s.bg3)));
            }
            let fg = if selected {
                s.accent_text()
            } else if hovered {
                s.text
            } else {
                s.text_dim
            };
            self.push_label(
                rect,
                opt.label(self.lang()),
                UiSize::Caption,
                if selected {
                    UiWeight::Semibold
                } else {
                    UiWeight::Regular
                },
                UiAlign::Center,
                fg,
                Some(clip),
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_slider(
        &mut self,
        id: SettingId,
        spec: twitty_ui::ui::settings_panel::SliderSpec,
        minus: Rect,
        track: Rect,
        plus: Rect,
        value_rect: Rect,
        clip: TextBounds,
    ) {
        let s = self.style;
        let val = read_slider_value(id, &self.view.config);
        let ratio = spec.ratio(val);
        let formatted = spec.format_value(val);
        self.push_label(
            value_rect,
            &formatted,
            UiSize::Caption,
            UiWeight::Regular,
            UiAlign::Center,
            s.text_dim,
            Some(clip),
        );

        // Minus button
        let minus_hover = self.view.state.hover == Some(SettingsHit::SliderMinus(id));
        self.shapes.push(Shape::RoundedRect {
            rect: minus,
            radii: [s.px(RADIUS_6); 4],
            fill: self.ctx.color(if minus_hover { s.bg3 } else { s.bg0 }),
            border: Some(Border {
                width: s.px(1.0),
                color: self.ctx.color(s.border),
            }),
            shadow: None,
        });
        let cy = minus.y + minus.h / 2.0;
        let inset = minus.w * 0.28;
        self.shapes.push(Shape::Capsule {
            from: [minus.x + inset, cy],
            to: [minus.right() - inset, cy],
            width: s.px(1.5).max(1.0),
            color: self
                .ctx
                .color(if minus_hover { s.text } else { s.text_dim }),
        });

        // Plus button
        let plus_hover = self.view.state.hover == Some(SettingsHit::SliderPlus(id));
        self.shapes.push(Shape::RoundedRect {
            rect: plus,
            radii: [s.px(RADIUS_6); 4],
            fill: self.ctx.color(if plus_hover { s.bg3 } else { s.bg0 }),
            border: Some(Border {
                width: s.px(1.0),
                color: self.ctx.color(s.border),
            }),
            shadow: None,
        });
        let p_side = s.px(12.0).min(plus.w).min(plus.h);
        Icon::Plus.draw(
            Rect::new(
                plus.x + (plus.w - p_side) / 2.0,
                plus.y + (plus.h - p_side) / 2.0,
                p_side,
                p_side,
            ),
            self.ctx.color(if plus_hover { s.text } else { s.text_dim }),
            self.shapes,
        );

        // Track + fill + thumb
        let bar_h = s.px(4.0).max(2.0);
        let bar_y = track.y + (track.h - bar_h) / 2.0;
        let bar = Rect::new(track.x, bar_y, track.w, bar_h);
        self.shapes
            .push(Shape::rounded(bar, bar_h / 2.0, self.ctx.color(s.bg3)));
        let fill_w = (track.w * ratio).max(0.0);
        if fill_w > 0.0 {
            self.shapes.push(Shape::rounded(
                Rect::new(track.x, bar_y, fill_w, bar_h),
                bar_h / 2.0,
                self.ctx.color(s.accent),
            ));
        }
        let thumb_d = s.px(12.0).min(track.h);
        let thumb_cx =
            (track.x + fill_w).clamp(track.x + thumb_d / 2.0, track.right() - thumb_d / 2.0);
        let thumb = Rect::new(
            thumb_cx - thumb_d / 2.0,
            track.y + (track.h - thumb_d) / 2.0,
            thumb_d,
            thumb_d,
        );
        self.shapes.push(Shape::RoundedRect {
            rect: thumb,
            radii: [thumb_d / 2.0; 4],
            fill: self.ctx.color(s.accent),
            border: Some(Border {
                width: s.px(1.5),
                color: self.ctx.color(s.bg1),
            }),
            shadow: None,
        });
    }

    pub(super) fn draw_swatches(&mut self, swatches: &[Rect]) {
        let s = self.style;
        let selected = read_swatch_index(&self.view.config);
        for (i, (swatch, &rect)) in ACCENT_SWATCHES.iter().zip(swatches).enumerate() {
            let color = swatch
                .hex
                .and_then(parse_hex)
                .unwrap_or(self.ctx.palette.cursor);
            let is_selected = selected == Some(i);
            let is_hovered = self.view.state.hover == Some(SettingsHit::Swatch(i));
            let radius = rect.w.min(rect.h) / 2.0;
            self.shapes.push(Shape::RoundedRect {
                rect,
                radii: [radius; 4],
                fill: self.ctx.color(color),
                border: if is_selected {
                    Some(Border {
                        width: s.px(2.0),
                        color: self.ctx.color(s.text),
                    })
                } else if is_hovered {
                    Some(Border {
                        width: s.px(1.5),
                        color: self.ctx.color(s.text_dim),
                    })
                } else {
                    Some(Border {
                        width: s.px(1.0),
                        color: self.ctx.color(s.border),
                    })
                },
                shadow: None,
            });
            if is_selected {
                let side = (rect.w * 0.56).max(6.0);
                let check = Rect::new(
                    rect.x + (rect.w - side) / 2.0,
                    rect.y + (rect.h - side) / 2.0,
                    side,
                    side,
                );
                Icon::Check.draw(check, self.ctx.color(contrasting_fg(color)), self.shapes);
            }
        }
    }

    pub(super) fn draw_text_field(
        &mut self,
        id: SettingId,
        input: Rect,
        placeholder: &str,
        focused: bool,
        clip: TextBounds,
    ) {
        let value = self.view.state.text_value(id, &self.view.config);
        self.draw_text_input(input, &value, placeholder, focused, clip);
        // Color fields show the color they hold at the reading end of the box.
        if SettingId::COLORS.contains(&id) && id != SettingId::ColorAnsi {
            if let Some(color) = parse_hex(&value) {
                let s = self.style;
                let side = (input.h - s.px(12.0)).max(0.0);
                let x = if self.mirrored() {
                    input.x + s.px(6.0)
                } else {
                    input.right() - s.px(6.0) - side
                };
                self.shapes.push(Shape::RoundedRect {
                    rect: Rect::new(x, input.y + (input.h - side) / 2.0, side, side),
                    radii: [s.px(3.0); 4],
                    fill: self.ctx.color(color),
                    border: Some(Border {
                        width: s.px(1.0),
                        color: self.ctx.color(s.border),
                    }),
                    shadow: None,
                });
            }
        }
    }

    /// A one-line text box with a placeholder and, when focused, a caret at the end.
    pub(super) fn draw_text_input(
        &mut self,
        input: Rect,
        value: &str,
        placeholder: &str,
        focused: bool,
        clip: TextBounds,
    ) {
        let s = self.style;
        self.shapes.push(Shape::RoundedRect {
            rect: input,
            radii: [s.px(RADIUS_6); 4],
            fill: self.ctx.color(s.bg0),
            border: Some(Border {
                width: s.px(if focused { 1.5 } else { 1.0 }),
                color: self.ctx.color(if focused { s.accent } else { s.border }),
            }),
            shadow: None,
        });
        let pad = s.px(8.0);
        let text_rect = Rect::new(
            input.x + pad,
            input.y,
            (input.w - 2.0 * pad).max(0.0),
            input.h,
        );
        if value.is_empty() {
            self.push_label(
                text_rect,
                placeholder,
                UiSize::Caption,
                UiWeight::Regular,
                self.start_align(),
                s.text_faint,
                Some(clip),
            );
            if focused {
                let cx = if self.mirrored() {
                    text_rect.right() - s.px(1.5)
                } else {
                    text_rect.x
                };
                let ch = s.px(14.0).min(input.h - s.px(8.0));
                self.shapes.push(Shape::rounded(
                    Rect::new(cx, input.y + (input.h - ch) / 2.0, s.px(1.5).max(1.0), ch),
                    0.0,
                    self.ctx.color(s.accent),
                ));
            }
        } else {
            let (_, buffer) =
                fit_query_tail(self.font, self.ctx.font_system, s.scale, value, text_rect.w);
            let w = label_width(&buffer);
            let mut label = UiLabel::new(buffer, text_rect, UiAlign::Start, s.text);
            label.bounds = intersect_bounds(label.bounds, clip);
            let caret_x = if label.buffer.layout_runs().next().is_some_and(|r| r.rtl) {
                (label.left - s.px(2.0)).max(text_rect.x)
            } else {
                (label.left + w + s.px(1.0)).min(text_rect.right() - s.px(1.5))
            };
            self.labels.push(label);
            if focused {
                let ch = s.px(14.0).min(input.h - s.px(8.0));
                self.shapes.push(Shape::rounded(
                    Rect::new(
                        caret_x,
                        input.y + (input.h - ch) / 2.0,
                        s.px(1.5).max(1.0),
                        ch,
                    ),
                    0.0,
                    self.ctx.color(s.accent),
                ));
            }
        }
    }

    /// A small rounded button; `primary` fills it with the accent colour.
    pub(super) fn draw_small_button(
        &mut self,
        rect: Rect,
        text: &str,
        hovered: bool,
        primary: bool,
        clip: TextBounds,
    ) {
        let s = self.style;
        let fill = match (primary, hovered) {
            (true, _) => s.accent,
            (false, true) => s.bg3,
            (false, false) => s.bg0,
        };
        self.shapes.push(Shape::RoundedRect {
            rect,
            radii: [s.px(RADIUS_6); 4],
            fill: self.ctx.color(fill),
            border: Some(Border {
                width: s.px(1.0),
                color: self.ctx.color(if primary { s.accent } else { s.border }),
            }),
            shadow: None,
        });
        let fg = if primary {
            contrasting_fg(s.accent)
        } else if hovered {
            s.text
        } else {
            s.text_dim
        };
        self.push_label(
            rect,
            text,
            UiSize::Caption,
            UiWeight::Semibold,
            UiAlign::Center,
            fg,
            Some(clip),
        );
    }

    pub(super) fn draw_picker_button(
        &mut self,
        id: SettingId,
        button: Rect,
        expanded: bool,
        clip: TextBounds,
    ) {
        let s = self.style;
        let hovered = self.view.state.hover == Some(SettingsHit::PickerToggle(id));
        self.shapes.push(Shape::RoundedRect {
            rect: button,
            radii: [s.px(RADIUS_6); 4],
            fill: self.ctx.color(if hovered { s.bg3 } else { s.bg0 }),
            border: Some(Border {
                width: s.px(1.0),
                color: self.ctx.color(if expanded { s.accent } else { s.border }),
            }),
            shadow: None,
        });
        let pad = s.px(8.0);
        let chev_side = s.px(12.0).min(button.h);
        let chev_y = button.y + (button.h - chev_side) / 2.0;
        let (chev_rect, mut inner) = if self.mirrored() {
            let cr = Rect::new(button.x + pad, chev_y, chev_side, chev_side);
            let ix = cr.right() + s.px(6.0);
            (
                cr,
                Rect::new(ix, button.y, (button.right() - pad - ix).max(0.0), button.h),
            )
        } else {
            let cr = Rect::new(
                button.right() - pad - chev_side,
                chev_y,
                chev_side,
                chev_side,
            );
            (
                cr,
                Rect::new(
                    button.x + pad,
                    button.y,
                    (cr.x - s.px(6.0) - (button.x + pad)).max(0.0),
                    button.h,
                ),
            )
        };
        let chevron = if expanded {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        };
        chevron.draw(chev_rect, self.ctx.color(s.text_dim), self.shapes);

        let value_text: String = match id {
            SettingId::Theme => {
                let name = self.view.state.effective_theme(&self.view.config);
                if let Some(pal) = Palette::named(name) {
                    inner = self.draw_theme_dots(inner, &pal);
                }
                name.to_owned()
            }
            SettingId::FontFamily => self.view.config.font_family.clone().unwrap_or_else(|| {
                self.lang()
                    .text("System monospace", "خط النظام أحادي العرض")
                    .to_owned()
            }),
            SettingId::UiFontFamily => {
                self.view.config.ui_font_family.clone().unwrap_or_else(|| {
                    self.lang()
                        .text("System sans-serif", "خط النظام الافتراضي")
                        .to_owned()
                })
            }
            _ => String::new(),
        };
        self.push_label(
            inner,
            &value_text,
            UiSize::Caption,
            UiWeight::Regular,
            self.start_align(),
            s.text,
            Some(clip),
        );
    }

    fn draw_theme_dots(&mut self, rect: Rect, palette: &Palette) -> Rect {
        let s = self.style;
        let dot = s.px(8.0).min(rect.h - s.px(6.0));
        let gap = s.px(3.0);
        let group_w = 3.0 * dot + 2.0 * gap;
        if rect.w < group_w + s.px(16.0) {
            return rect;
        }
        let dy = rect.y + (rect.h - dot) / 2.0;
        let colors = [palette.background, palette.foreground, palette.cursor];
        if self.mirrored() {
            let start_x = rect.right() - group_w;
            for (i, c) in colors.into_iter().enumerate() {
                self.shapes.push(Shape::RoundedRect {
                    rect: Rect::new(start_x + i as f32 * (dot + gap), dy, dot, dot),
                    radii: [dot / 2.0; 4],
                    fill: self.ctx.color(c),
                    border: Some(Border {
                        width: s.px(0.75),
                        color: self.ctx.color(s.border),
                    }),
                    shadow: None,
                });
            }
            Rect::new(
                rect.x,
                rect.y,
                (start_x - s.px(6.0) - rect.x).max(0.0),
                rect.h,
            )
        } else {
            for (i, c) in colors.into_iter().enumerate() {
                self.shapes.push(Shape::RoundedRect {
                    rect: Rect::new(rect.x + i as f32 * (dot + gap), dy, dot, dot),
                    radii: [dot / 2.0; 4],
                    fill: self.ctx.color(c),
                    border: Some(Border {
                        width: s.px(0.75),
                        color: self.ctx.color(s.border),
                    }),
                    shadow: None,
                });
            }
            let nx = rect.x + group_w + s.px(6.0);
            Rect::new(nx, rect.y, (rect.right() - nx).max(0.0), rect.h)
        }
    }

    pub(super) fn draw_expanded_picker(
        &mut self,
        setting: SettingId,
        picker: &ExpandedPickerLayout,
        vp: Rect,
        clip: TextBounds,
    ) {
        let s = self.style;
        let clamped_top = picker.rect.y.max(vp.y);
        let clamped_bottom = picker.rect.bottom().min(vp.bottom());
        if clamped_bottom <= clamped_top {
            return;
        }
        let box_rect = Rect::new(
            picker.rect.x,
            clamped_top,
            picker.rect.w,
            clamped_bottom - clamped_top,
        );
        self.shapes.push(Shape::RoundedRect {
            rect: box_rect,
            radii: [s.px(RADIUS_8); 4],
            fill: self.ctx.color(s.bg0),
            border: Some(Border {
                width: s.px(1.0),
                color: self.ctx.color(s.accent),
            }),
            shadow: None,
        });

        let Some(exp) = self.view.state.expanded.as_ref() else {
            return;
        };

        if let Some(search) = picker.search {
            if search.y >= vp.y - 1.0 && search.bottom() <= vp.bottom() + 1.0 {
                let placeholder = self.lang().text("Search fonts…", "دوّر في الخطوط…");
                self.draw_search_box(search, &exp.query, placeholder, clip);
            }
        }

        let opts = self.view.state.filtered_picker_options(setting, &exp.query);
        let committed = current_picker_value(setting, &self.view.config);

        if opts.is_empty() {
            let empty_rect = Rect::new(
                box_rect.x + s.px(8.0),
                picker.search.map_or(box_rect.y, |sr| sr.bottom()) + s.px(4.0),
                (box_rect.w - s.px(16.0)).max(0.0),
                s.px(26.0),
            );
            if empty_rect.bottom() <= vp.bottom() {
                self.push_label(
                    empty_rect,
                    self.lang().text("No matching fonts", "لا توجد خطوط مطابقة"),
                    UiSize::Caption,
                    UiWeight::Regular,
                    self.start_align(),
                    s.text_dim,
                    Some(clip),
                );
            }
            return;
        }

        for &(opt_idx, item_rect) in &picker.items {
            if item_rect.y < vp.y - 1.0 || item_rect.bottom() > vp.bottom() + 1.0 {
                continue;
            }
            let Some(opt) = opts.get(opt_idx) else {
                continue;
            };
            let is_highlighted = opt_idx == exp.highlighted;
            let is_hovered =
                self.view.state.hover == Some(SettingsHit::PickerItem(setting, opt_idx));
            let is_committed = match (&opt.value, &committed) {
                (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
                (None, None) => true,
                _ => false,
            };

            if is_highlighted || is_hovered {
                self.shapes.push(Shape::rounded(
                    item_rect,
                    s.px(RADIUS_6),
                    self.ctx.color(if is_highlighted {
                        s.accent_tint(s.bg2)
                    } else {
                        s.bg2
                    }),
                ));
            }

            let pad = s.px(8.0);
            let check_side = s.px(14.0).min(item_rect.h);
            let check_y = item_rect.y + (item_rect.h - check_side) / 2.0;
            let (check_rect, mut label_rect) = if self.mirrored() {
                let cr = Rect::new(item_rect.x + pad, check_y, check_side, check_side);
                let lx = cr.right() + s.px(6.0);
                (
                    cr,
                    Rect::new(
                        lx,
                        item_rect.y,
                        (item_rect.right() - pad - lx).max(0.0),
                        item_rect.h,
                    ),
                )
            } else {
                let cr = Rect::new(
                    item_rect.right() - pad - check_side,
                    check_y,
                    check_side,
                    check_side,
                );
                (
                    cr,
                    Rect::new(
                        item_rect.x + pad,
                        item_rect.y,
                        (cr.x - s.px(6.0) - (item_rect.x + pad)).max(0.0),
                        item_rect.h,
                    ),
                )
            };

            if setting == SettingId::Theme {
                if let Some(pal) = opt.value.as_deref().and_then(Palette::named) {
                    label_rect = self.draw_theme_dots(label_rect, &pal);
                }
            }
            if is_committed {
                Icon::Check.draw(check_rect, self.ctx.color(s.accent), self.shapes);
            }
            self.push_label(
                label_rect,
                opt.label(self.lang()),
                UiSize::Caption,
                if is_committed {
                    UiWeight::Semibold
                } else {
                    UiWeight::Regular
                },
                self.start_align(),
                s.text,
                Some(clip),
            );
        }
    }

    pub(super) fn draw_search_box(
        &mut self,
        rect: Rect,
        query: &str,
        placeholder: &str,
        clip: TextBounds,
    ) {
        let s = self.style;
        self.shapes.push(Shape::RoundedRect {
            rect,
            radii: [s.px(RADIUS_8); 4],
            fill: self.ctx.color(s.bg2),
            border: Some(Border {
                width: s.px(1.0),
                color: self.ctx.color(s.border),
            }),
            shadow: None,
        });
        let side = s.px(14.0).min(rect.h).min(rect.w / 3.0);
        let pad = s.px(8.0);
        let iy = rect.y + (rect.h - side) / 2.0;
        let (icon_rect, text_rect) = if self.mirrored() {
            let ir = Rect::new(rect.right() - pad - side, iy, side, side);
            (
                ir,
                Rect::new(
                    rect.x + pad,
                    rect.y,
                    (ir.x - s.px(6.0) - (rect.x + pad)).max(0.0),
                    rect.h,
                ),
            )
        } else {
            let ir = Rect::new(rect.x + pad, iy, side, side);
            let tx = ir.right() + s.px(6.0);
            (
                ir,
                Rect::new(tx, rect.y, (rect.right() - pad - tx).max(0.0), rect.h),
            )
        };
        Icon::Search.draw(icon_rect, self.ctx.color(s.text_dim), self.shapes);
        if query.is_empty() {
            self.push_label(
                text_rect,
                placeholder,
                UiSize::Body,
                UiWeight::Regular,
                self.start_align(),
                s.text_dim,
                Some(clip),
            );
        } else {
            let (_, buffer) =
                fit_query_tail(self.font, self.ctx.font_system, s.scale, query, text_rect.w);
            let mut label = UiLabel::new(buffer, text_rect, UiAlign::Start, s.text);
            label.bounds = intersect_bounds(label.bounds, clip);
            self.labels.push(label);
        }
    }
}
