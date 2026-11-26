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
    Refresh,
    ChevronRight,
    ChevronDown,
    Copy,
    CopyCommand,
    Rerun,
    Zoom,
    Composer,
    Project,
    Layout,
    Broadcast,
}

impl Icon {
    /// Pictograms for feature commands that appear in the palette.
    pub fn for_command(name: &str) -> Option<Self> {
        match name {
            "composer.open" => Some(Self::Composer),
            "project.open" => Some(Self::Project),
            "layout.save" | "layout.open" | "layout.delete" => Some(Self::Layout),
            "input.broadcast" => Some(Self::Broadcast),
            _ => None,
        }
    }

    /// Append primitives in drawing order. `color` comes from `DrawContext::color`.
    pub fn draw(self, rect: Rect, color: [f32; 4], out: &mut Vec<Shape>) {
        let side = rect.w.min(rect.h).max(0.0);
        if side == 0.0 {
            return;
        }
        let unit = side / 16.0;
        // Snap the optical center to a half pixel. Preserve symmetry across fractional DPI,
        // and use the same physical stroke for capsules and outlines.
        let origin = |start: f32, length: f32| (2.0 * start + length).round() * 0.5 - side * 0.5;
        let icon = IconGeometry {
            origin: [origin(rect.x, rect.w), origin(rect.y, rect.h)],
            unit,
            width: (1.5 * unit * 4.0).round().max(1.0) / 4.0,
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
                icon.outline(out, [2.0, 2.0, 10.0, 10.0], 5.0);
                icon.stroke(out, [10.5, 10.5], [13.5, 13.5]);
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
                icon.stroke(out, [4.0, 4.0], [12.0, 12.0]);
                icon.stroke(out, [4.0, 12.0], [12.0, 4.0]);
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
            Self::Refresh => {
                line(
                    out,
                    &[
                        [12.9, 6.8],
                        [11.6, 4.4],
                        [9.1, 3.2],
                        [6.4, 3.4],
                        [4.2, 5.1],
                        [3.2, 7.6],
                        [3.8, 10.3],
                        [5.8, 12.3],
                        [8.5, 12.9],
                        [11.0, 12.0],
                        [12.6, 9.9],
                    ],
                );
                line(out, &[[10.1, 6.9], [13.0, 6.9], [13.0, 4.0]]);
            }
            Self::ChevronRight => line(out, &[[5.5, 3.8], [10.5, 8.0], [5.5, 12.2]]),
            Self::ChevronDown => line(out, &[[3.8, 5.5], [8.0, 10.5], [12.2, 5.5]]),
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
            Self::Composer => {
                icon.outline(out, [2.0, 3.0, 12.0, 10.0], 2.0);
                line(out, &[[4.8, 6.0], [7.0, 8.0], [4.8, 10.0]]);
                icon.stroke(out, [9.5, 6.0], [9.5, 10.0]);
            }
            Self::Project => {
                line(
                    out,
                    &[
                        [2.5, 12.8],
                        [2.5, 3.8],
                        [6.4, 3.8],
                        [8.0, 5.3],
                        [13.5, 5.3],
                        [13.5, 12.8],
                        [2.5, 12.8],
                    ],
                );
                icon.stroke(out, [5.4, 8.4], [10.6, 8.4]);
            }
            Self::Layout => {
                icon.outline(out, [2.0, 2.5, 12.0, 11.0], 2.0);
                icon.stroke(out, [6.0, 3.3], [6.0, 12.7]);
                icon.stroke(out, [6.0, 8.0], [13.2, 8.0]);
            }
            Self::Broadcast => {
                icon.stroke(out, [8.0, 8.0], [8.0, 12.5]);
                for direction in [-1.0, 1.0] {
                    let x = |distance: f32| 8.0 + direction * distance;
                    line(out, &[[x(2.4), 5.3], [x(3.1), 6.8], [x(2.4), 8.3]]);
                    line(
                        out,
                        &[
                            [x(4.4), 3.6],
                            [x(5.5), 5.0],
                            [x(5.8), 6.8],
                            [x(5.5), 8.6],
                            [x(4.4), 10.0],
                        ],
                    );
                }
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
    width: f32,
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
            width: self.width,
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
                width: self.width,
                color: self.color,
            }),
            shadow: None,
        });
    }
}
