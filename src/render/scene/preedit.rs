//! The input-method composition (preedit) drawn inline at the cursor, and where the IME's
//! candidate popup goes.

use cosmic_text::Buffer;
use glyphon::TextBounds;

use super::context::DrawContext;
use crate::layout::Rect;
use crate::render::style::Quad;
use crate::render::text::bidi::LRM;

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

/// The parts of `bounds` left and right of the horizontal span `left..right`, so text drawn
/// under the preedit box is clipped away instead of showing through it.
pub fn bounds_outside(bounds: TextBounds, left: f32, right: f32) -> [TextBounds; 2] {
    [
        TextBounds {
            right: bounds.right.min(left.floor() as i32),
            ..bounds
        },
        TextBounds {
            left: bounds.left.max(right.ceil() as i32),
            ..bounds
        },
    ]
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

/// The composition being typed and its shaped text.
#[derive(Default)]
pub struct PreeditState {
    pub text: String,
    pub cursor: Option<(usize, usize)>,
    buffer: Option<Buffer>,
    /// Last rectangle reported with `set_ime_cursor_area`.
    last_area: Option<ImeArea>,
}

/// Where the IME should put its candidate popup: x, y, width, height in physical pixels.
pub(crate) type ImeArea = (i32, i32, u32, u32);

impl PreeditState {
    /// Set (or clear with `None`) the composition.
    pub fn set(&mut self, preedit: Option<(String, Option<(usize, usize)>)>) {
        match preedit {
            Some((text, cursor)) => {
                self.text = text;
                self.cursor = cursor;
            }
            None => {
                self.text.clear();
                self.cursor = None;
                self.buffer = None;
            }
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        !self.text.is_empty()
    }

    /// Shape the preedit at `caret` (x, y, extends_left; x includes the padding) and push its
    /// background, underline and caret (the caret replaces `cursor_quads`). Returns the preedit
    /// box (left, y, width) when a preedit is visible, and the candidate popup area when it
    /// moved since it was last reported.
    pub fn layout(
        &mut self,
        ctx: &mut DrawContext,
        caret: Option<(f32, f32, bool, Rect)>,
        quads: &mut Vec<Quad>,
        cursor_quads: &mut Vec<Quad>,
    ) -> (Option<(f32, f32, f32)>, Option<ImeArea>) {
        let Some((caret_x, y, extends_left, pane)) = caret else {
            self.last_area = None;
            return (None, None);
        };
        let pane_w = pane.w.max(0.0);
        let pane_h = pane.h.max(0.0);
        let cw = ctx.metrics.char_width.max(0.0).min(pane_w);
        let lh = ctx.metrics.line_height.max(0.0).min(pane_h);
        let y = safe_clamp(y, pane.y, pane.bottom() - lh);

        let mut area = (safe_clamp(caret_x, pane.x, pane.right() - cw), cw);
        let mut origin = None;
        if self.is_active() {
            let rtl = !self.text.is_ascii();
            let shaped = if rtl {
                format!("{LRM}{}", self.text)
            } else {
                self.text.clone()
            };
            let buffer = ctx.label(&shaped);
            let width = buffer
                .layout_runs()
                .map(|r| r.line_w)
                .fold(0.0_f32, f32::max)
                .max(cw);
            let visible_width = width.min(pane_w);
            let left = safe_clamp(
                preedit_left(caret_x, extends_left, width),
                pane.x,
                pane.right() - visible_width,
            );
            let prefix = if rtl { LRM.len() } else { 0 };
            let cursor_byte = self.cursor.map_or(self.text.len(), |c| c.0) + prefix;
            let cx = left + cursor_offset(&buffer, cursor_byte, width);

            let width = visible_width;
            let t = ctx.metrics.stroke().max(0.0).min(width).min(lh);
            let fg = ctx.color(ctx.palette.foreground);
            quads.push((left, y, width, lh, ctx.color(ctx.palette.background)));
            quads.push((left, y + lh - t, width, t, fg));
            cursor_quads.clear();
            cursor_quads.push((
                safe_clamp(cx, left, left + width - t),
                y,
                t,
                lh,
                ctx.color(ctx.palette.cursor),
            ));
            self.buffer = Some(buffer);
            area = (left, width);
            origin = Some((left, y, width));
        }

        let rect = (
            area.0.round() as i32,
            y.round() as i32,
            ((area.0 + area.1).round() - area.0.round()).max(0.0) as u32,
            ((y + lh).round() - y.round()).max(0.0) as u32,
        );
        let moved = self.last_area != Some(rect);
        self.last_area = Some(rect);
        (origin, moved.then_some(rect))
    }

    /// The shaped preedit, once laid out.
    pub fn buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }
}

/// Clamp `val` into `[min, max]` without panicking when `min > max` or inputs are non-finite.
fn safe_clamp(val: f32, min: f32, max: f32) -> f32 {
    if !val.is_finite() || !max.is_finite() || max <= min {
        min
    } else {
        val.clamp(min, max)
    }
}
