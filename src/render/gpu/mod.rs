//! Everything that touches wgpu: device and surface, the quad pipeline and the glyphon text
//! pipeline. It draws a frame the CPU side (`render::scene`) has already laid out.

pub mod context;
pub mod quad_pipeline;
pub mod text_pipeline;

use std::sync::Arc;
use winit::window::Window;

use crate::render::scene::frame::TextRun;
use crate::render::style::Quad;
use context::GpuContext;
use cosmic_text::FontSystem;
use quad_pipeline::QuadRenderer;
use text_pipeline::TextPipeline;

pub struct Gpu {
    pub ctx: GpuContext,
    quads: QuadRenderer,
    text: TextPipeline,
}

impl Gpu {
    /// Open the GPU for `window`; returns the surface configuration it was set up with.
    pub async fn new(window: Arc<Window>) -> anyhow::Result<(Self, wgpu::SurfaceConfiguration)> {
        let (ctx, config) = GpuContext::new(window).await?;
        let (width, height) = (config.width, config.height);
        let quads = QuadRenderer::new(&ctx.device, config.format);
        quads.update_screen_size(&ctx.queue, width as f32, height as f32);
        let text = TextPipeline::new(&ctx.device, &ctx.queue, config.format, (width, height));
        Ok((Gpu { ctx, quads, text }, config))
    }

    /// Apply a new surface size from `config`.
    pub fn resize(&mut self, config: &wgpu::SurfaceConfiguration) {
        let (width, height) = (config.width, config.height);
        self.ctx.surface.configure(&self.ctx.device, config);
        self.text.resize(&self.ctx.queue, width, height);
        self.quads
            .update_screen_size(&self.ctx.queue, width as f32, height as f32);
    }

    /// See `GpuContext::acquire`.
    pub fn acquire(
        &self,
        config: &wgpu::SurfaceConfiguration,
        window: &Window,
    ) -> Option<wgpu::SurfaceTexture> {
        self.ctx.acquire(config, window)
    }

    /// Draw `quads` then `runs` over a window cleared to `background` (surface color space)
    /// at `opacity`, and present it.
    pub fn draw<'a>(
        &mut self,
        surface_texture: wgpu::SurfaceTexture,
        quads: &[Quad],
        runs: impl IntoIterator<Item = TextRun<'a>>,
        font_system: &mut FontSystem,
        background: [f32; 4],
        opacity: f32,
    ) -> anyhow::Result<()> {
        let GpuContext { device, queue, .. } = &self.ctx;
        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.quads.set_rects(device, queue, quads);
        self.text.prepare(device, queue, font_system, runs)?;

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
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

            self.quads.render(&mut pass);
            self.text.render(&mut pass)?;
        }

        queue.submit(Some(encoder.finish()));
        queue.present(surface_texture);

        self.text.trim();

        Ok(())
    }
}
