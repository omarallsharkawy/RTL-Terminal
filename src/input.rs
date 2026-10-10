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
    Search,
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

/// Cursor keys: `ESC [ X` (`ESC O X` in application mode), `ESC [ 1 ; m X` with modifiers.
fn cursor_key(final_byte: char, mod_code: u8, app_cursor: bool) -> Vec<u8> {
    if mod_code > 1 {
        format!("\x1b[1;{mod_code}{final_byte}").into_bytes()
    } else if app_cursor {
        format!("\x1bO{final_byte}").into_bytes()
    } else {
        format!("\x1b[{final_byte}").into_bytes()
    }
}

/// Editing and function keys: `ESC [ n ~`, or `ESC [ n ; m ~` with modifiers.
fn tilde_key(code: u8, mod_code: u8) -> Vec<u8> {
    if mod_code > 1 {
        format!("\x1b[{code};{mod_code}~").into_bytes()
    } else {
        format!("\x1b[{code}~").into_bytes()
    }
}

/// F1-F4: `ESC O P..S`, or `ESC [ 1 ; m P..S` with modifiers.
fn ss3_key(final_byte: char, mod_code: u8) -> Vec<u8> {
    if mod_code > 1 {
        format!("\x1b[1;{mod_code}{final_byte}").into_bytes()
    } else {
        format!("\x1bO{final_byte}").into_bytes()
    }
}

pub fn handle_key(
    event: &KeyEvent,
    modifiers: ModifiersState,
    app_cursor: bool,
    kitty_keyboard: bool,
) -> Option<InputAction> {
    let physical = match event.physical_key {
        PhysicalKey::Code(c) => Some(c),
        _ => None,
    };
    handle_key_raw_mode(
        physical,
        &event.logical_key,
        event.text.as_deref(),
        event.state,
        modifiers,
        app_cursor,
        kitty_keyboard,
    )
}

#[allow(dead_code)]
pub fn handle_key_raw(
    physical: Option<KeyCode>,
    logical: &Key,
    text: Option<&str>,
    state: ElementState,
    modifiers: ModifiersState,
    app_cursor: bool,
) -> Option<InputAction> {
    handle_key_raw_mode(physical, logical, text, state, modifiers, app_cursor, false)
}

pub fn handle_key_raw_mode(
    physical: Option<KeyCode>,
    logical: &Key,
    text: Option<&str>,
    state: ElementState,
    modifiers: ModifiersState,
    app_cursor: bool,
    kitty_keyboard: bool,
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

    let mod_code = 1 + (shift as u8) + (alt as u8 * 2) + (ctrl as u8 * 4);

    // AltGr arrives as Ctrl+Alt on Windows: let symbols such as @ € { } \\ through untouched
    // instead of turning them into Ctrl+Alt+letter control codes.
    if ctrl && alt {
        if let Some(t) = text {
            if !t.is_empty()
                && t.chars()
                    .all(|c| !c.is_control() && !c.is_ascii_alphanumeric())
            {
                return Some(InputAction::Bytes(t.as_bytes().to_vec()));
            }
        }
    }

    // Ctrl + Shift + F -> Scrollback Search
    if ctrl && shift {
        if let Some(KeyCode::KeyF) = physical {
            return Some(InputAction::Search);
        }
        if let Key::Character(ref s) = logical {
            if s.eq_ignore_ascii_case("f") || s == "ف" {
                return Some(InputAction::Search);
            }
        }
    }

    // Standard clipboard, zoom and edit shortcuts always take precedence
    if ctrl {
        if let Some(key_code) = physical {
            match key_code {
                KeyCode::Equal | KeyCode::NumpadAdd => return Some(InputAction::ZoomIn),
                KeyCode::Minus | KeyCode::NumpadSubtract => return Some(InputAction::ZoomOut),
                KeyCode::Digit0 | KeyCode::Numpad0 => return Some(InputAction::ZoomReset),
                KeyCode::BracketLeft => return Some(InputAction::Bytes(vec![27])),
                KeyCode::Backslash => return Some(InputAction::Bytes(vec![28])),
                KeyCode::BracketRight => return Some(InputAction::Bytes(vec![29])),
                KeyCode::Slash => return Some(InputAction::Bytes(vec![31])),
                _ => {}
            }
        }

        // Ctrl + Space -> NUL (0x00) or Kitty CSI u
        let is_space = match logical {
            Key::Named(NamedKey::Space) => true,
            Key::Character(ref s) if s == " " => true,
            _ => physical == Some(KeyCode::Space),
        };
        if is_space {
            if kitty_keyboard && mod_code > 1 {
                return Some(InputAction::Bytes(
                    format!("\x1b[32;{}u", mod_code).into_bytes(),
                ));
            } else {
                return Some(InputAction::Bytes(vec![0]));
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
                'a' if shift => return Some(InputAction::SelectAll),
                'z' => {
                    if shift {
                        return Some(InputAction::Bytes(vec![25])); // Redo / ^Y
                    } else {
                        return Some(InputAction::Bytes(vec![26])); // Undo / SIGTSTP / ^Z
                    }
                }
                ch if ch.is_ascii_lowercase() => {
                    let code = (ch as u8) - b'a' + 1;
                    if !kitty_keyboard {
                        // Standard VT/ANSI terminal mode:
                        // Shift is ignored for Ctrl+letter, Alt adds ESC prefix
                        let bytes = if alt { vec![0x1b, code] } else { vec![code] };
                        return Some(InputAction::Bytes(bytes));
                    } else if !shift && !alt {
                        // In Kitty mode without Shift/Alt: bare Ctrl+letter sends standard control code (1..=26)
                        return Some(InputAction::Bytes(vec![code]));
                    }
                    // In Kitty mode with Shift or Alt (e.g. Ctrl+Shift+P, Ctrl+Alt+D):
                    // falls through to Kitty CSI u encoding below
                }
                _ => {}
            }
        }
    }

    // Kitty Keyboard Protocol (CSI u)
    if kitty_keyboard && mod_code > 1 {
        match logical {
            Key::Named(NamedKey::Enter) => {
                return Some(InputAction::Bytes(
                    format!("\x1b[13;{}u", mod_code).into_bytes(),
                ));
            }
            Key::Named(NamedKey::Tab) => {
                return Some(InputAction::Bytes(
                    format!("\x1b[9;{}u", mod_code).into_bytes(),
                ));
            }
            Key::Named(NamedKey::Backspace) => {
                return Some(InputAction::Bytes(
                    format!("\x1b[127;{}u", mod_code).into_bytes(),
                ));
            }
            Key::Named(NamedKey::Escape) => {
                return Some(InputAction::Bytes(
                    format!("\x1b[27;{}u", mod_code).into_bytes(),
                ));
            }
            _ => {
                if let Some(letter) = match_ctrl_letter(physical, logical) {
                    let cp = letter as u32;
                    return Some(InputAction::Bytes(
                        format!("\x1b[{};{}u", cp, mod_code).into_bytes(),
                    ));
                }
            }
        }
    }

    // 2. Named Navigation / Control Keys
    if let Key::Named(named) = logical {
        let esc_prefixed = |mut bytes: Vec<u8>| {
            if alt {
                bytes.insert(0, 0x1b);
            }
            Some(InputAction::Bytes(bytes))
        };
        match named {
            NamedKey::Backspace => {
                // Ctrl+Backspace sends ^H (delete word in PowerShell/PSReadLine), like Windows Terminal.
                return esc_prefixed(vec![if ctrl { 0x08 } else { 0x7f }]);
            }
            NamedKey::Enter => return esc_prefixed(vec![b'\r']),
            NamedKey::Tab if shift => return esc_prefixed(b"\x1b[Z".to_vec()),
            NamedKey::Tab => return esc_prefixed(vec![b'\t']),
            NamedKey::Escape => return esc_prefixed(vec![0x1b]),
            NamedKey::Copy => return Some(InputAction::Copy),
            NamedKey::Paste => return Some(InputAction::Paste),
            NamedKey::Cut => return Some(InputAction::Cut),
            NamedKey::Undo => return Some(InputAction::Bytes(vec![26])),
            _ => {}
        }
        // Alt alone keeps the ESC-prefixed form (Meta); Ctrl/Shift combos use xterm
        // `CSI 1;m` parameters (Alt then adds 2 to m).
        let alt_only = alt && !ctrl && !shift;
        let mod_code = if alt_only { 1 } else { mod_code };
        let bytes = match named {
            NamedKey::ArrowUp => Some(cursor_key('A', mod_code, app_cursor)),
            NamedKey::ArrowDown => Some(cursor_key('B', mod_code, app_cursor)),
            NamedKey::ArrowRight => Some(cursor_key('C', mod_code, app_cursor)),
            NamedKey::ArrowLeft => Some(cursor_key('D', mod_code, app_cursor)),
            NamedKey::Home => Some(cursor_key('H', mod_code, app_cursor)),
            NamedKey::End => Some(cursor_key('F', mod_code, app_cursor)),
            NamedKey::Insert => Some(tilde_key(2, mod_code)),
            NamedKey::Delete => Some(tilde_key(3, mod_code)),
            NamedKey::PageUp => Some(tilde_key(5, mod_code)),
            NamedKey::PageDown => Some(tilde_key(6, mod_code)),
            NamedKey::F1 => Some(ss3_key('P', mod_code)),
            NamedKey::F2 => Some(ss3_key('Q', mod_code)),
            NamedKey::F3 => Some(ss3_key('R', mod_code)),
            NamedKey::F4 => Some(ss3_key('S', mod_code)),
            NamedKey::F5 => Some(tilde_key(15, mod_code)),
            NamedKey::F6 => Some(tilde_key(17, mod_code)),
            NamedKey::F7 => Some(tilde_key(18, mod_code)),
            NamedKey::F8 => Some(tilde_key(19, mod_code)),
            NamedKey::F9 => Some(tilde_key(20, mod_code)),
            NamedKey::F10 => Some(tilde_key(21, mod_code)),
            NamedKey::F11 => Some(tilde_key(23, mod_code)),
            NamedKey::F12 => Some(tilde_key(24, mod_code)),
            _ => None,
        };
        if let Some(mut bytes) = bytes {
            if alt_only {
                bytes.insert(0, 0x1b);
            }
            return Some(InputAction::Bytes(bytes));
        }
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
/// Prepare clipboard text for the PTY. With bracketed paste the shell receives it verbatim
/// (multi-line pastes are not executed line by line); otherwise newlines become carriage
/// returns like a typed Enter, without doubling Windows CRLF line endings.
pub fn encode_paste(text: &str, bracketed: bool) -> String {
    if bracketed {
        // Drop ESC so pasted content can't close the bracket early and inject keystrokes.
        let body: String = text.chars().filter(|&c| c != '\x1b').collect();
        format!("\x1b[200~{body}\x1b[201~")
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r")
    }
}

pub fn format_mouse_seq(
    mode: alacritty_terminal::term::TermMode,
    btn: u8,
    col: usize,
    row: usize,
    is_release: bool,
) -> Vec<u8> {
    if mode.contains(alacritty_terminal::term::TermMode::SGR_MOUSE) {
        let suffix = if is_release { 'm' } else { 'M' };
        format!("[<{btn};{col};{row}{suffix}").into_bytes()
    } else {
        let b = if is_release { 3 } else { btn };
        let b_char = 32u8.saturating_add(b);
        // Legacy encoding caps coordinates at 223 (cast after clamping to avoid wrap-around).
        let c_char = 32 + col.min(223) as u8;
        let r_char = 32 + row.min(223) as u8;
        vec![0x1b, b'[', b'M', b_char, c_char, r_char]
    }
}
