//! Regressions found while auditing key, shortcut and link handling.
use alacritty_terminal::term::TermMode;
use twitty::input::{handle_key_mode_raw, InputAction};
use winit::event::ElementState;
use winit::keyboard::{Key, KeyCode, ModifiersState};

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
