//! The Settings overlay: a large centred panel with a section list on the start side and
//! controls on the other side, mirrored for Arabic. Uses `UiStyle` tokens and shares exact
//! physical-pixel geometry with `ui::settings_panel::SettingsLayout`.

mod about;
mod ai;
mod commands;
mod controls;
mod keys;

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
use crate::ui::settings_commands::{display_name, mode_label, FormField};
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
            SettingsSection::Appearance | SettingsSection::Behaviour => self.draw_controls(),
            SettingsSection::Ai => self.draw_ai(),
            SettingsSection::Commands => self.draw_commands(),
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
                SettingsSection::Commands => Icon::Rerun,
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
            (ControlKind::Toggle(_), ControlGeometry::Toggle { track }) => {
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
