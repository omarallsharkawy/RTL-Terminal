//! What a frame is drawn with: fonts, colors and cell sizes, borrowed from the renderer.

use cosmic_text::{Buffer, FontSystem};

use crate::metrics::CellMetrics;
use crate::row::{visual_cell_extents, CachedRow, RowCursor};
use crate::scene::row_quads::quads_with_extents;
use crate::style::target_color;
use crate::text::shaper::{shape_label, RowShaper};
use twitty_core::color::{Palette, Rgba};
use twitty_core::terminal::LineData;

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

    #[doc(hidden)]
    pub fn build_row(
        &mut self,
        hash: u64,
        line: &LineData,
        cursor: Option<RowCursor>,
    ) -> CachedRow {
        let segments = self.shaper().build_row(line, cursor);
        let m = &self.metrics;
        let visual = visual_cell_extents(line, &segments, m.char_width);
        let quads = quads_with_extents(
            self.palette,
            self.is_srgb,
            (m.char_width, m.line_height, m.font_px()),
            line,
            &visual,
        );
        CachedRow::with_extents(hash, line, segments, quads, visual)
    }
}
