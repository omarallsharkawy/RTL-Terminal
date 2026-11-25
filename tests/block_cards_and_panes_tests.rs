//! Window-free and offscreen-GPU tests for Warp-style command block cards and cmux-style pane
//! features (zoom indicator, flash outline, rename mode, shortcuts, and dark/light rendering).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use alacritty_terminal::index::Side;
use cosmic_text::{fontdb, FontSystem};
use twitty::blocks::cards::{hit_test_cards, layout_cards, BlockButton, BlockHover};
use twitty::blocks::{encode_rerun, rerun_text, BlockList};
use twitty::color::Palette;
use twitty::commands;
use twitty::config::TwittyConfig;
use twitty::input::{handle_key_raw, InputAction};
use twitty::layout::{PaneId, Rect, SidebarPosition};
use twitty::render::gpu::offscreen::Offscreen;
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::frame::{build_frame_with_cards, FrameOptions};
use twitty::render::scene::preedit::PreeditState;
use twitty::render::scene::window_chrome::{ChromeView, PaneHeaderView};
use twitty::render::shapes::Shape;
use twitty::render::text::ui::UiFont;
use twitty::render::ui_style::UiStyle;
use twitty::render::{CellMetrics, PaneFrame, RowCache};
use twitty::terminal::Terminal;
use twitty::ui::chrome::{
    PaneFlash, PaneHeaderLayout, TopBarLayout, WindowLayout, PANE_FLASH_DURATION,
};
use twitty::ui::palette::{PaletteMode, PaletteState, RenameCmd, RENAME_TAB_PLACEHOLDER};
use winit::event::ElementState;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey};

const A: &str = "\x1b]133;A\x07";
const B: &str = "\x1b]133;B\x07";
const C: &str = "\x1b]133;C\x07";

fn d(code: i32) -> String {
    format!("\x1b]133;D;{code}\x07")
}

fn metrics(scale: f32) -> CellMetrics {
    CellMetrics {
        font_size: 15.0,
        scale,
        char_width: 9.0 * scale,
        line_height: 23.0 * scale,
        padding_left: 8.0 * scale,
        padding_top: 6.0 * scale,
    }
}

fn feed(term: &mut Terminal, blocks: &mut BlockList, clock: Instant, text: &str) {
    term.process_bytes(text.as_bytes());
    for event in term.take_shell_events() {
        blocks.on_event(&event, term, clock);
    }
    blocks.refresh(term);
}

fn center(r: Rect) -> (f64, f64) {
    (f64::from(r.x + r.w * 0.5), f64::from(r.y + r.h * 0.5))
}

#[test]
fn card_rects_visible_partially_scrolled_wrapped_and_alt_screen() {
    let mut term = Terminal::with_scrollback(20, 6, 100);
    let mut blocks = BlockList::new();
    let mut clock = Instant::now();

    // Block 1: short command + 1 line output.
    feed(
        &mut term,
        &mut blocks,
        clock,
        &format!("{A}$ {B}ls\r\n{C}file\r\n"),
    );
    clock += Duration::from_millis(1200);
    feed(&mut term, &mut blocks, clock, &d(0));

    // Block 2: long wrapped output across multiple rows.
    let wrapped = "x".repeat(45); // 20 cols -> wraps across 3 rows
    feed(
        &mut term,
        &mut blocks,
        clock,
        &format!("{A}$ {B}cat long\r\n{C}{wrapped}\r\n"),
    );
    clock += Duration::from_millis(2300);
    feed(&mut term, &mut blocks, clock, &d(1));

    // Block 3: another command pushing block 1 partially into scrollback, plus waiting prompt.
    feed(
        &mut term,
        &mut blocks,
        clock,
        &format!("{A}$ {B}pwd\r\n{C}/tmp\r\n{}{A}$ ", d(0)),
    );

    let m = metrics(1.0);
    let pane_rect = Rect::new(0.0, 0.0, 400.0, 200.0);

    // At live bottom: block 1 is scrolled off/partially scrolled, block 2 wraps 4 rows,
    // block 3 is at bottom; waiting prompt does NOT get a card.
    let decor = blocks.decor(&term);
    let cards = layout_cards(pane_rect, m, 6, &decor, None, None, |_| 0.0);
    assert!(
        cards.len() >= 2,
        "expected visible command cards: {cards:?}"
    );
    for card in &cards {
        // Card stays inside pane_rect and expands 4 px left of column 0 (padding_left=8 -> x=4).
        assert!(card.card.x >= pane_rect.x);
        assert!(card.card.right() <= pane_rect.right());
        assert_eq!(card.card.x, pane_rect.x + m.padding_left - 4.0);
        // Strip sits on left edge of card, clear of column 0.
        assert_eq!(card.strip.x, card.card.x);
        assert!(card.strip.right() <= pane_rect.x + m.padding_left);
    }
    // Adjacent cards never overlap vertically.
    for pair in cards.windows(2) {
        assert!(
            pair[0].card.bottom() < pair[1].card.y,
            "adjacent cards must have a gap: {:?} vs {:?}",
            pair[0].card,
            pair[1].card
        );
    }

    // Scroll up 1 line so the first visible block is partially clipped at top or bottom.
    term.scroll_display(2);
    let scrolled_decor = blocks.decor(&term);
    let scrolled_cards = layout_cards(pane_rect, m, 6, &scrolled_decor, None, None, |_| 0.0);
    let clipped_bottom = scrolled_cards
        .iter()
        .find(|c| c.clipped_bottom)
        .expect("expected a card clipped at bottom when scrolled up");
    assert_eq!(clipped_bottom.radii[2], 0.0);
    assert_eq!(clipped_bottom.radii[3], 0.0);
    assert_eq!(clipped_bottom.card.bottom(), pane_rect.bottom());

    // Scroll back to bottom where the wrapped block starts above row 0.
    term.scroll_to_bottom();
    let bottom_decor = blocks.decor(&term);
    let bottom_cards = layout_cards(pane_rect, m, 6, &bottom_decor, None, None, |_| 0.0);
    if let Some(clipped_top) = bottom_cards.iter().find(|c| c.clipped_top) {
        assert_eq!(clipped_top.radii[0], 0.0);
        assert_eq!(clipped_top.radii[1], 0.0);
        assert_eq!(clipped_top.card.y, pane_rect.y);
    }

    // Alternate screen: no cards at all.
    feed(&mut term, &mut blocks, clock, "\x1b[?1049hhello alt");
    let alt_decor = blocks.decor(&term);
    assert!(alt_decor.is_empty());
    assert!(layout_cards(pane_rect, m, 6, &alt_decor, None, None, |_| 0.0).is_empty());
}

#[test]
fn toolbar_hit_tests_and_meta_labels() {
    let mut term = Terminal::with_scrollback(60, 10, 100);
    let mut blocks = BlockList::new();
    let t0 = Instant::now();

    feed(
        &mut term,
        &mut blocks,
        t0,
        &format!("{A}$ {B}echo hi\r\n{C}hi\r\n"),
    );
    feed(
        &mut term,
        &mut blocks,
        t0 + Duration::from_millis(2300),
        &d(1),
    );
    // A block without a command (C directly without prompt/B).
    feed(
        &mut term,
        &mut blocks,
        t0 + Duration::from_millis(2400),
        &format!("{C}raw\r\n"),
    );
    feed(
        &mut term,
        &mut blocks,
        t0 + Duration::from_millis(2450),
        &d(0),
    );
    feed(
        &mut term,
        &mut blocks,
        t0 + Duration::from_millis(2500),
        &format!("{A}$ "),
    );

    let decor = blocks.decor(&term);
    assert_eq!(decor[0].meta.as_deref(), Some("2.3s · exit 1"));
    assert_eq!(
        decor[1].meta, None,
        "instant exit 0 has no dim meta clutter"
    );
    assert!(decor[0].has_command);
    assert!(!decor[1].has_command);

    let m = metrics(1.5);
    let pane_rect = Rect::new(10.0, 20.0, 720.0, 400.0);
    let cards = layout_cards(pane_rect, m, 10, &decor, Some(decor[0].id), None, |_| 80.0);
    let tb = cards[0].toolbar.expect("hovered block gets toolbar");
    assert!(tb.meta.is_some(), "meta label rect fits");

    // Hit-test each of the 4 buttons on block 0.
    for (btn, rect, enabled) in tb.buttons {
        assert!(enabled, "{btn:?} should be enabled when command is present");
        let (x, y) = center(rect);
        assert_eq!(
            hit_test_cards(pane_rect, m, 10, &decor, None, x, y),
            Some((decor[0].id, Some(btn)))
        );
    }

    // Card body away from toolbar hits the block with button=None.
    let (bx, by) = (
        f64::from(cards[0].card.x + 20.0),
        f64::from(cards[0].card.bottom() - 4.0),
    );
    assert_eq!(
        hit_test_cards(pane_rect, m, 10, &decor, None, bx, by),
        Some((decor[0].id, None))
    );

    // Outside any card returns None.
    assert_eq!(
        hit_test_cards(pane_rect, m, 10, &decor, None, 0.0, 0.0),
        None
    );

    // Block without a command disables CopyCommand and Rerun while keeping CopyOutput and AskAgent.
    let cards2 = layout_cards(pane_rect, m, 10, &decor, Some(decor[1].id), None, |_| 0.0);
    let tb2 = cards2[1].toolbar.expect("hovered block 2 gets toolbar");
    for (btn, rect, enabled) in tb2.buttons {
        let (x, y) = center(rect);
        let expected_enabled = matches!(btn, BlockButton::CopyOutput | BlockButton::AskAgent);
        assert_eq!(enabled, expected_enabled, "{btn:?}");
        assert_eq!(
            hit_test_cards(pane_rect, m, 10, &decor, None, x, y),
            Some((decor[1].id, expected_enabled.then_some(btn)))
        );
    }

    // Status bar on row 0 suppresses toolbar on row 0.
    let hidden = layout_cards(pane_rect, m, 10, &decor, Some(decor[0].id), Some(0), |_| {
        80.0
    });
    assert!(hidden[0].toolbar.is_none());
}

#[test]
fn rerun_text_has_no_newline_and_is_paste_safe() {
    assert_eq!(rerun_text("ls -la\r\n"), "ls -la");
    assert_eq!(rerun_text("echo one\necho two\n"), "echo one echo two");
    assert_eq!(rerun_text("  echo مرحبا\u{202E}\n\r"), "echo مرحبا");
    assert_eq!(rerun_text("   \r\n  "), "");

    for bracketed in [false, true] {
        let bytes = encode_rerun("git status\r\n\x1b[201~rm -rf /\n", bracketed);
        assert!(!bytes.contains(&b'\n'), "rerun must never emit LF");
        assert!(!bytes.contains(&b'\r'), "rerun must never emit CR");
        if bracketed {
            assert!(bytes.starts_with(b"\x1b[200~"));
            assert!(bytes.ends_with(b"\x1b[201~"));
            let inner = &bytes[6..bytes.len() - 6];
            assert!(!inner.windows(6).any(|w| w == b"\x1b[201~"));
        }
    }
    assert!(encode_rerun("   \n", true).is_empty());
}

#[test]
fn rename_tab_mode_arabic_empty_clears_and_bounded_length() {
    assert_eq!(RENAME_TAB_PLACEHOLDER, "Rename tab… / غيّر اسم التاب…");

    let none = ModifiersState::empty();
    let mut state = PaletteState {
        mode: PaletteMode::RenameTab,
        ..PaletteState::default()
    };

    // Arabic title saved on Enter.
    state.push_text("مشروع تويتي");
    assert_eq!(
        state.rename_key(&Key::Named(NamedKey::Enter), None, none),
        RenameCmd::Save(Some("مشروع تويتي".into()))
    );

    // Escape cancels.
    assert_eq!(
        state.rename_key(&Key::Named(NamedKey::Escape), None, none),
        RenameCmd::Cancel
    );

    // Empty or whitespace-only clears the custom title.
    let mut empty = PaletteState {
        mode: PaletteMode::RenameTab,
        ..PaletteState::default()
    };
    assert_eq!(
        empty.rename_key(&Key::Named(NamedKey::Enter), None, none),
        RenameCmd::Save(None)
    );
    empty.push_text("   ");
    assert_eq!(
        empty.rename_key(&Key::Named(NamedKey::Enter), None, none),
        RenameCmd::Save(None)
    );

    // Bounded length (200 chars) and bidi-override/control stripping.
    let mut long = PaletteState {
        mode: PaletteMode::RenameTab,
        ..PaletteState::default()
    };
    long.push_text(&format!("ع{}\u{202E}\x07", "أ".repeat(500)));
    let RenameCmd::Save(Some(saved)) = long.rename_key(&Key::Named(NamedKey::Enter), None, none)
    else {
        panic!("expected bounded title");
    };
    assert!(saved.chars().count() <= 200);
    assert!(!saved.contains('\u{202E}'));
    assert!(!saved.contains('\x07'));
}

#[test]
fn cmux_pane_commands_shortcuts_flash_and_zoom_indicator() {
    let cs = ModifiersState::CONTROL | ModifiersState::SHIFT;
    let cases = [
        ("pane.zoom", InputAction::TogglePaneZoom, KeyCode::KeyZ),
        (
            "notifications.jump-unread",
            InputAction::JumpUnread,
            KeyCode::KeyJ,
        ),
        ("pane.flash", InputAction::FlashPane, KeyCode::KeyH),
        ("tab.rename", InputAction::RenameTab, KeyCode::KeyN),
        (
            "pane.close-others",
            InputAction::CloseOtherPanes,
            KeyCode::KeyK,
        ),
    ];
    let all = commands::all();
    for (name, action, key) in cases {
        let cmd = all
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("missing command {name}"));
        assert!(!cmd.title_en.is_empty() && !cmd.title_ar.is_empty());
        assert!(cmd.shortcut.is_some());
        assert_eq!(commands::action(name), Some(action.clone()));
        assert_eq!(
            handle_key_raw(
                Some(key),
                &Key::Dead(None),
                None,
                ElementState::Pressed,
                cs,
                false
            ),
            Some(action),
            "{name} shortcut"
        );
        // Plain Ctrl+letter stays for the shell!
        assert!(
            matches!(
                handle_key_raw(
                    Some(key),
                    &Key::Dead(None),
                    None,
                    ElementState::Pressed,
                    ModifiersState::CONTROL,
                    false
                ),
                Some(InputAction::Bytes(_))
            ),
            "plain Ctrl+{key:?} must reach the shell"
        );
    }

    // PaneFlash fades smoothly and expires cleanly.
    let mut flash = PaneFlash::default();
    let t0 = Instant::now();
    assert_eq!(flash.intensity(PaneId(1), t0), 0.0);
    flash.trigger(PaneId(1), t0);
    assert!((flash.intensity(PaneId(1), t0) - 1.0).abs() < 0.001);
    assert_eq!(flash.intensity(PaneId(2), t0), 0.0);
    let mid = t0 + PANE_FLASH_DURATION / 2;
    let mid_v = flash.intensity(PaneId(1), mid);
    assert!(mid_v > 0.0 && mid_v < 1.0);
    assert!(flash.tick(mid).is_some());
    let end = t0 + PANE_FLASH_DURATION;
    assert_eq!(flash.intensity(PaneId(1), end), 0.0);
    assert_eq!(flash.tick(end), None);
    assert!(!flash.is_active());

    // Zoom indicator & flash outline in window chrome.
    let mut fonts = FontSystem::new();
    let ui_font = UiFont::default();
    let palette = Palette::default();
    let m = metrics(1.0);
    let win = WindowLayout::new(m, (800.0, 500.0), 1, (SidebarPosition::Hidden, 220.0), true);
    let header_layout = PaneHeaderLayout::new(win.content, 1.0, true);
    let style = UiStyle::new(&palette, None, 1.0);

    let mut build_shapes = |zoomed: bool, flash_val: f32| {
        let mut ctx = DrawContext {
            font_system: &mut fonts,
            family: None,
            palette: &palette,
            metrics: m,
            is_srgb: false,
        };
        let mut cache = RowCache::default();
        let mut ime = PreeditState::default();
        let opts = FrameOptions {
            window_focused: true,
            cursor_style: "block",
            cursor_blink_on: true,
            surface: (800.0, 500.0),
        };
        let mut frame = twitty::render::scene::frame::build_frame(
            &mut ctx,
            &opts,
            &mut cache,
            &mut ime,
            &[],
            &[],
            &[],
        );
        let panes = [PaneHeaderView {
            id: PaneId(1),
            layout: header_layout,
            title: "bash".into(),
            agent: false,
            focused: true,
            attention: false,
            zoomed,
            flash: flash_val,
        }];
        let view = ChromeView {
            layout: &win,
            top: TopBarLayout::new(win.top_bar, 1.0, None),
            diff: None,
            unread: 0,
            sidebar_visible: false,
            hover: None,
            panes: &panes,
            tabs: &[],
        };
        frame.add_chrome(&mut ctx, style, &ui_font, &view);
        frame.shapes.chrome.len()
    };

    let base_count = build_shapes(false, 0.0);
    let zoomed_count = build_shapes(true, 0.0);
    let flashed_count = build_shapes(false, 0.8);
    assert!(zoomed_count > base_count, "zoom indicator adds shapes");
    assert_eq!(flashed_count, base_count + 1, "flash adds outline shape");
}

#[test]
fn config_block_cards_defaults_true_and_parses_false() {
    assert!(TwittyConfig::default().block_cards);
    let default_parsed: TwittyConfig = serde_json::from_str("{}").unwrap();
    assert!(default_parsed.block_cards);
    let disabled: TwittyConfig = serde_json::from_str(r#"{"block_cards": false}"#).unwrap();
    assert!(!disabled.block_cards);
}

const NASKH: &[u8] = include_bytes!("../assets/fonts/NotoNaskhArabic-Regular.ttf");
const MONO: &[u8] = include_bytes!("golden/fonts/NotoSansMono-Subset.ttf");

fn test_font_system() -> FontSystem {
    let mut db = fontdb::Database::new();
    db.load_font_data(MONO.to_vec());
    db.load_font_data(NASKH.to_vec());
    db.set_monospace_family("Noto Sans Mono");
    db.set_sans_serif_family("Noto Sans Mono");
    FontSystem::new_with_locale_and_db("en-US".into(), db)
}

static GPU: Mutex<()> = Mutex::new(());

#[test]
fn headless_render_two_block_cards_ok_and_failed_dark_and_light() {
    for theme_name in ["tokyo-night", "github-light"] {
        let mut font_system = test_font_system();
        let ui_font = UiFont::new(&mut font_system, Some("Noto Sans Mono"));
        let palette = Palette::named(theme_name).unwrap();
        let m = CellMetrics::measure(&mut font_system, None, 15.0, 1.0);
        let style = UiStyle::new(&palette, None, m.scale);

        let cols = 48;
        let rows = 8;
        let width = (m.padding_left * 2.0 + cols as f32 * m.char_width).ceil();
        let height = (m.padding_top * 2.0 + rows as f32 * m.line_height).ceil();

        let mut term = Terminal::with_scrollback(cols, rows, 100);
        let mut blocks = BlockList::new();
        let t0 = Instant::now();
        // Block 1: ok (exit 0)
        feed(
            &mut term,
            &mut blocks,
            t0,
            &format!("{A}$ {B}echo ok\r\n{C}ok\r\n"),
        );
        feed(
            &mut term,
            &mut blocks,
            t0 + Duration::from_millis(1200),
            &d(0),
        );
        // Block 2: failed (exit 1) with Arabic output
        feed(
            &mut term,
            &mut blocks,
            t0 + Duration::from_millis(1300),
            &format!("{A}$ {B}cat err\r\n{C}خطأ في الملف\r\n"),
        );
        feed(
            &mut term,
            &mut blocks,
            t0 + Duration::from_millis(3600),
            &d(1),
        );
        feed(
            &mut term,
            &mut blocks,
            t0 + Duration::from_millis(3700),
            &format!("{A}$ "),
        );

        // Select text inside block 1 so selection highlight sits above block 1's card background.
        term.start_selection(0, 1, Side::Left, 1);
        term.update_selection(1, 1, Side::Right);

        let decor = blocks.decor(&term);
        assert_eq!(decor.len(), 3);
        let ok_id = decor[0].id;
        let fail_id = decor[1].id;
        assert_eq!(decor[1].meta.as_deref(), Some("2.3s · exit 1"));

        let (lines, cursor) = term.snapshot();
        let pane_rect = Rect::new(0.0, 0.0, width, height);
        let panes = [PaneFrame {
            id: PaneId(1),
            rect: pane_rect,
            lines,
            cursor,
            focused: true,
            status: None,
            blocks: decor.clone(),
        }];

        let hover = Some(BlockHover {
            pane: PaneId(1),
            block: fail_id,
            button: Some(BlockButton::Rerun),
        });
        let mut cache = RowCache::default();
        let mut ime = PreeditState::default();
        let mut ctx = DrawContext {
            font_system: &mut font_system,
            family: None,
            palette: &palette,
            metrics: m,
            is_srgb: false,
        };
        let opts = FrameOptions {
            window_focused: true,
            cursor_style: "block",
            cursor_blink_on: true,
            surface: (width, height),
        };
        let frame = build_frame_with_cards(
            &mut ctx,
            &opts,
            &mut cache,
            &mut ime,
            &[],
            &panes,
            &[],
            style,
            &ui_font,
            hover,
        );

        // 2 cards + 2 status strips in underlay (drawn behind quads).
        assert_eq!(frame.shapes.underlay.len(), 4, "{theme_name}");
        // Floating toolbar + icons + ok meta pill in chrome (drawn above quads).
        assert!(!frame.shapes.chrome.is_empty(), "{theme_name}");

        // Verify ok strip has style.ok and failed strip has style.danger.
        let ok_color = palette.ansi[2].to_array();
        let danger_color = palette.ansi[1].to_array();
        assert!(frame
            .shapes
            .underlay
            .iter()
            .any(|s| matches!(s, Shape::RoundedRect { fill, .. } if *fill == ok_color)));
        assert!(frame
            .shapes
            .underlay
            .iter()
            .any(|s| matches!(s, Shape::RoundedRect { fill, .. } if *fill == danger_color)));

        // Verify terminal row text on block 2's first row is clipped out of the floating toolbar.
        let cards = layout_cards(pane_rect, m, rows, &decor, Some(fail_id), None, |_| 80.0);
        let tb_rect = cards
            .iter()
            .find(|c| c.id == fail_id)
            .unwrap()
            .toolbar
            .unwrap()
            .rect;
        let runs = frame.text_runs(&panes, &cache, &ime);
        let row2_y = m.padding_top + 2.0 * m.line_height;
        for run in runs.iter().filter(|r| (r.top - row2_y).abs() < 0.5) {
            let overlaps = (run.bounds.left as f32) < tb_rect.right()
                && (run.bounds.right as f32) > tb_rect.x
                && (run.bounds.top as f32) < tb_rect.bottom()
                && (run.bounds.bottom as f32) > tb_rect.y;
            assert!(
                !overlaps,
                "{theme_name}: row text overlaps floating toolbar: {:?} vs {:?}",
                run.bounds, tb_rect
            );
        }

        // Offscreen GPU pixel check (runs under Mesa llvmpipe; skips only if unavailable and
        // TWITTY_REQUIRE_GOLDEN is unset).
        let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
        let mut target = match Offscreen::new(width as u32, height as u32) {
            Ok(t) => t,
            Err(e) => {
                assert!(
                    std::env::var("TWITTY_REQUIRE_GOLDEN")
                        .ok()
                        .filter(|v| !v.is_empty() && v != "0")
                        .is_none(),
                    "TWITTY_REQUIRE_GOLDEN is set but offscreen failed: {e}"
                );
                continue;
            }
        };
        let pixels = target
            .render_layers(
                (&frame.quads, &frame.shapes),
                runs,
                &mut font_system,
                frame.background,
                1.0,
            )
            .expect("offscreen render");
        let w = width as usize;
        let pixel = |x: f32, y: f32| -> [u8; 4] {
            let xi = (x.round() as usize).min(w - 1);
            let yi = (y.round() as usize).min(height as usize - 1);
            let off = (yi * w + xi) * 4;
            [
                pixels[off],
                pixels[off + 1],
                pixels[off + 2],
                pixels[off + 3],
            ]
        };
        let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let close = |a: [u8; 4], b: [f32; 4]| -> bool {
            a[0].abs_diff(to8(b[0])) <= 12
                && a[1].abs_diff(to8(b[1])) <= 12
                && a[2].abs_diff(to8(b[2])) <= 12
        };

        let ok_card = cards.iter().find(|c| c.id == ok_id).unwrap();
        let fail_card = cards.iter().find(|c| c.id == fail_id).unwrap();

        // Card interior (away from text/toolbar) matches bg2 and differs from bg0.
        let bg_sample = pixel(ok_card.card.x + 180.0, ok_card.card.y + 10.0);
        let outside_sample = pixel(pane_rect.right() - 1.0, pane_rect.bottom() - 2.0);
        assert!(
            close(bg_sample, style.bg2.to_array()),
            "{theme_name}: card fill {bg_sample:?} != bg2 {:?}",
            style.bg2.to_array()
        );
        assert!(
            close(outside_sample, style.bg0.to_array()),
            "{theme_name}: outside {outside_sample:?} != bg0 {:?}",
            style.bg0.to_array()
        );
        assert_ne!(
            bg_sample, outside_sample,
            "{theme_name}: card must contrast with bg0"
        );

        // Left status strips match ok (green) and danger (red).
        let ok_strip = pixel(
            ok_card.strip.x + ok_card.strip.w * 0.5,
            ok_card.strip.y + ok_card.strip.h * 0.5,
        );
        let fail_strip = pixel(
            fail_card.strip.x + fail_card.strip.w * 0.5,
            fail_card.strip.y + fail_card.strip.h * 0.5,
        );
        assert!(
            close(ok_strip, ok_color),
            "{theme_name}: ok strip {ok_strip:?} != {ok_color:?}"
        );
        assert!(
            close(fail_strip, danger_color),
            "{theme_name}: fail strip {fail_strip:?} != {danger_color:?}"
        );
    }
}
