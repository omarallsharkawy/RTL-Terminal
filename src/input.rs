use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};

pub fn handle_key(event: &KeyEvent, modifiers: ModifiersState, app_cursor: bool) -> Option<Vec<u8>> {
    if !event.state.is_pressed() {
        return None;
    }

    let ctrl = modifiers.control_key();

    if ctrl {
        match &event.logical_key {
            Key::Character(ch) => {
                if let Some(c) = ch.chars().next() {
                    let c_lower = c.to_ascii_lowercase();
                    if c_lower >= 'a' && c_lower <= 'z' {
                        let ascii_code = (c_lower as u8) - b'a' + 1;
                        return Some(vec![ascii_code]);
                    }
                    if c == '@' {
                        return Some(vec![0]);
                    }
                    if c == '[' {
                        return Some(vec![27]);
                    }
                    if c == '\\' {
                        return Some(vec![28]);
                    }
                    if c == ']' {
                        return Some(vec![29]);
                    }
                    if c == '^' {
                        return Some(vec![30]);
                    }
                    if c == '_' {
                        return Some(vec![31]);
                    }
                }
            }
            _ => {}
        }
    }

    match &event.logical_key {
        Key::Named(named) => match named {
            NamedKey::Enter => return Some(vec![b'\r']),
            NamedKey::Backspace => return Some(vec![0x7f]),
            NamedKey::Tab => {
                if modifiers.shift_key() {
                    return Some(b"\x1b[Z".to_vec());
                } else {
                    return Some(vec![b'\t']);
                }
            }
            NamedKey::Escape => return Some(vec![0x1b]),
            NamedKey::ArrowUp => {
                if app_cursor {
                    return Some(b"\x1bOA".to_vec());
                } else {
                    return Some(b"\x1b[A".to_vec());
                }
            }
            NamedKey::ArrowDown => {
                if app_cursor {
                    return Some(b"\x1bOB".to_vec());
                } else {
                    return Some(b"\x1b[B".to_vec());
                }
            }
            NamedKey::ArrowRight => {
                if app_cursor {
                    return Some(b"\x1bOC".to_vec());
                } else {
                    return Some(b"\x1b[C".to_vec());
                }
            }
            NamedKey::ArrowLeft => {
                if app_cursor {
                    return Some(b"\x1bOD".to_vec());
                } else {
                    return Some(b"\x1b[D".to_vec());
                }
            }
            NamedKey::Home => {
                if app_cursor {
                    return Some(b"\x1bOH".to_vec());
                } else {
                    return Some(b"\x1b[H".to_vec());
                }
            }
            NamedKey::End => {
                if app_cursor {
                    return Some(b"\x1bOF".to_vec());
                } else {
                    return Some(b"\x1b[F".to_vec());
                }
            }
            NamedKey::PageUp => return Some(b"\x1b[5~".to_vec()),
            NamedKey::PageDown => return Some(b"\x1b[6~".to_vec()),
            NamedKey::Delete => return Some(b"\x1b[3~".to_vec()),
            NamedKey::Insert => return Some(b"\x1b[2~".to_vec()),
            _ => {}
        },
        _ => {}
    }

    if !ctrl {
        if let Some(ref text) = event.text {
            if !text.is_empty() {
                return Some(text.as_bytes().to_vec());
            }
        }
    }

    None
}
