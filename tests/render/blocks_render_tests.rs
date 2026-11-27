//! Command blocks drawn headless: separators, status gutters, the selection tint and the badge
//! next to Arabic text.
use cosmic_text::FontSystem;
use std::time::Instant;
use twitty::blocks::BlockList;
use twitty::color::Palette;
use twitty::layout::{PaneId, Rect};
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::frame::{build_frame, Frame, FrameOptions};
use twitty::render::scene::preedit::PreeditState;
use twitty::render::{CellMetrics, PaneFrame, RowCache};
use twitty::terminal::Terminal;

const SURFACE: (f32, f32) = (800.0, 600.0);
const LH: f32 = 23.0;
const PAD_TOP: f32 = 6.0;

fn metrics() -> CellMetrics {
    CellMetrics {
        font_size: 15.0,
        scale: 1.0,
        char_width: 9.0,
        line_height: LH,
        padding_left: 8.0,
        padding_top: PAD_TOP,
    }
}

struct Setup {
    font_system: FontSystem,
    palette: Palette,
    cache: RowCache,
    ime: PreeditState,
}

impl Setup {
    fn new() -> Self {
        Setup {
            font_system: FontSystem::new(),
            palette: Palette::default(),
            cache: RowCache::default(),
            ime: PreeditState::default(),
        }
    }

    fn frame(&mut self, panes: &[PaneFrame]) -> Frame {
        let mut ctx = DrawContext {
            font_system: &mut self.font_system,
            family: None,
            palette: &self.palette,
            metrics: metrics(),
            is_srgb: false,
        };
        let opts = FrameOptions {
            window_focused: true,
            cursor_style: "block",
            cursor_blink_on: true,
            surface: SURFACE,
        };
        build_frame(
            &mut ctx,
            &opts,
            &mut self.cache,
            &mut self.ime,
            &[],
            panes,
            &[],
        )
    }
}

/// A terminal that ran `ls` (fine) and an Arabic `cat` that failed, and shows a new prompt.
fn session() -> (Terminal, BlockList) {
    let mut term = Terminal::new(40, 8, |_| {});
    let mut blocks = BlockList::new();
    let text = "\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07a b\r\n\x1b]133;D;0\x07\
                \x1b]133;A\x07$ \x1b]133;B\x07cat ملف\r\n\x1b]133;C\x07لا يوجد\r\n\x1b]133;D;1\x07\
                \x1b]133;A\x07$ ";
    term.process_bytes(text.as_bytes());
    let now = Instant::now();
    for event in term.take_shell_events() {
        blocks.on_event(&event, &term, now);
    }
    (term, blocks)
}

fn pane(term: &Terminal, blocks: &BlockList) -> PaneFrame {
    let (lines, cursor) = term.snapshot();
    PaneFrame {
        id: PaneId(1),
        rect: Rect::new(0.0, 0.0, 800.0, 600.0),
        lines,
        cursor,
        focused: true,
        status: None,
        blocks: blocks.decor(term),
    }
}

fn row_y(row: usize) -> f32 {
    PAD_TOP + row as f32 * LH
}

#[test]
fn gutters_separators_and_badge() {
    let mut s = Setup::new();
    let (term, blocks) = session();
    let pane = pane(&term, &blocks);
    let frame = s.frame(std::slice::from_ref(&pane));
    let green = s.palette.ansi[2].to_array();
    let red = s.palette.ansi[1].to_array();
    // Gutters in the left padding: rows 0-1 green (`ls`), rows 2-3 red (`cat`).
    let gutter = |color: [f32; 4]| {
        frame
            .quads
            .iter()
            .find(|q| q.4 == color && q.0 < 8.0 && q.2 <= 4.0)
            .copied()
            .unwrap_or_else(|| panic!("no gutter {color:?}"))
    };
    let ok = gutter(green);
    assert!(ok.1 >= row_y(0) && ok.1 + ok.3 <= row_y(2) + 0.5, "{ok:?}");
    let failed = gutter(red);
    assert!(
        failed.1 >= row_y(2) && failed.1 + failed.3 <= row_y(4) + 0.5,
        "{failed:?}"
    );
    // Separators above the second block and above the waiting prompt, across the pane.
    let separators: Vec<_> = frame
        .quads
        .iter()
        .filter(|q| q.2 == 800.0 && q.3 == 1.0)
        .map(|q| q.1)
        .collect();
    assert_eq!(separators, vec![row_y(2), row_y(4)]);

    // The failed block's badge sits at the end of its first row; that row's text is clipped
    // before it, so the Arabic command never runs under it.
    let runs = frame.text_runs(std::slice::from_ref(&pane), &s.cache, &s.ime);
    let on_row: Vec<_> = runs.iter().filter(|r| r.top == row_y(2)).collect();
    let badge = on_row
        .iter()
        .max_by(|a, b| a.left.total_cmp(&b.left))
        .unwrap();
    let grid_right = 8.0 + 40.0 * 9.0;
    let badge_area_left = grid_right - 9.0 * 6.0;
    assert!(
        badge.left > badge_area_left && badge.left < grid_right,
        "{}",
        badge.left
    );
    for run in on_row.iter().filter(|r| r.left < badge.left) {
        assert!(run.bounds.right as f32 <= badge.left, "row text clipped");
    }
    // Font advances can make the shaper emit several grid-aligned runs for `$ ls`.
    let ls_runs: Vec<_> = runs.iter().filter(|r| r.top == row_y(0)).collect();
    assert!(!ls_runs.is_empty(), "missing `ls` row text");
    for run in ls_runs {
        assert!(
            run.left < badge_area_left,
            "unexpected `ls` badge at {}",
            run.left
        );
    }
}

#[test]
fn selected_block_is_tinted_under_its_rows() {
    let mut s = Setup::new();
    let (term, mut blocks) = session();
    assert!(blocks.select_at_row(&term, 3));
    let selected = pane(&term, &blocks);
    let frame = s.frame(std::slice::from_ref(&selected));
    // A full-width quad over rows 2-3 (the `cat` block).
    let at = frame
        .quads
        .iter()
        .position(|q| q.0 == 0.0 && q.2 == 800.0 && q.3 == 2.0 * LH)
        .expect("tint");
    assert_eq!(frame.quads[at].1, row_y(2));
    let (term, blocks) = session();
    let unselected = s.frame(std::slice::from_ref(&pane(&term, &blocks)));
    assert!(!unselected
        .quads
        .iter()
        .any(|q| q.2 == 800.0 && q.3 == 2.0 * LH));
    let red = s.palette.ansi[1].to_array();
    let gutter = frame.quads.iter().position(|q| q.4 == red).unwrap();
    assert!(at < gutter, "tint is drawn first, under the rows");
}

#[test]
fn panes_without_blocks_draw_nothing_extra() {
    let mut s = Setup::new();
    let mut term = Terminal::new(40, 8, |_| {});
    term.process_bytes("$ ls\r\na b\r\n$ ".as_bytes());
    let plain = pane(&term, &BlockList::new());
    assert!(plain.blocks.is_empty());
    let before = s.frame(std::slice::from_ref(&plain)).quads.len();
    // The same text from an integrated shell, before any command ran: still nothing.
    let mut term = Terminal::new(40, 8, |_| {});
    let mut blocks = BlockList::new();
    term.process_bytes("\x1b]133;A\x07$ ".as_bytes());
    for event in term.take_shell_events() {
        blocks.on_event(&event, &term, Instant::now());
    }
    assert!(blocks.decor(&term).is_empty());
    let (term, blocks) = session();
    let with = s
        .frame(std::slice::from_ref(&pane(&term, &blocks)))
        .quads
        .len();
    assert!(with > before);
}
