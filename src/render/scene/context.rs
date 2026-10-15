//! What a frame is drawn with: fonts, colors and cell sizes, borrowed from the renderer.

use cosmic_text::{Buffer, FontSystem};

use crate::color::{Palette, Rgba};
use crate::render::metrics::CellMetrics;
use crate::render::scene::row_quads::build_row_quads;
use crate::render::style::{target_color, Quad};
use crate::render::text::segment::CachedSegment;
use crate::render::text::shaper::{shape_label, RowShaper};
use crate::terminal::LineData;

/// Borrowed per frame, so CPU-side drawing never needs the GPU half of the renderer.
pub struct DrawContext<'a> {
    pub font_system: &'a mut FontSystem,
    /// Configured font family; `None` is the generic monospace.
    pub family: Option<&'static str>,
    pub palette: &'a Palette,
    pub metrics: CellMetrics,
    /// Whether the surface is sRGB (colors are then linearized).
    pub is_srgb: bool,
}

impl DrawContext<'_> {
    /// `rgba` as the surface expects it.
    pub fn color(&self, rgba: Rgba) -> [f32; 4] {
        target_color(rgba, self.is_srgb)
    }

    /// A one-line label shaped in the grid font.
    pub fn label(&mut self, text: &str) -> Buffer {
        let size = (self.metrics.font_px(), self.metrics.line_height);
        shape_label(self.font_system, self.family, size, text)
    }

    pub fn shaper(&mut self) -> RowShaper<'_> {
        RowShaper {
            font_px: self.metrics.font_px(),
            font_system: self.font_system,
            palette: self.palette,
            char_width: self.metrics.char_width,
            line_height: self.metrics.line_height,
            family: self.family,
        }
    }

    pub fn row_quads(&self, line: &LineData, segments: &[CachedSegment]) -> Vec<Quad> {
        let m = &self.metrics;
        build_row_quads(
            self.palette,
            self.is_srgb,
            (m.char_width, m.line_height, m.font_px()),
            line,
            segments,
        )
    }
}
