//! Rendering, split by what each part needs:
//!
//! - `text`: fonts, bidi helpers and row shaping (cosmic-text only).
//! - `row`: a shaped row, its hit testing and caret placement, and the per-pane row cache.
//! - `metrics`: cell size and pane geometry.
//! - `style`, `scene`: the frame laid out on the CPU (quads and text runs), testable headless.
//! - `gpu`: wgpu surface, quad pipeline and glyphon text pipeline.
//!
//! `Renderer` ties them together for the app.

pub mod gpu;
pub mod icons;
pub mod metrics;
pub mod row;
pub mod scene;
pub mod shapes;
pub mod style;
pub mod text;
pub mod ui_style;

pub use gpu::text_pipeline::text_color_mode;
pub use metrics::CellMetrics;
pub use row::cache::RowCache;
pub use row::{visual_cell_extents, CachedRow, RowCursor};
pub use scene::boxdraw::try_render_box_or_block;
pub use scene::cursor::{cursor_cell_width, geometry_under_block_cursor};
pub use scene::row_quads::build_row_quads;
pub use style::{cell_colors, Quad};

pub use text::segment::CachedSegment;
pub use text::shaper::{measure_char_width, RowShaper};

use alacritty_terminal::index::Side;
use cosmic_text::FontSystem;
use std::sync::Arc;
use winit::window::Window;

use crate::blocks::cards::BlockHover;
use crate::color::{Palette, Rgba};
use crate::layout::{PaneId, Rect};
use crate::terminal::{CursorState, LineData};
use gpu::Gpu;
use scene::context::DrawContext;
use scene::frame::{build_frame, build_frame_with_cards, FrameOptions};
use scene::palette::{PaletteLayout, PaletteView};
use scene::preedit::PreeditState;
use scene::sidebar::LabelCache;
use scene::window_chrome::ChromeView;
use text::fonts::{installed_family, new_font_system};
use text::ui::UiFont;
use ui_style::UiStyle;

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
    /// Command blocks on screen (`BlockList::decor`); empty for most panes.
    pub blocks: Vec<crate::blocks::BlockDecor>,
}

/// A tab in the tab bar.
pub struct TabLabel {
    pub title: String,
    pub active: bool,
}

/// The terminal renderer: lays out each frame on the CPU (`scene`) and draws it on the GPU
/// (`gpu`). Its state is reached through methods, so the app never depends on its layout.
pub(crate) struct Renderer {
    window: Arc<Window>,
    /// Surface size and format.
    config: wgpu::SurfaceConfiguration,
    gpu: Gpu,
    font_system: FontSystem,
    /// Configured terminal font family (leaked once per change; `None` = monospace).
    font_family: Option<&'static str>,
    palette: Palette,
    ui_accent: Option<Rgba>,
    ui_font: UiFont,
    metrics: CellMetrics,
    is_srgb: bool,
    /// Whether the window has keyboard focus.
    focused: bool,
    /// Window background opacity (1.0 = solid).
    opacity: f32,
    /// Configured cursor style ("block", "beam"/"bar", "underline").
    cursor_style: String,
    /// Blink phase: while false the cursor is not drawn (and its block glyph is not recolored).
    cursor_blink_on: bool,
    /// Shaped rows of every pane, so a pane that did not change is not reshaped.
    row_cache: RowCache,
    sidebar_labels: LabelCache,
    /// Input-method composition drawn at the cursor.
    ime: PreeditState,
    /// The command palette, while it is open.
    palette_view: Option<PaletteView>,
    /// The docked command composer, while it is visible.
    composer_view: Option<scene::composer::ComposerView>,
    /// The code review panel, while it is open.
    review_view: Option<crate::review::panel::View>,
    review_labels: scene::review::ReviewCache,
    /// Render OSC 133 command blocks as rounded cards with a hover toolbar.
    block_cards: bool,
    /// Block card (and optional toolbar button) currently hovered by the pointer.
    block_hover: Option<BlockHover>,
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
            ui_accent: None,
            ui_font: UiFont::default(),
            metrics: CellMetrics {
                font_size: initial_font_size,
                scale,
                char_width: 0.0,
                line_height: 0.0,
                padding_left: 0.0,
                padding_top: 0.0,
            },
            is_srgb,
            focused: true,
            opacity: 1.0,
            cursor_style: "block".into(),
            cursor_blink_on: true,
            row_cache: RowCache::default(),
            sidebar_labels: LabelCache::default(),
            ime: Default::default(),
            palette_view: None,
            composer_view: None,
            review_view: None,
            review_labels: Default::default(),
            block_cards: true,
            block_hover: None,
        };
        renderer.update_metrics();
        Ok(renderer)
    }

    /// The current cell metrics.
    pub fn metrics(&self) -> CellMetrics {
        self.metrics
    }

    pub fn window(&self) -> &Arc<Window> {
        &self.window
    }

    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    /// Use another color theme (cached rows hold colors, so they are rebuilt).
    pub(crate) fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
        self.row_cache.clear();
    }

    /// The configured cursor style ("block", "beam"/"bar", "underline").
    pub(crate) fn set_cursor_style(&mut self, style: String) {
        self.cursor_style = style;
    }

    pub(crate) fn is_focused(&self) -> bool {
        self.focused
    }

    /// The cursor blink phase drawn by the next `render`.
    pub fn cursor_blink(&self) -> bool {
        self.cursor_blink_on
    }

    pub(crate) fn set_cursor_blink(&mut self, on: bool) {
        self.cursor_blink_on = on;
    }

    pub fn surface_size(&self) -> (f32, f32) {
        (self.config.width as f32, self.config.height as f32)
    }

    pub(crate) fn set_ui_style(&mut self, family: Option<&str>, accent: Option<Rgba>) {
        self.ui_font = UiFont::new(&mut self.font_system, family);
        self.ui_accent = accent;
        self.sidebar_labels.clear();
    }

    pub(crate) fn palette_rows(&self, max: usize) -> usize {
        PaletteLayout::new(self.metrics.scale, self.surface_size(), max).rows()
    }

    /// Recompute cell metrics from the font size and scale factor.
    fn update_metrics(&mut self) {
        self.metrics = CellMetrics::measure(
            &mut self.font_system,
            self.font_family,
            self.metrics.font_size,
            self.metrics.scale,
        );
        self.row_cache.clear();
    }

    /// Use `name` for the terminal grid. An uninstalled family falls back to monospace.
    pub(crate) fn set_font_family(&mut self, name: Option<&str>) {
        let name = name.map(str::trim).filter(|n| !n.is_empty());
        self.font_family = match name {
            Some(n) => match installed_family(&self.font_system, n) {
                Some(installed) => Some(Box::leak(installed.into_boxed_str())),
                None => {
                    crate::config::warn(format!(
                        "twitty: font_family {n:?} is not installed, using monospace"
                    ));
                    None
                }
            },
            None => None,
        };
        self.update_metrics();
    }

    pub(crate) fn set_font_size(&mut self, new_size: f32) {
        self.metrics.font_size = new_size;
        self.update_metrics();
    }

    pub(crate) fn set_scale_factor(&mut self, scale: f32) {
        self.metrics.scale = if scale.is_finite() {
            scale.clamp(0.25, 16.0)
        } else {
            1.0
        };
        self.update_metrics();
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Enable or disable command block cards (default true).
    pub(crate) fn set_block_cards(&mut self, enabled: bool) {
        self.block_cards = enabled;
    }

    /// Update the hovered block card / toolbar button.
    pub(crate) fn set_block_hover(&mut self, hover: Option<BlockHover>) {
        self.block_hover = hover;
    }

    /// Forget the cached rows of a closed pane.
    pub(crate) fn forget_pane(&mut self, id: PaneId) {
        self.row_cache.forget(id);
    }

    /// Show the command palette (`Some`) or hide it.
    pub(crate) fn set_command_palette(&mut self, view: Option<PaletteView>) {
        self.palette_view = view;
    }

    /// Show the command composer (`Some`) or hide it.
    pub(crate) fn set_composer(&mut self, view: Option<scene::composer::ComposerView>) {
        self.composer_view = view;
    }

    /// Show the code review panel (`Some`) or hide it.
    pub(crate) fn set_review(&mut self, view: Option<crate::review::panel::View>) {
        self.review_view = view;
    }

    /// Hit-test physical pixel `(x, y)` against the open command palette, if any.
    pub(crate) fn palette_hit(&self, x: f64, y: f64) -> scene::palette::PaletteHit {
        let Some(view) = &self.palette_view else {
            return scene::palette::PaletteHit::Outside;
        };
        PaletteLayout::new(self.metrics.scale, self.surface_size(), view.items.len()).hit(x, y)
    }

    /// Set (or clear with `None`) the IME composition drawn inline at the cursor.
    pub(crate) fn set_preedit(&mut self, preedit: Option<(String, Option<(usize, usize)>)>) {
        self.ime.set(preedit);
    }

    pub(crate) fn set_opacity(&mut self, opacity: f32) {
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
    pub(crate) fn divider_gap(&self) -> f32 {
        self.metrics().divider_gap()
    }

    /// Grid size (cols, rows) of a pane occupying `rect`.
    pub(crate) fn compute_grid_size(&self, rect: Rect) -> (usize, usize) {
        self.metrics().grid_size(rect)
    }

    /// Map a pixel to the cell drawn there and its side. Unlike `pixel_to_cell`, this follows
    /// reordered Arabic glyphs, so selections and mouse reports match what is on screen.
    pub(crate) fn pixel_to_visual_cell(
        &self,
        pane: PaneId,
        rect: Rect,
        x: f64,
        y: f64,
    ) -> (usize, usize, Side) {
        self.row_cache.cell_at(pane, self.metrics, rect, x, y)
    }

    pub(crate) fn sidebar_cards(
        &mut self,
        entries: Vec<crate::sidebar::Entry>,
        area: Rect,
    ) -> Vec<crate::sidebar::cards::Card> {
        self.sidebar_labels.cards(
            &self.ui_font,
            &mut self.font_system,
            entries,
            area,
            self.metrics.scale,
        )
    }

    /// Draw the tab bar (with two or more tabs), every pane and the dividers between them.
    pub fn render(
        &mut self,
        chrome: &ChromeView,
        panes: &[PaneFrame],
        dividers: &[Rect],
        sidebar: Option<&crate::sidebar::cards::View>,
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
            metrics: self.metrics,
            font_system: &mut self.font_system,
            family: self.font_family,
            palette: &self.palette,
            is_srgb: self.is_srgb,
        };
        let style = UiStyle::new(&self.palette, self.ui_accent, self.metrics.scale);
        let mut frame = if self.block_cards {
            build_frame_with_cards(
                &mut ctx,
                &opts,
                &mut self.row_cache,
                &mut self.ime,
                &[],
                panes,
                dividers,
                style,
                &self.ui_font,
                self.block_hover,
                chrome.language,
            )
        } else {
            build_frame(
                &mut ctx,
                &opts,
                &mut self.row_cache,
                &mut self.ime,
                &[],
                panes,
                dividers,
            )
        };
        if let Some(view) = sidebar {
            frame.add_sidebar(
                &mut ctx,
                view,
                &mut self.sidebar_labels,
                style,
                &self.ui_font,
            );
        } else {
            self.sidebar_labels.clear();
        }
        if let Some(view) = &self.review_view {
            frame.add_review(
                &mut ctx,
                view,
                &mut self.review_labels,
                style,
                &self.ui_font,
                chrome.language,
            );
        } else {
            self.review_labels.clear();
        }
        frame.add_chrome(&mut ctx, style, &self.ui_font, chrome);
        if let Some(view) = &self.composer_view {
            frame.add_composer(&mut ctx, style, &self.ui_font, view);
        }
        if let Some(view) = &self.palette_view {
            frame.add_palette(&mut ctx, style, &self.ui_font, view);
        }
        if let Some((x, y, w, h)) = frame.ime_area {
            self.window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(x, y),
                winit::dpi::PhysicalSize::new(w, h),
            );
        }
        let runs = frame.text_runs(panes, &self.row_cache, &self.ime);
        self.gpu.draw(
            surface_texture,
            (&frame.quads, &frame.shapes),
            runs,
            &mut self.font_system,
            frame.background,
            self.opacity,
        )
    }
}
