//! The Settings overlay: a large centred panel with a section list on the start side and
//! controls on the other side, mirrored for Arabic. Uses `UiStyle` tokens and shares exact
//! physical-pixel geometry with `ui::settings_panel::SettingsLayout`.

use glyphon::TextBounds;

use super::context::DrawContext;
use super::palette::fit_query_tail;
use crate::color::{parse_hex, Palette, Rgba};
use crate::config::TwittyConfig;
use crate::input::keybindings::{command_titles, KeyBindings};
use crate::layout::Rect;
use crate::render::icons::Icon;
use crate::render::shapes::{Border, Shadow, Shape};
use crate::render::text::ui::{label_width, UiAlign, UiFont, UiLabel, UiSize, UiWeight};
use crate::render::ui_style::{
    UiStyle, OVERLAY_BLUR, OVERLAY_OFFSET, OVERLAY_SHADOW, RADIUS_12, RADIUS_6, RADIUS_8,
};
use crate::ui::chrome::UiLanguage;
use crate::ui::settings_panel::{
    current_picker_value, read_bool, read_segmented_index, read_slider_value, read_swatch_index,
    ControlGeometry, ControlKind, ControlRowLayout, ExpandedPickerLayout, FocusZone, SettingId,
    SettingsHit, SettingsLayout, SettingsPanelState, SettingsSection, ACCENT_SWATCHES,
};

/// Snapshot of settings panel state + committed config passed to the renderer.
#[derive(Clone, Debug)]
pub struct SettingsView {
    pub state: SettingsPanelState,
    pub config: TwittyConfig,
}

/// Draw the settings overlay into `shapes`, returning the outer panel rect (to clip underlying
/// text) and the shaped UI labels.
pub(crate) fn settings_overlay(
    ctx: &mut DrawContext,
    style: UiStyle,
    font: &UiFont,
    view: &SettingsView,
    surface: (f32, f32),
    shapes: &mut Vec<Shape>,
) -> (Option<Rect>, Vec<UiLabel>) {
    let layout = SettingsLayout::build(surface, style.scale, &view.state, &view.config);
    if layout.panel.w <= 0.0 || layout.panel.h <= 0.0 {
        return (None, Vec::new());
    }
    let mut drawer = SettingsDrawer {
        ctx,
        style,
        font,
        view,
        layout: &layout,
        shapes,
        labels: Vec::new(),
    };
    drawer.draw();
    (Some(layout.panel), drawer.labels)
}

struct SettingsDrawer<'a, 'b> {
    ctx: &'a mut DrawContext<'b>,
    style: UiStyle,
    font: &'a UiFont,
    view: &'a SettingsView,
    layout: &'a SettingsLayout,
    shapes: &'a mut Vec<Shape>,
    labels: Vec<UiLabel>,
}

impl SettingsDrawer<'_, '_> {
    fn lang(&self) -> UiLanguage {
        self.view.state.language
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

        // Panel shell with shadow and border.
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

        match self.view.state.section {
            SettingsSection::Appearance | SettingsSection::Behaviour | SettingsSection::Ai => {
                self.draw_controls()
            }
            SettingsSection::Keys => self.draw_keys(),
            SettingsSection::About => self.draw_about(),
        }

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
        // Bottom divider rule under the header.
        let rule_h = s.px(1.0).max(1.0);
        self.shapes.push(Shape::rounded(
            Rect::new(header.x, header.bottom() - rule_h, header.w, rule_h),
            0.0,
            self.ctx.color(s.border),
        ));
        let title = self.lang().text("Settings", "الإعدادات");
        self.push_label(
            l.header_title,
            title,
            UiSize::Title,
            UiWeight::Semibold,
            self.start_align(),
            s.text,
            None,
        );

        let close_hover = self.view.state.hover == Some(SettingsHit::CloseButton);
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

        // Vertical divider between sidebar and content.
        self.shapes
            .push(Shape::rounded(l.divider, 0.0, self.ctx.color(s.border)));

        for &(section, rect) in &l.sections {
            if rect.w <= 0.0 || rect.h <= 0.0 {
                continue;
            }
            let active = self.view.state.section == section;
            let hovered = self.view.state.hover == Some(SettingsHit::Section(section));
            let sidebar_focused = self.view.state.focus == FocusZone::Sidebar;

            if active {
                self.shapes.push(Shape::RoundedRect {
                    rect,
                    radii: [s.px(RADIUS_8); 4],
                    fill: self.ctx.color(s.accent_tint(s.bg2)),
                    border: sidebar_focused.then(|| Border {
                        width: s.px(1.0),
                        color: self.ctx.color(s.accent),
                    }),
                    shadow: None,
                });
            } else if hovered {
                self.shapes
                    .push(Shape::rounded(rect, s.px(RADIUS_8), self.ctx.color(s.bg2)));
            }

            let pad = s.px(10.0);
            let icon_side = s.px(14.0).min(rect.h).min(rect.w / 3.0);
            let icon_y = rect.y + (rect.h - icon_side) / 2.0;
            let (icon_rect, label_rect) = if self.mirrored() {
                let ir = Rect::new(rect.right() - pad - icon_side, icon_y, icon_side, icon_side);
                let lr = Rect::new(
                    rect.x + pad,
                    rect.y,
                    (ir.x - s.px(8.0) - (rect.x + pad)).max(0.0),
                    rect.h,
                );
                (ir, lr)
            } else {
                let ir = Rect::new(rect.x + pad, icon_y, icon_side, icon_side);
                let lx = ir.right() + s.px(8.0);
                let lr = Rect::new(lx, rect.y, (rect.right() - pad - lx).max(0.0), rect.h);
                (ir, lr)
            };

            let icon = match section {
                SettingsSection::Appearance => Icon::Agent,
                SettingsSection::Behaviour => Icon::Terminal,
                SettingsSection::Ai => Icon::Explain,
                SettingsSection::Keys => Icon::CopyCommand,
                SettingsSection::About => Icon::Folder,
            };
            let fg = if active || hovered {
                s.text
            } else {
                s.text_dim
            };
            icon.draw(icon_rect, self.ctx.color(fg), self.shapes);
            self.push_label(
                label_rect,
                section.label(self.lang()),
                UiSize::Body,
                if active {
                    UiWeight::Semibold
                } else {
                    UiWeight::Regular
                },
                self.start_align(),
                fg,
                None,
            );
        }
    }

    fn draw_controls(&mut self) {
        let vp = self.layout.viewport;
        let clip = rect_to_bounds(vp);
        for (idx, row) in self.layout.rows.iter().enumerate() {
            let focused =
                self.view.state.focus == FocusZone::Content && self.view.state.selected == idx;
            // Only paint rows that fit inside the content viewport so shapes never bleed past it.
            if row.rect.y >= vp.y - 1.0 && row.rect.bottom() <= vp.bottom() + 1.0 {
                self.draw_control_row(row, focused, clip);
            }
            if let Some(picker) = &row.picker {
                self.draw_expanded_picker(row.id, picker, vp, clip);
            }
        }
    }

    fn draw_control_row(&mut self, row: &ControlRowLayout, focused: bool, clip: TextBounds) {
        let s = self.style;
        let desc = row.id.descriptor();
        let row_hovered = matches!(
            self.view.state.hover,
            Some(
                SettingsHit::Toggle(id)
                    | SettingsHit::Segment(id, _)
                    | SettingsHit::SliderMinus(id)
                    | SettingsHit::SliderPlus(id)
                    | SettingsHit::SliderTrack(id, _)
                    | SettingsHit::TextField(id)
                    | SettingsHit::PickerToggle(id)
            ) if id == row.id
        ) || (row.id == SettingId::UiAccent
            && matches!(self.view.state.hover, Some(SettingsHit::Swatch(_))))
            || (row.id == SettingId::ProjectMemory
                && self.view.state.hover == Some(SettingsHit::OpenMemory));

        if focused {
            self.shapes.push(Shape::RoundedRect {
                rect: row.rect,
                radii: [s.px(RADIUS_8); 4],
                fill: self.ctx.color(s.bg2),
                border: Some(Border {
                    width: s.px(1.0),
                    color: self.ctx.color(s.accent),
                }),
                shadow: None,
            });
        } else if row_hovered {
            self.shapes.push(Shape::rounded(
                row.rect,
                s.px(RADIUS_8),
                self.ctx.color(s.bg2),
            ));
        }

        // Row label (+ optional restart / new-shells subtitle).
        if let Some(note) = desc.note(self.lang()) {
            let top_h = (row.label.h * 0.56).round();
            let top = Rect::new(row.label.x, row.label.y + s.px(2.0), row.label.w, top_h);
            let bottom = Rect::new(
                row.label.x,
                row.label.y + top_h - s.px(2.0),
                row.label.w,
                (row.label.h - top_h).max(0.0),
            );
            self.push_label(
                top,
                desc.label(self.lang()),
                UiSize::Body,
                UiWeight::Regular,
                self.start_align(),
                s.text,
                Some(clip),
            );
            self.push_label(
                bottom,
                note,
                UiSize::Small,
                UiWeight::Regular,
                self.start_align(),
                s.text_dim,
                Some(clip),
            );
        } else {
            self.push_label(
                row.label,
                desc.label(self.lang()),
                UiSize::Body,
                UiWeight::Regular,
                self.start_align(),
                s.text,
                Some(clip),
            );
        }

        // Control widget.
        match (&desc.kind, &row.geometry) {
            (ControlKind::Toggle, ControlGeometry::Toggle { track }) => {
                self.draw_toggle(row.id, *track);
            }
            (
                ControlKind::Segmented(options),
                ControlGeometry::Segmented {
                    container,
                    segments,
                },
            ) => {
                self.draw_segmented(row.id, options, *container, segments, clip);
            }
            (
                ControlKind::Slider(spec),
                ControlGeometry::Slider {
                    minus,
                    track,
                    plus,
                    value,
                },
            ) => {
                self.draw_slider(row.id, *spec, *minus, *track, *plus, *value, clip);
            }
            (ControlKind::Swatches, ControlGeometry::Swatches { swatches }) => {
                self.draw_swatches(swatches);
            }
            (
                ControlKind::TextField {
                    placeholder_en,
                    placeholder_ar,
                },
                ControlGeometry::TextField { input },
            ) => {
                let placeholder = self.lang().text(placeholder_en, placeholder_ar);
                self.draw_text_field(row.id, *input, placeholder, focused, clip);
            }
            (ControlKind::ListPicker { .. }, ControlGeometry::ListPicker { button }) => {
                let expanded = self
                    .view
                    .state
                    .expanded
                    .as_ref()
                    .is_some_and(|e| e.setting == row.id);
                self.draw_picker_button(row.id, *button, expanded, clip);
            }
            (ControlKind::ActionButton, ControlGeometry::ActionButton { button }) => {
                let hovered = row.id == SettingId::ProjectMemory
                    && self.view.state.hover == Some(SettingsHit::OpenMemory);
                self.shapes.push(Shape::RoundedRect {
                    rect: *button,
                    radii: [s.px(RADIUS_6); 4],
                    fill: self.ctx.color(if hovered { s.bg3 } else { s.bg0 }),
                    border: Some(Border {
                        width: s.px(1.0),
                        color: self.ctx.color(if hovered { s.accent } else { s.border }),
                    }),
                    shadow: None,
                });
                let text = self.lang().text("Open…", "فتح…");
                self.push_label(
                    *button,
                    text,
                    UiSize::Caption,
                    UiWeight::Semibold,
                    UiAlign::Center,
                    s.text,
                    Some(clip),
                );
            }
            _ => {}
        }
    }

    fn draw_toggle(&mut self, id: SettingId, track: Rect) {
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

    fn draw_segmented(
        &mut self,
        id: SettingId,
        options: &[crate::ui::settings_panel::SegmentedOption],
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
    fn draw_slider(
        &mut self,
        id: SettingId,
        spec: crate::ui::settings_panel::SliderSpec,
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

    fn draw_swatches(&mut self, swatches: &[Rect]) {
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

    fn draw_text_field(
        &mut self,
        id: SettingId,
        input: Rect,
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
        let value = match id {
            SettingId::AgentCommand => self.view.config.agent_command.as_deref().unwrap_or(""),
            _ => "",
        };
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

    fn draw_picker_button(
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

    fn draw_expanded_picker(
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

    fn draw_search_box(&mut self, rect: Rect, query: &str, placeholder: &str, clip: TextBounds) {
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

    fn draw_keys(&mut self) {
        let s = self.style;
        let clip = rect_to_bounds(self.layout.viewport);
        let bindings = KeyBindings::from_config(&self.view.config.keybindings);
        let has_overrides = !self.view.config.keybindings.is_empty();

        if let Some(search) = self.layout.keys_search {
            let placeholder = self.lang().text("Search shortcuts…", "دوّر في الاختصارات…");
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
                self.lang().text("Reset all", "رجّع الكل"),
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
                self.lang().text(
                    "Press a new shortcut…  Esc cancels · Backspace clears",
                    "اضغط الاختصار الجديد…  Esc للإلغاء · Backspace للمسح",
                )
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
                    self.lang()
                        .text("No matching shortcuts", "لا توجد اختصارات مطابقة"),
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
                self.lang().text("Cancel", "إلغاء")
            } else {
                self.lang().text("Change", "غيّر")
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
                    self.lang().text("Reset", "رجّع الافتراضي"),
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
                    self.lang().text("Press keys…", "اضغط مفتاحاً…"),
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
                    self.lang().text("Unbound", "غير معيّن"),
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

    fn draw_about(&mut self) {
        let s = self.style;
        let clip = rect_to_bounds(self.layout.viewport);

        if let Some(hdr) = self.layout.about_header {
            self.shapes.push(Shape::RoundedRect {
                rect: hdr,
                radii: [s.px(RADIUS_8); 4],
                fill: self.ctx.color(s.bg2),
                border: Some(Border {
                    width: s.px(1.0),
                    color: self.ctx.color(s.border),
                }),
                shadow: None,
            });
            let pad = s.px(14.0);
            let title_rect = Rect::new(
                hdr.x + pad,
                hdr.y + s.px(10.0),
                (hdr.w - 2.0 * pad).max(0.0),
                s.px(24.0),
            );
            let sub_rect = Rect::new(
                hdr.x + pad,
                title_rect.bottom() + s.px(2.0),
                (hdr.w - 2.0 * pad).max(0.0),
                s.px(20.0),
            );
            self.push_label(
                title_rect,
                concat!("Twitty ", env!("CARGO_PKG_VERSION")),
                UiSize::Title,
                UiWeight::Semibold,
                self.start_align(),
                s.text,
                Some(clip),
            );
            self.push_label(
                sub_rect,
                self.lang().text(
                    "Native Bidirectional RTL Terminal",
                    "طرفية أصلية ثنائية الاتجاه للعربية",
                ),
                UiSize::Caption,
                UiWeight::Regular,
                self.start_align(),
                s.text_dim,
                Some(clip),
            );
        }

        let config_path_display = self.view.state.config_path.as_deref().unwrap_or("—");

        let items = [
            (
                self.lang().text("Version", "الإصدار"),
                env!("CARGO_PKG_VERSION"),
                false,
            ),
            (
                self.lang().text("License", "الرخصة"),
                "AGPL-3.0-or-later",
                false,
            ),
            (
                self.lang().text("Config file", "ملف الإعدادات"),
                config_path_display,
                true,
            ),
        ];

        for (i, (&row, (key_label, val_text, has_copy))) in
            self.layout.about_info_rows.iter().zip(items).enumerate()
        {
            let focused = has_copy
                && self.view.state.focus == FocusZone::Content
                && self.view.state.selected == 0;
            self.shapes.push(Shape::RoundedRect {
                rect: row,
                radii: [s.px(RADIUS_8); 4],
                fill: self.ctx.color(s.bg2),
                border: focused.then(|| Border {
                    width: s.px(1.0),
                    color: self.ctx.color(s.accent),
                }),
                shadow: None,
            });
            let pad = s.px(12.0);
            let avail = if has_copy {
                if let Some(btn) = self.layout.copy_button {
                    if self.mirrored() {
                        Rect::new(
                            btn.right() + s.px(10.0),
                            row.y,
                            (row.right() - pad - (btn.right() + s.px(10.0))).max(0.0),
                            row.h,
                        )
                    } else {
                        Rect::new(
                            row.x + pad,
                            row.y,
                            (btn.x - s.px(10.0) - (row.x + pad)).max(0.0),
                            row.h,
                        )
                    }
                } else {
                    Rect::new(row.x + pad, row.y, (row.w - 2.0 * pad).max(0.0), row.h)
                }
            } else {
                Rect::new(row.x + pad, row.y, (row.w - 2.0 * pad).max(0.0), row.h)
            };

            if i == 2 {
                // Config file row: stack label on top and full path underneath.
                let top_h = (avail.h * 0.48).round();
                let top = Rect::new(avail.x, avail.y + s.px(3.0), avail.w, top_h);
                let bot = Rect::new(
                    avail.x,
                    avail.y + top_h - s.px(2.0),
                    avail.w,
                    (avail.h - top_h).max(0.0),
                );
                self.push_label(
                    top,
                    key_label,
                    UiSize::Caption,
                    UiWeight::Semibold,
                    self.start_align(),
                    s.text,
                    Some(clip),
                );
                self.push_label(
                    bot,
                    val_text,
                    UiSize::Small,
                    UiWeight::Regular,
                    self.start_align(),
                    s.text_dim,
                    Some(clip),
                );
            } else {
                let key_w = (avail.w * 0.42).min(s.px(140.0));
                let (k_rect, v_rect) = if self.mirrored() {
                    let kr = Rect::new(avail.right() - key_w, avail.y, key_w, avail.h);
                    let vr = Rect::new(
                        avail.x,
                        avail.y,
                        (kr.x - s.px(8.0) - avail.x).max(0.0),
                        avail.h,
                    );
                    (kr, vr)
                } else {
                    let kr = Rect::new(avail.x, avail.y, key_w, avail.h);
                    let vx = kr.right() + s.px(8.0);
                    let vr = Rect::new(vx, avail.y, (avail.right() - vx).max(0.0), avail.h);
                    (kr, vr)
                };
                self.push_label(
                    k_rect,
                    key_label,
                    UiSize::Body,
                    UiWeight::Regular,
                    self.start_align(),
                    s.text_dim,
                    Some(clip),
                );
                self.push_label(
                    v_rect,
                    val_text,
                    UiSize::Body,
                    UiWeight::Semibold,
                    self.end_align(),
                    s.text,
                    Some(clip),
                );
            }
        }

        if let Some(btn) = self.layout.copy_button {
            let hovered = self.view.state.hover == Some(SettingsHit::CopyConfigPath);
            let copied = self.view.state.copied_config_path;
            self.shapes.push(Shape::RoundedRect {
                rect: btn,
                radii: [s.px(RADIUS_6); 4],
                fill: self.ctx.color(if copied {
                    s.accent
                } else if hovered {
                    s.bg3
                } else {
                    s.bg0
                }),
                border: Some(Border {
                    width: s.px(1.0),
                    color: self.ctx.color(if copied { s.accent } else { s.border }),
                }),
                shadow: None,
            });
            let fg = if copied { s.accent_text() } else { s.text };
            let icon_side = s.px(13.0).min(btn.h);
            let pad = s.px(8.0);
            let iy = btn.y + (btn.h - icon_side) / 2.0;
            let (icon_rect, text_rect) = if self.mirrored() {
                let ir = Rect::new(btn.right() - pad - icon_side, iy, icon_side, icon_side);
                (
                    ir,
                    Rect::new(
                        btn.x + pad,
                        btn.y,
                        (ir.x - s.px(4.0) - (btn.x + pad)).max(0.0),
                        btn.h,
                    ),
                )
            } else {
                let ir = Rect::new(btn.x + pad, iy, icon_side, icon_side);
                let tx = ir.right() + s.px(4.0);
                (
                    ir,
                    Rect::new(tx, btn.y, (btn.right() - pad - tx).max(0.0), btn.h),
                )
            };
            let icon = if copied { Icon::Check } else { Icon::Copy };
            icon.draw(icon_rect, self.ctx.color(fg), self.shapes);
            let text = if copied {
                self.lang().text("Copied", "تم النسخ")
            } else {
                self.lang().text("Copy path", "نسخ المسار")
            };
            self.push_label(
                text_rect,
                text,
                UiSize::Caption,
                UiWeight::Semibold,
                UiAlign::Center,
                fg,
                Some(clip),
            );
        }
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

fn contrasting_fg(bg: Rgba) -> Rgba {
    let [r, g, b, _] = bg.to_linear();
    let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let v = if lum > 0.179 { 0.0 } else { 1.0 };
    Rgba::new(v, v, v, 1.0)
}

fn rect_to_bounds(r: Rect) -> TextBounds {
    TextBounds {
        left: r.x.ceil() as i32,
        top: r.y.ceil() as i32,
        right: r.right().floor() as i32,
        bottom: r.bottom().floor() as i32,
    }
}

fn intersect_bounds(a: TextBounds, b: TextBounds) -> TextBounds {
    let left = a.left.max(b.left);
    let top = a.top.max(b.top);
    TextBounds {
        left,
        top,
        right: a.right.min(b.right).max(left),
        bottom: a.bottom.min(b.bottom).max(top),
    }
}
