//! Application keypad mode (DECKPAM) and UTF-8 mouse coordinates (mode 1005).
use alacritty_terminal::term::TermMode;
use twitty::input::{encode_mouse_for_mode, handle_key_mode_raw, InputAction};
use winit::event::ElementState;
use winit::keyboard::{Key, KeyCode, ModifiersState, SmolStr};

fn numpad(code: KeyCode, text: &str, mode: TermMode) -> Option<InputAction> {
    let logical = Key::Character(SmolStr::new(text));
    handle_key_mode_raw(
        Some(code),
        &logical,
        None,
        Some(text),
        ElementState::Pressed,
        false,
        ModifiersState::empty(),
        mode,
    )
}

#[test]
fn numpad_sends_ss3_codes_in_application_keypad_mode() {
    let app = TermMode::APP_KEYPAD;
    for (code, text, want) in [
        (KeyCode::Numpad1, "1", "\x1bOq"),
        (KeyCode::Numpad0, "0", "\x1bOp"),
        (KeyCode::NumpadAdd, "+", "\x1bOk"),
        (KeyCode::NumpadDecimal, ".", "\x1bOn"),
    ] {
        assert_eq!(
            numpad(code, text, app),
            Some(InputAction::Bytes(want.as_bytes().to_vec())),
            "{code:?}"
        );
    }
    // Normal keypad mode: plain digits; the top-row digit is never affected.
    assert_eq!(
        numpad(KeyCode::Numpad1, "1", TermMode::NONE),
        Some(InputAction::Bytes(b"1".to_vec()))
    );
    assert_eq!(
        numpad(KeyCode::Digit1, "1", app),
        Some(InputAction::Bytes(b"1".to_vec()))
    );
}

#[test]
fn utf8_mouse_mode_reports_columns_past_223() {
    let m = ModifiersState::empty();
    let seq = encode_mouse_for_mode(0, 299, 4, true, m, TermMode::UTF8_MOUSE);
    let mut want = b"\x1b[M ".to_vec();
    want.extend_from_slice(char::from_u32(300 + 32).unwrap().to_string().as_bytes());
    want.push(5 + 32);
    assert_eq!(seq, want);
    // SGR wins when both are on; legacy stays capped.
    assert_eq!(
        encode_mouse_for_mode(
            0,
            299,
            4,
            true,
            m,
            TermMode::UTF8_MOUSE | TermMode::SGR_MOUSE
        ),
        b"\x1b[<0;300;5M"
    );
    assert_eq!(
        encode_mouse_for_mode(0, 299, 4, true, m, TermMode::NONE)[4],
        255
    );
}
