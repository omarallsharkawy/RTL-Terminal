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
    quads: &mut Vec<Quad>,
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
        // Four corners: the channels between the double lines stay open.
        '╬' => {
            rect(x, y0, x1, y1);
            rect(x0, y, x1, y0);
            rect(x2, y0, xr, y1);
            rect(x2, y, x3, y0);
            rect(x, y2, x1, y3);
            rect(x0, y3, x1, yb);
            rect(x2, y2, xr, y3);
            rect(x2, y3, x3, yb);
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
            rect(x0, y1, xr, y2);
            true
        }
        // Down double, left single.
        '╖' => {
            rect(x0, y1, x1, yb);
            rect(x2, y1, x3, yb);
            rect(x, y1, x3, y2);
            true
        }
        // Up double, right single.
        '╙' => {
            rect(x0, y, x1, y2);
            rect(x2, y, x3, y2);
            rect(x0, y1, xr, y2);
            true
        }
        // Up double, left single.
        '╜' => {
            rect(x0, y, x1, y2);
            rect(x2, y, x3, y2);
            rect(x, y1, x3, y2);
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
