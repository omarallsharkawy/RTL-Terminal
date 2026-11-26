//! The wgpu device and the window surface.

use std::sync::Arc;
use winit::window::Window;

/// Choose a surface texture format (preferring non-sRGB) or return an error when none exist.
pub fn choose_surface_format(
    formats: &[wgpu::TextureFormat],
) -> anyhow::Result<wgpu::TextureFormat> {
    formats
        .iter()
        .copied()
        .find(|f| !f.is_srgb())
        .or_else(|| formats.first().copied())
        .ok_or_else(|| anyhow::anyhow!("GPU surface reported no supported texture formats"))
}

/// Choose a composite alpha mode (preferring pre/post-multiplied transparency) or return an error
/// when none exist.
pub fn choose_alpha_mode(
    alpha_modes: &[wgpu::CompositeAlphaMode],
) -> anyhow::Result<wgpu::CompositeAlphaMode> {
    [
        wgpu::CompositeAlphaMode::PreMultiplied,
        wgpu::CompositeAlphaMode::PostMultiplied,
    ]
    .into_iter()
    .find(|m| alpha_modes.contains(m))
    .or_else(|| alpha_modes.first().copied())
    .ok_or_else(|| anyhow::anyhow!("GPU surface reported no supported alpha modes"))
}

/// Device, queue and the surface frames are presented to.
pub(crate) struct GpuContext {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl GpuContext {
    /// Open a device for `window` and configure its surface. Prefers a real GPU, falling back
    /// to a software adapter; prefers a non-sRGB format, low-latency present modes and a
    /// see-through (pre/post-multiplied) surface.
    pub async fn new(window: Arc<Window>) -> anyhow::Result<(Self, wgpu::SurfaceConfiguration)> {
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window)?;
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
        let format = choose_surface_format(&surface_caps.formats)?;

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

        // A pre/post-multiplied surface lets the compositor show what is behind the window.
        let alpha_mode = choose_alpha_mode(&surface_caps.alpha_modes)?;

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

        Ok((
            GpuContext {
                surface,
                device,
                queue,
            },
            config,
        ))
    }

    /// The next surface texture, or `None` when no frame can be drawn now. A surface that no
    /// longer matches the window (resize, display change, GPU reset) is reconfigured and a
    /// redraw requested.
    pub(crate) fn acquire(
        &self,
        config: &wgpu::SurfaceConfiguration,
        window: &Window,
    ) -> Option<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => Some(t),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, config);
                window.request_redraw();
                None
            }
            _ => None,
        }
    }
}
