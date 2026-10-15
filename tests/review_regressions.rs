//! Regression tests for the 2026-10-10 code review (docs/code-review-2026-10-10.md).
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor};
use twitty::color::Palette;
use twitty::input::{encode_mouse, encode_paste, handle_key_raw, InputAction};
use twitty::render::cell_colors;
use twitty::terminal::{CellData, Terminal};
use winit::event::ElementState;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey};

fn row_text(cells: &[CellData]) -> String {
    let mut s = String::new();
    for c in cells {
        c.push_text(&mut s);
    }
    s.trim_end().to_string()
}

fn key(
    logical: Key,
    physical: Option<KeyCode>,
    text: Option<&str>,
    mods: ModifiersState,
) -> Option<InputAction> {
    handle_key_raw(physical, &logical, text, ElementState::Pressed, mods, false)
}

fn bytes(action: Option<InputAction>) -> Vec<u8> {
    match action {
        Some(InputAction::Bytes(b)) => b,
        other => panic!("expected bytes, got {other:?}"),
    }
}

// #1 — scrolled-back rows must be visible, not blank.
#[test]
fn scrollback_rows_are_rendered() {
    let mut term = Terminal::new(20, 4, |_| {});
    for i in 0..10 {
        term.process_bytes(format!("line{i}\r\n").as_bytes());
    }
    term.scroll_display(3);
    let (lines, cursor) = term.snapshot();
    assert_eq!(row_text(&lines[0].cells), "line4");
    assert_eq!(row_text(&lines[3].cells), "line7");
    // The live cursor (bottom row) is now below the viewport.
    assert!(!cursor.is_visible);

    term.scroll_to_bottom();
    let (lines, cursor) = term.snapshot();
    assert_eq!(row_text(&lines[0].cells), "line7");
    assert!(cursor.is_visible);
}

// #2 — Arabic harakat / combining marks must survive the snapshot.
#[test]
fn combining_marks_are_kept() {
    let mut term = Terminal::new(20, 4, |_| {});
    term.process_bytes("مَرْحَبًا".as_bytes());
    let (lines, _) = term.snapshot();
    assert_eq!(row_text(&lines[0].cells), "مَرْحَبًا");
    assert_eq!(lines[0].cells[0].zerowidth, vec!['\u{064E}']);
}

// #5 — wide chars occupy a spacer cell that contributes no text.
#[test]
fn wide_char_spacers_add_no_text() {
    let mut term = Terminal::new(20, 4, |_| {});
    term.process_bytes("中a".as_bytes());
    let (lines, _) = term.snapshot();
    assert!(lines[0].cells[1].is_spacer());
    assert_eq!(row_text(&lines[0].cells), "中a");
}

// #4 — reverse video, dim and hidden attributes.
#[test]
fn cell_flags_affect_colors() {
    let palette = Palette::default();
    let mut cell = CellData::blank();
    cell.fg = AnsiColor::Named(NamedColor::Red);

    cell.flags = Flags::INVERSE;
    let (fg, bg) = cell_colors(&palette, &cell);
    assert_eq!(fg, palette.background);
    assert_eq!(bg, palette.ansi[1]);

    cell.flags = Flags::HIDDEN;
    let (fg, bg) = cell_colors(&palette, &cell);
    assert_eq!(fg, bg);

    cell.flags = Flags::DIM;
    let (fg, _) = cell_colors(&palette, &cell);
    assert_ne!(fg, palette.ansi[1]);
}

// #6 — bracketed paste wraps text and strips ESC; plain paste maps newlines to CR.
#[test]
fn paste_encoding() {
    assert_eq!(
        encode_paste("ls\nrm -rf x\x1b[201~", true),
        b"\x1b[200~ls\nrm -rf x[201~\x1b[201~".to_vec()
    );
    assert_eq!(encode_paste("a\r\nb\nc", false), b"a\rb\rc".to_vec());
}

// #8 — Alt prefix, function keys, modified navigation, extra Ctrl combos.
#[test]
fn extended_key_encoding() {
    let none = ModifiersState::empty();
    let alt = ModifiersState::ALT;
    let ctrl = ModifiersState::CONTROL;

    assert_eq!(
        bytes(key(
            Key::Character("b".into()),
            Some(KeyCode::KeyB),
            Some("b"),
            alt
        )),
        b"\x1bb"
    );
    assert_eq!(
        bytes(key(Key::Named(NamedKey::F1), None, None, none)),
        b"\x1bOP"
    );
    assert_eq!(
        bytes(key(Key::Named(NamedKey::F5), None, None, none)),
        b"\x1b[15~"
    );
    assert_eq!(
        bytes(key(Key::Named(NamedKey::F12), None, None, ctrl)),
        b"\x1b[24;5~"
    );
    assert_eq!(
        bytes(key(Key::Named(NamedKey::ArrowLeft), None, None, ctrl)),
        b"\x1b[1;5D"
    );
    assert_eq!(
        bytes(key(
            Key::Named(NamedKey::Home),
            None,
            None,
            ModifiersState::SHIFT
        )),
        b"\x1b[1;2H"
    );
    assert_eq!(
        bytes(key(
            Key::Named(NamedKey::Space),
            Some(KeyCode::Space),
            Some(" "),
            ctrl
        )),
        vec![0]
    );
    assert_eq!(
        bytes(key(
            Key::Character("/".into()),
            Some(KeyCode::Slash),
            None,
            ctrl
        )),
        vec![0x1f]
    );
    assert_eq!(
        bytes(key(
            Key::Character("m".into()),
            Some(KeyCode::KeyM),
            None,
            ctrl
        )),
        vec![0x0d]
    );
    assert_eq!(
        bytes(key(
            Key::Named(NamedKey::Backspace),
            Some(KeyCode::Backspace),
            None,
            ctrl
        )),
        vec![0x08]
    );
    // Ctrl works by physical key even on an Arabic layout.
    assert_eq!(
        bytes(key(
            Key::Character("ؤ".into()),
            Some(KeyCode::KeyC),
            Some("ؤ"),
            ctrl
        )),
        vec![3]
    );
    // AltGr (Ctrl+Alt on Windows) symbols pass through.
    assert_eq!(
        bytes(key(
            Key::Character("@".into()),
            Some(KeyCode::KeyQ),
            Some("@"),
            ctrl | alt
        )),
        b"@"
    );
}

// #16 — SGR vs legacy mouse encoding with modifiers.
#[test]
fn mouse_encoding() {
    let none = ModifiersState::empty();
    assert_eq!(encode_mouse(0, 4, 9, true, none, true), b"\x1b[<0;5;10M");
    assert_eq!(encode_mouse(0, 4, 9, false, none, true), b"\x1b[<0;5;10m");
    assert_eq!(
        encode_mouse(64, 0, 0, true, ModifiersState::CONTROL, true),
        b"\x1b[<80;1;1M"
    );
    assert_eq!(
        encode_mouse(0, 4, 9, true, none, false),
        vec![0x1b, b'[', b'M', 32, 37, 42]
    );
    assert_eq!(
        encode_mouse(0, 4, 9, false, none, false),
        vec![0x1b, b'[', b'M', 35, 37, 42]
    );
    // Legacy coordinates are capped instead of overflowing.
    assert_eq!(encode_mouse(0, 500, 0, true, none, false)[4], 255);
}
