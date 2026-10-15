//! Pins the key tables behind the legacy and kitty encoders: function keys, Ctrl bytes,
//! shortcuts and keypad codes.
use alacritty_terminal::term::TermMode;
use twitty::input::{handle_key_mode_raw, handle_key_raw, terminal_key_raw, InputAction};
use winit::event::ElementState::{self, Pressed};
use winit::keyboard::{Key, KeyCode, ModifiersState as M, NamedKey, SmolStr};

fn ch(c: &str) -> Key {
    Key::Character(SmolStr::new(c))
}

fn legacy(physical: KeyCode, logical: Key, text: Option<&str>, m: M) -> Option<InputAction> {
    handle_key_raw(Some(physical), &logical, text, Pressed, m, false)
}

fn kitty(
    physical: KeyCode,
    logical: Key,
    state: ElementState,
    m: M,
    mode: TermMode,
) -> Option<Vec<u8>> {
    terminal_key_raw(Some(physical), &logical, None, None, state, false, m, mode)
}

fn bytes(b: &[u8]) -> Option<InputAction> {
    Some(InputAction::Bytes(b.to_vec()))
}

#[test]
fn legacy_function_keys() {
    let named = |k: NamedKey, m: M| legacy(KeyCode::F1, Key::Named(k), None, m);
    assert_eq!(named(NamedKey::F1, M::empty()), bytes(b"\x1bOP"));
    assert_eq!(named(NamedKey::F3, M::empty()), bytes(b"\x1bOR"));
    assert_eq!(named(NamedKey::F3, M::SHIFT), bytes(b"\x1b[1;2R"));
    assert_eq!(named(NamedKey::F5, M::empty()), bytes(b"\x1b[15~"));
    assert_eq!(named(NamedKey::F12, M::CONTROL), bytes(b"\x1b[24;5~"));
    assert_eq!(named(NamedKey::Home, M::empty()), bytes(b"\x1b[H"));
    assert_eq!(named(NamedKey::Delete, M::ALT), bytes(b"\x1b[3;3~"));
    assert_eq!(
        handle_key_raw(
            None,
            &Key::Named(NamedKey::End),
            None,
            Pressed,
            M::empty(),
            true
        ),
        bytes(b"\x1bOF")
    );
}

#[test]
fn legacy_control_keys() {
    let named = |k: NamedKey, m: M| legacy(KeyCode::Enter, Key::Named(k), None, m);
    assert_eq!(named(NamedKey::Enter, M::ALT), bytes(b"\x1b\r"));
    assert_eq!(named(NamedKey::Backspace, M::empty()), bytes(b"\x7f"));
    assert_eq!(named(NamedKey::Backspace, M::CONTROL), bytes(b"\x08"));
    assert_eq!(named(NamedKey::Tab, M::SHIFT), bytes(b"\x1b[Z"));
    assert_eq!(named(NamedKey::Escape, M::empty()), bytes(b"\x1b"));
}

#[test]
fn ctrl_bytes_follow_the_physical_key() {
    // An Arabic layout: the logical key is not Latin, the physical key decides.
    assert_eq!(
        legacy(KeyCode::KeyD, ch("ي"), Some("ي"), M::CONTROL),
        bytes(b"\x04")
    );
    assert_eq!(
        legacy(KeyCode::Digit2, ch("2"), None, M::CONTROL),
        bytes(b"\x00")
    );
    assert_eq!(
        legacy(KeyCode::BracketRight, ch("]"), None, M::CONTROL | M::ALT),
        bytes(b"\x1b\x1d")
    );
}

#[test]
fn altgr_symbols_and_meta_text() {
    assert_eq!(
        legacy(KeyCode::KeyQ, ch("@"), Some("@"), M::CONTROL | M::ALT),
        bytes(b"@")
    );
    assert_eq!(
        legacy(KeyCode::KeyA, ch("a"), Some("a"), M::ALT),
        bytes(b"\x1ba")
    );
}

#[test]
fn shortcuts_follow_the_typed_letter() {
    let cs = M::CONTROL | M::SHIFT;
    // AZERTY: the key labelled C sits where QWERTY has... C, but A and Q swap.
    assert_eq!(
        legacy(KeyCode::KeyQ, ch("A"), None, M::CONTROL),
        bytes(b"\x01")
    );
    assert_eq!(
        legacy(KeyCode::KeyE, ch("E"), None, cs),
        Some(InputAction::SplitRight)
    );
    assert_eq!(
        legacy(KeyCode::NumpadAdd, ch("+"), None, M::CONTROL),
        Some(InputAction::ZoomIn)
    );
    assert_eq!(
        legacy(
            KeyCode::PageUp,
            Key::Named(NamedKey::PageUp),
            None,
            M::CONTROL
        ),
        Some(InputAction::PrevTab)
    );
    assert_eq!(
        legacy(KeyCode::Tab, Key::Named(NamedKey::Tab), None, cs),
        Some(InputAction::PrevTab)
    );
    // Ctrl+Shift+PageUp is not a shortcut.
    assert_ne!(
        legacy(KeyCode::PageUp, Key::Named(NamedKey::PageUp), None, cs),
        Some(InputAction::PrevTab)
    );
}

#[test]
fn kitty_function_and_keypad_keys() {
    let d = TermMode::DISAMBIGUATE_ESC_CODES;
    let all = TermMode::REPORT_ALL_KEYS_AS_ESC;
    let f3 = Key::Named(NamedKey::F3);
    assert_eq!(
        kitty(KeyCode::F3, f3.clone(), Pressed, M::empty(), d),
        Some(b"\x1b[13~".to_vec())
    );
    assert_eq!(
        kitty(KeyCode::F3, f3, Pressed, M::SHIFT, d),
        Some(b"\x1b[13;2~".to_vec())
    );
    assert_eq!(
        kitty(KeyCode::Numpad7, ch("7"), Pressed, M::empty(), all),
        Some(b"\x1b[57406u".to_vec())
    );
    // A base layout key is reported for a non-Latin layout.
    let alt = all | TermMode::REPORT_ALTERNATE_KEYS;
    assert_eq!(
        handle_key_mode_raw(
            Some(KeyCode::Semicolon),
            &ch("ك"),
            Some(&ch("ك")),
            None,
            Pressed,
            false,
            M::empty(),
            alt,
        ),
        bytes("\x1b[1603::59u".as_bytes())
    );
}
