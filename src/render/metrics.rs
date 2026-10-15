//! Cell and window geometry in physical pixels. Pure arithmetic (plus one font measurement), so
//! it can be tested without a window or GPU.

use alacritty_terminal::index::Side;
use cosmic_text::FontSystem;

use super::text::shaper::measure_char_width;
use crate::layout::{tab_rects, Rect};

/// Line height as a multiple of the font size (tall enough for Arabic harakat).
const LINE_HEIGHT_FACTOR: f32 = 1.55;
/// Widest a tab in the tab bar gets, in cells.
const MAX_TAB_CELLS: f32 = 28.0;
/// Logical (DPI-independent) window padding.
const PADDING_LEFT: f32 = 8.0;
const PADDING_TOP: f32 = 6.0;

/// Size of one terminal cell and the padding around a pane's grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    /// Font size in logical points (what the user zooms and what is saved).
    pub font_size: f32,
    /// Window scale factor (1.0 = 96 DPI).
    pub scale: f32,
    pub char_width: f32,
    pub line_height: f32,
    pub padding_left: f32,
    pub padding_top: f32,
}

impl CellMetrics {
    /// Metrics of `family` at `font_size` points (clamped to 8–48) on a `scale` display.
    pub fn measure(
        font_system: &mut FontSystem,
        family: Option<&'static str>,
        font_size: f32,
        scale: f32,
    ) -> Self {
        let font_size = font_size.clamp(8.0, 48.0);
        let font_px = font_size * scale;
        let line_height = (font_px * LINE_HEIGHT_FACTOR).round();
        let char_width = measure_char_width(font_system, family, font_px, line_height);
        CellMetrics {
            font_size,
            scale,
            char_width,
            line_height,
            padding_left: (PADDING_LEFT * scale).round(),
            padding_top: (PADDING_TOP * scale).round(),
        }
    }

    /// Font size in physical pixels.
    pub fn font_px(&self) -> f32 {
        self.font_size * self.scale
    }

    /// Width of thin cursor strokes (beam, hollow outline, preedit caret and underline).
    pub fn stroke(&self) -> f32 {
        (self.scale * 1.5).round().max(1.0)
    }

    /// Width of the divider between panes.
    pub fn divider_gap(&self) -> f32 {
        self.scale.round().max(1.0)
    }

    /// Height of the tab bar (shown only with two or more tabs).
    pub fn tab_bar_height(&self, tab_count: usize) -> f32 {
        if tab_count >= 2 {
            self.line_height
        } else {
            0.0
        }
    }

    /// Where each tab goes in a window `width` wide; empty with fewer than two tabs (no tab
    /// bar).
    pub fn tab_rects(&self, tab_count: usize, width: f32) -> Vec<Rect> {
        if tab_count >= 2 {
            let max_tab = MAX_TAB_CELLS * self.char_width;
            tab_rects(tab_count, width, self.line_height, max_tab)
        } else {
            Vec::new()
        }
    }

    /// The area of a `width` x `height` window below the tab bar, where panes are laid out.
    pub fn content_rect(&self, tab_count: usize, (width, height): (f32, f32)) -> Rect {
        let bar = self.tab_bar_height(tab_count);
        Rect::new(0.0, bar, width, (height - bar).max(1.0))
    }

    /// Grid size (cols, rows) of a pane occupying `rect`.
    pub fn grid_size(&self, rect: Rect) -> (usize, usize) {
        // The grid must fit inside the pane, or a shell in a small split draws past its edge.
        // Two columns is the floor: alacritty writes out of bounds for a wide character in one.
        let avail_w = (rect.w - self.padding_left * 2.0).max(0.0);
        let avail_h = (rect.h - self.padding_top * 2.0).max(0.0);
        let cols = (avail_w / self.char_width).floor() as usize;
        let rows = (avail_h / self.line_height).floor() as usize;
        (cols.max(2), rows.max(1))
    }

    /// Map a physical pixel position to a (col, row) cell of the pane in `rect`, clamped to
    /// its grid.
    pub fn pixel_to_cell(&self, rect: Rect, x: f64, y: f64) -> (usize, usize) {
        let (cols, rows) = self.grid_size(rect);
        let col = ((x - (rect.x + self.padding_left) as f64) / self.char_width as f64).max(0.0);
        let row = ((y - (rect.y + self.padding_top) as f64) / self.line_height as f64).max(0.0);
        ((col as usize).min(cols - 1), (row as usize).min(rows - 1))
    }

    /// Which half of its cell the pixel falls in (selections include a cell only when the
    /// pointer passes its middle).
    pub fn pixel_side(&self, rect: Rect, x: f64) -> Side {
        let rel = (x - (rect.x + self.padding_left) as f64) / self.char_width as f64;
        if rel.rem_euclid(1.0) < 0.5 {
            Side::Left
        } else {
            Side::Right
        }
    }
}
