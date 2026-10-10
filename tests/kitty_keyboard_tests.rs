use alacritty_terminal::term::TermMode;
use std::sync::{Arc, Mutex};
use twitty::input::{handle_key_mode_raw, handle_key_raw, InputAction};
use twitty::terminal::Terminal;
use winit::event::ElementState;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, SmolStr};

const D: TermMode = TermMode::DISAMBIGUATE_ESC_CODES;
const EV: TermMode = TermMode::REPORT_EVENT_TYPES;
const ALL: TermMode = TermMode::REPORT_ALL_KEYS_AS_ESC;

fn mods(list: &[ModifiersState]) -> ModifiersState {
    list.iter().fold(ModifiersState::empty(), |acc, m| acc | *m)
}

fn ch(c: &str) -> Key {
    Key::Character(SmolStr::new(c))
}

fn key(
    physical: KeyCode,
    logical: Key,
    text: Option<&str>,
    state: ElementState,
    m: ModifiersState,
    mode: TermMode,
) -> Option<InputAction> {
    let unshifted = match &logical {
        Key::Character(s) => Key::Character(SmolStr::new(s.to_lowercase())),
        other => other.clone(),
    };
    handle_key_mode_raw(
        Some(physical),
        &logical,
        Some(&unshifted),
        text,
        state,
        false,
        m,
        mode,
    )
}

fn bytes(s: &str) -> Option<InputAction> {
    Some(InputAction::Bytes(s.as_bytes().to_vec()))
}

fn press(
    physical: KeyCode,
    logical: Key,
    m: ModifiersState,
    mode: TermMode,
) -> Option<InputAction> {
    key(physical, logical, None, ElementState::Pressed, m, mode)
}

#[test]
fn shift_enter_is_csi_u_with_disambiguate() {
    let r = press(
        KeyCode::Enter,
        Key::Named(NamedKey::Enter),
        ModifiersState::SHIFT,
        D,
    );
    assert_eq!(r, bytes("\x1b[13;2u"));
}

#[test]
fn plain_enter_stays_legacy() {
    let r = press(
        KeyCode::Enter,
        Key::Named(NamedKey::Enter),
        ModifiersState::empty(),
        D,
    );
    assert_eq!(r, bytes("\r"));
}

#[test]
fn ctrl_tab_and_shift_tab() {
    let r = press(
        KeyCode::Tab,
        Key::Named(NamedKey::Tab),
        ModifiersState::CONTROL,
        D,
    );
    assert_eq!(r, bytes("\x1b[9;5u"));
    let r = press(
        KeyCode::Tab,
        Key::Named(NamedKey::Tab),
        ModifiersState::empty(),
        D,
    );
    assert_eq!(r, bytes("\t"));
}

#[test]
fn escape_is_csi_u() {
    let r = press(
        KeyCode::Escape,
        Key::Named(NamedKey::Escape),
        ModifiersState::empty(),
        D,
    );
    assert_eq!(r, bytes("\x1b[27u"));
}

#[test]
fn ctrl_a_and_alt_a() {
    let r = press(KeyCode::KeyA, ch("a"), ModifiersState::CONTROL, D);
    assert_eq!(r, bytes("\x1b[97;5u"));
    let r = press(KeyCode::KeyA, ch("a"), ModifiersState::ALT, D);
    assert_eq!(r, bytes("\x1b[97;3u"));
}

#[test]
fn plain_text_stays_text_with_disambiguate_only() {
    let r = key(
        KeyCode::KeyA,
        ch("a"),
        Some("a"),
        ElementState::Pressed,
        ModifiersState::empty(),
        D,
    );
    assert_eq!(r, bytes("a"));
    let r = key(
        KeyCode::KeyA,
        ch("A"),
        Some("A"),
        ElementState::Pressed,
        ModifiersState::SHIFT,
        D,
    );
    assert_eq!(r, bytes("A"));
}

#[test]
fn report_all_keys_encodes_plain_text() {
    let r = key(
        KeyCode::KeyA,
        ch("a"),
        Some("a"),
        ElementState::Pressed,
        ModifiersState::empty(),
        ALL,
    );
    assert_eq!(r, bytes("\x1b[97u"));
    // Enter and Backspace too.
    let r = press(
        KeyCode::Enter,
        Key::Named(NamedKey::Enter),
        ModifiersState::empty(),
        ALL,
    );
    assert_eq!(r, bytes("\x1b[13u"));
}

#[test]
fn alternate_keys_and_associated_text() {
    let mode = ALL | TermMode::REPORT_ALTERNATE_KEYS | TermMode::REPORT_ASSOCIATED_TEXT;
    let r = key(
        KeyCode::KeyA,
        ch("A"),
        Some("A"),
        ElementState::Pressed,
        ModifiersState::SHIFT,
        mode,
    );
    assert_eq!(r, bytes("\x1b[97:65;2;65u"));
    let r = key(
        KeyCode::KeyA,
        ch("a"),
        Some("a"),
        ElementState::Pressed,
        ModifiersState::empty(),
        mode,
    );
    assert_eq!(r, bytes("\x1b[97;;97u"));
}

#[test]
fn release_event_reported_with_event_types() {
    let m = ModifiersState::CONTROL;
    let r = key(
        KeyCode::KeyA,
        ch("a"),
        None,
        ElementState::Released,
        m,
        D | EV,
    );
    assert_eq!(r, bytes("\x1b[97;5:3u"));
    let r = key(
        KeyCode::Escape,
        Key::Named(NamedKey::Escape),
        None,
        ElementState::Released,
        ModifiersState::empty(),
        D | EV,
    );
    assert_eq!(r, bytes("\x1b[27;1:3u"));
    // Without flag 2 releases are dropped.
    let r = key(KeyCode::KeyA, ch("a"), None, ElementState::Released, m, D);
    assert_eq!(r, None);
    // Plain Enter has no release without report-all.
    let r = key(
        KeyCode::Enter,
        Key::Named(NamedKey::Enter),
        None,
        ElementState::Released,
        ModifiersState::empty(),
        D | EV,
    );
    assert_eq!(r, None);
}

#[test]
fn repeat_event_type() {
    let r = handle_key_mode_raw(
        Some(KeyCode::KeyA),
        &ch("a"),
        Some(&ch("a")),
        None,
        ElementState::Pressed,
        true,
        ModifiersState::CONTROL,
        D | EV,
    );
    assert_eq!(r, bytes("\x1b[97;5:2u"));
}

#[test]
fn functional_keys() {
    let m = ModifiersState::SHIFT;
    assert_eq!(
        press(KeyCode::ArrowUp, Key::Named(NamedKey::ArrowUp), m, D),
        bytes("\x1b[1;2A")
    );
    assert_eq!(
        press(
            KeyCode::ArrowUp,
            Key::Named(NamedKey::ArrowUp),
            ModifiersState::empty(),
            D
        ),
        bytes("\x1b[A")
    );
    assert_eq!(
        press(KeyCode::Delete, Key::Named(NamedKey::Delete), m, D),
        bytes("\x1b[3;2~")
    );
    assert_eq!(
        press(KeyCode::F5, Key::Named(NamedKey::F5), m, D),
        bytes("\x1b[15;2~")
    );
    assert_eq!(
        press(KeyCode::F1, Key::Named(NamedKey::F1), m, D),
        bytes("\x1b[1;2P")
    );
}

#[test]
fn app_shortcuts_survive_kitty_modes() {
    let m = mods(&[ModifiersState::CONTROL, ModifiersState::SHIFT]);
    let all = TermMode::KITTY_KEYBOARD_PROTOCOL;
    assert_eq!(
        press(KeyCode::KeyC, ch("C"), m, all),
        Some(InputAction::Copy)
    );
    assert_eq!(
        press(KeyCode::KeyV, ch("V"), m, all),
        Some(InputAction::Paste)
    );
    assert_eq!(
        press(KeyCode::Equal, ch("="), ModifiersState::CONTROL, all),
        Some(InputAction::ZoomIn)
    );
    assert_eq!(
        press(KeyCode::Digit0, ch("0"), ModifiersState::CONTROL, all),
        Some(InputAction::ZoomReset)
    );
}

#[test]
fn legacy_unchanged_without_flags() {
    let cases = [
        (
            KeyCode::Enter,
            Key::Named(NamedKey::Enter),
            ModifiersState::SHIFT,
        ),
        (
            KeyCode::Escape,
            Key::Named(NamedKey::Escape),
            ModifiersState::empty(),
        ),
        (KeyCode::KeyA, ch("a"), ModifiersState::CONTROL),
        (
            KeyCode::Tab,
            Key::Named(NamedKey::Tab),
            ModifiersState::CONTROL,
        ),
        (
            KeyCode::ArrowUp,
            Key::Named(NamedKey::ArrowUp),
            ModifiersState::empty(),
        ),
    ];
    for (p, l, m) in cases {
        for app_cursor in [false, true] {
            let mode = if app_cursor {
                TermMode::APP_CURSOR
            } else {
                TermMode::empty()
            };
            let new = handle_key_mode_raw(
                Some(p),
                &l,
                Some(&l),
                None,
                ElementState::Pressed,
                false,
                m,
                mode,
            );
            let old = handle_key_raw(Some(p), &l, None, ElementState::Pressed, m, app_cursor);
            assert_eq!(new, old);
        }
    }
    assert_eq!(
        press(
            KeyCode::Enter,
            Key::Named(NamedKey::Enter),
            ModifiersState::SHIFT,
            TermMode::empty()
        ),
        bytes("\r")
    );
}

#[test]
fn terminal_push_pop_and_query() {
    let replies = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = replies.clone();
    let mut term = Terminal::new(80, 24, move |s| sink.lock().unwrap().push(s));

    assert!(!term.mode().contains(D));
    term.process_bytes(b"\x1b[>1u");
    assert!(term.mode().contains(D));

    term.process_bytes(b"\x1b[?u");
    assert_eq!(replies.lock().unwrap().last().unwrap(), "\x1b[?1u");

    term.process_bytes(b"\x1b[>3u");
    assert!(term.mode().contains(D | EV));

    term.process_bytes(b"\x1b[<u");
    assert!(term.mode().contains(D));
    assert!(!term.mode().contains(EV));

    term.process_bytes(b"\x1b[<u");
    assert!(!term.mode().intersects(TermMode::KITTY_KEYBOARD_PROTOCOL));
}
