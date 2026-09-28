use twitty::input::{handle_key_raw, InputAction};
use winit::event::ElementState;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey};

#[test]
fn test_arboard_clipboard() {
    match arboard::Clipboard::new() {
        Ok(mut cb) => {
            let t = "twitty_test_clipboard";
            let _ = cb.set_text(t);
            if let Ok(read) = cb.get_text() {
                assert_eq!(read, t);
            }
        }
        Err(e) => {
            eprintln!(
                "Notice: system clipboard not available in headless/CI environment ({:?}), skipping live interaction.",
                e
            );
        }
    }
}

#[test]
fn test_ctrl_c_physical_key() {
    let mut modifiers = ModifiersState::empty();
    modifiers.insert(ModifiersState::CONTROL);

    let action = handle_key_raw(
        Some(KeyCode::KeyC),
        &Key::Named(NamedKey::Copy),
        None,
        ElementState::Pressed,
        modifiers,
        false,
    );
    assert_eq!(
        action,
        Some(InputAction::CopyOrInterrupt),
        "Ctrl+C must produce CopyOrInterrupt (copy if selection, or SIGINT if empty)"
    );
}

#[test]
fn test_zoom_shortcuts() {
    let mut modifiers = ModifiersState::empty();
    modifiers.insert(ModifiersState::CONTROL);

    // Ctrl + Equal (Zoom In)
    let action = handle_key_raw(
        Some(KeyCode::Equal),
        &Key::Character("=".into()),
        None,
        ElementState::Pressed,
        modifiers,
        false,
    );
    assert_eq!(action, Some(InputAction::ZoomIn));

    // Ctrl + Minus (Zoom Out)
    let action = handle_key_raw(
        Some(KeyCode::Minus),
        &Key::Character("-".into()),
        None,
        ElementState::Pressed,
        modifiers,
        false,
    );
    assert_eq!(action, Some(InputAction::ZoomOut));
}

#[test]
fn test_clipboard_shortcuts() {
    let mut modifiers = ModifiersState::empty();
    modifiers.insert(ModifiersState::CONTROL);
    modifiers.insert(ModifiersState::SHIFT);

    let action = handle_key_raw(
        Some(KeyCode::KeyV),
        &Key::Character("V".into()),
        None,
        ElementState::Pressed,
        modifiers,
        false,
    );
    assert_eq!(action, Some(InputAction::Paste));

    // Ctrl+V without shift must also paste
    let mut ctrl_only = ModifiersState::empty();
    ctrl_only.insert(ModifiersState::CONTROL);
    let action_no_shift = handle_key_raw(
        Some(KeyCode::KeyV),
        &Key::Character("v".into()),
        None,
        ElementState::Pressed,
        ctrl_only,
        false,
    );
    assert_eq!(action_no_shift, Some(InputAction::Paste));

    // Ctrl+X must trigger Cut
    let action_cut = handle_key_raw(
        Some(KeyCode::KeyX),
        &Key::Character("x".into()),
        None,
        ElementState::Pressed,
        ctrl_only,
        false,
    );
    assert_eq!(action_cut, Some(InputAction::Cut));

    // Ctrl+Z must trigger Undo / SIGTSTP (byte 26)
    let action_undo = handle_key_raw(
        Some(KeyCode::KeyZ),
        &Key::Character("z".into()),
        None,
        ElementState::Pressed,
        ctrl_only,
        false,
    );
    assert_eq!(action_undo, Some(InputAction::Bytes(vec![26])));

    // Arabic keyboard layout verification (Physical key unidentified or Arabic logical)
    let action_ar_c = handle_key_raw(
        None,
        &Key::Character("ؤ".into()),
        None,
        ElementState::Pressed,
        ctrl_only,
        false,
    );
    assert_eq!(action_ar_c, Some(InputAction::CopyOrInterrupt));

    let action_ar_v = handle_key_raw(
        None,
        &Key::Character("ر".into()),
        None,
        ElementState::Pressed,
        ctrl_only,
        false,
    );
    assert_eq!(action_ar_v, Some(InputAction::Paste));

    let action_ar_x = handle_key_raw(
        None,
        &Key::Character("ء".into()),
        None,
        ElementState::Pressed,
        ctrl_only,
        false,
    );
    assert_eq!(action_ar_x, Some(InputAction::Cut));

    let action_ar_z = handle_key_raw(
        None,
        &Key::Character("ئ".into()),
        None,
        ElementState::Pressed,
        ctrl_only,
        false,
    );
    assert_eq!(action_ar_z, Some(InputAction::Bytes(vec![26])));
}
