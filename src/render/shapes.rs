//! UI geometry in physical pixels. Use `UiStyle::px` to scale logical dimensions.
//! Colors are straight alpha in the render target's color space (`DrawContext::color`).

use crate::layout::Rect;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Border {
    pub width: f32,
    pub color: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    /// Distance over which the shadow fades out, in physical pixels.
    pub blur: f32,
    pub offset: [f32; 2],
    pub color: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    RoundedRect {
        rect: Rect,
        /// Clockwise from the upper left. Each radius is clamped to half the shorter side.
        radii: [f32; 4],
        fill: [f32; 4],
        border: Option<Border>,
        shadow: Option<Shadow>,
    },
    Capsule {
        from: [f32; 2],
        to: [f32; 2],
        width: f32,
        color: [f32; 4],
    },
}

impl Shape {
    pub fn rounded(rect: Rect, radius: f32, fill: [f32; 4]) -> Self {
        Self::RoundedRect {
            rect,
            radii: [radius; 4],
            fill,
            border: None,
            shadow: None,
        }
    }

    /// Raster bounds including a one-pixel antialias fringe and the offset shadow.
    pub fn bounds(self) -> Option<Rect> {
        Some(self.instance()?.bounds_rect())
    }

    fn instance(self) -> Option<ShapeInstance> {
        let (bounds, geometry, radii, fill, border, shadow, effects) = match self {
            Self::RoundedRect {
                rect,
                radii,
                fill,
                border,
                shadow,
            } => {
                if rect.w <= 0.0 || rect.h <= 0.0 {
                    return None;
                }
                let half = rect.w.min(rect.h) / 2.0;
                let border_color = border.map_or([0.0; 4], |b| b.color);
                let edge = border.map_or(0.0, |b| b.width.clamp(0.0, half));
                let (shadow_color, offset, blur) = shadow
                    .filter(|s| s.color[3] > 0.0)
                    .map_or(([0.0; 4], [0.0; 2], 0.0), |s| {
                        (s.color, s.offset, s.blur.max(0.0))
                    });
                let [dx, dy] = offset;
                let bounds = [
                    rect.x + (dx - blur).min(0.0) - 1.0,
                    rect.y + (dy - blur).min(0.0) - 1.0,
                    rect.right() + (dx + blur).max(0.0) + 1.0,
                    rect.bottom() + (dy + blur).max(0.0) + 1.0,
                ];
                (
                    bounds,
                    [
                        rect.x + rect.w / 2.0,
                        rect.y + rect.h / 2.0,
                        rect.w / 2.0,
                        rect.h / 2.0,
                    ],
                    radii.map(|r| r.clamp(0.0, half)),
                    fill,
                    border_color,
                    shadow_color,
                    [edge, blur, dx, dy],
                )
            }
            Self::Capsule {
                from,
                to,
                width,
                color,
            } => {
                if width <= 0.0 {
                    return None;
                }
                let padding = width / 2.0 + 1.0;
                let bounds = [
                    from[0].min(to[0]) - padding,
                    from[1].min(to[1]) - padding,
                    from[0].max(to[0]) + padding,
                    from[1].max(to[1]) + padding,
                ];
                (
                    bounds,
                    [from[0], from[1], to[0], to[1]],
                    [width / 2.0, 0.0, 0.0, 0.0],
                    color,
                    [0.0; 4],
                    [0.0; 4],
                    [-1.0, 0.0, 0.0, 0.0], // Negative border width tags capsules.
                )
            }
        };
        Some(ShapeInstance {
            bounds,
            geometry,
            radii,
            fill,
            border,
            shadow,
            effects,
        })
    }
}

/// Uploaded once per shape. The shader expands each instance to a six-vertex rectangle.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ShapeInstance {
    bounds: [f32; 4],
    geometry: [f32; 4],
    radii: [f32; 4],
    fill: [f32; 4],
    border: [f32; 4],
    shadow: [f32; 4],
    effects: [f32; 4],
}

impl ShapeInstance {
    pub fn bounds_rect(&self) -> Rect {
        let [x, y, right, bottom] = self.bounds;
        Rect::new(x, y, right - x, bottom - y)
    }
}

/// Append instances in painter order. A layer is uploaded and drawn as a contiguous range.
pub fn append_instances(shapes: &[Shape], out: &mut Vec<ShapeInstance>) -> std::ops::Range<u32> {
    let start = out.len() as u32;
    out.extend(shapes.iter().filter_map(|shape| shape.instance()));
    start..out.len() as u32
}

/// Back to front: `underlay` draws behind plain quads (command block cards), `chrome` and
/// `overlay` draw above plain quads. All glyphon text is drawn after every shape layer.
#[derive(Default)]
pub struct ShapeLayers {
    pub underlay: Vec<Shape>,
    pub chrome: Vec<Shape>,
    pub overlay: Vec<Shape>,
}
