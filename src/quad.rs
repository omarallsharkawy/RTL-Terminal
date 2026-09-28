use wgpu::util::DeviceExt;

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
    vertex_count: usize,
}

impl QuadRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad shader"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
struct Uniforms {
    screen_size: vec2<f32>,
    padding: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(model: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let ndc_x = (model.position.x / uniforms.screen_size.x) * 2.0 - 1.0;
    let ndc_y = 1.0 - (model.position.y / uniforms.screen_size.y) * 2.0;
    out.clip_position = vec4<f32>(ndc_x, ndc_y, 0.0, 1.0);
    out.color = model.color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}
"#
                .into(),
            ),
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad uniform buffer"),
            size: std::mem::size_of::<ScreenUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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

        let pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
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
            vertex_count: 0,
        }
    }

    pub fn update_screen_size(&self, queue: &wgpu::Queue, width: f32, height: f32) {
        let uniform = ScreenUniform {
            screen_size: [width.max(1.0), height.max(1.0)],
            _padding: [0.0, 0.0],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniform));
    }

    pub fn set_rects(&mut self, device: &wgpu::Device, quads: &[(f32, f32, f32, f32, [f32; 4])]) {
        if quads.is_empty() {
            self.vertex_count = 0;
            return;
        }

        let mut vertices = Vec::with_capacity(quads.len() * 6);
        for &(x, y, w, h, color) in quads {
            let x2 = x + w;
            let y2 = y + h;
            vertices.push(QuadVertex { position: [x, y], color });
            vertices.push(QuadVertex { position: [x2, y], color });
            vertices.push(QuadVertex { position: [x, y2], color });

            vertices.push(QuadVertex { position: [x2, y], color });
            vertices.push(QuadVertex { position: [x2, y2], color });
            vertices.push(QuadVertex { position: [x, y2], color });
        }

        self.vertex_count = vertices.len();
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("quad vertex buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        self.vertex_buffer = Some(buffer);
    }

    pub fn render<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        if let Some(ref buffer) = self.vertex_buffer {
            if self.vertex_count > 0 {
                render_pass.set_pipeline(&self.pipeline);
                render_pass.set_bind_group(0, &self.bind_group, &[]);
                render_pass.set_vertex_buffer(0, buffer.slice(..));
                render_pass.draw(0..self.vertex_count as u32, 0..1);
            }
        }
    }
}

/// Direct geometric rendering of box-drawing and block elements.
/// Avoids font margin gaps and ensures seamless borders.
pub fn try_render_box_or_block(
    c: char,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
    quads: &mut Vec<(f32, f32, f32, f32, [f32; 4])>,
) -> bool {
    let line_w = 1.5;
    let mid_x = x + (w - line_w) * 0.5;
    let mid_y = y + (h - line_w) * 0.5;

    match c {
        // Full & partial blocks
        '█' => {
            quads.push((x, y, w, h, color));
            true
        }
        '▀' => {
            quads.push((x, y, w, h * 0.5, color));
            true
        }
        '▄' => {
            quads.push((x, y + h * 0.5, w, h * 0.5, color));
            true
        }
        '▌' => {
            quads.push((x, y, w * 0.5, h, color));
            true
        }
        '▐' => {
            quads.push((x + w * 0.5, y, w * 0.5, h, color));
            true
        }
        // Vertical line
        '│' | '┃' => {
            quads.push((mid_x, y, line_w, h, color));
            true
        }
        // Horizontal line
        '─' | '━' => {
            quads.push((x, mid_y, w, line_w, color));
            true
        }
        // Corners
        '┌' | '╭' => {
            quads.push((mid_x, mid_y, line_w, h * 0.5, color));
            quads.push((mid_x, mid_y, w * 0.5, line_w, color));
            true
        }
        '┐' | '╮' => {
            quads.push((mid_x, mid_y, line_w, h * 0.5, color));
            quads.push((x, mid_y, w * 0.5 + line_w, line_w, color));
            true
        }
        '└' | '╰' => {
            quads.push((mid_x, y, line_w, h * 0.5 + line_w, color));
            quads.push((mid_x, mid_y, w * 0.5, line_w, color));
            true
        }
        '┘' | '╯' => {
            quads.push((mid_x, y, line_w, h * 0.5 + line_w, color));
            quads.push((x, mid_y, w * 0.5 + line_w, line_w, color));
            true
        }
        // T-junctions
        '├' => {
            quads.push((mid_x, y, line_w, h, color));
            quads.push((mid_x, mid_y, w * 0.5, line_w, color));
            true
        }
        '┤' => {
            quads.push((mid_x, y, line_w, h, color));
            quads.push((x, mid_y, w * 0.5 + line_w, line_w, color));
            true
        }
        '┬' => {
            quads.push((x, mid_y, w, line_w, color));
            quads.push((mid_x, mid_y, line_w, h * 0.5, color));
            true
        }
        '┴' => {
            quads.push((x, mid_y, w, line_w, color));
            quads.push((mid_x, y, line_w, h * 0.5 + line_w, color));
            true
        }
        '┼' => {
            quads.push((mid_x, y, line_w, h, color));
            quads.push((x, mid_y, w, line_w, color));
            true
        }
        _ => false,
    }
}
