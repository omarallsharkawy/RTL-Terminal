//! The command palette: Arabic and English matching, its keys, and its overlay.
use cosmic_text::FontSystem;
use twitty::color::Palette;
use twitty::commands;
use twitty::layout::{PaneId, Rect};
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::frame::{build_frame, FrameOptions};
use twitty::render::scene::palette::{label_width, panel_rect, PaletteItem, PaletteView};
use twitty::render::scene::preedit::PreeditState;
use twitty::render::{CellMetrics, PaneFrame, RowCache};
use twitty::terminal::Terminal;
use twitty::ui::palette::{fold, matches, PaletteCmd, PaletteState};
use winit::keyboard::{Key, ModifiersState, NamedKey};

fn first(query: &str) -> &'static str {
    let all = commands::all();
    let hits = matches(&all, query);
    all[*hits
        .first()
        .unwrap_or_else(|| panic!("no match for {query:?}"))]
    .name
}

#[test]
fn english_queries() {
    assert_eq!(first("split right"), "split.right");
    assert_eq!(first("new tab"), "tab.new");
    assert_eq!(first("copy out"), "block.copy-output");
    assert_eq!(first("SIDEBAR"), "sidebar.toggle");
    assert_eq!(first("palette"), "palette.open");
}

#[test]
fn arabic_queries_ignore_tashkeel_and_hamza_forms() {
    // "قسّم" is stored with a shadda; people type it without.
    assert_eq!(first("قسم يمين"), "split.right");
    assert_eq!(first("اظهار"), "sidebar.toggle");
    assert_eq!(first("تاب جديد"), "tab.new");
    assert_eq!(first("انسخ ناتج"), "block.copy-output");
    assert_eq!(fold("إظهارٌ ـآخر"), "اظهار اخر");
    assert_eq!(fold("مكتبة"), "مكتبه");
}

#[test]
fn empty_query_lists_everything_and_nonsense_nothing() {
    let all = commands::all();
    assert_eq!(matches(&all, "  ").len(), all.len());
    assert!(matches(&all, "zzzqqq").is_empty());
}

#[test]
fn keys_drive_selection_and_run() {
    let all = commands::all();
    let mut p = PaletteState::default();
    let none = ModifiersState::empty();
    for c in "zoom".chars() {
        let s = c.to_string();
        assert_eq!(
            p.key(&all, &Key::Character(s.as_str().into()), Some(&s), none),
            PaletteCmd::None
        );
    }
    assert_eq!(p.query, "zoom");
    let hits = matches(&all, "zoom");
    assert_eq!(hits.len(), 3);
    let down = Key::Named(NamedKey::ArrowDown);
    let up = Key::Named(NamedKey::ArrowUp);
    p.key(&all, &down, None, none);
    assert_eq!(p.selected, 1);
    p.key(&all, &up, None, none);
    p.key(&all, &up, None, none);
    assert_eq!(p.selected, 2, "wraps to the last");
    assert_eq!(
        p.key(&all, &Key::Named(NamedKey::Enter), None, none),
        PaletteCmd::Run(all[hits[2]].name)
    );
    assert_eq!(
        p.key(&all, &Key::Named(NamedKey::Escape), None, none),
        PaletteCmd::Close
    );
}

#[test]
fn editing_the_query() {
    let all = commands::all();
    let mut p = PaletteState::default();
    p.push_text("split\u{7} down");
    assert_eq!(p.query, "split down", "control characters dropped");
    p.selected = 1;
    p.key(
        &all,
        &Key::Named(NamedKey::Backspace),
        None,
        ModifiersState::empty(),
    );
    assert_eq!((p.query.as_str(), p.selected), ("split dow", 0));
    p.key(
        &all,
        &Key::Named(NamedKey::Backspace),
        None,
        ModifiersState::CONTROL,
    );
    assert_eq!(p.query, "split ");
    // Ctrl+letter is not text.
    p.key(
        &all,
        &Key::Character("a".into()),
        Some("a"),
        ModifiersState::CONTROL,
    );
    assert_eq!(p.query, "split ");
    // Enter with no results does nothing.
    p.push_text("zzzz");
    assert_eq!(
        p.key(
            &all,
            &Key::Named(NamedKey::Enter),
            None,
            ModifiersState::empty()
        ),
        PaletteCmd::None
    );
    p.push_text(&"x".repeat(1000));
    assert_eq!(p.query.chars().count(), 200);
}

#[test]
fn overlay_draws_above_panes_with_arabic_on_the_right() {
    let mut font_system = FontSystem::new();
    let palette = Palette::default();
    let metrics = CellMetrics {
        font_size: 15.0,
        scale: 1.0,
        char_width: 9.0,
        line_height: 23.0,
        padding_left: 8.0,
        padding_top: 6.0,
    };
    let mut ctx = DrawContext {
        font_system: &mut font_system,
        family: None,
        palette: &palette,
        metrics,
        is_srgb: false,
    };
    let surface = (900.0, 600.0);
    let opts = FrameOptions {
        window_focused: true,
        cursor_style: "block",
        cursor_blink_on: true,
        surface,
    };
    let mut term = Terminal::new(40, 4, |_| {});
    term.process_bytes(b"hello");
    let (lines, cursor) = term.snapshot();
    let panes = vec![PaneFrame {
        id: PaneId(1),
        rect: Rect::new(0.0, 0.0, surface.0, surface.1),
        lines,
        cursor,
        focused: true,
        status: None,
        blocks: Vec::new(),
    }];
    let mut cache = RowCache::default();
    let mut ime = PreeditState::default();
    let mut frame = build_frame(&mut ctx, &opts, &mut cache, &mut ime, &[], &panes, &[]);
    let quads_before = frame.quads.len();
    let runs_before = frame.text_runs(&panes, &cache, &ime).len();
    let view = PaletteView {
        query: "قسم".into(),
        items: vec![
            PaletteItem {
                title_en: "Split right".into(),
                title_ar: "قسّم يمين".into(),
                shortcut: Some("Ctrl+Shift+E".into()),
            },
            PaletteItem {
                title_en: "Split down".into(),
                title_ar: "قسّم تحت".into(),
                shortcut: None,
            },
        ],
        selected: 0,
    };
    frame.add_palette(&mut ctx, &view);
    let panel = panel_rect(9.0, 23.0, surface, 2);
    assert!(frame.quads.len() > quads_before, "panel quads added");
    // Panel quads come after the panes' quads, so they cover them.
    let (x, y, w, _, _) = frame.quads[quads_before];
    assert!(x < panel.x && y < panel.y && w > panel.w, "border first");
    let runs = frame.text_runs(&panes, &cache, &ime);
    let overlay = &runs[runs_before..];
    assert_eq!(
        overlay.len(),
        1 + 2 * 2,
        "query + (english, arabic) per item"
    );
    let arabic = &overlay[2];
    let right = arabic.left + label_width(arabic.buffer);
    assert!(
        (right - (panel.right() - 9.0)).abs() < 1.0,
        "Arabic title ends at the panel's right padding: {right} vs {}",
        panel.right()
    );
    assert!(overlay[1].left < arabic.left, "English starts on the left");
}
