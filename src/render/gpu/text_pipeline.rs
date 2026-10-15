//! The glyphon text pipeline: glyph atlas, viewport and text renderer.

use cosmic_text::FontSystem;
use glyphon::{
    Cache, ColorMode, Resolution, SwashCache, TextArea, TextAtlas, TextRenderer, Viewport,
};

use crate::render::scene::frame::TextRun;

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

pub struct TextPipeline {
    swash_cache: SwashCache,
    atlas: TextAtlas,
    renderer: TextRenderer,
    viewport: Viewport,
}

impl TextPipeline {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        (width, height): (u32, u32),
    ) -> Self {
        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let mut viewport = Viewport::new(device, &cache);
        viewport.update(queue, Resolution { width, height });

        let mut atlas = TextAtlas::with_color_mode(
            device,
            queue,
            &cache,
            format,
            text_color_mode(format.is_srgb()),
        );
        let renderer =
            TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
        TextPipeline {
            swash_cache,
            atlas,
            renderer,
            viewport,
        }
    }

    pub fn resize(&mut self, queue: &wgpu::Queue, width: u32, height: u32) {
        self.viewport.update(queue, Resolution { width, height });
    }

    /// Rasterize and upload the glyphs of `runs` for the next `render`.
    pub fn prepare<'a>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        font_system: &mut FontSystem,
        runs: impl IntoIterator<Item = TextRun<'a>>,
    ) -> anyhow::Result<()> {
        let areas = runs.into_iter().map(|run| TextArea {
            buffer: run.buffer,
            left: run.left,
            top: run.top,
            scale: 1.0,
            bounds: run.bounds,
            default_color: run.color,
            custom_glyphs: &[],
        });
        self.renderer
            .prepare(
                device,
                queue,
                font_system,
                &mut self.atlas,
                &self.viewport,
                areas,
                &mut self.swash_cache,
            )
            .map_err(|e| anyhow::anyhow!("Text prepare error: {:?}", e))
    }

    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) -> anyhow::Result<()> {
        self.renderer
            .render(&self.atlas, &self.viewport, pass)
            .map_err(|e| anyhow::anyhow!("Text render error: {:?}", e))
    }

    /// Free atlas space taken by glyphs not used this frame.
    pub fn trim(&mut self) {
        self.atlas.trim();
    }
}
