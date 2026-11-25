//! Everything that touches wgpu: device and surface, quad and shape pipelines and glyphon text. It draws a frame the CPU side (`render::scene`) has already laid out, either to
//! the window (`Gpu`) or to a texture read back to memory (`offscreen`, for golden tests).

pub mod context;
pub mod offscreen;
pub mod quad_pipeline;
mod shape_pipeline;
pub mod text_pipeline;

use std::sync::Arc;
use winit::window::Window;

use crate::render::scene::frame::TextRun;
use crate::render::shapes::ShapeLayers;
use crate::render::style::Quad;
use context::GpuContext;
use cosmic_text::FontSystem;
use quad_pipeline::QuadRenderer;
use shape_pipeline::ShapeRenderer;
use text_pipeline::TextPipeline;

pub(crate) struct Gpu {
    pub ctx: GpuContext,
    quads: QuadRenderer,
    shapes: ShapeRenderer,
    text: TextPipeline,
}

impl Gpu {
    /// Open the GPU for `window`; returns the surface configuration it was set up with.
    pub async fn new(window: Arc<Window>) -> anyhow::Result<(Self, wgpu::SurfaceConfiguration)> {
        let (ctx, config) = GpuContext::new(window).await?;
        let (width, height) = (config.width, config.height);
        let quads = QuadRenderer::new(&ctx.device, config.format);
        quads.update_screen_size(&ctx.queue, width as f32, height as f32);
        let shapes = ShapeRenderer::new(&ctx.device, config.format);
        shapes.resize(&ctx.queue, width, height);
        let text = TextPipeline::new(&ctx.device, &ctx.queue, config.format, (width, height));
        Ok((
            Gpu {
                ctx,
                quads,
                shapes,
                text,
            },
            config,
        ))
    }

    /// Apply a new surface size from `config`.
    pub fn resize(&mut self, config: &wgpu::SurfaceConfiguration) {
        let (width, height) = (config.width, config.height);
        self.ctx.surface.configure(&self.ctx.device, config);
        self.text.resize(&self.ctx.queue, width, height);
        self.shapes.resize(&self.ctx.queue, width, height);
        self.quads
            .update_screen_size(&self.ctx.queue, width as f32, height as f32);
    }

    /// See `GpuContext::acquire`.
    pub(crate) fn acquire(
        &self,
        config: &wgpu::SurfaceConfiguration,
        window: &Window,
    ) -> Option<wgpu::SurfaceTexture> {
        self.ctx.acquire(config, window)
    }

    /// Draw plain quads, shapes and text over a window cleared to `background` (surface color space)
    /// at `opacity`, and present it.
    pub fn draw<'a>(
        &mut self,
        surface_texture: wgpu::SurfaceTexture,
        geometry: (&[Quad], &ShapeLayers),
        runs: impl IntoIterator<Item = TextRun<'a>>,
        font_system: &mut FontSystem,
        background: [f32; 4],
        opacity: f32,
    ) -> anyhow::Result<()> {
        let GpuContext { device, queue, .. } = &self.ctx;
        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.quads.set_rects(device, queue, geometry.0);
        self.shapes.prepare(device, queue, geometry.1);
        self.text.prepare(device, queue, font_system, runs)?;

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("twitty render encoder"),
        });
        encode_pass(
            &mut encoder,
            &view,
            (&self.quads, &self.shapes, &self.text),
            background,
            opacity,
        )?;

        queue.submit(Some(encoder.finish()));
        queue.present(surface_texture);

        self.text.trim();

        Ok(())
    }
}

/// Record a frame's render pass into `encoder`: clear `view` to `background` at `opacity`,
/// then draw the prepared quads, shapes and text over it. Shared by the window and offscreen targets.
pub(crate) fn encode_pass(
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    (quads, shapes, text): (&QuadRenderer, &ShapeRenderer, &TextPipeline),
    background: [f32; 4],
    opacity: f32,
) -> anyhow::Result<()> {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("twitty render pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color {
                    // Premultiplied, so transparency keeps the theme's tint.
                    r: (background[0] * opacity) as f64,
                    g: (background[1] * opacity) as f64,
                    b: (background[2] * opacity) as f64,
                    a: opacity as f64,
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
    quads.render(&mut pass);
    shapes.render(&mut pass);
    text.render(&mut pass)
}
