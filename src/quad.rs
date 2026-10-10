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

        let mut vertices = Vec::with_capacity(quads.len() * 6);
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
            queue.write_buffer(buffer, 0, bytemuck::cast_slice(&vertices));
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

/// Light stroke width: a tenth of the smaller cell side, rounded to whole pixels, at least 1px.
/// Heavy strokes are twice this width and double lines use it for each of their two lines.
fn stroke_width(w: f32, h: f32) -> f32 {
    (w.min(h) * 0.1).round().max(1.0)
}

/// Arm weights of a single or heavy line character, as [left, right, up, down].
/// 0 = no arm, 1 = light, 2 = heavy. Mixed light/heavy characters are not listed.
fn line_arms(c: char) -> Option<[u8; 4]> {
    Some(match c {
        '─' => [1, 1, 0, 0],
        '━' => [2, 2, 0, 0],
        '│' => [0, 0, 1, 1],
        '┃' => [0, 0, 2, 2],
        '┌' | '╭' => [0, 1, 0, 1],
        '┐' | '╮' => [1, 0, 0, 1],
        '└' | '╰' => [0, 1, 1, 0],
        '┘' | '╯' => [1, 0, 1, 0],
        '├' => [0, 1, 1, 1],
        '┤' => [1, 0, 1, 1],
        '┬' => [1, 1, 0, 1],
        '┴' => [1, 1, 1, 0],
        '┼' => [1, 1, 1, 1],
        '┏' => [0, 2, 0, 2],
        '┓' => [2, 0, 0, 2],
        '┗' => [0, 2, 2, 0],
        '┛' => [2, 0, 2, 0],
        '┣' => [0, 2, 2, 2],
        '┫' => [2, 0, 2, 2],
        '┳' => [2, 2, 0, 2],
        '┻' => [2, 2, 2, 0],
        '╋' => [2, 2, 2, 2],
        _ => return None,
    })
}

/// Dashed lines as (horizontal, segment count, heavy).
fn dash_spec(c: char) -> Option<(bool, usize, bool)> {
    Some(match c {
        '┄' => (true, 3, false),
        '┅' => (true, 3, true),
        '┈' => (true, 4, false),
        '┉' => (true, 4, true),
        '╌' => (true, 2, false),
        '╍' => (true, 2, true),
        '┆' => (false, 3, false),
        '┇' => (false, 3, true),
        '┊' => (false, 4, false),
        '┋' => (false, 4, true),
        '╎' => (false, 2, false),
        '╏' => (false, 2, true),
        _ => return None,
    })
}

/// Quadrant bits of a quadrant block: 1 = upper left, 2 = upper right, 4 = lower left, 8 = lower right.
fn quadrant_mask(c: char) -> u8 {
    match c {
        '▘' => 0b0001,
        '▝' => 0b0010,
        '▖' => 0b0100,
        '▗' => 0b1000,
        '▚' => 0b1001,
        '▞' => 0b0110,
        '▙' => 0b1101,
        '▛' => 0b0111,
        '▜' => 0b1011,
        '▟' => 0b1110,
        _ => 0,
    }
}

/// Direct geometric rendering of box-drawing and block elements.
/// Avoids font margin gaps and ensures seamless borders.
///
/// Light lines are `stroke_width` thick and centred in the cell on whole pixels, so
/// neighbouring cells join exactly. Double lines are two light lines with one light
/// line of gap between them, laid out on the grid `x0..x3` / `y0..y3` below.
pub fn try_render_box_or_block(
    c: char,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
    quads: &mut Vec<(f32, f32, f32, f32, [f32; 4])>,
) -> bool {
    let t = stroke_width(w, h);
    let lx = (x + (w - t) * 0.5).round();
    let ly = (y + (h - t) * 0.5).round();
    let (xr, yb) = (x + w, y + h);
    // Heavy bands start half a stroke earlier so they stay centred on the light position.
    let heavy_off = (t * 0.5).floor();

    // Double-line grid: outer/inner lines of the vertical (x) and horizontal (y) pairs.
    let (x0, x1, x2, x3) = (lx - t, lx, lx + t, lx + 2.0 * t);
    let (y0, y1, y2, y3) = (ly - t, ly, ly + t, ly + 2.0 * t);

    let mut rect = |x0: f32, y0: f32, x1: f32, y1: f32| {
        if x1 > x0 && y1 > y0 {
            quads.push((x0, y0, x1 - x0, y1 - y0, color));
        }
    };

    if let Some([l, r, u, d]) = line_arms(c) {
        let heavy = [l, r, u, d].contains(&2);
        let tt = if heavy { 2.0 * t } else { t };
        let bx = if heavy { lx - heavy_off } else { lx };
        let by = if heavy { ly - heavy_off } else { ly };
        // A straight run through the cell is one bar, a single arm runs from its edge to the centre.
        if l > 0 && r > 0 {
            rect(x, by, xr, by + tt);
        } else if r > 0 {
            rect(bx, by, xr, by + tt);
        } else if l > 0 {
            rect(x, by, bx + tt, by + tt);
        }
        if u > 0 && d > 0 {
            rect(bx, y, bx + tt, yb);
        } else if d > 0 {
            rect(bx, by, bx + tt, yb);
        } else if u > 0 {
            rect(bx, y, bx + tt, by + tt);
        }
        return true;
    }

    if let Some((horizontal, n, heavy)) = dash_spec(c) {
        let tt = if heavy { 2.0 * t } else { t };
        let bx = if heavy { lx - heavy_off } else { lx };
        let by = if heavy { ly - heavy_off } else { ly };
        let (start, len) = if horizontal { (x, w) } else { (y, h) };
        let slot = len / n as f32;
        // Each slot holds one dash; a light stroke of gap is split evenly on both sides.
        for i in 0..n {
            let a = (start + slot * i as f32 + t * 0.5).round();
            let b = (start + slot * (i + 1) as f32 - t * 0.5).round();
            if horizontal {
                rect(a, by, b, by + tt);
            } else {
                rect(bx, a, bx + tt, b);
            }
        }
        return true;
    }

    let mask = quadrant_mask(c);
    if mask > 0 {
        let (mx, my) = (x + w * 0.5, y + h * 0.5);
        if (mask & 0b0001) != 0 {
            rect(x, y, mx, my);
        }
        if (mask & 0b0010) != 0 {
            rect(mx, y, xr, my);
        }
        if (mask & 0b0100) != 0 {
            rect(x, my, mx, yb);
        }
        if (mask & 0b1000) != 0 {
            rect(mx, my, xr, yb);
        }
        return true;
    }

    match c {
        // Full & partial blocks
        '█' => {
            rect(x, y, xr, yb);
            true
        }
        '▀' => {
            rect(x, y, xr, y + h * 0.5);
            true
        }
        '▐' => {
            rect(x + w * 0.5, y, xr, yb);
            true
        }
        // Lower n/8 blocks, ▁ (n = 1) to ▇ (n = 7); ▄ is n = 4.
        '\u{2581}'..='\u{2587}' => {
            let n = (c as u32 - 0x2580) as f32;
            rect(x, yb - h * n / 8.0, xr, yb);
            true
        }
        // Left n/8 blocks, ▉ (n = 7) to ▏ (n = 1); ▌ is n = 4.
        '\u{2589}'..='\u{258F}' => {
            let n = (0x2590 - c as u32) as f32;
            rect(x, y, x + w * n / 8.0, yb);
            true
        }
        '▔' => {
            rect(x, y, xr, y + h / 8.0);
            true
        }
        '▕' => {
            rect(xr - w / 8.0, y, xr, yb);
            true
        }
        // Shades: the full cell at reduced alpha.
        '░' | '▒' | '▓' => {
            let f = match c {
                '░' => 0.25,
                '▒' => 0.5,
                _ => 0.75,
            };
            quads.push((x, y, w, h, [color[0], color[1], color[2], color[3] * f]));
            true
        }
        // Single lines: a straight run crosses the cell, a bar ends at the edges.
        // Double lines, pure and mixed with single lines.
        '═' => {
            rect(x, y0, xr, y1);
            rect(x, y2, xr, y3);
            true
        }
        '║' => {
            rect(x0, y, x1, yb);
            rect(x2, y, x3, yb);
            true
        }
        '╔' => {
            rect(x0, y0, xr, y1);
            rect(x0, y0, x1, yb);
            rect(x2, y2, xr, y3);
            rect(x2, y2, x3, yb);
            true
        }
        '╗' => {
            rect(x, y0, x3, y1);
            rect(x2, y0, x3, yb);
            rect(x, y2, x1, y3);
            rect(x0, y2, x1, yb);
            true
        }
        '╚' => {
            rect(x0, y2, xr, y3);
            rect(x0, y, x1, y3);
            rect(x2, y0, xr, y1);
            rect(x2, y, x3, y1);
            true
        }
        '╝' => {
            rect(x, y2, x3, y3);
            rect(x2, y, x3, y3);
            rect(x, y0, x1, y1);
            rect(x0, y, x1, y1);
            true
        }
        '╠' => {
            rect(x0, y, x1, yb);
            rect(x2, y, x3, y1);
            rect(x2, y2, x3, yb);
            rect(x3, y0, xr, y1);
            rect(x3, y2, xr, y3);
            true
        }
        '╣' => {
            rect(x2, y, x3, yb);
            rect(x0, y, x1, y1);
            rect(x0, y2, x1, yb);
            rect(x, y0, x0, y1);
            rect(x, y2, x0, y3);
            true
        }
        '╦' => {
            rect(x, y0, xr, y1);
            rect(x, y2, x1, y3);
            rect(x2, y2, xr, y3);
            rect(x0, y2, x1, yb);
            rect(x2, y2, x3, yb);
            true
        }
        '╩' => {
            rect(x, y2, xr, y3);
            rect(x, y0, x1, y1);
            rect(x2, y0, xr, y1);
            rect(x0, y, x1, y1);
            rect(x2, y, x3, y1);
            true
        }
        '╬' => {
            rect(x, y0, xr, y1);
            rect(x, y2, xr, y3);
            rect(x0, y, x1, yb);
            rect(x2, y, x3, yb);
            true
        }
        // Down single, right double.
        '╒' => {
            rect(x1, y0, xr, y1);
            rect(x2, y2, xr, y3);
            rect(x1, y0, x2, yb);
            true
        }
        // Down single, left double.
        '╕' => {
            rect(x, y0, x2, y1);
            rect(x, y2, x1, y3);
            rect(x1, y0, x2, yb);
            true
        }
        // Up single, right double.
        '╘' => {
            rect(x1, y2, xr, y3);
            rect(x2, y0, xr, y1);
            rect(x1, y, x2, y3);
            true
        }
        // Up single, left double.
        '╛' => {
            rect(x, y2, x2, y3);
            rect(x, y0, x1, y1);
            rect(x1, y, x2, y3);
            true
        }
        // Down double, right single.
        '╓' => {
            rect(x0, y1, x1, yb);
            rect(x2, y1, x3, yb);
            rect(x2, y1, xr, y2);
            true
        }
        // Down double, left single.
        '╖' => {
            rect(x0, y1, x1, yb);
            rect(x2, y1, x3, yb);
            rect(x, y1, x1, y2);
            true
        }
        // Up double, right single.
        '╙' => {
            rect(x0, y, x1, y2);
            rect(x2, y, x3, y2);
            rect(x2, y1, xr, y2);
            true
        }
        // Up double, left single.
        '╜' => {
            rect(x0, y, x1, y2);
            rect(x2, y, x3, y2);
            rect(x, y1, x1, y2);
            true
        }
        // Vertical single, right double.
        '╞' => {
            rect(x1, y, x2, yb);
            rect(x2, y0, xr, y1);
            rect(x2, y2, xr, y3);
            true
        }
        // Vertical single, left double.
        '╡' => {
            rect(x1, y, x2, yb);
            rect(x, y0, x1, y1);
            rect(x, y2, x1, y3);
            true
        }
        // Vertical double, right single.
        '╟' => {
            rect(x0, y, x1, yb);
            rect(x2, y, x3, yb);
            rect(x2, y1, xr, y2);
            true
        }
        // Vertical double, left single.
        '╢' => {
            rect(x0, y, x1, yb);
            rect(x2, y, x3, yb);
            rect(x, y1, x1, y2);
            true
        }
        // Down single, horizontal double.
        '╤' => {
            rect(x, y0, xr, y1);
            rect(x, y2, xr, y3);
            rect(x1, y2, x2, yb);
            true
        }
        // Down double, horizontal single.
        '╥' => {
            rect(x, y1, xr, y2);
            rect(x0, y1, x1, yb);
            rect(x2, y1, x3, yb);
            true
        }
        // Up single, horizontal double.
        '╧' => {
            rect(x, y0, xr, y1);
            rect(x, y2, xr, y3);
            rect(x1, y, x2, y1);
            true
        }
        // Up double, horizontal single.
        '╨' => {
            rect(x, y1, xr, y2);
            rect(x0, y, x1, y2);
            rect(x2, y, x3, y2);
            true
        }
        // Vertical single, horizontal double.
        '╪' => {
            rect(x, y0, xr, y1);
            rect(x, y2, xr, y3);
            rect(x1, y, x2, yb);
            true
        }
        // Vertical double, horizontal single.
        '╫' => {
            rect(x, y1, xr, y2);
            rect(x0, y, x1, yb);
            rect(x2, y, x3, yb);
            true
        }
        _ => false,
    }
}
