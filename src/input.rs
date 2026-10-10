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
    /// Open interactive scrollback search.
    Search,
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

/// xterm modifier parameter: 1 + Shift(1) + Alt(2) + Ctrl(4).
fn modifier_param(modifiers: ModifiersState) -> u8 {
    1 + modifiers.shift_key() as u8
        + 2 * modifiers.alt_key() as u8
        + 4 * modifiers.control_key() as u8
}

/// Cursor-style keys: `ESC [ X`, `ESC O X` in application mode, `ESC [ 1 ; m X` with modifiers.
fn cursor_key(final_byte: char, mods: u8, app_cursor: bool) -> Vec<u8> {
    if mods > 1 {
        format!("\x1b[1;{mods}{final_byte}").into_bytes()
    } else if app_cursor {
        format!("\x1bO{final_byte}").into_bytes()
    } else {
        format!("\x1b[{final_byte}").into_bytes()
    }
}

/// Tilde-style keys: `ESC [ n ~` or `ESC [ n ; m ~`.
fn tilde_key(code: u8, mods: u8) -> Vec<u8> {
    if mods > 1 {
        format!("\x1b[{code};{mods}~").into_bytes()
    } else {
        format!("\x1b[{code}~").into_bytes()
    }
}

/// F1–F4 use SS3 P..S (CSI 1;m P..S with modifiers).
fn ss3_function_key(final_byte: char, mods: u8) -> Vec<u8> {
    if mods > 1 {
        format!("\x1b[1;{mods}{final_byte}").into_bytes()
    } else {
        format!("\x1bO{final_byte}").into_bytes()
    }
}

fn with_alt(alt: bool, mut bytes: Vec<u8>) -> Vec<u8> {
    if alt {
        bytes.insert(0, 0x1b);
    }
    bytes
}

/// Control byte for Ctrl+<physical key>, independent of the active layout (works on Arabic layouts).
fn ctrl_byte(key_code: KeyCode) -> Option<u8> {
    let letter = match key_code {
        KeyCode::KeyA => b'a',
        KeyCode::KeyB => b'b',
        KeyCode::KeyC => b'c',
        KeyCode::KeyD => b'd',
        KeyCode::KeyE => b'e',
        KeyCode::KeyF => b'f',
        KeyCode::KeyG => b'g',
        KeyCode::KeyH => b'h',
        KeyCode::KeyI => b'i',
        KeyCode::KeyJ => b'j',
        KeyCode::KeyK => b'k',
        KeyCode::KeyL => b'l',
        KeyCode::KeyM => b'm',
        KeyCode::KeyN => b'n',
        KeyCode::KeyO => b'o',
        KeyCode::KeyP => b'p',
        KeyCode::KeyQ => b'q',
        KeyCode::KeyR => b'r',
        KeyCode::KeyS => b's',
        KeyCode::KeyT => b't',
        KeyCode::KeyU => b'u',
        KeyCode::KeyV => b'v',
        KeyCode::KeyW => b'w',
        KeyCode::KeyX => b'x',
        KeyCode::KeyY => b'y',
        KeyCode::KeyZ => b'z',
        KeyCode::Space | KeyCode::Digit2 => return Some(0x00),
        KeyCode::BracketLeft | KeyCode::Digit3 => return Some(0x1b),
        KeyCode::Backslash | KeyCode::Digit4 => return Some(0x1c),
        KeyCode::BracketRight | KeyCode::Digit5 => return Some(0x1d),
        KeyCode::Digit6 => return Some(0x1e),
        KeyCode::Slash | KeyCode::Digit7 => return Some(0x1f),
        _ => return None,
    };
    Some(letter - b'a' + 1)
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
    let alt = modifiers.alt_key();
    let mods = modifier_param(modifiers);

    // AltGr arrives as Ctrl+Alt on Windows: let symbols like @ € { } through untouched.
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

    // 1. App shortcuts and Ctrl combos by physical key (layout-independent).
    if let Some(key_code) = physical {
        if ctrl && shift && !alt {
            match key_code {
                KeyCode::KeyC => return Some(InputAction::Copy),
                KeyCode::KeyV => return Some(InputAction::Paste),
                KeyCode::KeyF => return Some(InputAction::Search),
                _ => {}
            }
        }
        if ctrl && !shift && !alt {
            match key_code {
                KeyCode::Equal | KeyCode::NumpadAdd => return Some(InputAction::ZoomIn),
                KeyCode::Minus | KeyCode::NumpadSubtract => return Some(InputAction::ZoomOut),
                KeyCode::Digit0 | KeyCode::Numpad0 => return Some(InputAction::ZoomReset),
                _ => {}
            }
        }
        if ctrl {
            if let Some(byte) = ctrl_byte(key_code) {
                return Some(InputAction::Bytes(with_alt(alt, vec![byte])));
            }
        }
    }

    // 2. Named navigation / control keys.
    if let Key::Named(named) = logical {
        let bytes = match named {
            NamedKey::Enter => Some(with_alt(alt, vec![b'\r'])),
            NamedKey::Backspace => {
                // Ctrl+Backspace deletes a word in most shells (^H), plain Backspace is DEL.
                let b = if ctrl { 0x08 } else { 0x7f };
                Some(with_alt(alt, vec![b]))
            }
            NamedKey::Tab if shift => Some(b"\x1b[Z".to_vec()),
            NamedKey::Tab => Some(with_alt(alt, vec![b'\t'])),
            NamedKey::Space if ctrl => Some(with_alt(alt, vec![0x00])),
            NamedKey::Escape => Some(with_alt(alt, vec![0x1b])),
            NamedKey::ArrowUp => Some(cursor_key('A', mods, app_cursor)),
            NamedKey::ArrowDown => Some(cursor_key('B', mods, app_cursor)),
            NamedKey::ArrowRight => Some(cursor_key('C', mods, app_cursor)),
            NamedKey::ArrowLeft => Some(cursor_key('D', mods, app_cursor)),
            NamedKey::Home => Some(cursor_key('H', mods, app_cursor)),
            NamedKey::End => Some(cursor_key('F', mods, app_cursor)),
            NamedKey::Insert => Some(tilde_key(2, mods)),
            NamedKey::Delete => Some(tilde_key(3, mods)),
            NamedKey::PageUp => Some(tilde_key(5, mods)),
            NamedKey::PageDown => Some(tilde_key(6, mods)),
            NamedKey::F1 => Some(ss3_function_key('P', mods)),
            NamedKey::F2 => Some(ss3_function_key('Q', mods)),
            NamedKey::F3 => Some(ss3_function_key('R', mods)),
            NamedKey::F4 => Some(ss3_function_key('S', mods)),
            NamedKey::F5 => Some(tilde_key(15, mods)),
            NamedKey::F6 => Some(tilde_key(17, mods)),
            NamedKey::F7 => Some(tilde_key(18, mods)),
            NamedKey::F8 => Some(tilde_key(19, mods)),
            NamedKey::F9 => Some(tilde_key(20, mods)),
            NamedKey::F10 => Some(tilde_key(21, mods)),
            NamedKey::F11 => Some(tilde_key(23, mods)),
            NamedKey::F12 => Some(tilde_key(24, mods)),
            _ => None,
        };
        if let Some(bytes) = bytes {
            return Some(InputAction::Bytes(bytes));
        }
    }

    // 3. Typed text (Arabic, Latin, numbers, symbols); Alt sends an ESC prefix (Meta).
    if !ctrl {
        if let Some(t) = text {
            if !t.is_empty() {
                return Some(InputAction::Bytes(with_alt(alt, t.as_bytes().to_vec())));
            }
        }
    }

    None
}

/// Prepare clipboard text for the PTY. With bracketed paste the shell receives it verbatim
/// (and won't run it line by line); otherwise newlines become carriage returns like a typed Enter.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        // Strip ESC so pasted content can't terminate the bracket early and inject commands.
        let body: String = text.chars().filter(|&c| c != '\x1b').collect();
        let mut out = b"\x1b[200~".to_vec();
        out.extend_from_slice(body.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// Encode a mouse report. `button` is the xterm button code (0-2 buttons, 3 release,
/// 64/65 wheel, +32 for motion) without modifier bits; col/row are 0-based.
pub fn encode_mouse(
    button: u8,
    col: usize,
    row: usize,
    pressed: bool,
    modifiers: ModifiersState,
    sgr: bool,
) -> Vec<u8> {
    let mut code = button;
    if modifiers.shift_key() {
        code += 4;
    }
    if modifiers.alt_key() {
        code += 8;
    }
    if modifiers.control_key() {
        code += 16;
    }

    if sgr {
        let suffix = if pressed { 'M' } else { 'm' };
        format!("\x1b[<{};{};{}{}", code, col + 1, row + 1, suffix).into_bytes()
    } else {
        // Legacy X10/normal encoding: releases are button 3, coordinates capped at 223.
        if !pressed {
            code = (code & !0b11) | 3;
        }
        let clamp = |v: usize| (v + 1).min(223) as u8 + 32;
        vec![0x1b, b'[', b'M', 32 + code, clamp(col), clamp(row)]
    }
}
