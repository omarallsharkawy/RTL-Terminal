//! A whole frame laid out without a GPU: quad order, status lines, the preedit clip.
use cosmic_text::FontSystem;
use twitty::color::Palette;
use twitty::layout::{PaneId, Rect};
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::frame::{build_frame, Frame, FrameOptions};
use twitty::render::scene::preedit::PreeditState;
use twitty::render::{CellMetrics, PaneFrame, RowCache, TabLabel};
use twitty::terminal::Terminal;

const SURFACE: (f32, f32) = (800.0, 600.0);

fn metrics() -> CellMetrics {
    CellMetrics {
        font_size: 15.0,
        scale: 1.0,
        char_width: 9.0,
        line_height: 23.0,
        padding_left: 8.0,
        padding_top: 6.0,
    }
}

fn pane(id: u64, rect: Rect, text: &str, focused: bool, status: Option<&str>) -> PaneFrame {
    let mut term = Terminal::new(40, 4, |_| {});
    term.process_bytes(text.as_bytes());
    let (lines, cursor) = term.snapshot();
    PaneFrame {
        id: PaneId(id),
        rect,
        lines,
        cursor,
        focused,
        status: status.map(str::to_string),
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

    fn frame(&mut self, tabs: &[TabLabel], panes: &[PaneFrame], dividers: &[Rect]) -> Frame {
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
            tabs,
            panes,
            dividers,
        )
    }
}

fn tabs() -> Vec<TabLabel> {
    vec![
        TabLabel {
            title: "one".into(),
            active: true,
        },
        TabLabel {
            title: "مرحبا".into(),
            active: false,
        },
    ]
}

#[test]
fn tab_bar_comes_first_and_dividers_last() {
    let mut s = Setup::new();
    let left = Rect::new(0.0, 23.0, 399.0, 577.0);
    let right = Rect::new(400.0, 23.0, 400.0, 577.0);
    let divider = Rect::new(399.0, 23.0, 1.0, 577.0);
    let panes = [
        pane(1, left, "left", true, None),
        pane(2, right, "right", false, None),
    ];
    let frame = s.frame(&tabs(), &panes, &[divider]);
    let first = frame.quads[0];
    assert_eq!(
        (first.0, first.1, first.2, first.3),
        (0.0, 0.0, SURFACE.0, 23.0)
    );
    let last = *frame.quads.last().unwrap();
    assert_eq!((last.0, last.1, last.2, last.3), (399.0, 23.0, 1.0, 577.0));
    // The block cursor of the focused pane is drawn after the unfocused pane's hollow one.
    let cursor = s.palette.cursor.to_array();
    let cursors: Vec<_> = frame.quads.iter().filter(|q| q.4 == cursor).collect();
    let solid = cursors
        .iter()
        .position(|q| q.3 == 23.0 && q.2 == 9.0)
        .unwrap();
    assert!(
        cursors[..solid].iter().any(|q| q.0 >= 400.0),
        "hollow cursor first"
    );
}

#[test]
fn status_line_hides_the_last_row_text() {
    let mut s = Setup::new();
    let rect = Rect::new(0.0, 0.0, 800.0, 600.0);
    let panes = [pane(1, rect, "a\r\nb\r\nc\r\nd", true, Some("find: x"))];
    let frame = s.frame(&[], &panes, &[]);
    let runs = frame.text_runs(&panes, &s.cache, &s.ime);
    let last_row_top = 6.0 + 3.0 * 23.0;
    let at_last_row: Vec<_> = runs.iter().filter(|r| r.top == last_row_top).collect();
    // Only the status line is drawn on the last row, from the left padding.
    assert_eq!(at_last_row.len(), 1);
    assert_eq!(at_last_row[0].left, 8.0);
    assert_eq!(runs.iter().filter(|r| r.top == 6.0).count(), 1);
}

#[test]
fn preedit_clips_the_row_under_it() {
    let mut s = Setup::new();
    let rect = Rect::new(0.0, 0.0, 800.0, 600.0);
    let panes = [pane(1, rect, "hello world", true, None)];
    s.ime.set(Some(("نص".into(), None)));
    let frame = s.frame(&[], &panes, &[]);
    assert!(
        frame.ime_area.is_some(),
        "first layout reports the popup area"
    );
    let runs = frame.text_runs(&panes, &s.cache, &s.ime);
    let row: Vec<_> = runs.iter().filter(|r| r.top == 6.0).collect();
    // The row's text is split around the preedit box, then the preedit itself is drawn.
    assert!(row.len() >= 3, "{}", row.len());
    let preedit = runs.last().unwrap();
    assert_eq!(preedit.top, 6.0);
    assert!(preedit.left >= 8.0 + 11.0 * 9.0 - 0.5);

    // Same caret: the popup area is not reported again.
    let frame = s.frame(&[], &panes, &[]);
    assert!(frame.ime_area.is_none());
}
