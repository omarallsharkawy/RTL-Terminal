//! Outlined chrome icons drawn into a physical-pixel box. A 16 px reference grid scales
//! uniformly into the box, with room for the antialias fringe at the intended 12–20 px sizes.

use super::shapes::{Border, Shape};
use crate::layout::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Menu,
    Search,
    Bell,
    SplitRight,
    SplitDown,
    Close,
    GitBranch,
    Folder,
    Plus,
    Check,
    Warning,
    Terminal,
    Agent,
    Diff,
    Copy,
    CopyCommand,
    Rerun,
    Zoom,
}

impl Icon {
    /// Append primitives in drawing order. `color` comes from `DrawContext::color`.
    pub fn draw(self, rect: Rect, color: [f32; 4], out: &mut Vec<Shape>) {
        let side = rect.w.min(rect.h).max(0.0);
        if side == 0.0 {
            return;
        }
        let icon = IconGeometry {
            origin: [
                rect.x + (rect.w - side) / 2.0,
                rect.y + (rect.h - side) / 2.0,
            ],
            unit: side / 16.0,
            color,
        };
        let line = |out: &mut Vec<Shape>, points: &[[f32; 2]]| {
            for pair in points.windows(2) {
                icon.stroke(out, pair[0], pair[1]);
            }
        };
        match self {
            Self::Menu => {
                for y in [4.0, 8.0, 12.0] {
                    icon.stroke(out, [3.0, y], [13.0, y]);
                }
            }
            Self::Search => {
                icon.outline(out, [2.5, 2.5, 9.5, 9.5], 5.0);
                icon.stroke(out, [10.8, 10.8], [13.6, 13.6]);
            }
            Self::Bell => {
                line(
                    out,
                    &[
                        [3.1, 10.8],
                        [4.4, 9.0],
                        [4.7, 5.7],
                        [5.4, 4.2],
                        [6.6, 3.3],
                        [9.4, 3.3],
                        [10.6, 4.2],
                        [11.3, 5.7],
                        [11.6, 9.0],
                        [12.9, 10.8],
                        [3.1, 10.8],
                    ],
                );
                icon.stroke(out, [7.0, 13.4], [9.0, 13.4]);
            }
            Self::SplitRight | Self::SplitDown => {
                icon.outline(out, [2.4, 3.0, 11.2, 10.0], 1.8);
                match self {
                    Self::SplitRight => icon.stroke(out, [8.0, 3.8], [8.0, 12.2]),
                    _ => icon.stroke(out, [3.2, 8.0], [12.8, 8.0]),
                }
            }
            Self::Close => {
                icon.stroke(out, [4.3, 4.3], [11.7, 11.7]);
                icon.stroke(out, [4.3, 11.7], [11.7, 4.3]);
            }
            Self::GitBranch => {
                for p in [[4.6, 4.0], [4.6, 12.0], [11.6, 4.0]] {
                    icon.outline(out, [p[0] - 1.6, p[1] - 1.6, 3.2, 3.2], 1.6);
                }
                icon.stroke(out, [4.6, 5.6], [4.6, 10.4]);
                line(
                    out,
                    &[[11.6, 5.6], [11.0, 7.1], [9.2, 8.0], [6.5, 8.0], [4.6, 9.2]],
                );
            }
            Self::Folder => {
                line(
                    out,
                    &[
                        [2.5, 12.3],
                        [2.5, 4.0],
                        [6.7, 4.0],
                        [8.1, 5.5],
                        [13.5, 5.5],
                        [13.5, 12.3],
                        [2.5, 12.3],
                    ],
                );
            }
            Self::Plus => {
                icon.stroke(out, [4.0, 8.0], [12.0, 8.0]);
                icon.stroke(out, [8.0, 4.0], [8.0, 12.0]);
            }
            Self::Check => line(out, &[[3.5, 8.3], [6.7, 11.2], [12.5, 4.8]]),
            Self::Warning => {
                line(out, &[[8.0, 2.6], [13.4, 12.9], [2.6, 12.9], [8.0, 2.6]]);
                icon.stroke(out, [8.0, 6.5], [8.0, 8.8]);
                icon.stroke(out, [8.0, 10.8], [8.0, 10.8]);
            }
            Self::Terminal => {
                icon.outline(out, [2.4, 3.0, 11.2, 10.0], 1.8);
                line(out, &[[5.0, 6.4], [6.8, 8.0], [5.0, 9.6]]);
                icon.stroke(out, [8.7, 9.6], [11.0, 9.6]);
            }
            Self::Agent => line(
                out,
                &[
                    [8.0, 2.5],
                    [9.7, 6.3],
                    [13.5, 8.0],
                    [9.7, 9.7],
                    [8.0, 13.5],
                    [6.3, 9.7],
                    [2.5, 8.0],
                    [6.3, 6.3],
                    [8.0, 2.5],
                ],
            ),
            Self::Diff => {
                icon.stroke(out, [3.9, 5.8], [12.1, 5.8]);
                icon.stroke(out, [8.0, 2.9], [8.0, 8.7]);
                icon.stroke(out, [3.9, 12.5], [12.1, 12.5]);
            }
            Self::Copy => {
                icon.outline(out, [5.4, 2.6, 8.0, 8.4], 1.5);
                icon.outline(out, [2.6, 5.0, 8.0, 8.4], 1.5);
            }
            Self::CopyCommand => {
                icon.outline(out, [2.5, 3.0, 11.0, 10.0], 1.8);
                line(out, &[[5.0, 6.2], [7.2, 8.0], [5.0, 9.8]]);
                icon.stroke(out, [9.0, 6.6], [11.2, 6.6]);
                icon.stroke(out, [9.0, 9.4], [11.2, 9.4]);
            }
            Self::Rerun => {
                line(
                    out,
                    &[
                        [4.3, 5.9],
                        [5.6, 4.0],
                        [8.0, 3.3],
                        [10.7, 4.2],
                        [12.4, 6.5],
                        [12.4, 9.5],
                        [10.7, 11.8],
                        [8.0, 12.7],
                        [5.4, 11.9],
                        [4.1, 10.1],
                    ],
                );
                line(out, &[[3.3, 3.8], [4.3, 5.9], [6.6, 5.4]]);
            }
            Self::Zoom => {
                line(out, &[[9.2, 3.5], [12.5, 3.5], [12.5, 6.8]]);
                icon.stroke(out, [12.1, 3.9], [9.2, 6.8]);
                line(out, &[[3.5, 9.2], [3.5, 12.5], [6.8, 12.5]]);
                icon.stroke(out, [3.9, 12.1], [6.8, 9.2]);
            }
        }
    }
}

struct IconGeometry {
    origin: [f32; 2],
    unit: f32,
    color: [f32; 4],
}

impl IconGeometry {
    fn point(&self, [x, y]: [f32; 2]) -> [f32; 2] {
        [
            self.origin[0] + x * self.unit,
            self.origin[1] + y * self.unit,
        ]
    }

    fn stroke(&self, out: &mut Vec<Shape>, from: [f32; 2], to: [f32; 2]) {
        out.push(Shape::Capsule {
            from: self.point(from),
            to: self.point(to),
            width: 1.45 * self.unit,
            color: self.color,
        });
    }

    fn outline(&self, out: &mut Vec<Shape>, [x, y, w, h]: [f32; 4], radius: f32) {
        let [x, y] = self.point([x, y]);
        out.push(Shape::RoundedRect {
            rect: Rect::new(x, y, w * self.unit, h * self.unit),
            radii: [radius * self.unit; 4],
            fill: [0.0; 4],
            border: Some(Border {
                width: 1.4 * self.unit,
                color: self.color,
            }),
            shadow: None,
        });
    }
}
