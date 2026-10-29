//! Font size and background opacity steps (zoom shortcuts, Ctrl/Ctrl+Shift+wheel).

pub(crate) const DEFAULT_FONT_SIZE: f32 = 14.5;
pub(crate) const MIN_FONT_SIZE: f32 = 8.0;
pub(crate) const MAX_FONT_SIZE: f32 = 48.0;
/// Opacity change per wheel notch.
pub(crate) const OPACITY_STEP: f32 = 0.05;
pub(crate) const MIN_OPACITY: f32 = 0.2;

pub(crate) fn clamp_font_size(size: f32) -> f32 {
    size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
}

/// Background opacity after `steps` wheel notches (positive is more opaque).
pub(crate) fn step_opacity(opacity: f32, steps: i32) -> f32 {
    (opacity + OPACITY_STEP * steps as f32).clamp(MIN_OPACITY, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_size_is_clamped() {
        assert_eq!(clamp_font_size(4.0), 8.0);
        assert_eq!(clamp_font_size(60.0), 48.0);
        assert_eq!(clamp_font_size(15.5), 15.5);
    }

    #[test]
    fn opacity_steps_are_clamped() {
        assert!((step_opacity(0.5, 2) - 0.6).abs() < 1e-6);
        assert_eq!(step_opacity(0.95, 3), 1.0);
        assert_eq!(step_opacity(0.25, -4), 0.2);
    }
}
