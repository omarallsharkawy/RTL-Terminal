use alacritty_terminal::index::Side;
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::vte::ansi::CursorShape;
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

/// Line height as a multiple of the font size (tall enough for Arabic harakat).
const LINE_HEIGHT_FACTOR: f32 = 1.55;
/// Logical (DPI-independent) window padding.
const PADDING_LEFT: f32 = 8.0;
const PADDING_TOP: f32 = 6.0;
/// Left-to-right mark prepended to RTL segments so prompts never flip direction.
const LRM: &str = "\u{200E}";
/// Right-to-left mark appended after spaces typed at the end of Arabic, so they stay on its left.
const RLM: &str = "\u{200F}";
/// Left-to-right isolate / pop directional isolate: wrap Latin words and numbers inside Arabic so
/// hashes like `7abaf40` are never torn into `7` … `abaf40`.
const LRI: &str = "\u{2066}";
const PDI: &str = "\u{2069}";

type Quad = (f32, f32, f32, f32, [f32; 4]);

#[derive(Clone)]
pub struct CachedSegment {
    pub seg_start: usize,
    pub seg_end: usize,
    /// X offset relative to the left padding.
    pub seg_x: f32,
    pub buffer: Buffer,
    pub has_rtl: bool,
    /// Byte offset (into the shaped text) where each cell of the segment starts.
    pub cell_bytes: Vec<usize>,
}

#[derive(Clone, Default)]
pub struct CachedRow {
    pub hash: u64,
    pub segments: Vec<CachedSegment>,
    /// Backgrounds, box drawing and text decorations, relative to the row origin.
    pub quads: Vec<Quad>,
}

/// Cursor details that change how a row is drawn (the glyph under a block cursor is recolored).
#[derive(Clone, Copy, PartialEq, Hash)]
pub struct RowCursor {
    pub col: usize,
    pub block: bool,
}

impl CachedRow {
    /// Where the cursor goes (relative to the left padding), and whether it should extend to the
    /// left of that point. After Arabic text the caret sits on the text's left, where the next
    /// Arabic letter will appear; after Latin text it sits on the right.
    pub fn cursor_anchor(&self, col: usize, char_width: f32) -> (f32, bool) {
        let row = self;
        let grid_x = col as f32 * char_width;
        let glyph_of = |seg: &CachedSegment, c: usize| {
            let target = seg.cell_bytes[c - seg.seg_start];
            seg.buffer.layout_runs().find_map(|run| {
                run.glyphs
                    .iter()
                    .find(|g| target >= g.start && target < g.end)
                    .map(|g| (seg.seg_x + g.x, seg.seg_x + g.x + g.w, g.level.is_rtl()))
            })
        };
        for (i, seg) in row.segments.iter().enumerate() {
            if !seg.has_rtl {
                continue;
            }
            if col >= seg.seg_start && col < seg.seg_end {
                // Inside the text: the insertion point is the cell's leading edge.
                return match glyph_of(seg, col) {
                    Some((_, right, true)) => (right, true),
                    Some((left, _, false)) => (left, false),
                    None => (grid_x, false),
                };
            }
            let next_start = row.segments.get(i + 1).map_or(usize::MAX, |s| s.seg_start);
            if col >= seg.seg_end && col < next_start {
                let gap = (col - seg.seg_end) as f32 * char_width;
                return match glyph_of(seg, seg.seg_end - 1) {
                    Some((left, _, true)) => (left - gap, true),
                    Some((_, right, false)) => (right + gap, false),
                    None => (grid_x, false),
                };
            }
        }
        (grid_x, false)
    }
}

/// Turns terminal rows into shaped text segments. Kept apart from the GPU state so the
/// bidi/cursor logic can be tested without a window.
pub struct RowShaper<'a> {
    pub font_system: &'a mut FontSystem,
    pub palette: &'a Palette,
    /// Font size in physical pixels.
    pub font_px: f32,
    pub char_width: f32,
    pub line_height: f32,
}

impl RowShaper<'_> {
    fn shape_buffer(&mut self, spans: &[(String, Attrs<'static>)], shaping: Shaping) -> Buffer {
        let metrics = Metrics::new(self.font_px, self.line_height);
        let default_attrs = Attrs::new().family(Family::Monospace);
        let mut buf = Buffer::new_empty(metrics);
        buf.set_rich_text(
            spans.iter().map(|(s, a)| (s.as_str(), a.clone())),
            &default_attrs,
            shaping,
            None,
        );
        buf.shape_until_scroll(self.font_system, false);
        buf
    }

    /// Shape cells `[start, end)` into one buffer. Returns the buffer plus each cell's byte offset.
    fn shape_cells(
        &mut self,
        cells: &[CellData],
        start: usize,
        end: usize,
        has_rtl: bool,
        cursor: Option<RowCursor>,
    ) -> (Buffer, Vec<usize>) {
        let mut spans: Vec<(String, Attrs<'static>)> = Vec::new();
        let mut cell_bytes = Vec::with_capacity(end - start);
        let mut byte = 0;
        let mut complex = has_rtl;
        if has_rtl {
            spans.push((LRM.to_string(), Attrs::new().family(Family::Monospace)));
            byte += LRM.len();
        }

        // Words (runs of non-blank cells) with Latin letters or digits and no Arabic.
        let mut isolate = vec![(false, false); end - start];
        if has_rtl {
            let mut c = start;
            while c < end {
                if cells[c].is_blank() {
                    c += 1;
                    continue;
                }
                let word_start = c;
                while c < end && !cells[c].is_blank() {
                    c += 1;
                }
                let word = &cells[word_start..c];
                if word.iter().any(|w| w.c.is_ascii_alphanumeric())
                    && !word.iter().any(|w| is_rtl_char(w.c))
                {
                    isolate[word_start - start].0 = true;
                    isolate[c - 1 - start].1 = true;
                }
            }
        }

        for (col, cell) in cells.iter().enumerate().take(end).skip(start) {
            let (opens, closes) = isolate[col - start];
            let mut text = String::new();
            if opens {
                text.push_str(LRI);
                byte += LRI.len();
            }
            cell_bytes.push(byte);
            byte += cell.text_len();
            cell.push_text(&mut text);
            if closes {
                text.push_str(PDI);
                byte += PDI.len();
            }
            if text.is_empty() {
                continue;
            }
            complex |= !cell.c.is_ascii() || !cell.zerowidth.is_empty();

            let (mut fg, bg) = cell_colors(self.palette, cell);
            if cursor.is_some_and(|c| c.block && c.col == col) {
                // Text under a block cursor takes the cell's background so it stays legible.
                fg = bg;
            }
            let mut attrs = Attrs::new()
                .family(Family::Monospace)
                .color(fg.to_glyphon());
            if cell.flags.contains(CellFlags::BOLD) {
                attrs = attrs.weight(cosmic_text::Weight::BOLD);
            }
            if cell.flags.contains(CellFlags::ITALIC) {
                attrs = attrs.style(cosmic_text::Style::Italic);
            }

            match spans.last_mut() {
                Some((existing, a))
                    if a.color_opt == attrs.color_opt
                        && a.weight == attrs.weight
                        && a.style == attrs.style =>
                {
                    existing.push_str(&text)
                }
                _ => spans.push((text, attrs)),
            }
        }

        // Spaces typed right after Arabic belong to it: keep them on its left, not the right.
        if has_rtl && end > start && cells[end - 1].is_blank() {
            let last_text = cells[start..end].iter().rev().find(|c| !c.is_blank());
            if last_text.is_some_and(|c| is_rtl_char(c.c)) {
                if let Some((text, _)) = spans.last_mut() {
                    text.push_str(RLM);
                }
            }
        }

        let shaping = if complex {
            Shaping::Advanced
        } else {
            Shaping::Basic
        };
        (self.shape_buffer(&spans, shaping), cell_bytes)
    }

    /// True when every glyph of an LTR buffer starts exactly at its cell's grid column.
    fn is_grid_aligned(&self, buffer: &Buffer, cell_bytes: &[usize]) -> bool {
        buffer.layout_runs().all(|run| {
            run.glyphs.iter().all(|glyph| {
                let cell = cell_bytes
                    .partition_point(|&b| b <= glyph.start)
                    .saturating_sub(1);
                (glyph.x - cell as f32 * self.char_width).abs() < 0.5
            })
        })
    }

    pub fn build_row(&mut self, line: &LineData, cursor: Option<RowCursor>) -> Vec<CachedSegment> {
        let cells = &line.cells;
        let cols = cells.len();
        let geom: Vec<bool> = cells
            .iter()
            .map(|cell| {
                let mut scratch = Vec::new();
                try_render_box_or_block(cell.c, 0.0, 0.0, 1.0, 1.0, [0.0; 4], &mut scratch)
            })
            .collect();

        let mut segments = Vec::new();
        let mut col = 0;
        while col < cols {
            while col < cols && (geom[col] || cells[col].is_blank()) {
                col += 1;
            }
            if col >= cols {
                break;
            }

            // Grow the segment, keeping short space runs so bidi context spans whole phrases.
            let start = col;
            let mut end = col;
            while end < cols && !geom[end] {
                if cells[end].is_blank() {
                    let mut peek = end;
                    while peek < cols && cells[peek].is_blank() {
                        peek += 1;
                    }
                    if peek - end >= 8 || peek == cols || geom[peek] {
                        break;
                    }
                }
                end += 1;
            }
            while end > start && cells[end - 1].is_blank() {
                end -= 1;
            }
            // Spaces the user just typed after Arabic text stay part of it (see RLM above).
            if let Some(c) = cursor {
                if c.col > end
                    && c.col <= cols
                    && is_rtl_char(cells[end - 1].c)
                    && cells[end..c.col].iter().all(|cell| cell.is_blank())
                {
                    end = c.col;
                }
            }
            col = end;

            let has_rtl = cells[start..end].iter().any(|c| is_rtl_char(c.c));
            let (buffer, cell_bytes) = self.shape_cells(cells, start, end, has_rtl, cursor);

            if has_rtl || self.is_grid_aligned(&buffer, &cell_bytes) {
                segments.push(CachedSegment {
                    seg_start: start,
                    seg_end: end,
                    seg_x: start as f32 * self.char_width,
                    buffer,
                    has_rtl,
                    cell_bytes,
                });
                continue;
            }

            // A fallback glyph (icon, emoji, CJK…) has a different advance: lock every cell to the grid.
            for c in start..end {
                if cells[c].is_blank() || cells[c].is_spacer() {
                    continue;
                }
                let (buffer, cell_bytes) = self.shape_cells(cells, c, c + 1, false, cursor);
                segments.push(CachedSegment {
                    seg_start: c,
                    seg_end: c + 1,
                    seg_x: c as f32 * self.char_width,
                    buffer,
                    has_rtl: false,
                    cell_bytes,
                });
            }
        }
        segments
    }
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
    pub row_caches: Vec<CachedRow>,
}

fn hash_row(cells: &[CellData], cursor: Option<RowCursor>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for cell in cells {
        cell.c.hash(&mut hasher);
        cell.zerowidth.hash(&mut hasher);
        hash_color(&cell.fg, &mut hasher);
        hash_color(&cell.bg, &mut hasher);
        cell.flags.bits().hash(&mut hasher);
        cell.selected.hash(&mut hasher);
    }
    cursor.hash(&mut hasher);
    hasher.finish()
}

fn hash_color<H: std::hash::Hasher>(color: &alacritty_terminal::vte::ansi::Color, hasher: &mut H) {
    use alacritty_terminal::vte::ansi::Color;
    use std::hash::Hash;
    match color {
        Color::Named(n) => (0u8, *n as u8).hash(hasher),
        Color::Spec(rgb) => (1u8, rgb.r, rgb.g, rgb.b).hash(hasher),
        Color::Indexed(idx) => (2u8, *idx).hash(hasher),
    }
}

fn is_rtl_char(c: char) -> bool {
    matches!(bidi_class(c), BidiClass::R | BidiClass::AL)
}

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a,
    }
}

/// Resolve a cell's effective (foreground, background) honoring INVERSE, DIM and HIDDEN.
pub fn cell_colors(palette: &Palette, cell: &CellData) -> (Rgba, Rgba) {
    let mut fg = palette.resolve(cell.fg, false);
    let mut bg = palette.resolve(cell.bg, true);
    if cell.flags.contains(CellFlags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if cell.flags.contains(CellFlags::DIM) {
        fg = mix(fg, bg, 0.34);
    }
    if cell.flags.contains(CellFlags::HIDDEN) {
        fg = bg;
    }
    if cell.selected {
        bg = palette.selection;
    }
    (fg, bg)
}

/// Measure the advance of one monospace cell at the given pixel size.
fn measure_char_width(font_system: &mut FontSystem, font_px: f32, line_height: f32) -> f32 {
    let mut buffer = Buffer::new_empty(Metrics::new(font_px, line_height));
    buffer.set_text(
        "MMMMMMMMMM",
        &Attrs::new().family(Family::Monospace),
        Shaping::Basic,
        None,
    );
    buffer.shape_until_scroll(font_system, false);
    buffer
        .layout_runs()
        .filter_map(|run| run.glyphs.first().map(|g| g.w))
        .find(|w| *w > 0.0)
        .unwrap_or((font_px * 0.6).round())
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

        let font_system = FontSystem::new();
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

        let scale = window.scale_factor() as f32;
        let mut renderer = Self {
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
            row_caches: Vec::new(),
        };
        renderer.update_metrics();
        Ok(renderer)
    }

    fn font_px(&self) -> f32 {
        self.font_size * self.scale
    }

    /// Recompute cell metrics from the font size and scale factor.
    fn update_metrics(&mut self) {
        self.font_size = self.font_size.clamp(8.0, 48.0);
        let font_px = self.font_px();
        self.line_height = (font_px * LINE_HEIGHT_FACTOR).round();
        self.char_width = measure_char_width(&mut self.font_system, font_px, self.line_height);
        self.padding_left = (PADDING_LEFT * self.scale).round();
        self.padding_top = (PADDING_TOP * self.scale).round();
        self.row_caches.clear();
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

    pub fn set_opacity(&mut self, opacity: f32) {
        self.opacity = opacity.clamp(0.2, 1.0);
    }

    /// The configured style replaces the default block; shapes an app asks for (vim insert
    /// mode's beam, for example) are kept.
    fn effective_cursor_shape(&self, shape: CursorShape) -> CursorShape {
        if shape != CursorShape::Block {
            return shape;
        }
        match self.cursor_style.as_str() {
            "beam" | "bar" => CursorShape::Beam,
            "underline" => CursorShape::Underline,
            _ => CursorShape::Block,
        }
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

    /// Map a physical pixel position to a (col, row) cell, clamped to the grid.
    pub fn pixel_to_cell(&self, x: f64, y: f64) -> (usize, usize) {
        let (cols, rows) = self.compute_grid_size();
        let col = ((x - self.padding_left as f64) / self.char_width as f64).max(0.0) as usize;
        let row = ((y - self.padding_top as f64) / self.line_height as f64).max(0.0) as usize;
        (col.min(cols - 1), row.min(rows - 1))
    }

    /// Which half of its cell the pixel falls in (selections include a cell only when the
    /// pointer passes its middle).
    pub fn pixel_side(&self, x: f64) -> Side {
        let rel = (x - self.padding_left as f64) / self.char_width as f64;
        if rel.fract() < 0.5 {
            Side::Left
        } else {
            Side::Right
        }
    }

    fn shaper(&mut self) -> RowShaper<'_> {
        RowShaper {
            font_px: self.font_px(),
            font_system: &mut self.font_system,
            palette: &self.palette,
            char_width: self.char_width,
            line_height: self.line_height,
        }
    }

    fn to_target_color(&self, rgba: Rgba) -> [f32; 4] {
        if self.is_srgb {
            rgba.to_linear()
        } else {
            rgba.to_array()
        }
    }

    fn build_row_quads(&self, line: &LineData) -> Vec<Quad> {
        let cw = self.char_width;
        let lh = self.line_height;
        let font_px = self.font_px();
        let thickness = (font_px / 14.0).round().max(1.0);
        let underline_y = ((lh + font_px) * 0.5).round().min(lh - thickness);
        let strike_y = (lh * 0.5).round();

        let mut quads = Vec::new();
        for (c, cell) in line.cells.iter().enumerate() {
            let (fg, bg) = cell_colors(&self.palette, cell);
            let x = c as f32 * cw;
            if bg != self.palette.background {
                quads.push((x, 0.0, cw, lh, self.to_target_color(bg)));
            }
            let fg_color = self.to_target_color(fg);
            if !cell.flags.contains(CellFlags::HIDDEN) {
                try_render_box_or_block(cell.c, x, 0.0, cw, lh, fg_color, &mut quads);
            }
            if cell.flags.contains(CellFlags::DOUBLE_UNDERLINE) {
                quads.push((x, underline_y - thickness * 2.0, cw, thickness, fg_color));
                quads.push((x, underline_y, cw, thickness, fg_color));
            } else if cell.flags.intersects(CellFlags::ALL_UNDERLINES) {
                quads.push((x, underline_y, cw, thickness, fg_color));
            }
            if cell.flags.contains(CellFlags::STRIKEOUT) {
                quads.push((x, strike_y, cw, thickness, fg_color));
            }
        }
        quads
    }

    fn cursor_quads(&self, x: f32, y: f32, shape: CursorShape, out: &mut Vec<Quad>) {
        let (w, h) = (self.char_width, self.line_height);
        let color = self.to_target_color(self.palette.cursor);
        let t = (self.scale * 1.5).round().max(1.0);
        let hollow = |out: &mut Vec<Quad>| {
            out.push((x, y, w, t, color));
            out.push((x, y + h - t, w, t, color));
            out.push((x, y, t, h, color));
            out.push((x + w - t, y, t, h, color));
        };
        match shape {
            CursorShape::Hidden => {}
            _ if !self.focused => hollow(out),
            CursorShape::Block => out.push((x, y, w, h, color)),
            CursorShape::HollowBlock => hollow(out),
            CursorShape::Beam => out.push((x, y, t, h, color)),
            CursorShape::Underline => out.push((x, y + h - t * 2.0, w, t * 2.0, color)),
        }
    }

    pub fn render(&mut self, lines: &[LineData], cursor: &CursorState) -> anyhow::Result<()> {
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

        let default_bg_color = self.to_target_color(self.palette.background);
        let shape = self.effective_cursor_shape(cursor.shape);
        let mut quads: Vec<Quad> = Vec::new();
        let mut cursor_quads: Vec<Quad> = Vec::new();

        if self.row_caches.len() != lines.len() {
            self.row_caches.resize(lines.len(), CachedRow::default());
        }

        for (r, line) in lines.iter().enumerate() {
            let y = self.padding_top + r as f32 * self.line_height;
            let row_cursor = (cursor.is_visible && cursor.row == r).then_some(RowCursor {
                col: cursor.col,
                block: self.focused && shape == CursorShape::Block,
            });

            let hash = hash_row(&line.cells, row_cursor);
            if self.row_caches[r].hash != hash {
                let segments = self.shaper().build_row(line, row_cursor);
                let row_quads = self.build_row_quads(line);
                self.row_caches[r] = CachedRow {
                    hash,
                    segments,
                    quads: row_quads,
                };
            }

            let cached = &self.row_caches[r];
            quads.extend(
                cached
                    .quads
                    .iter()
                    .map(|&(x, qy, w, h, c)| (self.padding_left + x, y + qy, w, h, c)),
            );

            if let Some(rc) = row_cursor {
                let (x, extends_left) = cached.cursor_anchor(rc.col, self.char_width);
                let mut x = self.padding_left + x;
                if extends_left {
                    x -= match shape {
                        CursorShape::Beam => (self.scale * 1.5).round().max(1.0),
                        _ => self.char_width,
                    };
                }
                self.cursor_quads(x.max(0.0), y, shape, &mut cursor_quads);
            }
        }

        // The cursor sits above cell backgrounds but below text (its glyph was recolored).
        quads.extend(cursor_quads);
        self.quad_renderer
            .set_rects(&self.device, &self.queue, &quads);

        let bounds = TextBounds {
            left: 0,
            top: 0,
            right: self.config.width as i32,
            bottom: self.config.height as i32,
        };
        let mut text_areas: Vec<TextArea> = Vec::new();
        for (r, cached_row) in self.row_caches.iter().enumerate() {
            let y = self.padding_top + r as f32 * self.line_height;
            for seg in &cached_row.segments {
                text_areas.push(TextArea {
                    buffer: &seg.buffer,
                    left: self.padding_left + seg.seg_x,
                    top: y,
                    scale: 1.0,
                    bounds,
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
