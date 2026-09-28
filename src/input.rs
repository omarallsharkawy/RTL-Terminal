use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

#[derive(Debug, PartialEq, Eq)]
pub enum InputAction {
    Bytes(Vec<u8>),
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Copy,
    Paste,
}

pub fn handle_key(
    event: &KeyEvent,
    modifiers: ModifiersState,
    app_cursor: bool,
) -> Option<InputAction> {
    let physical = match event.physical_key {
        PhysicalKey::Code(c) => Some(c),
        _ => None,
    };
    handle_key_raw(
        physical,
        &event.logical_key,
        event.text.as_deref(),
        event.state,
        modifiers,
        app_cursor,
    )
}

pub fn handle_key_raw(
    physical: Option<KeyCode>,
    logical: &Key,
    text: Option<&str>,
    state: ElementState,
    modifiers: ModifiersState,
    app_cursor: bool,
) -> Option<InputAction> {
    if !state.is_pressed() {
        return None;
    }

    let ctrl = modifiers.control_key();
    let shift = modifiers.shift_key();

    // 1. Physical Key shortcuts with Ctrl (Layout-independent! Works in Arabic & English layout!)
    if let Some(key_code) = physical {
        if ctrl {
            if shift {
                match key_code {
                    KeyCode::KeyC => return Some(InputAction::Copy),
                    KeyCode::KeyV => return Some(InputAction::Paste),
                    _ => {}
                }
            } else {
                match key_code {
                    // Zoom
                    KeyCode::Equal | KeyCode::NumpadAdd => return Some(InputAction::ZoomIn),
                    KeyCode::Minus | KeyCode::NumpadSubtract => return Some(InputAction::ZoomOut),
                    KeyCode::Digit0 | KeyCode::Numpad0 => return Some(InputAction::ZoomReset),

                    KeyCode::KeyC => return Some(InputAction::Bytes(vec![3])),  // SIGINT / Ctrl+C
                    KeyCode::KeyD => return Some(InputAction::Bytes(vec![4])),  // EOF / Ctrl+D
                    KeyCode::KeyZ => return Some(InputAction::Bytes(vec![26])), // SIGTSTP / Ctrl+Z
                    KeyCode::KeyL => return Some(InputAction::Bytes(vec![12])), // Clear screen
                    KeyCode::KeyA => return Some(InputAction::Bytes(vec![1])),
                    KeyCode::KeyB => return Some(InputAction::Bytes(vec![2])),
                    KeyCode::KeyE => return Some(InputAction::Bytes(vec![5])),
                    KeyCode::KeyF => return Some(InputAction::Bytes(vec![6])),
                    KeyCode::KeyG => return Some(InputAction::Bytes(vec![7])),
                    KeyCode::KeyH => return Some(InputAction::Bytes(vec![8])),
                    KeyCode::KeyI => return Some(InputAction::Bytes(vec![9])),
                    KeyCode::KeyJ => return Some(InputAction::Bytes(vec![10])),
                    KeyCode::KeyK => return Some(InputAction::Bytes(vec![11])),
                    KeyCode::KeyN => return Some(InputAction::Bytes(vec![14])),
                    KeyCode::KeyO => return Some(InputAction::Bytes(vec![15])),
                    KeyCode::KeyP => return Some(InputAction::Bytes(vec![16])), // Command palette
                    KeyCode::KeyQ => return Some(InputAction::Bytes(vec![17])),
                    KeyCode::KeyR => return Some(InputAction::Bytes(vec![18])),
                    KeyCode::KeyS => return Some(InputAction::Bytes(vec![19])),
                    KeyCode::KeyT => return Some(InputAction::Bytes(vec![20])),
                    KeyCode::KeyU => return Some(InputAction::Bytes(vec![21])),
                    KeyCode::KeyV => return Some(InputAction::Bytes(vec![22])),
                    KeyCode::KeyW => return Some(InputAction::Bytes(vec![23])),
                    KeyCode::KeyX => return Some(InputAction::Bytes(vec![24])),
                    KeyCode::KeyY => return Some(InputAction::Bytes(vec![25])),
                    KeyCode::BracketLeft => return Some(InputAction::Bytes(vec![27])),
                    KeyCode::Backslash => return Some(InputAction::Bytes(vec![28])),
                    KeyCode::BracketRight => return Some(InputAction::Bytes(vec![29])),
                    _ => {}
                }
            }
        }
    }

    // 2. Named Navigation / Control Keys
    match logical {
        Key::Named(named) => match named {
            NamedKey::Enter => return Some(InputAction::Bytes(vec![b'\r'])),
            NamedKey::Backspace => return Some(InputAction::Bytes(vec![0x7f])),
            NamedKey::Tab => {
                if shift {
                    return Some(InputAction::Bytes(b"\x1b[Z".to_vec()));
                } else {
                    return Some(InputAction::Bytes(vec![b'\t']));
                }
            }
            NamedKey::Escape => return Some(InputAction::Bytes(vec![0x1b])),
            NamedKey::ArrowUp => {
                if app_cursor {
                    return Some(InputAction::Bytes(b"\x1bOA".to_vec()));
                } else {
                    return Some(InputAction::Bytes(b"\x1b[A".to_vec()));
                }
            }
            NamedKey::ArrowDown => {
                if app_cursor {
                    return Some(InputAction::Bytes(b"\x1bOB".to_vec()));
                } else {
                    return Some(InputAction::Bytes(b"\x1b[B".to_vec()));
                }
            }
            NamedKey::ArrowRight => {
                if app_cursor {
                    return Some(InputAction::Bytes(b"\x1bOC".to_vec()));
                } else {
                    return Some(InputAction::Bytes(b"\x1b[C".to_vec()));
                }
            }
            NamedKey::ArrowLeft => {
                if app_cursor {
                    return Some(InputAction::Bytes(b"\x1bOD".to_vec()));
                } else {
                    return Some(InputAction::Bytes(b"\x1b[D".to_vec()));
                }
            }
            NamedKey::Home => {
                if app_cursor {
                    return Some(InputAction::Bytes(b"\x1bOH".to_vec()));
                } else {
                    return Some(InputAction::Bytes(b"\x1b[H".to_vec()));
                }
            }
            NamedKey::End => {
                if app_cursor {
                    return Some(InputAction::Bytes(b"\x1bOF".to_vec()));
                } else {
                    return Some(InputAction::Bytes(b"\x1b[F".to_vec()));
                }
            }
            NamedKey::PageUp => return Some(InputAction::Bytes(b"\x1b[5~".to_vec())),
            NamedKey::PageDown => return Some(InputAction::Bytes(b"\x1b[6~".to_vec())),
            NamedKey::Delete => return Some(InputAction::Bytes(b"\x1b[3~".to_vec())),
            NamedKey::Insert => return Some(InputAction::Bytes(b"\x1b[2~".to_vec())),
            _ => {}
        },
        _ => {}
    }

    // 3. Normal typed text (Arabic, Latin, numbers, symbols)
    if !ctrl {
        if let Some(t) = text {
            if !t.is_empty() {
                return Some(InputAction::Bytes(t.as_bytes().to_vec()));
            }
        }
    }

    None
}
