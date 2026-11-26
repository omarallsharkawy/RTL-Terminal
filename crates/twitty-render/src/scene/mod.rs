//! CPU scene primitives shared by the app and the rendering pipelines.

use cosmic_text::Buffer;
use glyphon::TextBounds;

pub use twitty_core::boxdraw;
pub mod context;
pub mod cursor;
pub mod preedit;
pub mod row_quads;

/// A shaped buffer and where to draw it.
pub struct TextRun<'a> {
    pub buffer: &'a Buffer,
    pub left: f32,
    pub top: f32,
    pub bounds: TextBounds,
    pub color: glyphon::Color,
}
