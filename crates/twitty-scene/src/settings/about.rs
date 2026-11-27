//! Drawing the About section.

use super::*;

impl SettingsDrawer<'_, '_> {
    pub(super) fn draw_about(&mut self) {
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
                self.lang().t("Native Bidirectional RTL Terminal"),
                UiSize::Caption,
                UiWeight::Regular,
                self.start_align(),
                s.text_dim,
                Some(clip),
            );
        }

        let config_path_display = self.view.state.config_path.as_deref().unwrap_or("—");

        let items = [
            (self.lang().t("Version"), env!("CARGO_PKG_VERSION"), false),
            (self.lang().t("License"), "AGPL-3.0-or-later", false),
            (self.lang().t("Config file"), config_path_display, true),
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
                self.lang().t("Copied")
            } else {
                self.lang().t("Copy path")
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
}
