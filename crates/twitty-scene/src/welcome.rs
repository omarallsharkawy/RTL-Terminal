//! CPU-side rendering for the first-run Welcome wizard overlay.
//!
//! Draws a centred card using `UiStyle` tokens, `Shape`, `Icon`, `UiFont`, and `UiLabel`,
//! sharing exact physical-pixel geometry with `ui::welcome::WelcomeLayout`. Returns the outer
//! card rect so `Frame::text_runs` clips terminal/chrome text behind the card.

use super::ai::wrap_lines;
use super::context::DrawContext;
use twitty_core::color::{Palette, Rgba};
use twitty_core::layout::Rect;
use twitty_render::icons::Icon;
use twitty_render::shapes::{Border, Shadow, Shape};
use twitty_render::text::labels::UiLabels;
use twitty_render::text::ui::{UiAlign, UiLabel, UiSize, UiWeight};
use twitty_render::ui_style::{
    UiStyle, OVERLAY_BLUR, OVERLAY_OFFSET, OVERLAY_SHADOW, RADIUS_12, RADIUS_6, RADIUS_8, SPACE_12,
    SPACE_4, SPACE_8,
};
use twitty_ui::config::TwittyConfig;
use twitty_ui::ui::chrome::{UiLanguage, UiLanguagePref};
use twitty_ui::ui::welcome::{
    WelcomeHit, WelcomeLayout, WelcomeState, WelcomeStep, WELCOME_THEMES,
};

/// Snapshot of welcome wizard state + live config passed to the renderer.
#[derive(Clone, Debug)]
pub struct WelcomeView {
    pub state: WelcomeState,
    pub config: TwittyConfig,
}

/// Draw the Welcome wizard overlay into `shapes`, returning its outer rect (to clip underlying
/// text) and shaped UI labels.
pub fn welcome_overlay<'a>(
    ctx: &mut DrawContext,
    style: UiStyle,
    font: impl Into<UiLabels<'a>>,
    view: &WelcomeView,
    surface: (f32, f32),
    shapes: &mut Vec<Shape>,
) -> (Option<Rect>, Vec<UiLabel>) {
    let layout = WelcomeLayout::build(surface, style.scale, &view.state);
    if layout.card.w <= 0.0 || layout.card.h <= 0.0 {
        return (None, Vec::new());
    }
    let mut drawer = WelcomeDrawer {
        ctx,
        style,
        font: font.into(),
        view,
        layout: &layout,
        shapes,
        labels: Vec::new(),
    };
    drawer.draw();
    (Some(layout.card), drawer.labels)
}

struct WelcomeDrawer<'a, 'b> {
    ctx: &'a mut DrawContext<'b>,
    style: UiStyle,
    font: UiLabels<'a>,
    view: &'a WelcomeView,
    layout: &'a WelcomeLayout,
    shapes: &'a mut Vec<Shape>,
    labels: Vec<UiLabel>,
}

impl WelcomeDrawer<'_, '_> {
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

        // Card shell with border and shadow.
        self.shapes.push(Shape::RoundedRect {
            rect: l.card,
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

        self.draw_header();
        self.draw_subtitle();

        match self.view.state.step {
            WelcomeStep::Welcome => self.draw_step_welcome(),
            WelcomeStep::Look => self.draw_step_look(),
            WelcomeStep::Shell => self.draw_step_shell(),
            WelcomeStep::Ai => self.draw_step_ai(),
            WelcomeStep::Shortcuts => self.draw_step_shortcuts(),
        }

        self.draw_footer();
    }

    fn draw_header(&mut self) {
        let s = self.style;
        let l = self.layout;
        let rule_h = s.hairline();
        self.shapes.push(Shape::rounded(
            Rect::new(l.header.x, l.header.bottom() - rule_h, l.header.w, rule_h),
            0.0,
            self.ctx.color(s.border),
        ));

        self.push_label(
            l.header_title,
            self.view.state.step.title(self.lang()),
            UiSize::Title,
            UiWeight::Semibold,
            self.start_align(),
            s.text,
        );

        let counter = format!(
            "{} / {}",
            self.view.state.step.index() + 1,
            WelcomeStep::ALL.len()
        );
        self.push_label(
            l.step_counter,
            &counter,
            UiSize::Caption,
            UiWeight::Regular,
            self.end_align(),
            s.text_dim,
        );

        let close_hover = self.view.state.hover == Some(WelcomeHit::Close);
        if close_hover {
            self.shapes.push(Shape::rounded(
                l.close_button,
                s.px(RADIUS_6),
                self.ctx.color(s.bg3),
            ));
        }
        let side = s.px(13.0).min(l.close_button.w).min(l.close_button.h);
        let icon_rect = Rect::new(
            l.close_button.x + (l.close_button.w - side) * 0.5,
            l.close_button.y + (l.close_button.h - side) * 0.5,
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

    fn draw_subtitle(&mut self) {
        let s = self.style;
        let rect = self.layout.subtitle;
        if rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }
        let text = self.view.state.step.subtitle(self.lang());
        self.push_wrapped(
            rect,
            text,
            UiSize::Caption,
            UiWeight::Regular,
            self.start_align(),
            s.text_dim,
            2,
        );
    }

    // ---- Step 1: Welcome & Language ----

    fn draw_step_welcome(&mut self) {
        let s = self.style;
        let l = self.layout;
        let lang = self.lang();

        // Hero card showing Twitty's Arabic-first identity.
        self.shapes.push(Shape::RoundedRect {
            rect: l.hero_box,
            radii: [s.px(RADIUS_8); 4],
            fill: self.ctx.color(s.bg0),
            border: Some(Border {
                width: s.hairline(),
                color: self.ctx.color(s.border),
            }),
            shadow: None,
        });

        let pad = s.px(SPACE_12);
        let icon_side = s.px(28.0).min(l.hero_box.h - 2.0 * pad).max(0.0);
        let icon_ltr = Rect::new(l.hero_box.x + pad, l.hero_box.y + pad, icon_side, icon_side);
        let icon_box = lang.mirror(icon_ltr, l.hero_box);
        self.shapes.push(Shape::rounded(
            icon_box,
            s.px(RADIUS_6),
            self.ctx.color(s.accent_tint(s.bg2)),
        ));
        let glyph_side = (icon_side * 0.62).max(0.0);
        Icon::Terminal.draw(
            Rect::new(
                icon_box.x + (icon_box.w - glyph_side) * 0.5,
                icon_box.y + (icon_box.h - glyph_side) * 0.5,
                glyph_side,
                glyph_side,
            ),
            self.ctx.color(s.accent),
            self.shapes,
        );

        let text_ltr = Rect::new(
            icon_ltr.right() + s.px(SPACE_12),
            l.hero_box.y + pad,
            (l.hero_box.right() - pad - (icon_ltr.right() + s.px(SPACE_12))).max(0.0),
            (l.hero_box.h - 2.0 * pad).max(0.0),
        );
        let text_area = lang.mirror(text_ltr, l.hero_box);
        let headline_h = s.px(22.0).min(text_area.h);
        self.push_label(
            Rect::new(text_area.x, text_area.y, text_area.w, headline_h),
            lang.t("Twitty · Arabic-first terminal"),
            UiSize::Body,
            UiWeight::Semibold,
            self.start_align(),
            s.text,
        );
        let sample_y = text_area.y + headline_h + s.px(SPACE_4);
        let sample_h = s.px(22.0).min((text_area.bottom() - sample_y).max(0.0));
        self.push_label(
            Rect::new(text_area.x, sample_y, text_area.w, sample_h),
            lang.t("Bidi shaping, command blocks, composer & local coding agents."),
            UiSize::Caption,
            UiWeight::Regular,
            self.start_align(),
            s.text_dim,
        );
        let preview_y = sample_y + sample_h + s.px(SPACE_8);
        let preview_h = (text_area.bottom() - preview_y).max(0.0).min(s.px(24.0));
        if preview_h > s.px(12.0) {
            let pill = Rect::new(text_area.x, preview_y, text_area.w, preview_h);
            self.shapes
                .push(Shape::rounded(pill, s.px(RADIUS_6), self.ctx.color(s.bg2)));
            let pill_inner = Rect::new(
                pill.x + s.px(SPACE_8),
                pill.y,
                (pill.w - 2.0 * s.px(SPACE_8)).max(0.0),
                pill.h,
            );
            self.push_label(
                pill_inner,
                "$ echo \"مرحباً بالعالم\"  ·  تم ✓",
                UiSize::Small,
                UiWeight::Regular,
                self.start_align(),
                s.accent,
            );
        }

        // Interface language segmented control.
        self.push_label(
            l.language_label,
            lang.t("Interface language"),
            UiSize::Body,
            UiWeight::Semibold,
            self.start_align(),
            s.text,
        );

        let focused = self.view.state.focused_control == 0;
        self.shapes.push(Shape::RoundedRect {
            rect: l.language_container,
            radii: [s.px(RADIUS_8); 4],
            fill: self.ctx.color(s.bg0),
            border: Some(Border {
                width: if focused { s.px(1.5) } else { s.hairline() },
                color: self.ctx.color(if focused { s.accent } else { s.border }),
            }),
            shadow: None,
        });

        let active_pref = self.view.config.ui_language_pref();
        for &(pref, rect) in &l.language_options {
            let selected = pref == active_pref;
            let hovered = self.view.state.hover == Some(WelcomeHit::LanguageOption(pref));
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
            let label = match pref {
                UiLanguagePref::English => "English",
                UiLanguagePref::Arabic => "العربية",
                UiLanguagePref::Auto => lang.t("Follow system"),
            };
            let fg = if selected {
                s.accent_text()
            } else if hovered {
                s.text
            } else {
                s.text_dim
            };
            self.push_label(
                rect,
                label,
                UiSize::Caption,
                if selected {
                    UiWeight::Semibold
                } else {
                    UiWeight::Regular
                },
                UiAlign::Center,
                fg,
            );
        }

        let hint = match active_pref {
            UiLanguagePref::Auto => match self.view.state.env_language {
                UiLanguage::Arabic => lang.t("System locale detected: Arabic"),
                UiLanguage::English => lang.t("System locale detected: English"),
            },
            UiLanguagePref::English => lang.t("Menus and controls stay in English."),
            UiLanguagePref::Arabic => lang.t("Menus and controls are mirrored for Arabic."),
        };
        self.push_label(
            l.language_hint,
            hint,
            UiSize::Small,
            UiWeight::Regular,
            self.start_align(),
            s.text_dim,
        );
    }

    // ---- Step 2: Look (Theme Swatches, Font Size, Transparency) ----

    fn draw_step_look(&mut self) {
        let s = self.style;
        let l = self.layout;
        let lang = self.lang();

        self.push_label(
            l.theme_label,
            lang.t("Theme"),
            UiSize::Body,
            UiWeight::Semibold,
            self.start_align(),
            s.text,
        );

        let active_idx = WelcomeState::active_theme_index(&self.view.config);
        let themes_focused = self.view.state.focused_control == 0;
        for (i, (spec, &rect)) in WELCOME_THEMES.iter().zip(&l.theme_swatches).enumerate() {
            let selected = active_idx == Some(i);
            let hovered = self.view.state.hover == Some(WelcomeHit::ThemeSwatch(i));
            let pal = Palette::named(spec.id).unwrap_or_default();

            let border_color = if selected {
                s.accent
            } else if hovered {
                s.text_dim
            } else if themes_focused && active_idx.is_none() && i == 0 {
                s.accent
            } else {
                s.border
            };
            let border_w = if selected { s.px(2.0) } else { s.hairline() };

            self.shapes.push(Shape::RoundedRect {
                rect,
                radii: [s.px(RADIUS_8); 4],
                fill: self.ctx.color(s.bg2),
                border: Some(Border {
                    width: border_w,
                    color: self.ctx.color(border_color),
                }),
                shadow: None,
            });

            // Miniature terminal preview inside the top portion of the swatch card.
            let inset = s.px(5.0);
            let preview_h = ((rect.h - 2.0 * inset) * 0.64).max(0.0);
            let preview = Rect::new(
                rect.x + inset,
                rect.y + inset,
                (rect.w - 2.0 * inset).max(0.0),
                preview_h,
            );
            self.shapes.push(Shape::RoundedRect {
                rect: preview,
                radii: [s.px(RADIUS_6); 4],
                fill: self.ctx.color(pal.background),
                border: Some(Border {
                    width: s.hairline(),
                    color: self.ctx.color(pal.background.mix(pal.foreground, 0.16)),
                }),
                shadow: None,
            });

            // Three miniature ANSI dots + two simulated prompt/output lines.
            let dot_r = s.px(3.0);
            let dot_y = preview.y + s.px(7.0);
            let dot_colors = [pal.ansi[1], pal.ansi[3], pal.ansi[2]];
            for (d, color) in dot_colors.into_iter().enumerate() {
                let dx_ltr = preview.x + s.px(7.0) + d as f32 * s.px(9.0);
                let dot = lang.mirror(Rect::new(dx_ltr, dot_y, dot_r * 2.0, dot_r * 2.0), preview);
                self.shapes
                    .push(Shape::rounded(dot, dot_r, self.ctx.color(color)));
            }

            let bar_h = s.px(4.0).max(2.0);
            let line1_y = preview.y + s.px(20.0);
            if line1_y + bar_h < preview.bottom() - s.px(4.0) {
                let prompt_w = (preview.w * 0.22).max(4.0);
                let cmd_w = (preview.w * 0.45).max(6.0);
                let p_rect = lang.mirror(
                    Rect::new(preview.x + s.px(7.0), line1_y, prompt_w, bar_h),
                    preview,
                );
                let c_rect = lang.mirror(
                    Rect::new(
                        preview.x + s.px(11.0) + prompt_w,
                        line1_y,
                        cmd_w.min((preview.w - s.px(18.0) - prompt_w).max(0.0)),
                        bar_h,
                    ),
                    preview,
                );
                self.shapes.push(Shape::rounded(
                    p_rect,
                    bar_h * 0.5,
                    self.ctx.color(pal.cursor),
                ));
                self.shapes.push(Shape::rounded(
                    c_rect,
                    bar_h * 0.5,
                    self.ctx.color(pal.foreground),
                ));
            }
            let line2_y = line1_y + s.px(9.0);
            if line2_y + bar_h < preview.bottom() - s.px(4.0) {
                let out_w = (preview.w * 0.58).max(6.0);
                let o_rect = lang.mirror(
                    Rect::new(preview.x + s.px(7.0), line2_y, out_w, bar_h),
                    preview,
                );
                self.shapes.push(Shape::rounded(
                    o_rect,
                    bar_h * 0.5,
                    self.ctx.color(pal.ansi[4]),
                ));
            }

            // Check badge when selected.
            if selected {
                let badge = s.px(15.0);
                let b_ltr = Rect::new(
                    preview.right() - badge - s.px(3.0),
                    preview.y + s.px(3.0),
                    badge,
                    badge,
                );
                let b_rect = lang.mirror(b_ltr, preview);
                self.shapes.push(Shape::rounded(
                    b_rect,
                    badge * 0.5,
                    self.ctx.color(s.accent),
                ));
                let check_s = badge * 0.65;
                Icon::Check.draw(
                    Rect::new(
                        b_rect.x + (badge - check_s) * 0.5,
                        b_rect.y + (badge - check_s) * 0.5,
                        check_s,
                        check_s,
                    ),
                    self.ctx.color(s.accent_text()),
                    self.shapes,
                );
            }

            let label_y = preview.bottom() + s.px(2.0);
            let label_rect = Rect::new(
                rect.x + s.px(4.0),
                label_y,
                (rect.w - s.px(8.0)).max(0.0),
                (rect.bottom() - label_y - s.px(2.0)).max(0.0),
            );
            self.push_label(
                label_rect,
                spec.label(lang),
                UiSize::Small,
                if selected {
                    UiWeight::Semibold
                } else {
                    UiWeight::Regular
                },
                UiAlign::Center,
                if selected { s.text } else { s.text_dim },
            );
        }

        // Font size stepper row.
        let font_focused = self.view.state.focused_control == 1;
        let font_hover = matches!(
            self.view.state.hover,
            Some(WelcomeHit::FontSizeMinus | WelcomeHit::FontSizePlus)
        );
        self.draw_row_shell(l.font_row, font_focused, font_hover);
        self.push_label(
            l.font_label,
            lang.t("Font size"),
            UiSize::Body,
            UiWeight::Regular,
            self.start_align(),
            s.text,
        );

        let minus_hover = self.view.state.hover == Some(WelcomeHit::FontSizeMinus);
        self.shapes.push(Shape::RoundedRect {
            rect: l.font_minus,
            radii: [s.px(RADIUS_6); 4],
            fill: self.ctx.color(if minus_hover { s.bg3 } else { s.bg0 }),
            border: Some(Border {
                width: s.hairline(),
                color: self.ctx.color(s.border),
            }),
            shadow: None,
        });
        let cy = l.font_minus.y + l.font_minus.h * 0.5;
        let inset = l.font_minus.w * 0.28;
        self.shapes.push(Shape::Capsule {
            from: [l.font_minus.x + inset, cy],
            to: [l.font_minus.right() - inset, cy],
            width: s.px(1.5).max(1.0),
            color: self
                .ctx
                .color(if minus_hover { s.text } else { s.text_dim }),
        });

        let size_val = self.view.config.font_size;
        let size_str = if (size_val - size_val.round()).abs() < 0.05 {
            format!("{:.0} pt", size_val.round())
        } else {
            format!("{size_val:.1} pt")
        };
        self.push_label(
            l.font_value,
            &size_str,
            UiSize::Caption,
            UiWeight::Semibold,
            UiAlign::Center,
            s.text,
        );

        let plus_hover = self.view.state.hover == Some(WelcomeHit::FontSizePlus);
        self.shapes.push(Shape::RoundedRect {
            rect: l.font_plus,
            radii: [s.px(RADIUS_6); 4],
            fill: self.ctx.color(if plus_hover { s.bg3 } else { s.bg0 }),
            border: Some(Border {
                width: s.hairline(),
                color: self.ctx.color(s.border),
            }),
            shadow: None,
        });
        let p_side = s.px(12.0).min(l.font_plus.w).min(l.font_plus.h);
        Icon::Plus.draw(
            Rect::new(
                l.font_plus.x + (l.font_plus.w - p_side) * 0.5,
                l.font_plus.y + (l.font_plus.h - p_side) * 0.5,
                p_side,
                p_side,
            ),
            self.ctx.color(if plus_hover { s.text } else { s.text_dim }),
            self.shapes,
        );

        // Transparency toggle row.
        let trans_on = WelcomeState::transparency_enabled(&self.view.config);
        let trans_focused = self.view.state.focused_control == 2;
        let trans_hover = self.view.state.hover == Some(WelcomeHit::TransparencyToggle);
        self.draw_row_shell(l.transparency_row, trans_focused, trans_hover);
        let opacity_pct = (self.view.config.background_opacity * 100.0).round() as i32;
        let trans_note = if trans_on {
            twitty_core::language::fill(lang.t("Translucent background ({}%)"), &[&opacity_pct])
        } else {
            lang.t("Solid opaque background (100%)").to_owned()
        };
        self.draw_two_line_label(
            l.transparency_label,
            lang.t("Window transparency"),
            &trans_note,
        );
        self.draw_toggle(l.transparency_track, trans_on);
    }

    // ---- Step 3: Shell Integration ----

    fn draw_step_shell(&mut self) {
        let s = self.style;
        let l = self.layout;
        let lang = self.lang();
        let shell = &self.view.state.shell;

        self.shapes.push(Shape::RoundedRect {
            rect: l.shell_card,
            radii: [s.px(RADIUS_8); 4],
            fill: self.ctx.color(s.bg0),
            border: Some(Border {
                width: s.hairline(),
                color: self.ctx.color(s.border),
            }),
            shadow: None,
        });

        let pad = s.px(SPACE_12);
        let top_h = s.px(32.0).min(l.shell_card.h * 0.5);
        let top_bar = Rect::new(
            l.shell_card.x + pad,
            l.shell_card.y + pad,
            (l.shell_card.w - 2.0 * pad).max(0.0),
            top_h,
        );

        let icon_side = s.px(18.0).min(top_h);
        let icon_ltr = Rect::new(
            top_bar.x,
            top_bar.y + (top_h - icon_side) * 0.5,
            icon_side,
            icon_side,
        );
        let badge_w = s.px(148.0).min(top_bar.w * 0.45);
        let badge_h = s.px(24.0).min(top_h);
        let badge_ltr = Rect::new(
            top_bar.right() - badge_w,
            top_bar.y + (top_h - badge_h) * 0.5,
            badge_w,
            badge_h,
        );
        let shell_title_ltr = Rect::new(
            icon_ltr.right() + s.px(SPACE_8),
            top_bar.y,
            (badge_ltr.x - s.px(SPACE_8) - (icon_ltr.right() + s.px(SPACE_8))).max(0.0),
            top_h,
        );

        let icon_rect = lang.mirror(icon_ltr, top_bar);
        let badge_rect = lang.mirror(badge_ltr, top_bar);
        let shell_title_rect = lang.mirror(shell_title_ltr, top_bar);

        Icon::Terminal.draw(icon_rect, self.ctx.color(s.accent), self.shapes);
        let detected_title =
            twitty_core::language::fill(lang.t("Detected shell: {}"), &[&shell.name]);
        self.push_label(
            shell_title_rect,
            &detected_title,
            UiSize::Body,
            UiWeight::Semibold,
            self.start_align(),
            s.text,
        );

        let (status_color, status_text) = if shell.marks_seen {
            (s.ok, lang.t("Active · OSC 133"))
        } else if !self.view.config.shell_integration {
            (s.text_faint, lang.t("Disabled"))
        } else if shell.supported {
            (s.warn, lang.t("Waiting for prompt"))
        } else {
            (s.text_dim, lang.t("No marks seen"))
        };
        self.shapes.push(Shape::RoundedRect {
            rect: badge_rect,
            radii: [badge_rect.h * 0.5; 4],
            fill: self.ctx.color(s.bg2),
            border: Some(Border {
                width: s.hairline(),
                color: self.ctx.color(status_color),
            }),
            shadow: None,
        });
        self.push_label(
            badge_rect,
            status_text,
            UiSize::Small,
            UiWeight::Semibold,
            UiAlign::Center,
            status_color,
        );

        let detail_y = top_bar.bottom() + s.px(SPACE_8);
        let detail_rect = Rect::new(
            l.shell_card.x + pad,
            detail_y,
            (l.shell_card.w - 2.0 * pad).max(0.0),
            (l.shell_card.bottom() - pad - detail_y).max(0.0),
        );
        let detail_text = if shell.marks_seen {
            lang.t("OSC 133 prompt marks are active in this session. Command blocks, smart composer, and notifications work automatically.")
        } else if shell.supported {
            lang.t("Twitty automatically injects lightweight hooks for bash, zsh, and fish without editing your startup files.")
        } else {
            lang.t("Automatic hooks cover bash, zsh, and fish; other shells still work if they emit standard OSC 133 marks.")
        };
        self.push_wrapped(
            detail_rect,
            detail_text,
            UiSize::Caption,
            UiWeight::Regular,
            self.start_align(),
            s.text_dim,
            2,
        );

        // Shell integration toggle row.
        let toggle_focused = self.view.state.focused_control == 0;
        let toggle_hover = self.view.state.hover == Some(WelcomeHit::ShellIntegrationToggle);
        self.draw_row_shell(l.shell_toggle_row, toggle_focused, toggle_hover);
        self.draw_two_line_label(
            l.shell_toggle_label,
            lang.t("Automatic shell integration"),
            lang.t("Applies to newly opened shells"),
        );
        self.draw_toggle(l.shell_toggle_track, self.view.config.shell_integration);
    }

    // ---- Step 4: AI & Coding Agents ----

    fn draw_step_ai(&mut self) {
        let s = self.style;
        let l = self.layout;
        let lang = self.lang();
        let info = &self.view.state.agents;

        for (agent, &rect) in info.agents.iter().zip(&l.agent_cards) {
            let fill = if agent.active {
                s.accent_tint(s.bg2)
            } else {
                s.bg0
            };
            let border_color = if agent.active { s.accent } else { s.border };
            self.shapes.push(Shape::RoundedRect {
                rect,
                radii: [s.px(RADIUS_8); 4],
                fill: self.ctx.color(fill),
                border: Some(Border {
                    width: if agent.active {
                        s.px(1.5)
                    } else {
                        s.hairline()
                    },
                    color: self.ctx.color(border_color),
                }),
                shadow: None,
            });

            let pad = s.px(10.0);
            let icon_side = s.px(16.0).min(rect.h);
            let icon_ltr = Rect::new(
                rect.x + pad,
                rect.y + (rect.h - icon_side) * 0.5,
                icon_side,
                icon_side,
            );
            let status_w = s.px(92.0).min(rect.w * 0.44);
            let status_h = s.px(20.0).min(rect.h - s.px(8.0));
            let status_ltr = Rect::new(
                rect.right() - pad - status_w,
                rect.y + (rect.h - status_h) * 0.5,
                status_w,
                status_h,
            );
            let name_ltr = Rect::new(
                icon_ltr.right() + s.px(SPACE_8),
                rect.y,
                (status_ltr.x - s.px(SPACE_8) - (icon_ltr.right() + s.px(SPACE_8))).max(0.0),
                rect.h,
            );

            let icon_rect = lang.mirror(icon_ltr, rect);
            let status_rect = lang.mirror(status_ltr, rect);
            let name_rect = lang.mirror(name_ltr, rect);

            let icon_color = if agent.active {
                s.accent
            } else if agent.installed {
                s.ok
            } else {
                s.text_faint
            };
            Icon::Agent.draw(icon_rect, self.ctx.color(icon_color), self.shapes);
            self.push_label(
                name_rect,
                agent.name,
                UiSize::Body,
                if agent.active || agent.installed {
                    UiWeight::Semibold
                } else {
                    UiWeight::Regular
                },
                self.start_align(),
                if agent.active || agent.installed {
                    s.text
                } else {
                    s.text_dim
                },
            );

            let (badge_bg, badge_fg, badge_text) = if agent.active {
                (s.accent, s.accent_text(), lang.t("Selected"))
            } else if agent.installed {
                (s.bg2, s.ok, lang.t("Installed"))
            } else {
                (s.bg2, s.text_faint, lang.t("Not found"))
            };
            self.shapes.push(Shape::rounded(
                status_rect,
                status_rect.h * 0.5,
                self.ctx.color(badge_bg),
            ));
            self.push_label(
                status_rect,
                badge_text,
                UiSize::Small,
                if agent.active {
                    UiWeight::Semibold
                } else {
                    UiWeight::Regular
                },
                UiAlign::Center,
                badge_fg,
            );
        }

        // Summary of which agent will be used.
        let summary = match &info.selected {
            Some(name) if info.from_config => twitty_core::language::fill(
                lang.t("Configured agent to use: {} (read-only)"),
                &[&name],
            ),
            Some(name) => twitty_core::language::fill(
                lang.t("Agent that will be used: {} (read-only)"),
                &[&name],
            ),
            None => lang
                .t("No coding agent found on PATH yet — install claude, codex, gemini, or opencode anytime.")
                .to_owned(),
        };
        self.push_label(
            l.agent_summary,
            &summary,
            UiSize::Caption,
            UiWeight::Regular,
            self.start_align(),
            if info.selected.is_some() {
                s.text
            } else {
                s.text_dim
            },
        );

        // AI on/off toggle row.
        let toggle_focused = self.view.state.focused_control == 0;
        let toggle_hover = self.view.state.hover == Some(WelcomeHit::AiToggle);
        self.draw_row_shell(l.ai_toggle_row, toggle_focused, toggle_hover);
        self.draw_two_line_label(
            l.ai_toggle_label,
            lang.t("Built-in AI features"),
            lang.t("Natural-language composer (#) and failed-block explanations"),
        );
        self.draw_toggle(l.ai_toggle_track, self.view.config.ai);
    }

    // ---- Step 5: Shortcuts Cheat Sheet ----

    fn draw_step_shortcuts(&mut self) {
        let s = self.style;
        let l = self.layout;
        let lang = self.lang();

        for (entry, &(row_rect, title_rect, chord_rect)) in
            self.view.state.shortcuts.iter().zip(&l.shortcut_rows)
        {
            if row_rect.bottom() > l.body.bottom() + 1.0 {
                break;
            }
            self.shapes.push(Shape::RoundedRect {
                rect: row_rect,
                radii: [s.px(RADIUS_6); 4],
                fill: self.ctx.color(s.bg0),
                border: Some(Border {
                    width: s.hairline(),
                    color: self.ctx.color(s.border),
                }),
                shadow: None,
            });
            self.push_label(
                title_rect,
                entry.title(lang),
                UiSize::Caption,
                UiWeight::Regular,
                self.start_align(),
                s.text,
            );
            let chord = entry.chord.as_deref().unwrap_or("—");
            self.shapes.push(Shape::RoundedRect {
                rect: chord_rect,
                radii: [s.px(RADIUS_6); 4],
                fill: self.ctx.color(s.bg2),
                border: Some(Border {
                    width: s.hairline(),
                    color: self.ctx.color(s.border),
                }),
                shadow: None,
            });
            self.push_label(
                chord_rect,
                chord,
                UiSize::Small,
                UiWeight::Semibold,
                UiAlign::Center,
                s.text_dim,
            );
        }
    }

    // ---- Footer (Skip, Dots, Back, Next/Finish) ----

    fn draw_footer(&mut self) {
        let s = self.style;
        let l = self.layout;
        let lang = self.lang();

        let rule_h = s.hairline();
        self.shapes.push(Shape::rounded(
            Rect::new(l.footer.x, l.footer.y, l.footer.w, rule_h),
            0.0,
            self.ctx.color(s.border),
        ));

        // Skip button.
        let skip_hover = self.view.state.hover == Some(WelcomeHit::Skip);
        if skip_hover {
            self.shapes.push(Shape::rounded(
                l.skip_button,
                s.px(RADIUS_8),
                self.ctx.color(s.bg2),
            ));
        }
        self.push_label(
            l.skip_button,
            lang.t("Skip"),
            UiSize::Caption,
            UiWeight::Regular,
            UiAlign::Center,
            if skip_hover { s.text } else { s.text_dim },
        );

        // Step indicator dots.
        for &(step, hit_rect) in &l.dots {
            let active = step == self.view.state.step;
            let hovered = self.view.state.hover == Some(WelcomeHit::StepDot(step));
            let dot_w = if active { s.px(16.0) } else { s.px(7.0) };
            let dot_h = s.px(7.0);
            let dot_rect = Rect::new(
                hit_rect.x + (hit_rect.w - dot_w) * 0.5,
                hit_rect.y + (hit_rect.h - dot_h) * 0.5,
                dot_w,
                dot_h,
            );
            let color = if active {
                s.accent
            } else if hovered {
                s.text_dim
            } else {
                s.text_faint
            };
            self.shapes
                .push(Shape::rounded(dot_rect, dot_h * 0.5, self.ctx.color(color)));
        }

        // Back button (hidden on Step 1).
        if let Some(back_rect) = l.back_button {
            let back_hover = self.view.state.hover == Some(WelcomeHit::Back);
            self.shapes.push(Shape::RoundedRect {
                rect: back_rect,
                radii: [s.px(RADIUS_8); 4],
                fill: self.ctx.color(if back_hover { s.bg3 } else { s.bg2 }),
                border: Some(Border {
                    width: s.hairline(),
                    color: self.ctx.color(s.border),
                }),
                shadow: None,
            });
            self.push_label(
                back_rect,
                lang.t("Back"),
                UiSize::Caption,
                UiWeight::Regular,
                UiAlign::Center,
                s.text,
            );
        }

        // Primary Next / Finish button.
        let next_hover = self.view.state.hover == Some(WelcomeHit::NextOrFinish);
        let fill = if next_hover {
            s.accent.mix(s.text, 0.1)
        } else {
            s.accent
        };
        self.shapes.push(Shape::rounded(
            l.next_button,
            s.px(RADIUS_8),
            self.ctx.color(fill),
        ));
        let next_label = if self.view.state.step.is_last() {
            lang.t("Finish")
        } else {
            lang.t("Next")
        };
        self.push_label(
            l.next_button,
            next_label,
            UiSize::Caption,
            UiWeight::Semibold,
            UiAlign::Center,
            s.accent_text(),
        );
    }

    // ---- Shared widget helpers ----

    fn draw_row_shell(&mut self, rect: Rect, focused: bool, hovered: bool) {
        let s = self.style;
        self.shapes.push(Shape::RoundedRect {
            rect,
            radii: [s.px(RADIUS_8); 4],
            fill: self
                .ctx
                .color(if focused || hovered { s.bg2 } else { s.bg0 }),
            border: Some(Border {
                width: if focused { s.px(1.5) } else { s.hairline() },
                color: self.ctx.color(if focused { s.accent } else { s.border }),
            }),
            shadow: None,
        });
    }

    fn draw_toggle(&mut self, track: Rect, on: bool) {
        let s = self.style;
        let radius = track.h * 0.5;
        self.shapes.push(Shape::RoundedRect {
            rect: track,
            radii: [radius; 4],
            fill: self.ctx.color(if on { s.accent } else { s.bg3 }),
            border: (!on).then(|| Border {
                width: s.hairline(),
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
            knob_side * 0.5,
            self.ctx
                .color(if on { s.accent_text() } else { s.text_dim }),
        ));
    }

    fn draw_two_line_label(&mut self, rect: Rect, title: &str, note: &str) {
        let s = self.style;
        let top_h = (rect.h * 0.54).round();
        let top = Rect::new(rect.x, rect.y + s.px(2.0), rect.w, top_h);
        let bottom = Rect::new(
            rect.x,
            rect.y + top_h - s.px(2.0),
            rect.w,
            (rect.h - top_h).max(0.0),
        );
        self.push_label(
            top,
            title,
            UiSize::Body,
            UiWeight::Regular,
            self.start_align(),
            s.text,
        );
        self.push_label(
            bottom,
            note,
            UiSize::Small,
            UiWeight::Regular,
            self.start_align(),
            s.text_dim,
        );
    }

    fn push_label(
        &mut self,
        rect: Rect,
        text: &str,
        size: UiSize,
        weight: UiWeight,
        align: UiAlign,
        color: Rgba,
    ) {
        if rect.w <= 1.0 || rect.h <= 1.0 || text.is_empty() {
            return;
        }
        let buf = self.font.fit(
            self.ctx.font_system,
            size,
            weight,
            self.style.scale,
            text,
            rect.w,
        );
        self.labels.push(UiLabel::new(buf, rect, align, color));
    }

    #[allow(clippy::too_many_arguments)]
    fn push_wrapped(
        &mut self,
        rect: Rect,
        text: &str,
        size: UiSize,
        weight: UiWeight,
        align: UiAlign,
        color: Rgba,
        max_lines: usize,
    ) {
        if rect.w <= 1.0 || rect.h <= 1.0 || text.is_empty() {
            return;
        }
        let line_h = (size.px(self.style.scale) * 1.45).ceil().max(1.0);
        let lines = wrap_lines(
            self.font.font(),
            self.ctx.font_system,
            size,
            weight,
            self.style.scale,
            text,
            rect.w,
        );
        for (i, line) in lines.iter().take(max_lines).enumerate() {
            let y = rect.y + i as f32 * line_h;
            if y >= rect.bottom() {
                break;
            }
            let r = Rect::new(rect.x, y, rect.w, line_h.min(rect.bottom() - y));
            self.push_label(r, line, size, weight, align, color);
        }
    }
}
