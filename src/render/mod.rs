pub mod metrics;
pub mod row;
pub mod scene;
pub mod style;
pub mod text;

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
use alacritty_terminal::vte::ansi::CursorShape;
use cosmic_text::{Buffer, FontSystem};
use glyphon::{
    Cache, ColorMode, Resolution, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer,
    Viewport,
};
use std::sync::Arc;
use winit::window::Window;

use crate::color::Palette;
use crate::layout::{PaneId, Rect};
use crate::quad::QuadRenderer;
use crate::terminal::{CursorState, LineData};
use row::hash_row;
use scene::chrome;
use scene::context::DrawContext;
use scene::cursor::{cursor_left, cursor_quads, effective_cursor_shape};
use scene::preedit::bounds_outside;
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
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub quad_renderer: QuadRenderer,
    pub font_system: FontSystem,
    /// Configured terminal font family (leaked once per change; `None` = monospace).
    pub font_family: Option<&'static str>,
    pub swash_cache: SwashCache,
    pub text_atlas: TextAtlas,
    pub text_renderer: TextRenderer,
    pub viewport: Viewport,
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

/// How glyphon treats text colors. `Accurate` linearizes them for an sRGB surface; on the
/// non-sRGB surface that is preferred, colors must go out as-is (`Web`), like the quads, or text
/// is drawn darker than its palette color.
pub fn text_color_mode(is_srgb: bool) -> ColorMode {
    if is_srgb {
        ColorMode::Accurate
    } else {
        ColorMode::Web
    }
}

impl Renderer {
    pub async fn new(window: Arc<Window>, initial_font_size: f32) -> anyhow::Result<Self> {
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone())?;
        let mut adapter = None;
        // Prefer a real GPU; fall back to a software adapter (e.g. WARP on Windows VMs / RDP).
        for force_fallback_adapter in [false, true] {
            if let Ok(a) = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: Some(&surface),
                    force_fallback_adapter,
                    apply_limit_buckets: false,
                })
                .await
            {
                adapter = Some(a);
                break;
            }
        }
        let adapter = adapter.ok_or_else(|| anyhow::anyhow!("No compatible GPU adapter found"))?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("twitty device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults()
                    .using_resolution(adapter.limits()),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                trace: wgpu::Trace::Off,
                memory_hints: Default::default(),
            })
            .await?;

        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);
        let is_srgb = format.is_srgb();

        let present_mode = if surface_caps
            .present_modes
            .contains(&wgpu::PresentMode::Mailbox)
        {
            wgpu::PresentMode::Mailbox
        } else if surface_caps
            .present_modes
            .contains(&wgpu::PresentMode::Immediate)
        {
            wgpu::PresentMode::Immediate
        } else {
            wgpu::PresentMode::AutoVsync
        };

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode,
            // A pre/post-multiplied surface lets the compositor show what is behind the window.
            alpha_mode: [
                wgpu::CompositeAlphaMode::PreMultiplied,
                wgpu::CompositeAlphaMode::PostMultiplied,
            ]
            .into_iter()
            .find(|m| surface_caps.alpha_modes.contains(m))
            .unwrap_or(surface_caps.alpha_modes[0]),
            view_formats: vec![],
            desired_maximum_frame_latency: 1,
            color_space: Default::default(),
        };
        surface.configure(&device, &config);

        let quad_renderer = QuadRenderer::new(&device, format);
        quad_renderer.update_screen_size(&queue, width as f32, height as f32);

        let font_system = new_font_system();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let mut viewport = Viewport::new(&device, &cache);
        viewport.update(&queue, Resolution { width, height });

        let mut text_atlas =
            TextAtlas::with_color_mode(&device, &queue, &cache, format, text_color_mode(is_srgb));
        let text_renderer = TextRenderer::new(
            &mut text_atlas,
            &device,
            wgpu::MultisampleState::default(),
            None,
        );

        let scale = window.scale_factor() as f32;
        let mut renderer = Self {
            window,
            surface,
            device,
            queue,
            config,
            quad_renderer,
            font_system,
            font_family: None,
            swash_cache,
            text_atlas,
            text_renderer,
            viewport,
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
            self.surface.configure(&self.device, &self.config);
            self.viewport
                .update(&self.queue, Resolution { width, height });
            self.quad_renderer
                .update_screen_size(&self.queue, width as f32, height as f32);
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
        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                // Swapchain no longer matches the window (resize, display change, GPU reset).
                self.surface.configure(&self.device, &self.config);
                self.window.request_redraw();
                return Ok(());
            }
            _ => return Ok(()),
        };
        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let (width, height) = (self.config.width as f32, self.config.height as f32);
        let m = self.metrics();
        let mut ctx = DrawContext {
            font_system: &mut self.font_system,
            family: self.font_family,
            palette: &self.palette,
            metrics: m,
            is_srgb: self.is_srgb,
        };
        let default_bg_color = ctx.color(ctx.palette.background);
        let cursor_color = ctx.color(ctx.palette.cursor);
        let mut quads: Vec<Quad> = Vec::new();
        // The focused pane's cursor is separate: a preedit replaces it.
        let mut focused_cursor: Vec<Quad> = Vec::new();
        let mut other_cursors: Vec<Quad> = Vec::new();
        let mut ime_caret: Option<(f32, f32, bool)> = None;
        let mut ime_bounds: Option<TextBounds> = None;
        let rect_bounds = |r: Rect| TextBounds {
            left: r.x as i32,
            top: r.y as i32,
            right: r.right() as i32,
            bottom: r.bottom() as i32,
        };

        let tab_geometry = chrome::tab_geometry(&ctx, tabs.len(), width);
        chrome::tab_bar_quads(&ctx, tabs, &tab_geometry, width, &mut quads);

        // Row caches are taken out of the map while drawing so shaping can borrow `self`.
        let mut caches: Vec<Vec<CachedRow>> = Vec::with_capacity(panes.len());
        for pane in panes {
            let rect = pane.rect;
            let (origin_x, origin_y) = (rect.x + m.padding_left, rect.y + m.padding_top);
            let mut rows = self.row_cache.take(pane.id, pane.lines.len());
            let shape = effective_cursor_shape(&self.cursor_style, pane.cursor.shape);
            let cursor_focused = self.focused && pane.focused;

            for (r, line) in pane.lines.iter().enumerate() {
                let y = origin_y + r as f32 * m.line_height;
                // The cursor row stays marked during the blink's off phase: shaping depends on it
                // (an Arabic line being typed keeps its LTR base), only the block recolor turns off.
                let row_cursor =
                    (pane.cursor.is_visible && pane.cursor.row == r).then_some(RowCursor {
                        col: pane.cursor.col,
                        block: cursor_focused
                            && self.cursor_blink_on
                            && shape == CursorShape::Block,
                    });

                let hash = hash_row(&line.cells, row_cursor);
                if rows[r].hash != hash {
                    let segments = ctx.shaper().build_row(line, row_cursor);
                    let row_quads = ctx.row_quads(line, &segments);
                    rows[r] = CachedRow::new(hash, line, segments, row_quads, m.char_width);
                }

                let cached = &rows[r];
                quads.extend(
                    cached
                        .quads
                        .iter()
                        .map(|&(x, qy, w, h, c)| (origin_x + x, y + qy, w, h, c)),
                );

                if pane.focused && pane.cursor.row == r {
                    let (x, ext) = cached.cursor_anchor(pane.cursor.col, m.char_width);
                    ime_caret = Some((origin_x + x, y, ext));
                    ime_bounds = Some(rect_bounds(rect));
                }

                // Only the focused pane's cursor blinks.
                if let Some(rc) = row_cursor.filter(|_| !pane.focused || self.cursor_blink_on) {
                    let (x, extends_left) = cached.cursor_anchor(rc.col, m.char_width);
                    let x = cursor_left(origin_x + x, extends_left, shape, &m);
                    let out = if pane.focused {
                        &mut focused_cursor
                    } else {
                        &mut other_cursors
                    };
                    let w = cursor_cell_width(line, rc.col, m.char_width);
                    cursor_quads((x, y, w), shape, cursor_focused, &m, cursor_color, out);
                    if let Some(cell) = line.cells.get(rc.col).filter(|_| rc.block) {
                        let rect = (x, y, w, m.line_height);
                        out.extend(geometry_under_block_cursor(
                            ctx.palette,
                            ctx.is_srgb,
                            cell,
                            rect,
                        ));
                    }
                }
            }
            caches.push(rows);
        }

        let (preedit_box, ime_area) =
            self.ime
                .layout(&mut ctx, ime_caret, &mut quads, &mut focused_cursor);
        if let Some((x, y, w, h)) = ime_area {
            self.window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(x, y),
                winit::dpi::PhysicalSize::new(w, h),
            );
        }
        // The cursor sits above cell backgrounds but below text (its glyph was recolored).
        quads.extend(other_cursors);
        quads.extend(focused_cursor);

        let mut statuses: Vec<Option<(Quad, Buffer, f32)>> = Vec::with_capacity(panes.len());
        for pane in panes {
            let status = match &pane.status {
                Some(text) => chrome::status_line(&mut ctx, pane.rect, pane.lines.len(), text),
                None => None,
            };
            if let Some((quad, _, _)) = &status {
                quads.push(*quad);
            }
            statuses.push(status);
        }

        quads.extend(chrome::divider_quads(&ctx, dividers));
        self.quad_renderer
            .set_rects(&self.device, &self.queue, &quads);

        let tab_texts = chrome::tab_titles(&mut ctx, tabs, &tab_geometry);
        let fg = ctx.palette.foreground.to_glyphon();

        let mut text_areas: Vec<TextArea> = Vec::new();
        for (buffer, rect, color) in &tab_texts {
            text_areas.push(TextArea {
                buffer,
                left: rect.x + m.char_width,
                top: rect.y,
                scale: 1.0,
                bounds: rect_bounds(*rect),
                default_color: *color,
                custom_glyphs: &[],
            });
        }
        for ((pane, rows), status) in panes.iter().zip(&caches).zip(&statuses) {
            let bounds = rect_bounds(pane.rect);
            for (r, cached_row) in rows.iter().enumerate() {
                if status.is_some() && r + 1 == rows.len() {
                    break;
                }
                let y = pane.rect.y + m.padding_top + r as f32 * m.line_height;
                // The row under the preedit is clipped around it, so its text does not show
                // through the preedit's background.
                let (parts, n) = match preedit_box {
                    Some((left, _, width)) if pane.focused && pane.cursor.row == r => {
                        (bounds_outside(bounds, left, left + width), 2)
                    }
                    _ => ([bounds, bounds], 1),
                };
                for seg in &cached_row.segments {
                    for &bounds in &parts[..n] {
                        text_areas.push(TextArea {
                            buffer: &seg.buffer,
                            left: pane.rect.x + m.padding_left + seg.seg_x,
                            top: y,
                            scale: 1.0,
                            bounds,
                            default_color: fg,
                            custom_glyphs: &[],
                        });
                    }
                }
            }
            if let Some((_, buffer, y)) = status {
                text_areas.push(TextArea {
                    buffer,
                    left: pane.rect.x + m.padding_left,
                    top: *y,
                    scale: 1.0,
                    bounds,
                    default_color: fg,
                    custom_glyphs: &[],
                });
            }
        }
        if let Some((left, top, _)) = preedit_box {
            let bounds =
                ime_bounds.unwrap_or_else(|| rect_bounds(Rect::new(0.0, 0.0, width, height)));
            text_areas.extend(self.ime.text_area((left, top), bounds, fg));
        }

        let prepared = self.text_renderer.prepare(
            &self.device,
            &self.queue,
            &mut self.font_system,
            &mut self.text_atlas,
            &self.viewport,
            text_areas,
            &mut self.swash_cache,
        );
        for (pane, rows) in panes.iter().zip(caches) {
            self.row_cache.put(pane.id, rows);
        }
        prepared.map_err(|e| anyhow::anyhow!("Text prepare error: {:?}", e))?;

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("twitty render encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("twitty render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            // Premultiplied, so transparency keeps the theme's tint.
                            r: (default_bg_color[0] * self.opacity) as f64,
                            g: (default_bg_color[1] * self.opacity) as f64,
                            b: (default_bg_color[2] * self.opacity) as f64,
                            a: self.opacity as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            self.quad_renderer.render(&mut pass);
            self.text_renderer
                .render(&self.text_atlas, &self.viewport, &mut pass)
                .map_err(|e| anyhow::anyhow!("Text render error: {:?}", e))?;
        }

        self.queue.submit(Some(encoder.finish()));
        self.queue.present(surface_texture);

        self.text_atlas.trim();

        Ok(())
    }
}
