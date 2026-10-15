//! Regressions found while auditing key, shortcut and link handling.
use alacritty_terminal::term::TermMode;
use twitty::input::{handle_key_mode_raw, terminal_key_raw, InputAction};
use winit::event::ElementState;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey};

const D: TermMode = TermMode::DISAMBIGUATE_ESC_CODES;
const EV: TermMode = TermMode::REPORT_EVENT_TYPES;

fn key(
    physical: KeyCode,
    logical: Key,
    state: ElementState,
    m: ModifiersState,
    mode: TermMode,
) -> Option<InputAction> {
    handle_key_mode_raw(Some(physical), &logical, None, None, state, false, m, mode)
}

fn bytes(b: &[u8]) -> Option<InputAction> {
    Some(InputAction::Bytes(b.to_vec()))
}

#[test]
fn ctrl_letter_follows_the_typed_latin_letter() {
    let ctrl = ModifiersState::CONTROL;
    // AZERTY: the key labelled A sits where US has Q. Ctrl+A must still be ^A, not ^Q.
    assert_eq!(
        key(
            KeyCode::KeyQ,
            Key::Character("a".into()),
            ElementState::Pressed,
            ctrl,
            TermMode::empty()
        ),
        bytes(&[0x01])
    );
    // Ctrl+Alt+W on AZERTY (physical Z).
    assert_eq!(
        key(
            KeyCode::KeyZ,
            Key::Character("w".into()),
            ElementState::Pressed,
            ctrl | ModifiersState::ALT,
            TermMode::empty()
        ),
        bytes(&[0x1b, 0x17])
    );
    // Arabic layout: no Latin letter typed, so the physical key decides.
    assert_eq!(
        key(
            KeyCode::KeyC,
            Key::Character("ؤ".into()),
            ElementState::Pressed,
            ctrl,
            TermMode::empty()
        ),
        bytes(&[0x03])
    );
}

#[test]
fn release_of_a_consumed_shortcut_is_not_reported() {
    let cs = ModifiersState::CONTROL | ModifiersState::SHIFT;
    let t = Key::Character("T".into());
    assert_eq!(
        key(KeyCode::KeyT, t.clone(), ElementState::Pressed, cs, D | EV),
        Some(InputAction::NewTab)
    );
    // The program never saw the press, so it must not see the release either.
    assert_eq!(
        key(KeyCode::KeyT, t, ElementState::Released, cs, D | EV),
        None
    );
    // Releases of ordinary keys are still reported.
    assert_eq!(
        key(
            KeyCode::KeyA,
            Key::Character("a".into()),
            ElementState::Released,
            ModifiersState::CONTROL,
            D | EV
        ),
        bytes(b"\x1b[97;5:3u")
    );
}

fn passthrough(physical: KeyCode, logical: Key, m: ModifiersState, mode: TermMode) -> Vec<u8> {
    terminal_key_raw(
        Some(physical),
        &logical,
        None,
        None,
        ElementState::Pressed,
        false,
        m,
        mode,
    )
    .expect("key should reach the program")
}

#[test]
fn layout_keys_can_be_sent_to_the_program() {
    let alt = ModifiersState::ALT;
    let ctrl = ModifiersState::CONTROL;
    let left = Key::Named(NamedKey::ArrowLeft);
    assert_eq!(
        passthrough(KeyCode::ArrowLeft, left.clone(), alt, TermMode::empty()),
        b"\x1b[1;3D"
    );
    assert_eq!(passthrough(KeyCode::ArrowLeft, left, alt, D), b"\x1b[1;3D");
    assert_eq!(
        passthrough(
            KeyCode::PageUp,
            Key::Named(NamedKey::PageUp),
            ctrl,
            TermMode::empty()
        ),
        b"\x1b[5;5~"
    );
    assert_eq!(
        passthrough(KeyCode::Tab, Key::Named(NamedKey::Tab), ctrl, D),
        b"\x1b[9;5u"
    );
}

fn url(text: &str, i: usize) -> Option<String> {
    twitty::links::find_url_at(text, i).map(|(_, _, u)| u)
}

#[test]
fn arabic_punctuation_and_closing_quotes_end_a_url() {
    // Arabic comma, semicolon, question mark and full stop right after a link.
    for (text, end) in [
        ("زر https://example.com/a، ثم", '،'),
        ("https://example.com/a؛", '؛'),
        ("هل رأيت https://example.com/a؟", '؟'),
        ("https://example.com/a۔", '۔'),
    ] {
        let i = text.chars().position(|c| c == 'h').unwrap() + 3;
        assert_eq!(
            url(text, i).as_deref(),
            Some("https://example.com/a"),
            "trailing {end}"
        );
    }
    // Guillemets and curly quotes wrap links in Arabic and European text.
    assert_eq!(
        url("«https://example.com/a»", 3).as_deref(),
        Some("https://example.com/a")
    );
    assert_eq!(
        url("“https://example.com/a”.", 3).as_deref(),
        Some("https://example.com/a")
    );
    // Arabic text inside the path is kept.
    assert_eq!(
        url("https://ar.wikipedia.org/wiki/مصر", 3).as_deref(),
        Some("https://ar.wikipedia.org/wiki/مصر")
    );
}
