//! Apply a panel transition to every layer before assembling the frame.

use cosmic_text::{Attrs, AttrsList, AttrsOwned, Buffer, FontSystem};
use glyphon::{Color, TextBounds};

use twitty_core::layout::Rect;
use twitty_render::shapes::Shape;
use twitty_render::text::ui::UiLabel;

#[derive(Clone, Copy)]
pub(super) struct Slide {
    pub opacity: f32,
    pub x: f32,
    pub y: f32,
}

impl Slide {
    pub fn new(opacity: f32, distance: [f32; 2]) -> Self {
        Self {
            opacity,
            x: (distance[0] * (1.0 - opacity)).round(),
            y: (distance[1] * (1.0 - opacity)).round(),
        }
    }

    pub fn rect(self, r: Rect) -> Rect {
        Rect::new(r.x + self.x, r.y + self.y, r.w, r.h)
    }

    fn bounds(self, b: TextBounds) -> TextBounds {
        let (x, y) = (self.x as i32, self.y as i32);
        TextBounds {
            left: b.left + x,
            right: b.right + x,
            top: b.top + y,
            bottom: b.bottom + y,
        }
    }

    fn color(self, c: Color) -> Color {
        Color::rgba(
            c.r(),
            c.g(),
            c.b(),
            (f32::from(c.a()) * self.opacity).round() as u8,
        )
    }

    pub fn shape(self, shape: &mut Shape) {
        match shape {
            Shape::RoundedRect {
                rect,
                fill,
                border,
                shadow,
                ..
            } => {
                *rect = self.rect(*rect);
                fill[3] *= self.opacity;
                if let Some(border) = border {
                    border.color[3] *= self.opacity;
                }
                if let Some(shadow) = shadow {
                    shadow.color[3] *= self.opacity;
                }
            }
            Shape::Capsule {
                from, to, color, ..
            } => {
                for point in [from, to] {
                    point[0] += self.x;
                    point[1] += self.y;
                }
                color[3] *= self.opacity;
            }
        }
    }

    pub fn label(self, label: &mut UiLabel) {
        self.text_position(&mut label.left, &mut label.top, &mut label.bounds);
        label.color = self.color(label.color);
    }

    pub fn text_position(self, left: &mut f32, top: &mut f32, bounds: &mut TextBounds) {
        *left += self.x;
        *top += self.y;
        *bounds = self.bounds(*bounds);
    }

    /// Diff and editor rows carry per-glyph colors, which override glyphon's default ink.
    pub fn buffer(self, buffer: &mut Buffer, fonts: &mut FontSystem) {
        let fade = |mut attrs: Attrs<'_>| {
            attrs.color_opt = attrs.color_opt.map(|c| self.color(c));
            AttrsOwned::new(&attrs)
        };
        for line in &mut buffer.lines {
            let original = line.attrs_list();
            let mut attrs = AttrsList::new(&fade(original.defaults()).as_attrs());
            for (range, span) in original.spans_iter() {
                attrs.add_span(range.clone(), &fade(span.as_attrs()).as_attrs());
            }
            line.set_attrs_list(attrs);
        }
        buffer.shape_until_scroll(fonts, false);
    }
}
