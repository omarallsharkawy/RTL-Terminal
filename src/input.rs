use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

#[derive(Debug, PartialEq, Eq)]
pub enum InputAction {
    Bytes(Vec<u8>),
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Copy,
    CopyOrInterrupt,
    Paste,
    Cut,
    SelectAll,
}

fn match_ctrl_letter(physical: Option<KeyCode>, logical: &Key) -> Option<char> {
    if let Some(code) = physical {
        match code {
            KeyCode::KeyA => return Some('a'),
            KeyCode::KeyB => return Some('b'),
            KeyCode::KeyC => return Some('c'),
            KeyCode::KeyD => return Some('d'),
            KeyCode::KeyE => return Some('e'),
            KeyCode::KeyF => return Some('f'),
            KeyCode::KeyG => return Some('g'),
            KeyCode::KeyH => return Some('h'),
            KeyCode::KeyI => return Some('i'),
            KeyCode::KeyJ => return Some('j'),
            KeyCode::KeyK => return Some('k'),
            KeyCode::KeyL => return Some('l'),
            KeyCode::KeyM => return Some('m'),
            KeyCode::KeyN => return Some('n'),
            KeyCode::KeyO => return Some('o'),
            KeyCode::KeyP => return Some('p'),
            KeyCode::KeyQ => return Some('q'),
            KeyCode::KeyR => return Some('r'),
            KeyCode::KeyS => return Some('s'),
            KeyCode::KeyT => return Some('t'),
            KeyCode::KeyU => return Some('u'),
            KeyCode::KeyV => return Some('v'),
            KeyCode::KeyW => return Some('w'),
            KeyCode::KeyX => return Some('x'),
            KeyCode::KeyY => return Some('y'),
            KeyCode::KeyZ => return Some('z'),
            _ => {}
        }
    }

    match logical {
        Key::Character(s) => {
            if s == "لا" || s == "\u{FEFB}" {
                return Some('b');
            }
            let ch = s.chars().next()?;
            match ch {
                'a' | 'A' | '\u{1}' | 'ش' => Some('a'),
                'b' | 'B' | '\u{2}' => Some('b'),
                'c' | 'C' | '\u{3}' | 'ؤ' => Some('c'),
                'd' | 'D' | '\u{4}' | 'ي' => Some('d'),
                'e' | 'E' | '\u{5}' | 'ث' => Some('e'),
                'f' | 'F' | '\u{6}' | 'ب' => Some('f'),
                'g' | 'G' | '\u{7}' | 'ل' => Some('g'),
                'h' | 'H' | '\u{8}' | 'ا' => Some('h'),
                'i' | 'I' | '\t' | 'ه' => Some('i'),
                'j' | 'J' | '\n' | 'ت' => Some('j'),
                'k' | 'K' | '\u{b}' | 'ن' => Some('k'),
                'l' | 'L' | '\u{c}' | 'م' => Some('l'),
                'm' | 'M' | '\r' | 'ة' => Some('m'),
                'n' | 'N' | '\u{e}' | 'ى' => Some('n'),
                'o' | 'O' | '\u{f}' | 'خ' => Some('o'),
                'p' | 'P' | '\u{10}' | 'ح' => Some('p'),
                'q' | 'Q' | '\u{11}' | 'ض' => Some('q'),
                'r' | 'R' | '\u{12}' | 'ق' => Some('r'),
                's' | 'S' | '\u{13}' | 'س' => Some('s'),
                't' | 'T' | '\u{14}' | 'ف' => Some('t'),
                'u' | 'U' | '\u{15}' | 'ع' => Some('u'),
                'v' | 'V' | '\u{16}' | 'ر' => Some('v'),
                'w' | 'W' | '\u{17}' | 'ص' => Some('w'),
                'x' | 'X' | '\u{18}' | 'ء' => Some('x'),
                'y' | 'Y' | '\u{19}' | 'غ' => Some('y'),
                'z' | 'Z' | '\u{1a}' | 'ئ' => Some('z'),
                _ => None,
            }
        }
        Key::Named(NamedKey::Copy) => Some('c'),
        Key::Named(NamedKey::Paste) => Some('v'),
        Key::Named(NamedKey::Cut) => Some('x'),
        Key::Named(NamedKey::Undo) => Some('z'),
        _ => None,
    }
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

    let mut ctrl = modifiers.control_key();
    let shift = modifiers.shift_key();
    let alt = modifiers.alt_key();

    if !ctrl {
        if let Key::Character(ref s) = logical {
            if let Some(ch) = s.chars().next() {
                let code = ch as u32;
                if (1..=26).contains(&code) && code != 9 && code != 10 && code != 13 {
                    ctrl = true;
                }
            }
        }
    }

    // 1. Control Key Combinations (works in Arabic & English layout, physical & logical)
    if ctrl {
        // Zoom shortcuts
        if let Some(key_code) = physical {
            match key_code {
                KeyCode::Equal | KeyCode::NumpadAdd => return Some(InputAction::ZoomIn),
                KeyCode::Minus | KeyCode::NumpadSubtract => return Some(InputAction::ZoomOut),
                KeyCode::Digit0 | KeyCode::Numpad0 => return Some(InputAction::ZoomReset),
                KeyCode::BracketLeft => return Some(InputAction::Bytes(vec![27])),
                KeyCode::Backslash => return Some(InputAction::Bytes(vec![28])),
                KeyCode::BracketRight => return Some(InputAction::Bytes(vec![29])),
                _ => {}
            }
        }

        if let Some(letter) = match_ctrl_letter(physical, logical) {
            match letter {
                'c' => {
                    if shift {
                        return Some(InputAction::Copy);
                    } else {
                        return Some(InputAction::CopyOrInterrupt);
                    }
                }
                'v' => return Some(InputAction::Paste),
                'x' => return Some(InputAction::Cut),
                'z' => {
                    if shift {
                        return Some(InputAction::Bytes(vec![25])); // Redo / ^Y
                    } else {
                        return Some(InputAction::Bytes(vec![26])); // Undo / SIGTSTP / ^Z
                    }
                }
                'a' => {
                    if shift {
                        return Some(InputAction::SelectAll);
                    } else {
                        return Some(InputAction::Bytes(vec![1]));
                    }
                }
                ch if ch.is_ascii_lowercase() => {
                    let code = (ch as u8) - b'a' + 1;
                    return Some(InputAction::Bytes(vec![code]));
                }
                _ => {}
            }
        }
    }

    // 2. Named Navigation / Control Keys
    match logical {
        Key::Named(named) => match named {
            NamedKey::Backspace => {
                let bytes = if alt { vec![0x1b, 0x7f] } else { vec![0x7f] };
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::Enter => {
                let bytes = if alt { vec![0x1b, b'\r'] } else { vec![b'\r'] };
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::Tab => {
                let mut bytes = if shift {
                    b"\x1b[Z".to_vec()
                } else {
                    vec![b'\t']
                };
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::Escape => {
                let bytes = if alt { vec![0x1b, 0x1b] } else { vec![0x1b] };
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::Copy => return Some(InputAction::Copy),
            NamedKey::Paste => return Some(InputAction::Paste),
            NamedKey::Cut => return Some(InputAction::Cut),
            NamedKey::Undo => return Some(InputAction::Bytes(vec![26])),
            NamedKey::ArrowUp => {
                let bytes = if ctrl && !alt && !shift {
                    b"\x1b[1;5A".to_vec()
                } else if shift && !alt && !ctrl {
                    b"\x1b[1;2A".to_vec()
                } else if app_cursor {
                    b"\x1bOA".to_vec()
                } else {
                    b"\x1b[A".to_vec()
                };
                let mut bytes = bytes;
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::ArrowDown => {
                let bytes = if ctrl && !alt && !shift {
                    b"\x1b[1;5B".to_vec()
                } else if shift && !alt && !ctrl {
                    b"\x1b[1;2B".to_vec()
                } else if app_cursor {
                    b"\x1bOB".to_vec()
                } else {
                    b"\x1b[B".to_vec()
                };
                let mut bytes = bytes;
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::ArrowRight => {
                let bytes = if ctrl && !alt && !shift {
                    b"\x1b[1;5C".to_vec()
                } else if shift && !alt && !ctrl {
                    b"\x1b[1;2C".to_vec()
                } else if app_cursor {
                    b"\x1bOC".to_vec()
                } else {
                    b"\x1b[C".to_vec()
                };
                let mut bytes = bytes;
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::ArrowLeft => {
                let bytes = if ctrl && !alt && !shift {
                    b"\x1b[1;5D".to_vec()
                } else if shift && !alt && !ctrl {
                    b"\x1b[1;2D".to_vec()
                } else if app_cursor {
                    b"\x1bOD".to_vec()
                } else {
                    b"\x1b[D".to_vec()
                };
                let mut bytes = bytes;
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::Home => {
                let bytes = if ctrl && !alt && !shift {
                    b"\x1b[1;5H".to_vec()
                } else if shift && !alt && !ctrl {
                    b"\x1b[1;2H".to_vec()
                } else if app_cursor {
                    b"\x1bOH".to_vec()
                } else {
                    b"\x1b[H".to_vec()
                };
                let mut bytes = bytes;
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::End => {
                let bytes = if ctrl && !alt && !shift {
                    b"\x1b[1;5F".to_vec()
                } else if shift && !alt && !ctrl {
                    b"\x1b[1;2F".to_vec()
                } else if app_cursor {
                    b"\x1bOF".to_vec()
                } else {
                    b"\x1b[F".to_vec()
                };
                let mut bytes = bytes;
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::PageUp => {
                let mut bytes = b"\x1b[5~".to_vec();
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::PageDown => {
                let mut bytes = b"\x1b[6~".to_vec();
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::Delete => {
                let mut bytes = b"\x1b[3~".to_vec();
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::Insert => {
                let mut bytes = b"\x1b[2~".to_vec();
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::F1 => {
                let mut bytes = b"\x1bOP".to_vec();
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::F2 => {
                let mut bytes = b"\x1bOQ".to_vec();
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::F3 => {
                let mut bytes = b"\x1bOR".to_vec();
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::F4 => {
                let mut bytes = b"\x1bOS".to_vec();
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::F5 => {
                let mut bytes = b"\x1b[15~".to_vec();
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::F6 => {
                let mut bytes = b"\x1b[17~".to_vec();
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::F7 => {
                let mut bytes = b"\x1b[18~".to_vec();
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::F8 => {
                let mut bytes = b"\x1b[19~".to_vec();
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::F9 => {
                let mut bytes = b"\x1b[20~".to_vec();
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::F10 => {
                let mut bytes = b"\x1b[21~".to_vec();
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::F11 => {
                let mut bytes = b"\x1b[23~".to_vec();
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            NamedKey::F12 => {
                let mut bytes = b"\x1b[24~".to_vec();
                if alt {
                    bytes.insert(0, 0x1b);
                }
                return Some(InputAction::Bytes(bytes));
            }
            _ => {}
        },
        _ => {}
    }

    // 3. Normal typed text (Arabic, Latin, numbers, symbols, with Alt ESC prefix support)
    if !ctrl {
        if let Some(t) = text {
            if !t.is_empty() {
                if alt {
                    let mut bytes = vec![0x1b];
                    bytes.extend_from_slice(t.as_bytes());
                    return Some(InputAction::Bytes(bytes));
                }
                return Some(InputAction::Bytes(t.as_bytes().to_vec()));
            }
        }
    }

    None
}
