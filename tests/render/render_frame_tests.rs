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
        blocks: Vec::new(),
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

#[test]
fn layered_gpu_passes_skip_cpu_overlay_clipping_and_align_splits() {
    use twitty::render::text::ui::UiFont;
    use twitty::render::ui_style::UiStyle;
    use twitty::ui::chrome::UiLanguage;
    use twitty::ui::context_menu::{ContextMenu, MenuAction, MenuEntry};
    use twitty::ui::toast::{ToastKind, Toasts};

    let mut s = Setup::new();
    let rect = Rect::new(0.0, 0.0, 800.0, 600.0);
    let panes = [pane(1, rect, "alpha\r\nbeta\r\ngamma\r\ndelta", true, None)];
    let mut frame = s.frame(&[], &panes, &[]);
    let base_runs_before = frame.text_runs(&panes, &s.cache, &s.ime).len();

    let style = UiStyle::new(&s.palette, None, 1.0);
    let ui_font = UiFont::default();
    let menu = ContextMenu::new(
        vec![
            MenuEntry::item("Copy", MenuAction::Command("edit.copy")),
            MenuEntry::item("Paste", MenuAction::Command("edit.paste")),
        ],
        (10.0, 10.0),
    )
    .unwrap();
    let menu_layout = menu.layout(rect, 1.0, UiLanguage::English);
    let mut toasts = Toasts::default();
    toasts.push(ToastKind::Success, "Saved", std::time::Instant::now());

    {
        let mut ctx = DrawContext {
            metrics: metrics(),
            font_system: &mut s.font_system,
            family: None,
            palette: &s.palette,
            is_srgb: false,
        };
        frame.add_context_menu(
            &mut ctx,
            style,
            &ui_font,
            &menu,
            &menu_layout,
            UiLanguage::English,
        );
        frame.add_toasts(
            &mut ctx,
            style,
            &ui_font,
            &toasts.visible(),
            UiLanguage::English,
        );
        frame.add_tooltip(&mut ctx, style, &ui_font, "Hint", (40.0, 40.0));
    }

    let splits = frame.overlay_splits();
    assert_eq!(splits.len(), 3, "context menu + toasts + tooltip splits");
    assert!(splits[0] > 0);
    assert!(splits[1] > splits[0]);
    assert_eq!(splits[2], frame.shapes.overlay.len());

    let layers = frame.text_layers(&panes, &s.cache, &s.ime);
    assert_eq!(
        layers.len(),
        1 + splits.len(),
        "base layer + one text layer per overlay split"
    );
    assert_eq!(
        layers[0].len(),
        base_runs_before,
        "GPU layered path must not split base terminal runs via CPU clip_runs"
    );
    assert!(!layers[1].is_empty(), "context menu labels");
    assert!(!layers[2].is_empty(), "toast labels");
    assert_eq!(layers[3].len(), 1, "tooltip label");
}
