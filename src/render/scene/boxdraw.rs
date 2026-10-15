//! Pixel-exact box drawing and block elements, as quads instead of font glyphs.

use crate::render::style::Quad;

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

/// Stops of the double-line grid along one axis: the cell edges and the edges of the two
/// light lines a double line is made of (`P0..P1` and `P2..P3`, with `P1..P2` open).
#[derive(Clone, Copy)]
enum G {
    /// The cell's left / top edge.
    Start,
    P0,
    P1,
    P2,
    P3,
    /// The cell's right / bottom edge.
    End,
}

/// A rectangle on the double-line grid: `[x0, y0, x1, y1]`.
type GridRect = [G; 4];

/// Rectangles of the double-line characters (pure, and mixed with single lines).
/// A single line in a mixed character runs on the light position, `P1..P2`.
fn double_line_rects(c: char) -> Option<&'static [GridRect]> {
    use G::*;
    Some(match c {
        '═' => &[[Start, P0, End, P1], [Start, P2, End, P3]],
        '║' => &[[P0, Start, P1, End], [P2, Start, P3, End]],
        '╔' => &[
            [P0, P0, End, P1],
            [P0, P0, P1, End],
            [P2, P2, End, P3],
            [P2, P2, P3, End],
        ],
        '╗' => &[
            [Start, P0, P3, P1],
            [P2, P0, P3, End],
            [Start, P2, P1, P3],
            [P0, P2, P1, End],
        ],
        '╚' => &[
            [P0, P2, End, P3],
            [P0, Start, P1, P3],
            [P2, P0, End, P1],
            [P2, Start, P3, P1],
        ],
        '╝' => &[
            [Start, P2, P3, P3],
            [P2, Start, P3, P3],
            [Start, P0, P1, P1],
            [P0, Start, P1, P1],
        ],
        '╠' => &[
            [P0, Start, P1, End],
            [P2, Start, P3, P1],
            [P2, P2, P3, End],
            [P3, P0, End, P1],
            [P3, P2, End, P3],
        ],
        '╣' => &[
            [P2, Start, P3, End],
            [P0, Start, P1, P1],
            [P0, P2, P1, End],
            [Start, P0, P0, P1],
            [Start, P2, P0, P3],
        ],
        '╦' => &[
            [Start, P0, End, P1],
            [Start, P2, P1, P3],
            [P2, P2, End, P3],
            [P0, P2, P1, End],
            [P2, P2, P3, End],
        ],
        '╩' => &[
            [Start, P2, End, P3],
            [Start, P0, P1, P1],
            [P2, P0, End, P1],
            [P0, Start, P1, P1],
            [P2, Start, P3, P1],
        ],
        // Four corners: the channels between the double lines stay open.
        '╬' => &[
            [Start, P0, P1, P1],
            [P0, Start, P1, P0],
            [P2, P0, End, P1],
            [P2, Start, P3, P0],
            [Start, P2, P1, P3],
            [P0, P3, P1, End],
            [P2, P2, End, P3],
            [P2, P3, P3, End],
        ],
        // Down single, right double.
        '╒' => &[[P1, P0, End, P1], [P2, P2, End, P3], [P1, P0, P2, End]],
        // Down single, left double.
        '╕' => &[[Start, P0, P2, P1], [Start, P2, P1, P3], [P1, P0, P2, End]],
        // Up single, right double.
        '╘' => &[[P1, P2, End, P3], [P2, P0, End, P1], [P1, Start, P2, P3]],
        // Up single, left double.
        '╛' => &[
            [Start, P2, P2, P3],
            [Start, P0, P1, P1],
            [P1, Start, P2, P3],
        ],
        // Down double, right single.
        '╓' => &[[P0, P1, P1, End], [P2, P1, P3, End], [P0, P1, End, P2]],
        // Down double, left single.
        '╖' => &[[P0, P1, P1, End], [P2, P1, P3, End], [Start, P1, P3, P2]],
        // Up double, right single.
        '╙' => &[[P0, Start, P1, P2], [P2, Start, P3, P2], [P0, P1, End, P2]],
        // Up double, left single.
        '╜' => &[
            [P0, Start, P1, P2],
            [P2, Start, P3, P2],
            [Start, P1, P3, P2],
        ],
        // Vertical single, right double.
        '╞' => &[[P1, Start, P2, End], [P2, P0, End, P1], [P2, P2, End, P3]],
        // Vertical single, left double.
        '╡' => &[
            [P1, Start, P2, End],
            [Start, P0, P1, P1],
            [Start, P2, P1, P3],
        ],
        // Vertical double, right single.
        '╟' => &[
            [P0, Start, P1, End],
            [P2, Start, P3, End],
            [P2, P1, End, P2],
        ],
        // Vertical double, left single.
        '╢' => &[
            [P0, Start, P1, End],
            [P2, Start, P3, End],
            [Start, P1, P1, P2],
        ],
        // Down single, horizontal double.
        '╤' => &[
            [Start, P0, End, P1],
            [Start, P2, End, P3],
            [P1, P2, P2, End],
        ],
        // Down double, horizontal single.
        '╥' => &[[Start, P1, End, P2], [P0, P1, P1, End], [P2, P1, P3, End]],
        // Up single, horizontal double.
        '╧' => &[
            [Start, P0, End, P1],
            [Start, P2, End, P3],
            [P1, Start, P2, P1],
        ],
        // Up double, horizontal single.
        '╨' => &[
            [Start, P1, End, P2],
            [P0, Start, P1, P2],
            [P2, Start, P3, P2],
        ],
        // Vertical single, horizontal double.
        '╪' => &[
            [Start, P0, End, P1],
            [Start, P2, End, P3],
            [P1, Start, P2, End],
        ],
        // Vertical double, horizontal single.
        '╫' => &[
            [Start, P1, End, P2],
            [P0, Start, P1, End],
            [P2, Start, P3, End],
        ],
        _ => return None,
    })
}

/// Where a cell's lines go, in pixels, and the quads they become.
struct Cell<'a> {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    /// Right and bottom edges.
    xr: f32,
    yb: f32,
    /// Light stroke width.
    t: f32,
    /// Top-left of a light line centred in the cell, on whole pixels.
    lx: f32,
    ly: f32,
    color: [f32; 4],
    quads: &'a mut Vec<Quad>,
}

impl Cell<'_> {
    /// Push the rectangle `x0..x1` x `y0..y1`, unless it is empty.
    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        if x1 > x0 && y1 > y0 {
            self.quads.push((x0, y0, x1 - x0, y1 - y0, self.color));
        }
    }

    /// Origin and thickness of a light or heavy line. Heavy bands start half a stroke earlier
    /// so they stay centred on the light position.
    fn band(&self, heavy: bool) -> (f32, f32, f32) {
        if heavy {
            let off = (self.t * 0.5).floor();
            (self.lx - off, self.ly - off, 2.0 * self.t)
        } else {
            (self.lx, self.ly, self.t)
        }
    }

    /// The pixel position of a double-line grid stop, along x or y.
    fn stop(&self, g: G, horizontal: bool) -> f32 {
        let (start, light, end) = if horizontal {
            (self.x, self.lx, self.xr)
        } else {
            (self.y, self.ly, self.yb)
        };
        match g {
            G::Start => start,
            G::P0 => light - self.t,
            G::P1 => light,
            G::P2 => light + self.t,
            G::P3 => light + 2.0 * self.t,
            G::End => end,
        }
    }

    /// Single or heavy lines with arms `[left, right, up, down]`.
    fn line_arms(&mut self, [l, r, u, d]: [u8; 4]) {
        let (bx, by, tt) = self.band([l, r, u, d].contains(&2));
        let (x, y, xr, yb) = (self.x, self.y, self.xr, self.yb);
        // A straight run through the cell is one bar, a single arm runs from its edge to the centre.
        if l > 0 && r > 0 {
            self.rect(x, by, xr, by + tt);
        } else if r > 0 {
            self.rect(bx, by, xr, by + tt);
        } else if l > 0 {
            self.rect(x, by, bx + tt, by + tt);
        }
        if u > 0 && d > 0 {
            self.rect(bx, y, bx + tt, yb);
        } else if d > 0 {
            self.rect(bx, by, bx + tt, yb);
        } else if u > 0 {
            self.rect(bx, y, bx + tt, by + tt);
        }
    }

    /// `n` dashes along the cell.
    fn dashes(&mut self, horizontal: bool, n: usize, heavy: bool) {
        let (bx, by, tt) = self.band(heavy);
        let (start, len) = if horizontal {
            (self.x, self.w)
        } else {
            (self.y, self.h)
        };
        let slot = len / n as f32;
        let t = self.t;
        // Each slot holds one dash; a light stroke of gap is split evenly on both sides.
        for i in 0..n {
            let a = (start + slot * i as f32 + t * 0.5).round();
            let b = (start + slot * (i + 1) as f32 - t * 0.5).round();
            if horizontal {
                self.rect(a, by, b, by + tt);
            } else {
                self.rect(bx, a, bx + tt, b);
            }
        }
    }

    /// The quadrants set in `mask` (see [`quadrant_mask`]).
    fn quadrants(&mut self, mask: u8) {
        let (x, y, xr, yb) = (self.x, self.y, self.xr, self.yb);
        let (mx, my) = (x + self.w * 0.5, y + self.h * 0.5);
        let corners = [
            (x, y, mx, my),
            (mx, y, xr, my),
            (x, my, mx, yb),
            (mx, my, xr, yb),
        ];
        for (bit, (x0, y0, x1, y1)) in corners.into_iter().enumerate() {
            if mask & (1 << bit) != 0 {
                self.rect(x0, y0, x1, y1);
            }
        }
    }

    /// Full, half and eighth blocks, and shades. False for other characters.
    fn block(&mut self, c: char) -> bool {
        let (x, y, w, h, xr, yb) = (self.x, self.y, self.w, self.h, self.xr, self.yb);
        match c {
            '█' => self.rect(x, y, xr, yb),
            '▀' => self.rect(x, y, xr, y + h * 0.5),
            '▐' => self.rect(x + w * 0.5, y, xr, yb),
            // Lower n/8 blocks, ▁ (n = 1) to ▇ (n = 7); ▄ is n = 4.
            '\u{2581}'..='\u{2587}' => {
                let n = (c as u32 - 0x2580) as f32;
                self.rect(x, yb - h * n / 8.0, xr, yb);
            }
            // Left n/8 blocks, ▉ (n = 7) to ▏ (n = 1); ▌ is n = 4.
            '\u{2589}'..='\u{258F}' => {
                let n = (0x2590 - c as u32) as f32;
                self.rect(x, y, x + w * n / 8.0, yb);
            }
            '▔' => self.rect(x, y, xr, y + h / 8.0),
            '▕' => self.rect(xr - w / 8.0, y, xr, yb),
            // Shades: the full cell at reduced alpha.
            '░' | '▒' | '▓' => {
                let f = match c {
                    '░' => 0.25,
                    '▒' => 0.5,
                    _ => 0.75,
                };
                let [r, g, b, a] = self.color;
                self.quads.push((x, y, w, h, [r, g, b, a * f]));
            }
            _ => return false,
        }
        true
    }

    /// Double lines, laid out on the grid of [`G`].
    fn double_lines(&mut self, rects: &[GridRect]) {
        for &[x0, y0, x1, y1] in rects {
            let (x0, x1) = (self.stop(x0, true), self.stop(x1, true));
            let (y0, y1) = (self.stop(y0, false), self.stop(y1, false));
            self.rect(x0, y0, x1, y1);
        }
    }
}

/// Direct geometric rendering of box-drawing and block elements.
/// Avoids font margin gaps and ensures seamless borders.
///
/// Light lines are `stroke_width` thick and centred in the cell on whole pixels, so
/// neighbouring cells join exactly. Double lines are two light lines with one light
/// line of gap between them, laid out on the grid of [`G`].
pub fn try_render_box_or_block(
    c: char,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
    quads: &mut Vec<Quad>,
) -> bool {
    let t = stroke_width(w, h);
    let mut cell = Cell {
        x,
        y,
        w,
        h,
        xr: x + w,
        yb: y + h,
        t,
        lx: (x + (w - t) * 0.5).round(),
        ly: (y + (h - t) * 0.5).round(),
        color,
        quads,
    };
    if let Some(arms) = line_arms(c) {
        cell.line_arms(arms);
    } else if let Some((horizontal, n, heavy)) = dash_spec(c) {
        cell.dashes(horizontal, n, heavy);
    } else if let Some(rects) = double_line_rects(c) {
        cell.double_lines(rects);
    } else {
        match quadrant_mask(c) {
            0 => return cell.block(c),
            mask => cell.quadrants(mask),
        }
    }
    true
}
