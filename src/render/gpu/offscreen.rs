//! Drawing a frame into a texture and reading it back as RGBA pixels, without a window.
//!
//! Used by the golden-image tests. By default only Mesa's software rasterizer is accepted
//! (llvmpipe, through lavapipe on Vulkan or through GL), so the pixels do not depend on the
//! GPU of the machine running the tests. `TWITTY_GPU_ADAPTER` overrides that: `any` takes
//! whatever adapter is available (software first), any other value picks the adapter whose
//! name contains it.

use cosmic_text::FontSystem;

use super::encode_pass;
use super::quad_pipeline::QuadRenderer;
use super::text_pipeline::TextPipeline;
use crate::render::scene::frame::TextRun;
use crate::render::style::Quad;

/// Format of the offscreen target. Non-sRGB, like the window surface the app prefers, so
/// colors go out exactly as the frame computed them.
pub(crate) const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// A texture to draw frames into, with the device and pipelines that draw them.
pub struct Offscreen {
    device: wgpu::Device,
    queue: wgpu::Queue,
    quads: QuadRenderer,
    text: TextPipeline,
    texture: wgpu::Texture,
    width: u32,
    height: u32,
}

/// The adapter offscreen frames are drawn with (see the module docs), or an error naming the
/// adapters that were found.
async fn pick_adapter(instance: &wgpu::Instance) -> anyhow::Result<wgpu::Adapter> {
    let wanted = std::env::var("TWITTY_GPU_ADAPTER")
        .ok()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());
    let mut adapters = instance.enumerate_adapters(wgpu::Backends::all()).await;
    // Software adapters first, then Vulkan before GL (the more complete backend).
    adapters.sort_by_key(|a| {
        let info = a.get_info();
        (
            info.device_type != wgpu::DeviceType::Cpu,
            info.backend != wgpu::Backend::Vulkan,
        )
    });
    let found: Vec<String> = adapters
        .iter()
        .map(|a| {
            let info = a.get_info();
            format!("{} ({:?}, {:?})", info.name, info.backend, info.device_type)
        })
        .collect();
    adapters
        .into_iter()
        .find(|a| {
            let info = a.get_info();
            match wanted.as_deref() {
                Some("any") => true,
                Some(name) => info.name.to_lowercase().contains(name),
                None => {
                    info.device_type == wgpu::DeviceType::Cpu
                        && info.name.to_lowercase().contains("llvmpipe")
                }
            }
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no llvmpipe GPU adapter among {found:?} (install Mesa's lavapipe, or set \
                 TWITTY_GPU_ADAPTER=any to use another one)"
            )
        })
}

impl Offscreen {
    /// A `width` x `height` target. Fails when no suitable adapter exists (see the module
    /// docs); callers that can do without pixels (tests) should skip then.
    pub fn new(width: u32, height: u32) -> anyhow::Result<Self> {
        pollster::block_on(Self::new_async(width.max(1), height.max(1)))
    }

    async fn new_async(width: u32, height: u32) -> anyhow::Result<Self> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pick_adapter(&instance).await?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("twitty offscreen device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(adapter.limits()),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                trace: wgpu::Trace::Off,
                memory_hints: Default::default(),
            })
            .await?;

        let quads = QuadRenderer::new(&device, FORMAT);
        quads.update_screen_size(&queue, width as f32, height as f32);
        let text = TextPipeline::new(&device, &queue, FORMAT, (width, height));
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("twitty offscreen target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Ok(Offscreen {
            device,
            queue,
            quads,
            text,
            texture,
            width,
            height,
        })
    }

    /// Target size in pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Draw `quads` then `runs` over a target cleared to `background` at `opacity` (as the
    /// window would be) and return its pixels: `width * height` RGBA8 values, row by row from
    /// the top, without padding.
    pub fn render<'a>(
        &mut self,
        quads: &[Quad],
        runs: impl IntoIterator<Item = TextRun<'a>>,
        font_system: &mut FontSystem,
        background: [f32; 4],
        opacity: f32,
    ) -> anyhow::Result<Vec<u8>> {
        let (device, queue) = (&self.device, &self.queue);
        self.quads.set_rects(device, queue, quads);
        self.text.prepare(device, queue, font_system, runs)?;

        let view = self
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("twitty offscreen encoder"),
        });
        encode_pass(
            &mut encoder,
            &view,
            &self.quads,
            &self.text,
            background,
            opacity,
        )?;

        // Rows of a texture-to-buffer copy are padded to 256 bytes.
        let row_bytes = self.width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_row = row_bytes.div_ceil(align) * align;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("twitty offscreen readback"),
            size: (padded_row * self.height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));

        let (tx, rx) = std::sync::mpsc::channel();
        readback.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = tx.send(result);
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| anyhow::anyhow!("GPU poll failed: {e}"))?;
        rx.recv()?
            .map_err(|e| anyhow::anyhow!("reading the frame back failed: {e}"))?;

        let mut pixels = Vec::with_capacity((row_bytes * self.height) as usize);
        {
            let data = readback.get_mapped_range(..)?;
            for row in data.chunks(padded_row as usize) {
                pixels.extend_from_slice(&row[..row_bytes as usize]);
            }
        }
        readback.unmap();
        self.text.trim();
        Ok(pixels)
    }
}
