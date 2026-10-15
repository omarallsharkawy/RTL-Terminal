//! The GPU pipeline that fills solid-colored rectangles (backgrounds, box drawing, cursors,
//! chrome). The quads themselves are built on the CPU by `render::scene`.

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct QuadVertex {
    pub position: [f32; 2],
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct ScreenUniform {
    screen_size: [f32; 2],
    _padding: [f32; 2],
}

pub struct QuadRenderer {
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    vertex_buffer: Option<wgpu::Buffer>,
    vertex_capacity: usize,
    vertex_count: usize,
    /// CPU-side vertices, kept between frames so they are not reallocated every frame.
    vertices: Vec<QuadVertex>,
}

impl QuadRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("quad.wgsl").into()),
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad uniform buffer"),
            size: std::mem::size_of::<ScreenUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("quad bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("quad bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quad pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quad render pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<QuadVertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: std::mem::size_of::<[f32; 2]>() as wgpu::BufferAddress,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x4,
                        },
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            pipeline,
            uniform_buffer,
            bind_group,
            vertex_buffer: None,
            vertex_capacity: 0,
            vertex_count: 0,
            vertices: Vec::new(),
        }
    }

    pub fn update_screen_size(&self, queue: &wgpu::Queue, width: f32, height: f32) {
        let uniform = ScreenUniform {
            screen_size: [width.max(1.0), height.max(1.0)],
            _padding: [0.0, 0.0],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniform));
    }

    pub fn set_rects(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        quads: &[(f32, f32, f32, f32, [f32; 4])],
    ) {
        if quads.is_empty() {
            self.vertex_count = 0;
            return;
        }

        let vertices = &mut self.vertices;
        vertices.clear();
        vertices.reserve(quads.len() * 6);
        for &(x, y, w, h, color) in quads {
            let x2 = x + w;
            let y2 = y + h;
            vertices.push(QuadVertex {
                position: [x, y],
                color,
            });
            vertices.push(QuadVertex {
                position: [x2, y],
                color,
            });
            vertices.push(QuadVertex {
                position: [x, y2],
                color,
            });

            vertices.push(QuadVertex {
                position: [x2, y],
                color,
            });
            vertices.push(QuadVertex {
                position: [x2, y2],
                color,
            });
            vertices.push(QuadVertex {
                position: [x, y2],
                color,
            });
        }

        self.vertex_count = vertices.len();
        // Reuse the GPU buffer across frames; grow it geometrically only when needed.
        if self.vertex_buffer.is_none() || self.vertex_capacity < vertices.len() {
            let capacity = vertices.len().next_power_of_two().max(1024);
            self.vertex_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("quad vertex buffer"),
                size: (capacity * std::mem::size_of::<QuadVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.vertex_capacity = capacity;
        }
        if let Some(ref buffer) = self.vertex_buffer {
            queue.write_buffer(buffer, 0, bytemuck::cast_slice(vertices));
        }
    }

    pub fn render<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        if let Some(ref buffer) = self.vertex_buffer {
            if self.vertex_count > 0 {
                render_pass.set_pipeline(&self.pipeline);
                render_pass.set_bind_group(0, &self.bind_group, &[]);
                render_pass.set_vertex_buffer(
                    0,
                    buffer.slice(..(self.vertex_count * std::mem::size_of::<QuadVertex>()) as u64),
                );
                render_pass.draw(0..self.vertex_count as u32, 0..1);
            }
        }
    }
}
