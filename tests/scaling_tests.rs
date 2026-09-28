#[test]
fn test_high_dpi_scaling_grid_dimensions() {
    // Base logical window size: 960 x 580
    let base_width = 960.0_f32;
    let base_height = 580.0_f32;
    let padding_left = 12.0_f32;
    let padding_top = 10.0_f32;
    let font_size = 14.5_f32;

    // Scale factors: 1.0 (Standard 96 DPI), 1.25 (120 DPI), 1.5 (144 DPI), 2.0 (HiDPI 4K)
    let scale_factors = [1.0_f32, 1.25, 1.5, 2.0];

    for &scale in &scale_factors {
        // Scaled physical dimensions
        let phys_width = base_width * scale;
        let phys_height = base_height * scale;

        // Font and cell geometry scaled by DPI factor
        let char_width = 9.0 * scale;
        let line_height = (font_size * 1.55).ceil() * scale;

        let usable_width = (phys_width - (padding_left * 2.0 * scale)).max(10.0);
        let usable_height = (phys_height - (padding_top * 2.0 * scale)).max(10.0);

        let cols = (usable_width / char_width).floor() as usize;
        let rows = (usable_height / line_height).floor() as usize;

        // Terminal must maintain standard minimum viewport density across all scales
        assert!(
            cols >= 80,
            "Cols at scale {} should be >= 80, got {}",
            scale,
            cols
        );
        assert!(
            rows >= 20,
            "Rows at scale {} should be >= 20, got {}",
            scale,
            rows
        );

        // Cells must not overflow physical window boundaries
        let total_content_width = (padding_left * scale) + (cols as f32 * char_width);
        let total_content_height = (padding_top * scale) + (rows as f32 * line_height);
        assert!(
            total_content_width <= phys_width,
            "Content width exceeds window width at scale {}",
            scale
        );
        assert!(
            total_content_height <= phys_height,
            "Content height exceeds window height at scale {}",
            scale
        );
    }
}
