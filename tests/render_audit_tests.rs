//! Regressions from the renderer audit: hidden text, text color mode, wide cursor and the
//! preedit covering the text under it.
use alacritty_terminal::term::cell::Flags as CellFlags;
use twitty::color::Palette;
use twitty::renderer::cell_colors;
use twitty::terminal::CellData;

fn hidden_cell() -> CellData {
    let mut cell = CellData::blank();
    cell.c = 'p';
    cell.flags = CellFlags::HIDDEN;
    cell
}

#[test]
fn hidden_text_stays_hidden_when_selected_or_matched() {
    let palette = Palette::default();
    let mut selected = hidden_cell();
    selected.selected = true;
    let (fg, bg) = cell_colors(&palette, &selected);
    assert_eq!(fg, bg, "selected hidden text must stay invisible");

    for m in [1, 2] {
        let mut matched = hidden_cell();
        matched.search_match = m;
        let (fg, bg) = cell_colors(&palette, &matched);
        assert_eq!(fg, bg, "search-matched hidden text must stay invisible");
    }
}
