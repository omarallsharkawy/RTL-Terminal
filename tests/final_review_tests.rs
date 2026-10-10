//! Regression tests for the final review fixes (paste, keys, mouse, OSC 52, shaping, cursor).
use alacritty_terminal::term::Osc52;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::CursorShape;
use cosmic_text::{Attrs, Family, FontSystem, Metrics};
use std::sync::{Arc, Mutex};
use twitty::color::{Palette, Rgba};
use twitty::config::TwittyConfig;
use twitty::input::{encode_paste, format_mouse_seq, handle_key_raw, InputAction};
use twitty::renderer::effective_cursor_shape;
use twitty::shaping::{cell_text_len, shape_row};
use twitty::terminal::{CursorState, Terminal};
use winit::event::ElementState;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey};

fn key(
    logical: Key,
    physical: Option<KeyCode>,
    text: Option<&str>,
    mods: ModifiersState,
) -> Vec<u8> {
    match handle_key_raw(physical, &logical, text, ElementState::Pressed, mods, false) {
        Some(InputAction::Bytes(b)) => b,
        other => panic!("expected bytes, got {other:?}"),
    }
}

#[test]
fn paste_does_not_double_windows_line_endings() {
    assert_eq!(encode_paste("a\r\nb\nc", false), "a\rb\rc");
}

#[test]
fn bracketed_paste_strips_escape_injection() {
    assert_eq!(
        encode_paste("ls\x1b[201~rm -rf ~\n", true),
        "\x1b[200~ls[201~rm -rf ~\n\x1b[201~"
    );
}

#[test]
fn modified_navigation_and_function_keys() {
    let ctrl = ModifiersState::CONTROL;
    let shift = ModifiersState::SHIFT;
    let alt = ModifiersState::ALT;
    let named = |k: NamedKey, m| key(Key::Named(k), None, None, m);

    assert_eq!(named(NamedKey::ArrowLeft, ctrl | shift), b"\x1b[1;6D");
    assert_eq!(named(NamedKey::ArrowUp, alt), b"\x1b\x1b[A");
    assert_eq!(named(NamedKey::ArrowRight, ctrl | alt), b"\x1b[1;7C");
    assert_eq!(named(NamedKey::Delete, ctrl), b"\x1b[3;5~");
    assert_eq!(named(NamedKey::PageUp, alt), b"\x1b\x1b[5~");
    assert_eq!(named(NamedKey::F1, shift), b"\x1b[1;2P");
    assert_eq!(named(NamedKey::F5, ModifiersState::empty()), b"\x1b[15~");
    assert_eq!(named(NamedKey::F12, ctrl), b"\x1b[24;5~");
    assert_eq!(named(NamedKey::Home, ModifiersState::empty()), b"\x1b[H");
}

#[test]
fn ctrl_backspace_slash_and_altgr() {
    let ctrl = ModifiersState::CONTROL;
    assert_eq!(
        key(
            Key::Named(NamedKey::Backspace),
            Some(KeyCode::Backspace),
            None,
            ctrl
        ),
        vec![0x08]
    );
    assert_eq!(
        key(
            Key::Named(NamedKey::Backspace),
            Some(KeyCode::Backspace),
            None,
            ModifiersState::empty()
        ),
        vec![0x7f]
    );
    assert_eq!(
        key(Key::Character("/".into()), Some(KeyCode::Slash), None, ctrl),
        vec![0x1f]
    );
    // AltGr+Q on a German layout (reported as Ctrl+Alt on Windows) must type '@'.
    assert_eq!(
        key(
            Key::Character("@".into()),
            Some(KeyCode::KeyQ),
            Some("@"),
            ctrl | ModifiersState::ALT
        ),
        b"@"
    );
    // Real Ctrl+Alt+letter still sends ESC + control code.
    assert_eq!(
        key(
            Key::Character("d".into()),
            Some(KeyCode::KeyD),
            Some("d"),
            ctrl | ModifiersState::ALT
        ),
        vec![0x1b, 4]
    );
}

#[test]
fn legacy_mouse_coordinates_are_capped_not_wrapped() {
    let seq = format_mouse_seq(TermMode::NONE, 0, 300, 5, false);
    assert_eq!(
        seq[4], 255,
        "column 300 must clamp to 223 (+32), not wrap around"
    );
    assert_eq!(seq[5], 37);
}

#[test]
fn osc52_policy_comes_from_config() {
    let mut config = TwittyConfig::default();
    assert_eq!(config.osc52_mode(), Osc52::CopyPaste);
    config.osc52 = Some("disabled".into());
    assert_eq!(config.osc52_mode(), Osc52::Disabled);
    config.osc52 = Some("only-copy".into());
    assert_eq!(config.osc52_mode(), Osc52::OnlyCopy);

    let stored = Arc::new(Mutex::new(None));
    let stored_clone = stored.clone();
    let mut term = Terminal::new_with_clipboard(
        80,
        24,
        |_| {},
        move |text| {
            *stored_clone.lock().unwrap() = Some(text);
        },
    );
    term.set_osc52(Osc52::Disabled);
    term.process_bytes(b"\x1b]52;c;dGVzdA==\x07");
    assert_eq!(
        *stored.lock().unwrap(),
        None,
        "disabled OSC 52 must not touch the clipboard"
    );
}

#[test]
fn cursor_shape_follows_style_and_focus() {
    assert_eq!(
        effective_cursor_shape(CursorShape::Block, "beam", true),
        CursorShape::Beam
    );
    assert_eq!(
        effective_cursor_shape(CursorShape::Block, "block", true),
        CursorShape::Block
    );
    // An explicit shape requested by the app (e.g. vim insert mode) wins over the style.
    assert_eq!(
        effective_cursor_shape(CursorShape::Underline, "beam", true),
        CursorShape::Underline
    );
    assert_eq!(
        effective_cursor_shape(CursorShape::Beam, "beam", false),
        CursorShape::HollowBlock
    );
    assert_eq!(
        effective_cursor_shape(CursorShape::Hidden, "beam", false),
        CursorShape::Hidden
    );
}

fn shape(
    text: &str,
    cursor: &CursorState,
) -> (Vec<twitty::terminal::LineData>, twitty::shaping::CachedRow) {
    let mut term = Terminal::new(40, 4, |_| {});
    term.process_bytes(text.as_bytes());
    let (lines, _) = term.snapshot();
    let mut font_system = FontSystem::new();
    let row = shape_row(
        &lines[0],
        0,
        cursor,
        9.0,
        23.0,
        8.0,
        Metrics::new(14.5, 23.0),
        &Attrs::new().family(Family::Monospace),
        &Palette::default(),
        &mut font_system,
        Rgba::from_rgb8(21, 22, 30),
    );
    (lines, row)
}

fn hidden_cursor() -> CursorState {
    CursorState {
        col: 0,
        row: 0,
        is_visible: false,
        shape: CursorShape::Hidden,
    }
}

#[test]
fn harakat_and_wide_spacers_map_to_shaped_bytes() {
    let mut term = Terminal::new(20, 4, |_| {});
    term.process_bytes("مَ中".as_bytes());
    let (lines, _) = term.snapshot();
    let cells = &lines[0].cells;
    assert_eq!(
        cell_text_len(&cells[0]),
        "مَ".len(),
        "base letter plus its fatha"
    );
    assert_eq!(cell_text_len(&cells[1]), "中".len());
    assert_eq!(
        cell_text_len(&cells[2]),
        0,
        "wide-char spacer contributes no text"
    );
}

#[test]
fn rtl_cells_with_harakat_get_distinct_positions() {
    let (_, row) = shape("مَرْحَبًا", &hidden_cursor());
    let seg = row
        .segments
        .iter()
        .find(|s| s.has_rtl)
        .expect("RTL segment");
    let xs: Vec<f32> = seg.cell_visual_x.iter().map(|&(x, _)| x).collect();
    // Arabic is laid out right-to-left: each later cell sits further left.
    for pair in xs.windows(2) {
        assert!(
            pair[1] < pair[0],
            "cell positions must follow glyphs, got {xs:?}"
        );
    }
}

#[test]
fn fallback_glyphs_do_not_shift_the_grid() {
    let (lines, row) = shape("ok ✓ 🚀 done", &hidden_cursor());
    let done_col = lines[0].cells.iter().position(|c| c.c == 'd').unwrap();
    // Find the glyph for 'd' wherever it was shaped, and check its absolute x.
    let absolute = row
        .segments
        .iter()
        .find_map(|seg| {
            let text = seg.buffer.lines[0].text().to_string();
            seg.buffer.layout_runs().find_map(|run| {
                run.glyphs
                    .iter()
                    .find(|g| text[g.start..].starts_with('d'))
                    .map(|g| seg.seg_x + g.x)
            })
        })
        .expect("glyph for 'd'");
    assert!(
        (absolute - (8.0 + done_col as f32 * 9.0)).abs() < 0.5,
        "'done' drifted off the grid: x={absolute}"
    );
}

fn caret(input: &str) -> ((f32, bool), twitty::shaping::CachedRow) {
    let mut term = Terminal::new(40, 4, |_| {});
    term.process_bytes(input.as_bytes());
    let (lines, cur) = term.snapshot();
    let cursor = CursorState {
        shape: CursorShape::Beam,
        ..cur
    };
    let mut font_system = FontSystem::new();
    let row = shape_row(
        &lines[0],
        0,
        &cursor,
        9.0,
        23.0,
        0.0,
        Metrics::new(14.5, 23.0),
        &Attrs::new().family(Family::Monospace),
        &Palette::default(),
        &mut font_system,
        Rgba::from_rgb8(21, 22, 30),
    );
    (row.cursor_anchor(cursor.col, 9.0, 0.0), row)
}

#[test]
fn caret_sits_left_of_typed_arabic_and_right_of_latin() {
    // "❯ " ends around x=18; the caret must sit right there, at the Arabic word's left edge.
    let ((x, extends_left), row) = caret("❯ هلا");
    let seg = row.segments.iter().find(|s| s.has_rtl).unwrap();
    let word_left = seg.cell_edges[2 - seg.seg_start..]
        .iter()
        .map(|e| e.0)
        .fold(f32::MAX, f32::min);
    assert!(
        extends_left && (x - word_left).abs() < 0.5 && x < 19.0,
        "Arabic caret at {x}"
    );

    let ((x, extends_left), _) = caret("❯ hello");
    assert!(
        !extends_left && (x - 63.0).abs() < 0.5,
        "Latin caret at {x}"
    );
}

#[test]
fn space_after_arabic_stays_on_its_left() {
    let ((x, extends_left), row) = caret("❯ هلا ");
    let seg = row.segments.iter().find(|s| s.has_rtl).unwrap();
    let space = seg.cell_edges.last().unwrap();
    // Column 2 holds the first Arabic letter ("❯" and a space come before it).
    let first_letter = seg.cell_edges[2 - seg.seg_start];
    assert!(
        space.0 < first_letter.0,
        "typed space must not jump right of the word"
    );
    assert!(
        extends_left && x <= first_letter.0,
        "caret stays left after the space"
    );
}
