//! Everything that touches wgpu: device and surface, quad and shape pipelines and glyphon text. It draws a frame the CPU side (`render::scene`) has already laid out, either to
//! the window (`Gpu`) or to a texture read back to memory (`offscreen`, for golden tests).

pub mod context;
pub mod offscreen;
pub mod quad_pipeline;
mod shape_pipeline;
pub mod text_pipeline;

use std::sync::Arc;
use winit::window::Window;

use crate::scene::TextRun;
use crate::shapes::ShapeLayers;
use crate::style::Quad;
use context::GpuContext;
use cosmic_text::FontSystem;
use quad_pipeline::QuadRenderer;
use shape_pipeline::ShapeRenderer;
use text_pipeline::TextPipeline;

#[doc(hidden)]
pub struct Gpu {
    ctx: GpuContext,
    quads: QuadRenderer,
    shapes: ShapeRenderer,
    text: TextPipeline,
}

impl Gpu {
    /// Open the GPU for `window`; returns the surface configuration it was set up with.
    pub async fn new(window: Arc<Window>) -> anyhow::Result<(Self, wgpu::SurfaceConfiguration)> {
        let (ctx, config) = GpuContext::new(window).await?;
        let (width, height) = (config.width, config.height);
        let quads = QuadRenderer::new(&ctx.device, config.format, config.alpha_mode);
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
    #[doc(hidden)]
    pub fn acquire(
        &self,
        config: &wgpu::SurfaceConfiguration,
        window: &Window,
    ) -> Option<wgpu::SurfaceTexture> {
        self.ctx.acquire(config, window)
    }

    /// Compose window material at `opacity`, draw text over it, and present it.
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

    /// Compose base terminal + chrome at `opacity`, draw base text, then draw each GPU overlay
    /// shape slice interleaved with its own text layer inside a single render pass.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_layered<'a, L, I>(
        &mut self,
        surface_texture: wgpu::SurfaceTexture,
        geometry: (&[Quad], &ShapeLayers),
        overlay_splits: &[usize],
        text_layers: L,
        font_system: &mut FontSystem,
        background: [f32; 4],
        opacity: f32,
    ) -> anyhow::Result<()>
    where
        L: IntoIterator<Item = I>,
        I: IntoIterator<Item = TextRun<'a>>,
    {
        let GpuContext { device, queue, .. } = &self.ctx;
        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.quads.set_rects(device, queue, geometry.0);
        self.shapes
            .prepare_layered(device, queue, geometry.1, overlay_splits);
        self.text
            .prepare_layered(device, queue, font_system, text_layers)?;

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("twitty layered render encoder"),
        });
        encode_layered_pass(
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

/// Compose geometry over the theme background, apply uniform material opacity, then draw text.
/// Shared by the window and offscreen targets.
pub(crate) fn encode_pass(
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    (quads, shapes, text): (&QuadRenderer, &ShapeRenderer, &TextPipeline),
    background: [f32; 4],
    opacity: f32,
) -> anyhow::Result<()> {
    let mut pass = begin_main_pass(encoder, view, background);
    shapes.render_underlay(&mut pass);
    quads.render(&mut pass);
    shapes.render(&mut pass);
    quads.apply_material_opacity(&mut pass, opacity);
    text.render(&mut pass)
}

/// Compose base geometry + chrome at `opacity`, draw base text, then draw each overlay's SDF
/// shapes directly over base text followed by that overlay's own text runs.
pub(crate) fn encode_layered_pass(
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    (quads, shapes, text): (&QuadRenderer, &ShapeRenderer, &TextPipeline),
    background: [f32; 4],
    opacity: f32,
) -> anyhow::Result<()> {
    let mut pass = begin_main_pass(encoder, view, background);
    shapes.render_underlay(&mut pass);
    quads.render(&mut pass);
    shapes.render_chrome(&mut pass);
    quads.apply_material_opacity(&mut pass, opacity);
    text.render_layer(0, &mut pass)?;
    let overlay_passes = shapes
        .overlay_pass_count()
        .max(text.layer_count().saturating_sub(1));
    for i in 0..overlay_passes {
        shapes.render_overlay_pass(i, &mut pass);
        text.render_layer(i + 1, &mut pass)?;
    }
    Ok(())
}

fn begin_main_pass<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    view: &'a wgpu::TextureView,
    background: [f32; 4],
) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("twitty render pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color {
                    r: f64::from(background[0]),
                    g: f64::from(background[1]),
                    b: f64::from(background[2]),
                    a: 1.0,
                }),
                store: wgpu::StoreOp::Store,
            },
            depth_slice: None,
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}
