use twitty::input::{handle_key_raw, InputAction};
use winit::event::ElementState;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey};

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
        Some(InputAction::Bytes(vec![3])),
        "Ctrl+C must produce ASCII 3 (ETX) regardless of layout"
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
}
