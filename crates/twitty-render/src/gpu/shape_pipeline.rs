//! Instanced signed-distance primitives, with one draw per nonempty layer.

use std::ops::Range;

use crate::shapes::{append_instances, ShapeInstance, ShapeLayers};

pub(crate) struct ShapeRenderer {
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    buffer: Option<(wgpu::Buffer, usize)>,
    instances: Vec<ShapeInstance>,
    layers: [Range<u32>; 3],
}

impl ShapeRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("UI shape shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shape.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("UI shape pipeline"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<ShapeInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4,
                        2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4,
                        6 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("UI shape screen size"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("UI shape bind group"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        Self {
            pipeline,
            uniform,
            bind_group,
            buffer: None,
            instances: Vec::new(),
            layers: [0..0, 0..0, 0..0],
        }
    }

    pub fn resize(&self, queue: &wgpu::Queue, width: u32, height: u32) {
        let screen = [width.max(1) as f32, height.max(1) as f32, 0.0, 0.0];
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&screen));
    }

    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, shapes: &ShapeLayers) {
        self.instances.clear();
        self.layers = [
            append_instances(&shapes.underlay, &mut self.instances),
            append_instances(&shapes.chrome, &mut self.instances),
            append_instances(&shapes.overlay, &mut self.instances),
        ];
        let len = self.instances.len();
        if len == 0 {
            return;
        }
        if self
            .buffer
            .as_ref()
            .is_none_or(|(_, capacity)| *capacity < len)
        {
            let capacity = len.next_power_of_two().max(64);
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("UI shape instances"),
                size: (capacity * size_of::<ShapeInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.buffer = Some((buffer, capacity));
        }
        if let Some((buffer, _)) = &self.buffer {
            queue.write_buffer(buffer, 0, bytemuck::cast_slice(&self.instances));
        }
    }

    pub fn render_underlay<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        self.render_layers(pass, &self.layers[..1]);
    }

    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        self.render_layers(pass, &self.layers[1..]);
    }

    fn render_layers<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, layers: &[Range<u32>]) {
        let Some((buffer, _)) = &self.buffer else {
            return;
        };
        for layer in layers.iter().filter(|layer| !layer.is_empty()) {
            let stride = size_of::<ShapeInstance>() as u64;
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            // Slice the buffer instead of using first_instance, which downlevel GL lacks.
            pass.set_vertex_buffer(
                0,
                buffer.slice(u64::from(layer.start) * stride..u64::from(layer.end) * stride),
            );
            pass.draw(0..6, 0..layer.end - layer.start);
        }
    }
}
