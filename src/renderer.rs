use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping};
use glyphon::{
    Cache, ColorMode, Resolution, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer,
    Viewport,
};
use std::sync::Arc;
use winit::window::Window;

use crate::color::{Palette, Rgba};
use crate::quad::{try_render_box_or_block, QuadRenderer};
use crate::shaping::{hash_row_with_cursor, shape_row, CachedRow};
use crate::terminal::{CursorState, LineData};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RenderStatus {
    Presented,
    Retry,
    Skipped,
}

pub struct Renderer {
    pub window: Arc<Window>,
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub quad_renderer: QuadRenderer,
    pub font_system: FontSystem,
    pub swash_cache: SwashCache,
    pub text_atlas: TextAtlas,
    pub text_renderer: TextRenderer,
    pub viewport: Viewport,
    pub palette: Palette,
    pub char_width: f32,
    pub line_height: f32,
    pub font_size: f32,
    pub scale: f32,
    pub padding_left: f32,
    pub padding_top: f32,
    pub is_srgb: bool,
    pub opacity: f32,
    pub cursor_style: String,
    pub font_family: Option<String>,
    pub row_caches: Vec<CachedRow>,
}

impl Renderer {
    pub async fn new(
        window: Arc<Window>,
        initial_font_size: f32,
        opacity: f32,
        cursor_style: String,
        font_family: Option<String>,
    ) -> anyhow::Result<Self> {
        let size = window.inner_size();
        let scale = window.scale_factor() as f32;
        let effective_font_size = initial_font_size * scale;
        // Ensure safe default dimensions if compositor has not yet completed initial layout
        let width = if size.width >= 100 { size.width } else { 960 };
        let height = if size.height >= 100 { size.height } else { 580 };

        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("twitty device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
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

        let allow_mailbox = std::env::var("TWITTY_PRESENT_MODE")
            .map(|v| v == "mailbox")
            .unwrap_or(false);
        let present_mode = if allow_mailbox
            && surface_caps
                .present_modes
                .contains(&wgpu::PresentMode::Mailbox)
        {
            wgpu::PresentMode::Mailbox
        } else if surface_caps
            .present_modes
            .contains(&wgpu::PresentMode::Fifo)
        {
            wgpu::PresentMode::Fifo
        } else {
            wgpu::PresentMode::AutoVsync
        };

        let alpha_mode = if surface_caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else if surface_caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PostMultiplied)
        {
            wgpu::CompositeAlphaMode::PostMultiplied
        } else {
            surface_caps.alpha_modes[0]
        };

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
            color_space: Default::default(),
        };
        surface.configure(&device, &config);

        let quad_renderer = QuadRenderer::new(&device, format);

        let mut font_system = FontSystem::new();
        // Embedded Arabic fallback font (Noto Naskh Arabic) ensures text renders reliably
        // on systems without Arabic fonts preinstalled.
        static EMBEDDED_ARABIC_FONT: &[u8] =
            include_bytes!("../assets/fonts/NotoNaskhArabic-Regular.ttf");
        font_system
            .db_mut()
            .load_font_data(EMBEDDED_ARABIC_FONT.to_vec());

        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let mut viewport = Viewport::new(&device, &cache);
        viewport.update(&queue, Resolution { width, height });

        let color_mode = if is_srgb {
            ColorMode::Accurate
        } else {
            ColorMode::Web
        };
        let mut text_atlas =
            TextAtlas::with_color_mode(&device, &queue, &cache, format, color_mode);
        let text_renderer = TextRenderer::new(
            &mut text_atlas,
            &device,
            wgpu::MultisampleState::default(),
            None,
        );

        let font_size = initial_font_size.clamp(8.0, 48.0);
        let line_height = (effective_font_size * 1.55).round();
        let padding_left = 8.0;
        let padding_top = 6.0;

        let char_width = Self::measure_char_width(
            &mut font_system,
            effective_font_size,
            line_height,
            font_family.as_deref(),
        );

        Ok(Self {
            window,
            surface,
            device,
            queue,
            config,
            quad_renderer,
            font_system,
            swash_cache,
            text_atlas,
            text_renderer,
            viewport,
            palette: Palette::default(),
            char_width,
            line_height,
            font_size,
            scale,
            padding_left,
            padding_top,
            is_srgb,
            opacity: opacity.clamp(0.1, 1.0),
            cursor_style,
            font_family,
            row_caches: Vec::new(),
        })
    }

    pub fn set_font_size(&mut self, new_size: f32) {
        self.scale = self.window.scale_factor() as f32;
        self.font_size = new_size.clamp(8.0, 48.0);
        let effective_font_size = self.font_size * self.scale;
        self.line_height = (effective_font_size * 1.55).round();

        self.char_width = Self::measure_char_width(
            &mut self.font_system,
            effective_font_size,
            self.line_height,
            self.font_family.as_deref(),
        );
        self.row_caches.clear();
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            let new_scale = self.window.scale_factor() as f32;
            if (new_scale - self.scale).abs() > f32::EPSILON {
                self.scale = new_scale;
                self.set_font_size(self.font_size);
            }

            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
            self.viewport
                .update(&self.queue, Resolution { width, height });
            self.quad_renderer
                .update_screen_size(&self.queue, width as f32, height as f32);
            self.row_caches.clear();
        }
    }

    pub fn measure_char_width(
        font_system: &mut FontSystem,
        effective_font_size: f32,
        line_height: f32,
        font_family: Option<&str>,
    ) -> f32 {
        let metrics = Metrics::new(effective_font_size, line_height);
        let mut test_buffer = Buffer::new_empty(metrics);
        let family = match font_family {
            Some(name) => Family::Name(name),
            None => Family::Monospace,
        };
        test_buffer.set_text(
            "MMMMMMMMMM",
            &Attrs::new().family(family),
            Shaping::Basic,
            None,
        );
        test_buffer.shape_until_scroll(font_system, false);

        let mut measured_width = (effective_font_size * 0.6).round();
        for run in test_buffer.layout_runs() {
            if let Some(glyph) = run.glyphs.first() {
                if glyph.w > 0.0 {
                    measured_width = glyph.w;
                }
            }
        }
        measured_width
    }

    pub fn compute_grid_dimensions(
        width: u32,
        height: u32,
        padding_left: f32,
        padding_top: f32,
        char_width: f32,
        line_height: f32,
    ) -> (usize, usize) {
        let avail_w = (width as f32 - padding_left * 2.0).max(10.0);
        let avail_h = (height as f32 - padding_top * 2.0).max(10.0);
        let cols = (avail_w / char_width).floor() as usize;
        let rows = (avail_h / line_height).floor() as usize;
        (cols.max(10), rows.max(4))
    }

    #[allow(dead_code)]
    #[allow(clippy::too_many_arguments)]
    pub fn compute_scaled_grid_size(
        font_system: &mut FontSystem,
        phys_width: u32,
        phys_height: u32,
        font_size: f32,
        scale: f32,
        padding_left: f32,
        padding_top: f32,
        font_family: Option<&str>,
    ) -> (usize, usize, f32, f32) {
        let effective_font_size = font_size * scale;
        let line_height = (effective_font_size * 1.55).round();
        let char_width =
            Self::measure_char_width(font_system, effective_font_size, line_height, font_family);
        let (cols, rows) = Self::compute_grid_dimensions(
            phys_width,
            phys_height,
            padding_left,
            padding_top,
            char_width,
            line_height,
        );
        (cols, rows, char_width, line_height)
    }

    pub fn compute_grid_size(&self) -> (usize, usize) {
        Self::compute_grid_dimensions(
            self.config.width,
            self.config.height,
            self.padding_left,
            self.padding_top,
            self.char_width,
            self.line_height,
        )
    }

    fn to_target_color(&self, rgba: Rgba) -> [f32; 4] {
        if self.is_srgb {
            rgba.to_linear()
        } else {
            rgba.to_array()
        }
    }

    pub fn render(
        &mut self,
        lines: &[LineData],
        cursor: &CursorState,
    ) -> anyhow::Result<RenderStatus> {
        let cur_size = self.window.inner_size();
        if cur_size.width > 0
            && cur_size.height > 0
            && (cur_size.width != self.config.width || cur_size.height != self.config.height)
        {
            self.resize(cur_size.width, cur_size.height);
        }

        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                match self.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(t)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
                    _ => return Ok(RenderStatus::Retry),
                }
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(RenderStatus::Retry);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return Ok(RenderStatus::Retry);
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                eprintln!("wgpu surface validation error");
                return Ok(RenderStatus::Skipped);
            }
        };
        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let effective_font_size = self.font_size * self.scale;
        let metrics = Metrics::new(effective_font_size, self.line_height);
        let family = match self.font_family.as_deref() {
            Some(name) => Family::Name(name),
            None => Family::Monospace,
        };
        let default_attrs = Attrs::new().family(family);
        let default_bg = self.palette.background;
        let mut background_quads = Vec::new();

        if self.row_caches.len() < lines.len() {
            self.row_caches.resize(
                lines.len(),
                CachedRow {
                    hash: 0,
                    segments: Vec::new(),
                    geom_cells: Vec::new(),
                    bg_cells: Vec::new(),
                    underline_cells: Vec::new(),
                    strikeout_cells: Vec::new(),
                },
            );
        }

        let mut cursor_visual_pos: Option<(f32, f32)> = None;

        // Update dirty rows and collect quads
        for (r, line) in lines.iter().enumerate() {
            let y = self.padding_top + r as f32 * self.line_height;
            let current_hash = hash_row_with_cursor(&line.cells, r, cursor);

            if self.row_caches[r].hash != current_hash {
                self.row_caches[r] = shape_row(
                    line,
                    r,
                    cursor,
                    self.char_width,
                    self.line_height,
                    self.padding_left,
                    metrics,
                    &default_attrs,
                    &self.palette,
                    &mut self.font_system,
                    default_bg,
                );
            }

            // Draw backgrounds and geometry from cached row data
            let cached = &self.row_caches[r];
            for &(c, bg_color) in &cached.bg_cells {
                let bg = self.palette.resolve(bg_color, true);
                let x = self.padding_left + c as f32 * self.char_width;
                background_quads.push((
                    x,
                    y,
                    self.char_width,
                    self.line_height,
                    self.to_target_color(bg),
                ));
            }
            for &(c, ch, fg_color) in &cached.geom_cells {
                let fg = self.palette.resolve(fg_color, false);
                let x = self.padding_left + c as f32 * self.char_width;
                let _ = try_render_box_or_block(
                    ch,
                    x,
                    y,
                    self.char_width,
                    self.line_height,
                    self.to_target_color(fg),
                    &mut background_quads,
                );
            }

            for &(c, fg, is_double) in &cached.underline_cells {
                let x = self.padding_left + c as f32 * self.char_width;
                let u_color = self.to_target_color(fg);
                let u_y = y + self.line_height - 2.0;
                background_quads.push((x, u_y, self.char_width, 1.0 * self.scale, u_color));
                if is_double {
                    background_quads.push((
                        x,
                        u_y - 2.0 * self.scale,
                        self.char_width,
                        1.0 * self.scale,
                        u_color,
                    ));
                }
            }
            for &(c, fg) in &cached.strikeout_cells {
                let x = self.padding_left + c as f32 * self.char_width;
                let s_y = y + (self.line_height * 0.5).round();
                background_quads.push((
                    x,
                    s_y,
                    self.char_width,
                    1.0 * self.scale,
                    self.to_target_color(fg),
                ));
            }

            // Cursor Calculation
            if cursor.is_visible && cursor.row == r {
                for seg in &cached.segments {
                    if cursor.col >= seg.seg_start && cursor.col <= seg.seg_end + 15 {
                        if seg.has_rtl {
                            if cursor.col >= seg.seg_end {
                                let text_width = seg
                                    .buffer
                                    .layout_runs()
                                    .map(|run| run.line_w)
                                    .fold(0.0, f32::max);
                                cursor_visual_pos = Some((seg.seg_x + text_width, y));
                            } else {
                                let mut byte_target: usize = line.cells[seg.seg_start..cursor.col]
                                    .iter()
                                    .map(|c| c.c.len_utf8())
                                    .sum();
                                byte_target += "\u{200E}".len();
                                for run in seg.buffer.layout_runs() {
                                    for glyph in run.glyphs.iter() {
                                        if byte_target >= glyph.start && byte_target < glyph.end {
                                            let gx = if glyph.level.is_rtl() {
                                                glyph.x + glyph.w
                                            } else {
                                                glyph.x
                                            };
                                            cursor_visual_pos = Some((seg.seg_x + gx, y));
                                            break;
                                        }
                                    }
                                }
                            }
                        } else {
                            let cx =
                                seg.seg_x + (cursor.col - seg.seg_start) as f32 * self.char_width;
                            cursor_visual_pos = Some((cx, y));
                        }
                        break;
                    }
                }
                if cursor_visual_pos.is_none() {
                    let cx = self.padding_left + cursor.col as f32 * self.char_width;
                    cursor_visual_pos = Some((cx, y));
                }
            }
        }

        // Custom Cursor Style Rendering
        if let Some((cx, cy)) = cursor_visual_pos {
            if cursor.is_visible {
                self.window.set_ime_cursor_area(
                    winit::dpi::PhysicalPosition::new(cx, cy),
                    winit::dpi::PhysicalSize::new(self.char_width, self.line_height),
                );
            }
            let cursor_color = self.to_target_color(self.palette.cursor);
            let effective_shape = match cursor.shape {
                alacritty_terminal::vte::ansi::CursorShape::Block => "block",
                alacritty_terminal::vte::ansi::CursorShape::Underline => "underline",
                alacritty_terminal::vte::ansi::CursorShape::Beam => "beam",
                alacritty_terminal::vte::ansi::CursorShape::HollowBlock => "hollow",
                alacritty_terminal::vte::ansi::CursorShape::Hidden => "hidden",
            };
            let shape_str = if effective_shape == "block" && self.cursor_style != "block" {
                // If terminal mode is at default block, check if user preferred a custom style
                self.cursor_style.as_str()
            } else {
                effective_shape
            };

            match shape_str {
                "hidden" => {}
                "block" => {
                    background_quads.push((
                        cx,
                        cy,
                        self.char_width,
                        self.line_height,
                        cursor_color,
                    ));
                }
                "hollow" => {
                    // 4 border quads for hollow box
                    let border = 1.0 * self.scale;
                    background_quads.push((cx, cy, self.char_width, border, cursor_color));
                    background_quads.push((
                        cx,
                        cy + self.line_height - border,
                        self.char_width,
                        border,
                        cursor_color,
                    ));
                    background_quads.push((cx, cy, border, self.line_height, cursor_color));
                    background_quads.push((
                        cx + self.char_width - border,
                        cy,
                        border,
                        self.line_height,
                        cursor_color,
                    ));
                }
                "underline" => {
                    background_quads.push((
                        cx,
                        cy + self.line_height - 2.0,
                        self.char_width,
                        2.0,
                        cursor_color,
                    ));
                }
                _ => {
                    // Sleek beam cursor (2.0px wide, vertically centered)
                    background_quads.push((
                        cx,
                        cy + 1.0,
                        2.0,
                        self.line_height - 2.0,
                        cursor_color,
                    ));
                }
            }
        }

        self.quad_renderer
            .set_rects(&self.device, &background_quads);

        // Prepare text areas directly from cached row buffers
        let mut text_areas: Vec<TextArea> = Vec::new();
        for (r, cached_row) in self.row_caches.iter().enumerate().take(lines.len()) {
            let y = self.padding_top + r as f32 * self.line_height;
            for seg in &cached_row.segments {
                text_areas.push(TextArea {
                    buffer: &seg.buffer,
                    left: seg.seg_x,
                    top: y,
                    scale: 1.0,
                    bounds: TextBounds {
                        left: 0,
                        top: 0,
                        right: self.config.width as i32,
                        bottom: self.config.height as i32,
                    },
                    default_color: self.palette.foreground.to_glyphon(),
                    custom_glyphs: &[],
                });
            }
        }

        self.text_renderer
            .prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.text_atlas,
                &self.viewport,
                text_areas,
                &mut self.swash_cache,
            )
            .map_err(|e| anyhow::anyhow!("Text prepare error: {:?}", e))?;

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("twitty render encoder"),
            });

        {
            let is_premultiplied =
                self.config.alpha_mode == wgpu::CompositeAlphaMode::PreMultiplied;
            let (clear_r, clear_g, clear_b) = if is_premultiplied {
                (
                    default_bg.r as f64 * self.opacity as f64,
                    default_bg.g as f64 * self.opacity as f64,
                    default_bg.b as f64 * self.opacity as f64,
                )
            } else {
                (
                    default_bg.r as f64,
                    default_bg.g as f64,
                    default_bg.b as f64,
                )
            };
            let clear_a = self.opacity as f64;

            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("twitty render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear_r,
                            g: clear_g,
                            b: clear_b,
                            a: clear_a,
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

        Ok(RenderStatus::Presented)
    }
}
