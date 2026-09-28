use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping};
use glyphon::{
    Cache, Resolution, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use std::sync::Arc;
use unicode_bidi::{bidi_class, BidiClass};
use winit::window::Window;

use crate::color::{Palette, Rgba};
use crate::quad::{try_render_box_or_block, QuadRenderer};
use crate::terminal::{CellData, CursorState, LineData};
use alacritty_terminal::vte::ansi::Color as AnsiColor;

#[derive(Clone)]
pub struct CachedSegment {
    pub seg_start: usize,
    pub seg_end: usize,
    pub seg_x: f32,
    pub buffer: Buffer,
    #[allow(dead_code)]
    pub has_rtl: bool,
}

#[derive(Clone)]
pub struct CachedRow {
    pub hash: u64,
    pub segments: Vec<CachedSegment>,
    pub geom_cells: Vec<(usize, char, alacritty_terminal::vte::ansi::Color)>,
    pub bg_cells: Vec<(usize, alacritty_terminal::vte::ansi::Color)>,
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
    pub padding_left: f32,
    pub padding_top: f32,
    pub is_srgb: bool,
    pub opacity: f32,
    pub cursor_style: String,
    pub row_caches: Vec<CachedRow>,
}

fn hash_color<H: std::hash::Hasher>(color: &alacritty_terminal::vte::ansi::Color, hasher: &mut H) {
    use std::hash::Hash;
    match color {
        alacritty_terminal::vte::ansi::Color::Named(n) => {
            0u8.hash(hasher);
            (*n as u8).hash(hasher);
        }
        alacritty_terminal::vte::ansi::Color::Spec(rgb) => {
            1u8.hash(hasher);
            rgb.r.hash(hasher);
            rgb.g.hash(hasher);
            rgb.b.hash(hasher);
        }
        alacritty_terminal::vte::ansi::Color::Indexed(idx) => {
            2u8.hash(hasher);
            idx.hash(hasher);
        }
    }
}

fn hash_cells(cells: &[CellData]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for cell in cells {
        cell.c.hash(&mut hasher);
        hash_color(&cell.fg, &mut hasher);
        hash_color(&cell.bg, &mut hasher);
        cell.flags.bits().hash(&mut hasher);
        cell.is_selected.hash(&mut hasher);
    }
    hasher.finish()
}

impl Renderer {
    pub async fn new(
        window: Arc<Window>,
        initial_font_size: f32,
        opacity: f32,
        cursor_style: String,
    ) -> anyhow::Result<Self> {
        let size = window.inner_size();
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
            desired_maximum_frame_latency: 1,
            color_space: Default::default(),
        };
        surface.configure(&device, &config);

        let quad_renderer = QuadRenderer::new(&device, format);

        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let mut viewport = Viewport::new(&device, &cache);
        viewport.update(&queue, Resolution { width, height });

        let mut text_atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text_renderer = TextRenderer::new(
            &mut text_atlas,
            &device,
            wgpu::MultisampleState::default(),
            None,
        );

        let font_size = initial_font_size.clamp(8.0, 48.0);
        let line_height = (font_size * 1.55).round();
        let padding_left = 8.0;
        let padding_top = 6.0;

        let metrics = Metrics::new(font_size, line_height);
        let mut test_buffer = Buffer::new_empty(metrics);
        test_buffer.set_text(
            "MMMMMMMMMM",
            &Attrs::new().family(Family::Name("JetBrainsMono Nerd Font")),
            Shaping::Basic,
            None,
        );
        test_buffer.shape_until_scroll(&mut font_system, false);

        let mut measured_width = 8.8;
        for run in test_buffer.layout_runs() {
            if let Some(glyph) = run.glyphs.first() {
                if glyph.w > 0.0 {
                    measured_width = glyph.w;
                }
            }
        }
        let char_width = measured_width;

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
            padding_left,
            padding_top,
            is_srgb,
            opacity: opacity.clamp(0.1, 1.0),
            cursor_style,
            row_caches: Vec::new(),
        })
    }

    pub fn set_font_size(&mut self, new_size: f32) {
        self.font_size = new_size.clamp(8.0, 48.0);
        self.line_height = (self.font_size * 1.55).round();

        let metrics = Metrics::new(self.font_size, self.line_height);
        let mut test_buffer = Buffer::new_empty(metrics);
        test_buffer.set_text(
            "MMMMMMMMMM",
            &Attrs::new().family(Family::Name("JetBrainsMono Nerd Font")),
            Shaping::Basic,
            None,
        );
        test_buffer.shape_until_scroll(&mut self.font_system, false);

        let mut measured_width = (self.font_size * 0.6).round();
        for run in test_buffer.layout_runs() {
            if let Some(glyph) = run.glyphs.first() {
                if glyph.w > 0.0 {
                    measured_width = glyph.w;
                }
            }
        }
        self.char_width = measured_width;
        self.row_caches.clear();
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
            self.row_caches.clear();
        }
    }

    pub fn compute_grid_size(&self) -> (usize, usize) {
        let avail_w = (self.config.width as f32 - self.padding_left * 2.0).max(10.0);
        let avail_h = (self.config.height as f32 - self.padding_top * 2.0).max(10.0);
        let cols = (avail_w / self.char_width).floor() as usize;
        let rows = (avail_h / self.line_height).floor() as usize;
        (cols.max(10), rows.max(4))
    }

    fn to_target_color(&self, rgba: Rgba) -> [f32; 4] {
        if self.is_srgb {
            rgba.to_linear()
        } else {
            rgba.to_array()
        }
    }

    pub fn render(&mut self, lines: &[LineData], cursor: &CursorState) -> anyhow::Result<()> {
        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated => return Ok(()),
            _ => return Ok(()),
        };
        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let metrics = Metrics::new(self.font_size, self.line_height);
        let default_attrs = Attrs::new().family(Family::Name("JetBrainsMono Nerd Font"));
        let default_bg = self.palette.background;
        let mut default_bg_color = self.to_target_color(default_bg);
        default_bg_color[3] = self.opacity;

        let is_premultiplied = self.config.alpha_mode == wgpu::CompositeAlphaMode::PreMultiplied;
        if is_premultiplied {
            default_bg_color[0] *= self.opacity;
            default_bg_color[1] *= self.opacity;
            default_bg_color[2] *= self.opacity;
        }

        let mut background_quads = Vec::new();
        let win_w = self.config.width as f32;
        let win_h = self.config.height as f32;

        // 1. Full window background with opacity
        background_quads.push((0.0, 0.0, win_w, win_h, default_bg_color));

        if self.row_caches.len() < lines.len() {
            self.row_caches.resize(
                lines.len(),
                CachedRow {
                    hash: 0,
                    segments: Vec::new(),
                    geom_cells: Vec::new(),
                    bg_cells: Vec::new(),
                },
            );
        }

        let mut cursor_visual_pos: Option<(f32, f32)> = None;

        // Update dirty rows and collect quads
        for (r, line) in lines.iter().enumerate() {
            let y = self.padding_top + r as f32 * self.line_height;
            let cols = line.cells.len();
            let current_hash = hash_cells(&line.cells);

            if self.row_caches[r].hash != current_hash {
                let mut geom_cells = Vec::new();
                let mut bg_cells = Vec::new();
                let mut geom_rendered = vec![false; cols];

                for (c, cell) in line.cells.iter().enumerate() {
                    if cell.is_selected {
                        bg_cells.push((
                            c,
                            AnsiColor::Spec(alacritty_terminal::vte::ansi::Rgb {
                                r: 54,
                                g: 74,
                                b: 130,
                            }),
                        ));
                    } else {
                        let bg = self.palette.resolve(cell.bg, true);
                        if bg != default_bg {
                            bg_cells.push((c, cell.bg));
                        }
                    }
                    let mut test_quads = Vec::new();
                    let rendered = try_render_box_or_block(
                        cell.c,
                        0.0,
                        0.0,
                        self.char_width,
                        self.line_height,
                        [0.0; 4],
                        &mut test_quads,
                    );
                    if rendered {
                        geom_rendered[c] = true;
                        geom_cells.push((c, cell.c, cell.fg));
                    }
                }

                let mut segments = Vec::new();
                let mut col_idx = 0;
                while col_idx < cols {
                    while col_idx < cols && (geom_rendered[col_idx] || line.cells[col_idx].c == ' ')
                    {
                        col_idx += 1;
                    }
                    if col_idx >= cols {
                        break;
                    }

                    let seg_start = col_idx;
                    let mut seg_end = col_idx;

                    while seg_end < cols && !geom_rendered[seg_end] {
                        if line.cells[seg_end].bg != line.cells[seg_start].bg {
                            break;
                        }

                        if line.cells[seg_end].c == ' ' {
                            let mut space_run = 0;
                            let mut peek = seg_end;
                            while peek < cols && line.cells[peek].c == ' ' && !geom_rendered[peek] {
                                space_run += 1;
                                peek += 1;
                            }
                            if (peek < cols && geom_rendered[peek])
                                || (peek < cols && line.cells[peek].bg != line.cells[seg_start].bg)
                            {
                                break;
                            }

                            let is_active_input_space = cursor.is_visible
                                && cursor.row == r
                                && cursor.col >= seg_end
                                && cursor.col <= peek;

                            if space_run >= 3 && !is_active_input_space {
                                break;
                            }
                        }
                        seg_end += 1;
                    }

                    let trim_limit =
                        if cursor.is_visible && cursor.row == r && cursor.col >= seg_start {
                            cursor.col
                        } else {
                            seg_start
                        };

                    while seg_end > trim_limit && line.cells[seg_end - 1].c == ' ' {
                        seg_end -= 1;
                    }

                    if seg_start < seg_end {
                        let seg_x = self.padding_left + seg_start as f32 * self.char_width;

                        let mut spans_data: Vec<(String, cosmic_text::Attrs)> = Vec::new();
                        let mut cur_text = String::new();
                        let mut cur_attrs: Option<cosmic_text::Attrs> = None;
                        let mut seg_has_rtl = false;

                        for cell in line.cells[seg_start..seg_end].iter() {
                            let class = bidi_class(cell.c);
                            if class == BidiClass::R || class == BidiClass::AL {
                                seg_has_rtl = true;
                            }

                            let fg = if cell.is_selected {
                                crate::color::Rgba::from_rgb8(245, 245, 255)
                            } else {
                                self.palette.resolve(cell.fg, false)
                            };
                            let mut attrs = default_attrs.clone().color(fg.to_glyphon());
                            if cell
                                .flags
                                .contains(alacritty_terminal::term::cell::Flags::BOLD)
                            {
                                attrs = attrs.weight(cosmic_text::Weight::BOLD);
                            }
                            if cell
                                .flags
                                .contains(alacritty_terminal::term::cell::Flags::ITALIC)
                            {
                                attrs = attrs.style(cosmic_text::Style::Italic);
                            }

                            let attrs_match = cur_attrs
                                .as_ref()
                                .map(|a| a.color_opt == attrs.color_opt && a.weight == attrs.weight)
                                .unwrap_or(false);

                            if attrs_match {
                                cur_text.push(cell.c);
                            } else {
                                if !cur_text.is_empty() {
                                    if let Some(prev) = cur_attrs.take() {
                                        spans_data.push((std::mem::take(&mut cur_text), prev));
                                    }
                                }
                                cur_text.push(cell.c);
                                cur_attrs = Some(attrs);
                            }
                        }

                        if !cur_text.is_empty() {
                            if let Some(prev) = cur_attrs.take() {
                                spans_data.push((cur_text, prev));
                            }
                        }

                        if !spans_data.is_empty() {
                            if seg_has_rtl {
                                spans_data.insert(0, ("‎".to_string(), default_attrs.clone()));
                            }

                            let mut buf = Buffer::new_empty(metrics);
                            let span_refs: Vec<(&str, cosmic_text::Attrs)> = spans_data
                                .iter()
                                .map(|(s, a)| (s.as_str(), a.clone()))
                                .collect();

                            let shaping_mode = if seg_has_rtl {
                                Shaping::Advanced
                            } else {
                                Shaping::Basic
                            };

                            buf.set_rich_text(span_refs, &default_attrs, shaping_mode, None);
                            buf.shape_until_scroll(&mut self.font_system, false);

                            segments.push(CachedSegment {
                                seg_start,
                                seg_end,
                                seg_x,
                                buffer: buf,
                                has_rtl: seg_has_rtl,
                            });
                        }
                    }

                    col_idx = seg_end;
                }

                self.row_caches[r] = CachedRow {
                    hash: current_hash,
                    segments,
                    geom_cells,
                    bg_cells,
                };
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

            // Cursor Calculation
            if cursor.is_visible && cursor.row == r {
                for seg in &cached.segments {
                    if cursor.col >= seg.seg_start && cursor.col <= seg.seg_end + 15 {
                        let cx = seg.seg_x + (cursor.col - seg.seg_start) as f32 * self.char_width;
                        cursor_visual_pos = Some((cx, y));
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
            let cursor_color = self.to_target_color(self.palette.cursor);
            match self.cursor_style.as_str() {
                "block" => {
                    background_quads.push((
                        cx,
                        cy,
                        self.char_width,
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
            let clear_r = if self.is_srgb {
                default_bg_color[0] as f64
            } else {
                default_bg.r as f64
            };
            let clear_g = if self.is_srgb {
                default_bg_color[1] as f64
            } else {
                default_bg.g as f64
            };
            let clear_b = if self.is_srgb {
                default_bg_color[2] as f64
            } else {
                default_bg.b as f64
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

        Ok(())
    }
}
