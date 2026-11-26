//! Shared theme and DPI tokens for Twitty's chrome. Geometry and font sizes are in logical
//! pixels until converted with `UiStyle::px`.

use crate::color::{Palette, Rgba};

pub const SPACE_4: f32 = 4.0;
pub const SPACE_8: f32 = 8.0;
pub const SPACE_12: f32 = 12.0;
pub const SPACE_16: f32 = 16.0;
pub const SPACE_24: f32 = 24.0;
pub const RADIUS_6: f32 = 6.0;
pub const RADIUS_8: f32 = 8.0;
pub const RADIUS_12: f32 = 12.0;
pub const HAIRLINE: f32 = 1.0;
pub const OVERLAY_BLUR: f32 = 16.0;
pub const OVERLAY_OFFSET: [f32; 2] = [0.0, 4.0];
pub const OVERLAY_SHADOW: Rgba = Rgba::new(0.0, 0.0, 0.0, 0.30);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiStyle {
    pub bg0: Rgba,
    pub bg1: Rgba,
    pub bg2: Rgba,
    pub bg3: Rgba,
    pub border: Rgba,
    pub accent: Rgba,
    pub text: Rgba,
    pub text_dim: Rgba,
    pub text_faint: Rgba,
    pub danger: Rgba,
    pub warn: Rgba,
    pub ok: Rgba,
    pub scale: f32,
}

impl UiStyle {
    pub fn new(theme: &Palette, accent: Option<Rgba>, scale: f32) -> Self {
        let bg0 = theme.background;
        let text = theme.foreground;
        let tone = |amount| bg0.mix(text, amount);
        Self {
            bg0,
            bg1: tone(0.035),
            bg2: tone(0.068),
            bg3: tone(0.12),
            border: tone(0.14),
            accent: accent.unwrap_or(theme.cursor),
            text,
            text_dim: tone(0.69),
            text_faint: tone(0.48),
            danger: theme.ansi[1],
            warn: theme.ansi[3],
            ok: theme.ansi[2],
            scale,
        }
    }

    pub fn px(self, logical: f32) -> f32 {
        logical * self.scale
    }

    /// Snap rules to whole physical pixels so neighboring surfaces share a crisp edge.
    pub fn hairline(self) -> f32 {
        self.px(HAIRLINE).round().max(1.0)
    }

    /// A readable foreground on an accent supplied by either the theme or the user.
    pub fn accent_text(self) -> Rgba {
        let [r, g, b, _] = self.accent.to_linear();
        let luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let value = if luminance > 0.179 { 0.0 } else { 1.0 };
        Rgba::new(value, value, value, 1.0)
    }

    pub fn accent_tint(self, surface: Rgba) -> Rgba {
        surface.mix(self.accent, 0.22)
    }
}
