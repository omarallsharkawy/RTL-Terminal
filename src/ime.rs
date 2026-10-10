//! Input-method (IME) support: pure helpers plus the renderer's inline preedit drawing.
//!
//! Avoiding double input (winit 0.30): a key the IME consumes is never delivered as
//! `KeyboardInput` (Wayland text-input grabs it, X11 drops it via `XFilterEvent`, Windows
//! turns it into `WM_IME_*`). Composed text arrives only as `Ime::Commit`, and plain keys
//! that the IME ignores arrive only as `KeyboardInput`. So the two paths never carry the same
//! character and no text de-duplication is needed. As a safety net, keyboard input is dropped
//! while a preedit is on screen, in case a platform leaks the key that drives composition.

use cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping};
use glyphon::{TextArea, TextBounds};

use crate::renderer::Renderer;

/// Whether a `KeyboardInput` event may reach the shell given the IME state.
pub fn should_forward_key(preedit_active: bool) -> bool {
    !preedit_active
}

/// Normalise winit's preedit: an empty string clears it, and the cursor range is clamped to
/// char boundaries inside the text.
pub fn normalize_preedit(
    text: String,
    cursor: Option<(usize, usize)>,
) -> Option<(String, Option<(usize, usize)>)> {
    if text.is_empty() {
        return None;
    }
    let fix = |mut i: usize| {
        i = i.min(text.len());
        while !text.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let cursor = cursor.map(|(a, b)| (fix(a), fix(b)));
    Some((text, cursor))
}

/// Left edge of the preedit box. The caret is where the next character would appear: after
/// Latin text the preedit grows to the right of it, after Arabic it grows to the left.
pub fn preedit_left(caret_x: f32, extends_left: bool, width: f32) -> f32 {
    let left = if extends_left {
        caret_x - width
    } else {
        caret_x
    };
    left.max(0.0)
}

/// Pixel offset (from the preedit's left edge) of the byte offset `byte` in shaped text.
fn cursor_offset(buffer: &Buffer, byte: usize, width: f32) -> f32 {
    for run in buffer.layout_runs() {
        for g in run.glyphs.iter() {
            if byte >= g.start && byte < g.end {
                return if g.level.is_rtl() { g.x + g.w } else { g.x };
            }
        }
    }
    // Past the last glyph: the end of the text (left edge if it is all right-to-left).
    let rtl_end = buffer
        .layout_runs()
        .last()
        .and_then(|r| r.glyphs.last())
        .is_some_and(|g| g.level.is_rtl());
    if rtl_end {
        0.0
    } else {
        width
    }
}

#[derive(Default)]
pub struct PreeditState {
    pub text: String,
    pub cursor: Option<(usize, usize)>,
    buffer: Option<Buffer>,
    /// Last rectangle reported with `set_ime_cursor_area`.
    last_area: Option<(i32, i32, u32, u32)>,
}

impl Renderer {
    /// Set (or clear with `None`) the IME composition drawn inline at the cursor.
    pub fn set_preedit(&mut self, preedit: Option<(String, Option<(usize, usize)>)>) {
        match preedit {
            Some((text, cursor)) => {
                self.ime.text = text;
                self.ime.cursor = cursor;
            }
            None => {
                self.ime.text.clear();
                self.ime.cursor = None;
                self.ime.buffer = None;
            }
        }
    }

    pub fn has_preedit(&self) -> bool {
        !self.ime.text.is_empty()
    }

    /// Shape the preedit at `caret` (x, y, extends_left; x includes the padding), push its
    /// background, underline and cursor, and tell the IME where to put its candidate popup.
    /// Returns the text origin when a preedit is visible.
    pub(crate) fn prepare_preedit(
        &mut self,
        caret: Option<(f32, f32, bool)>,
        quads: &mut Vec<(f32, f32, f32, f32, [f32; 4])>,
        cursor_quads: &mut Vec<(f32, f32, f32, f32, [f32; 4])>,
    ) -> Option<(f32, f32)> {
        let (caret_x, y, extends_left) = caret?;
        let (cw, lh) = (self.char_width, self.line_height);

        let mut area = (caret_x, cw);
        let mut origin = None;
        if self.has_preedit() {
            let mut buffer = Buffer::new_empty(Metrics::new(self.font_px(), lh));
            let attrs = Attrs::new().family(Family::Monospace);
            let rtl = !self.ime.text.is_ascii();
            let shaped = if rtl {
                format!("\u{200E}{}", self.ime.text)
            } else {
                self.ime.text.clone()
            };
            buffer.set_text(&shaped, &attrs, Shaping::Advanced, None);
            buffer.shape_until_scroll(&mut self.font_system, false);
            let width = buffer
                .layout_runs()
                .map(|r| r.line_w)
                .fold(0.0_f32, f32::max)
                .max(cw);
            let left = preedit_left(caret_x, extends_left, width);
            let prefix = if rtl { "\u{200E}".len() } else { 0 };
            let cursor_byte = self.ime.cursor.map_or(self.ime.text.len(), |c| c.0) + prefix;
            let cx = left + cursor_offset(&buffer, cursor_byte, width);

            let t = (self.scale * 1.5).round().max(1.0);
            let fg = self.to_target_color(self.palette.foreground);
            quads.push((
                left,
                y,
                width,
                lh,
                self.to_target_color(self.palette.background),
            ));
            quads.push((left, y + lh - t, width, t, fg));
            cursor_quads.clear();
            cursor_quads.push((
                cx.clamp(left, left + width - t),
                y,
                t,
                lh,
                self.to_target_color(self.palette.cursor),
            ));
            self.ime.buffer = Some(buffer);
            area = (left, width);
            origin = Some((left, y));
        }

        let rect = (
            area.0.round() as i32,
            y.round() as i32,
            area.1.ceil() as u32,
            lh.ceil() as u32,
        );
        if self.ime.last_area != Some(rect) {
            self.ime.last_area = Some(rect);
            self.window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(rect.0, rect.1),
                winit::dpi::PhysicalSize::new(rect.2, rect.3),
            );
        }
        origin
    }
}

impl PreeditState {
    /// Text area for the shaped preedit; borrows only this state so it can sit beside other areas.
    pub(crate) fn text_area(
        &self,
        origin: (f32, f32),
        bounds: TextBounds,
        color: glyphon::Color,
    ) -> Option<TextArea<'_>> {
        Some(TextArea {
            buffer: self.buffer.as_ref()?,
            left: origin.0,
            top: origin.1,
            scale: 1.0,
            bounds,
            default_color: color,
            custom_glyphs: &[],
        })
    }
}
