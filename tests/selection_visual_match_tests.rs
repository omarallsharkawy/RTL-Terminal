use cosmic_text::{Attrs, Family, FontSystem, Metrics};
use twitty::color::{Palette, Rgba};
use twitty::shaping::shape_row;
use twitty::terminal::Terminal;

#[test]
fn test_selection_visual_x_corresponds_to_cells() {
    let mut term = Terminal::new(80, 24, |_| {});
    term.process_bytes("لقيت حاجتين. خليني أتأكد منهم\r\n".as_bytes());
    let (lines, cursor) = term.snapshot();

    let mut font_system = FontSystem::new();
    let metrics = Metrics::new(14.5, 23.0);
    let default_attrs = Attrs::new().family(Family::Monospace);
    let palette = Palette::default();
    let default_bg = Rgba::from_rgb8(21, 22, 30);
    let char_width = 9.0f32;
    let padding_left = 8.0f32;

    let cached = shape_row(
        &lines[0],
        0,
        &cursor,
        char_width,
        23.0,
        padding_left,
        metrics,
        &default_attrs,
        &palette,
        &mut font_system,
        default_bg,
    );

    // For cell 0 ('ل'), visual_x_for_cell must return its exact visual x coordinates
    let (x0, w0) = cached.visual_x_for_cell(0, char_width, padding_left);
    assert!(
        x0 > padding_left,
        "Arabic cell 0 in RTL must be positioned in RTL sequence"
    );
    assert!(w0 > 0.0);

    // cell_from_visual_x at that exact coordinate must map back to cell 0
    let mapped_col = cached.cell_from_visual_x(x0 + w0 * 0.5, char_width, padding_left, 80);
    assert_eq!(
        mapped_col, 0,
        "Clicking visual position of cell 0 must resolve to cell 0"
    );
}
