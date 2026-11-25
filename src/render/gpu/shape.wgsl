@group(0) @binding(0) var<uniform> screen: vec4<f32>;

struct Instance {
    @location(0) bounds: vec4<f32>,
    @location(1) geometry: vec4<f32>,
    @location(2) radii: vec4<f32>,
    @location(3) fill: vec4<f32>,
    @location(4) border: vec4<f32>,
    @location(5) shadow: vec4<f32>,
    @location(6) effects: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) geometry: vec4<f32>,
    @location(1) @interpolate(flat) radii: vec4<f32>,
    @location(2) @interpolate(flat) fill: vec4<f32>,
    @location(3) @interpolate(flat) border: vec4<f32>,
    @location(4) @interpolate(flat) shadow: vec4<f32>,
    @location(5) @interpolate(flat) effects: vec4<f32>,
};

@vertex
fn vs_main(in: Instance, @builtin(vertex_index) vertex: u32) -> VertexOutput {
    let corners = array(vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
                        vec2(1.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0));
    let pixel = mix(in.bounds.xy, in.bounds.zw, corners[vertex]);
    let ndc = pixel / screen.xy * vec2(2.0, -2.0) + vec2(-1.0, 1.0);
    return VertexOutput(vec4(ndc, 0.0, 1.0), in.geometry, in.radii,
                        in.fill, in.border, in.shadow, in.effects);
}

fn rounded_distance(p: vec2<f32>, half_size: vec2<f32>, radii: vec4<f32>) -> f32 {
    let left = select(radii.x, radii.w, p.y >= 0.0);
    let right = select(radii.y, radii.z, p.y >= 0.0);
    let radius = select(left, right, p.x >= 0.0);
    let q = abs(p) - half_size + radius;
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2(0.0))) - radius;
}

fn coverage(distance: f32) -> f32 {
    return clamp(0.5 - distance, 0.0, 1.0);
}

fn premul(color: vec4<f32>, coverage: f32) -> vec4<f32> {
    let alpha = color.a * coverage;
    return vec4(color.rgb * alpha, alpha);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let p = in.clip.xy - in.geometry.xy;
    let edge = in.effects.x;
    if edge < 0.0 {
        let axis = in.geometry.zw - in.geometry.xy;
        let t = clamp(dot(p, axis) / max(dot(axis, axis), 0.000001), 0.0, 1.0);
        return premul(in.fill, coverage(length(p - axis * t) - in.radii.x));
    }
    let distance = rounded_distance(p, in.geometry.zw, in.radii);
    let outer = coverage(distance);
    let inner = coverage(distance + edge);
    let surface = premul(in.border, outer - inner) + premul(in.fill, inner);
    if in.shadow.a == 0.0 {
        return surface;
    }
    let d = rounded_distance(p - in.effects.zw, in.geometry.zw, in.radii);
    let blur = max(in.effects.y, 0.5);
    let shadow = premul(in.shadow, 1.0 - smoothstep(-blur, blur, d));
    return surface + shadow * (1.0 - surface.a);
}
