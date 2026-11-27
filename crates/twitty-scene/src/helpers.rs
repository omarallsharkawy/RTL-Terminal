//! Shared color, rect, and text-bounds helpers used across `twitty-scene` modules.

use alacritty_terminal::vte::ansi::Rgb;
use glyphon::TextBounds;
use twitty_core::color::Rgba;
use twitty_core::layout::Rect;

/// Convert a normalized `Rgba` color into 8-bit `Rgb`.
pub(crate) fn rgb8(c: Rgba) -> Rgb {
    let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgb {
        r: to8(c.r),
        g: to8(c.g),
        b: to8(c.b),
    }
}

/// Pick black or white foreground with strong WCAG contrast against `bg`.
pub(crate) fn contrasting_fg(bg: Rgba) -> Rgba {
    let [r, g, b, _] = bg.to_linear();
    let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let v = if lum > 0.179 { 0.0 } else { 1.0 };
    Rgba::new(v, v, v, 1.0)
}

/// Shrink `rect` on both axes, clamping each inset to half the extent so the origin stays inside.
pub(crate) fn inset_clamped(rect: Rect, dx: f32, dy: f32) -> Rect {
    let dx = dx.min(rect.w / 2.0);
    let dy = dy.min(rect.h / 2.0);
    Rect::new(
        rect.x + dx,
        rect.y + dy,
        rect.w - 2.0 * dx,
        rect.h - 2.0 * dy,
    )
}

/// Shrink `rect` by `(dx, dy)` on each side, clamping resulting width/height to zero.
pub(crate) fn inset(rect: Rect, dx: f32, dy: f32) -> Rect {
    Rect::new(
        rect.x + dx,
        rect.y + dy,
        (rect.w - 2.0 * dx).max(0.0),
        (rect.h - 2.0 * dy).max(0.0),
    )
}

/// Outward integer bounds enclosing `r` (`floor`/`ceil`).
pub(crate) fn rect_to_bounds(r: Rect) -> TextBounds {
    TextBounds {
        left: r.x.floor() as i32,
        top: r.y.floor() as i32,
        right: r.right().ceil() as i32,
        bottom: r.bottom().ceil() as i32,
    }
}

/// Inward integer bounds clipped strictly inside `r` (`ceil`/`floor`).
pub(crate) fn rect_to_bounds_inward(r: Rect) -> TextBounds {
    TextBounds {
        left: r.x.ceil() as i32,
        top: r.y.ceil() as i32,
        right: r.right().floor() as i32,
        bottom: r.bottom().floor() as i32,
    }
}

/// Intersection of two `TextBounds` rectangles (preserving raw coordinates).
pub(crate) fn intersect_bounds(a: TextBounds, b: TextBounds) -> TextBounds {
    TextBounds {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    }
}

/// Non-inverted intersection of two `TextBounds` rectangles (`right >= left`, `bottom >= top`).
pub(crate) fn intersect_bounds_clamped(a: TextBounds, b: TextBounds) -> TextBounds {
    let left = a.left.max(b.left);
    let top = a.top.max(b.top);
    TextBounds {
        left,
        top,
        right: a.right.min(b.right).max(left),
        bottom: a.bottom.min(b.bottom).max(top),
    }
}
