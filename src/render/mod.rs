pub mod gpu;
pub mod metrics;
pub mod row;
pub mod scene;
pub mod style;
pub mod text;

pub use gpu::text_pipeline::text_color_mode;
pub use metrics::CellMetrics;
pub use row::cache::RowCache;
pub use row::{visual_cell_extents, CachedRow, RowCursor};
pub use scene::boxdraw::try_render_box_or_block;
pub use scene::cursor::{cursor_cell_width, geometry_under_block_cursor};
pub use scene::row_quads::build_row_quads;
pub use style::{cell_colors, target_color, Quad};

pub use text::segment::CachedSegment;
pub use text::shaper::{measure_char_width, shape_label, RowShaper};

use alacritty_terminal::index::Side;
use cosmic_text::FontSystem;
use std::sync::Arc;
use winit::window::Window;

use crate::color::Palette;
use crate::layout::{PaneId, Rect};
use crate::terminal::{CursorState, LineData};
use gpu::Gpu;
use scene::context::DrawContext;
use scene::frame::{build_frame, FrameOptions};
use text::fonts::{installed_family, new_font_system};

/// Everything the renderer needs to draw one pane.
pub struct PaneFrame {
    pub id: PaneId,
    /// Where the pane goes in the window (physical pixels, padding included).
    pub rect: Rect,
    pub lines: Vec<LineData>,
    pub cursor: CursorState,
    /// The pane keyboard input goes to; the others get a hollow cursor.
    pub focused: bool,
    /// One-line bar drawn over the pane's last row (search query), if any.
    pub status: Option<String>,
}

/// A tab in the tab bar.
pub struct TabLabel {
    pub title: String,
    pub active: bool,
}

pub struct Renderer {
    pub window: Arc<Window>,
    /// Surface size and format.
    pub config: wgpu::SurfaceConfiguration,
    gpu: Gpu,
    pub font_system: FontSystem,
    /// Configured terminal font family (leaked once per change; `None` = monospace).
    pub font_family: Option<&'static str>,
    pub palette: Palette,
    /// Font size in logical points (what the user zooms and what is saved).
    pub font_size: f32,
    /// Window scale factor (1.0 = 96 DPI).
    pub scale: f32,
    /// Cell metrics in physical pixels.
    pub char_width: f32,
    pub line_height: f32,
    pub padding_left: f32,
    pub padding_top: f32,
    pub is_srgb: bool,
    pub focused: bool,
    /// Window background opacity (1.0 = solid).
    pub opacity: f32,
    /// Configured cursor style ("block", "beam"/"bar", "underline").
    pub cursor_style: String,
    /// Blink phase: while false the cursor is not drawn (and its block glyph is not recolored).
    pub cursor_blink_on: bool,
    /// Shaped rows of every pane, so a pane that did not change is not reshaped.
    pub row_cache: RowCache,
    /// Input-method composition drawn at the cursor.
    pub ime: crate::ime::PreeditState,
}

impl Renderer {
    pub async fn new(window: Arc<Window>, initial_font_size: f32) -> anyhow::Result<Self> {
        let (gpu, config) = Gpu::new(window.clone()).await?;
        let is_srgb = config.format.is_srgb();

        let scale = window.scale_factor() as f32;
        let mut renderer = Self {
            window,
            config,
            gpu,
            font_system: new_font_system(),
            font_family: None,
            palette: Palette::default(),
            font_size: initial_font_size,
            scale,
            char_width: 0.0,
            line_height: 0.0,
            padding_left: 0.0,
            padding_top: 0.0,
            is_srgb,
            focused: true,
            opacity: 1.0,
            cursor_style: "block".into(),
            cursor_blink_on: true,
            row_cache: RowCache::default(),
            ime: Default::default(),
        };
        renderer.update_metrics();
        Ok(renderer)
    }

    /// The current cell metrics.
    pub fn metrics(&self) -> CellMetrics {
        CellMetrics {
            font_size: self.font_size,
            scale: self.scale,
            char_width: self.char_width,
            line_height: self.line_height,
            padding_left: self.padding_left,
            padding_top: self.padding_top,
        }
    }

    /// Recompute cell metrics from the font size and scale factor.
    fn update_metrics(&mut self) {
        let m = CellMetrics::measure(
            &mut self.font_system,
            self.font_family,
            self.font_size,
            self.scale,
        );
        self.font_size = m.font_size;
        self.char_width = m.char_width;
        self.line_height = m.line_height;
        self.padding_left = m.padding_left;
        self.padding_top = m.padding_top;
        self.row_cache.clear();
    }

    /// Use `name` for the terminal grid. An uninstalled family falls back to monospace.
    pub fn set_font_family(&mut self, name: Option<&str>) {
        let name = name.map(str::trim).filter(|n| !n.is_empty());
        self.font_family = match name {
            Some(n) => match installed_family(&self.font_system, n) {
                Some(installed) => Some(Box::leak(installed.into_boxed_str())),
                None => {
                    eprintln!("twitty: font_family {n:?} is not installed, using monospace");
                    None
                }
            },
            None => None,
        };
        self.update_metrics();
    }

    pub fn set_font_size(&mut self, new_size: f32) {
        self.font_size = new_size;
        self.update_metrics();
    }

    pub fn set_scale_factor(&mut self, scale: f32) {
        self.scale = scale.max(0.25);
        self.update_metrics();
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Forget the cached rows of a closed pane.
    pub fn forget_pane(&mut self, id: PaneId) {
        self.row_cache.forget(id);
    }

    /// Set (or clear with `None`) the IME composition drawn inline at the cursor.
    pub fn set_preedit(&mut self, preedit: Option<(String, Option<(usize, usize)>)>) {
        self.ime.set(preedit);
    }

    pub fn has_preedit(&self) -> bool {
        self.ime.is_active()
    }

    pub fn set_opacity(&mut self, opacity: f32) {
        self.opacity = opacity.clamp(0.2, 1.0);
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            self.gpu.resize(&self.config);
            self.row_cache.clear();
        }
    }

    /// Width of the divider between panes.
    pub fn divider_gap(&self) -> f32 {
        self.metrics().divider_gap()
    }

    /// Height of the tab bar (shown only with two or more tabs).
    pub fn tab_bar_height(&self, tab_count: usize) -> f32 {
        self.metrics().tab_bar_height(tab_count)
    }

    /// The window area below the tab bar, where panes are laid out.
    pub fn content_rect(&self, tab_count: usize) -> Rect {
        let size = (self.config.width as f32, self.config.height as f32);
        self.metrics().content_rect(tab_count, size)
    }

    /// Grid size (cols, rows) of a pane occupying `rect`.
    pub fn compute_grid_size(&self, rect: Rect) -> (usize, usize) {
        self.metrics().grid_size(rect)
    }

    /// Map a physical pixel position to a (col, row) cell of the pane in `rect`, clamped to
    /// its grid.
    pub fn pixel_to_cell(&self, rect: Rect, x: f64, y: f64) -> (usize, usize) {
        self.metrics().pixel_to_cell(rect, x, y)
    }

    /// Map a pixel to the cell drawn there and its side. Unlike `pixel_to_cell`, this follows
    /// reordered Arabic glyphs, so selections and mouse reports match what is on screen.
    pub fn pixel_to_visual_cell(
        &self,
        pane: PaneId,
        rect: Rect,
        x: f64,
        y: f64,
    ) -> (usize, usize, Side) {
        let (col, row) = self.pixel_to_cell(rect, x, y);
        let (cols, _) = self.compute_grid_size(rect);
        let rel_x = (x - (rect.x + self.padding_left) as f64) as f32;
        match self.row_cache.row(pane, row) {
            Some(cached) => {
                let (col, side) = cached.hit_test(rel_x, self.char_width, cols);
                (col, row, side)
            }
            None => (col, row, self.pixel_side(rect, x)),
        }
    }

    /// Which half of its cell the pixel falls in (selections include a cell only when the
    /// pointer passes its middle).
    pub fn pixel_side(&self, rect: Rect, x: f64) -> Side {
        self.metrics().pixel_side(rect, x)
    }

    /// Draw the tab bar (with two or more tabs), every pane and the dividers between them.
    pub fn render(
        &mut self,
        tabs: &[TabLabel],
        panes: &[PaneFrame],
        dividers: &[Rect],
    ) -> anyhow::Result<()> {
        let Some(surface_texture) = self.gpu.acquire(&self.config, &self.window) else {
            return Ok(());
        };

        let opts = FrameOptions {
            window_focused: self.focused,
            cursor_style: &self.cursor_style,
            cursor_blink_on: self.cursor_blink_on,
            surface: (self.config.width as f32, self.config.height as f32),
        };
        let mut ctx = DrawContext {
            metrics: self.metrics(),
            font_system: &mut self.font_system,
            family: self.font_family,
            palette: &self.palette,
            is_srgb: self.is_srgb,
        };
        let frame = build_frame(
            &mut ctx,
            &opts,
            &mut self.row_cache,
            &mut self.ime,
            tabs,
            panes,
            dividers,
        );
        if let Some((x, y, w, h)) = frame.ime_area {
            self.window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(x, y),
                winit::dpi::PhysicalSize::new(w, h),
            );
        }
        let runs = frame.text_runs(panes, &self.row_cache, &self.ime);
        self.gpu.draw(
            surface_texture,
            &frame.quads,
            runs,
            &mut self.font_system,
            frame.background,
            self.opacity,
        )
    }
}
