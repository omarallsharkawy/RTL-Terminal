use crate::layout::FocusDir;
use alacritty_terminal::term::TermMode;
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

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
    NewTab,
    /// Close the focused pane (its tab, or the window, when it is the last one).
    ClosePane,
    NextTab,
    PrevTab,
    /// Split the focused pane side by side.
    SplitRight,
    /// Split the focused pane top and bottom.
    SplitDown,
    /// Move focus to the neighbouring pane.
    Focus(FocusDir),
}

pub fn handle_key(
    event: &KeyEvent,
    modifiers: ModifiersState,
    mode: TermMode,
) -> Option<InputAction> {
    let physical = match event.physical_key {
        PhysicalKey::Code(c) => Some(c),
        _ => None,
    };
    let unshifted = event.key_without_modifiers();
    handle_key_mode_raw(
        physical,
        &event.logical_key,
        Some(&unshifted),
        event.text.as_deref(),
        event.state,
        event.repeat,
        modifiers,
        mode,
    )
}

/// Shortcuts handled by Twitty itself, independent of the terminal's keyboard mode.
/// The key a shortcut refers to: the Latin letter or named key that was typed when there is
/// one (so shortcuts follow the key labels on AZERTY and work with virtual keyboards such as
/// `wtype`), otherwise the physical key (non-Latin layouts like Arabic).
fn shortcut_key(physical: Option<KeyCode>, logical: &Key) -> Option<KeyCode> {
    let from_logical = match logical {
        Key::Character(s) => {
            let mut chars = s.chars();
            match (chars.next().map(|c| c.to_ascii_lowercase()), chars.next()) {
                (Some(c @ 'a'..='z'), None) => {
                    const LETTERS: [KeyCode; 26] = [
                        KeyCode::KeyA,
                        KeyCode::KeyB,
                        KeyCode::KeyC,
                        KeyCode::KeyD,
                        KeyCode::KeyE,
                        KeyCode::KeyF,
                        KeyCode::KeyG,
                        KeyCode::KeyH,
                        KeyCode::KeyI,
                        KeyCode::KeyJ,
                        KeyCode::KeyK,
                        KeyCode::KeyL,
                        KeyCode::KeyM,
                        KeyCode::KeyN,
                        KeyCode::KeyO,
                        KeyCode::KeyP,
                        KeyCode::KeyQ,
                        KeyCode::KeyR,
                        KeyCode::KeyS,
                        KeyCode::KeyT,
                        KeyCode::KeyU,
                        KeyCode::KeyV,
                        KeyCode::KeyW,
                        KeyCode::KeyX,
                        KeyCode::KeyY,
                        KeyCode::KeyZ,
                    ];
                    Some(LETTERS[(c as u8 - b'a') as usize])
                }
                _ => None,
            }
        }
        Key::Named(NamedKey::Tab) => Some(KeyCode::Tab),
        Key::Named(NamedKey::PageUp) => Some(KeyCode::PageUp),
        Key::Named(NamedKey::PageDown) => Some(KeyCode::PageDown),
        Key::Named(NamedKey::ArrowLeft) => Some(KeyCode::ArrowLeft),
        Key::Named(NamedKey::ArrowRight) => Some(KeyCode::ArrowRight),
        Key::Named(NamedKey::ArrowUp) => Some(KeyCode::ArrowUp),
        Key::Named(NamedKey::ArrowDown) => Some(KeyCode::ArrowDown),
        _ => None,
    };
    from_logical.or(physical)
}

fn app_shortcut(key_code: KeyCode, modifiers: ModifiersState) -> Option<InputAction> {
    let (ctrl, shift, alt) = (
        modifiers.control_key(),
        modifiers.shift_key(),
        modifiers.alt_key(),
    );
    if ctrl && shift && !alt {
        match key_code {
            KeyCode::KeyC => return Some(InputAction::Copy),
            KeyCode::KeyV => return Some(InputAction::Paste),
            KeyCode::KeyF => return Some(InputAction::Search),
            KeyCode::KeyT => return Some(InputAction::NewTab),
            KeyCode::KeyW => return Some(InputAction::ClosePane),
            KeyCode::KeyE => return Some(InputAction::SplitRight),
            KeyCode::KeyO => return Some(InputAction::SplitDown),
            _ => {}
        }
    }
    if ctrl && !alt {
        match key_code {
            KeyCode::Tab if shift => return Some(InputAction::PrevTab),
            KeyCode::Tab => return Some(InputAction::NextTab),
            KeyCode::PageDown if !shift => return Some(InputAction::NextTab),
            KeyCode::PageUp if !shift => return Some(InputAction::PrevTab),
            _ => {}
        }
    }
    if alt && !ctrl && !shift {
        match key_code {
            KeyCode::ArrowLeft => return Some(InputAction::Focus(FocusDir::Left)),
            KeyCode::ArrowRight => return Some(InputAction::Focus(FocusDir::Right)),
            KeyCode::ArrowUp => return Some(InputAction::Focus(FocusDir::Up)),
            KeyCode::ArrowDown => return Some(InputAction::Focus(FocusDir::Down)),
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
    None
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
    if let Some(action) = shortcut_key(physical, logical).and_then(|k| app_shortcut(k, modifiers)) {
        return Some(action);
    }
    if let Some(key_code) = physical {
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

/// Kitty modifier parameter: 1 + Shift(1) + Alt(2) + Ctrl(4) + Super(8).
fn kitty_modifier_param(modifiers: ModifiersState) -> u8 {
    modifier_param(modifiers) + 8 * modifiers.super_key() as u8
}

/// Unshifted ASCII character a physical key produces on a US layout (the "base layout key").
fn physical_base_char(key_code: KeyCode) -> Option<char> {
    let c = match key_code {
        KeyCode::KeyA => 'a',
        KeyCode::KeyB => 'b',
        KeyCode::KeyC => 'c',
        KeyCode::KeyD => 'd',
        KeyCode::KeyE => 'e',
        KeyCode::KeyF => 'f',
        KeyCode::KeyG => 'g',
        KeyCode::KeyH => 'h',
        KeyCode::KeyI => 'i',
        KeyCode::KeyJ => 'j',
        KeyCode::KeyK => 'k',
        KeyCode::KeyL => 'l',
        KeyCode::KeyM => 'm',
        KeyCode::KeyN => 'n',
        KeyCode::KeyO => 'o',
        KeyCode::KeyP => 'p',
        KeyCode::KeyQ => 'q',
        KeyCode::KeyR => 'r',
        KeyCode::KeyS => 's',
        KeyCode::KeyT => 't',
        KeyCode::KeyU => 'u',
        KeyCode::KeyV => 'v',
        KeyCode::KeyW => 'w',
        KeyCode::KeyX => 'x',
        KeyCode::KeyY => 'y',
        KeyCode::KeyZ => 'z',
        KeyCode::Digit0 => '0',
        KeyCode::Digit1 => '1',
        KeyCode::Digit2 => '2',
        KeyCode::Digit3 => '3',
        KeyCode::Digit4 => '4',
        KeyCode::Digit5 => '5',
        KeyCode::Digit6 => '6',
        KeyCode::Digit7 => '7',
        KeyCode::Digit8 => '8',
        KeyCode::Digit9 => '9',
        KeyCode::Minus => '-',
        KeyCode::Equal => '=',
        KeyCode::BracketLeft => '[',
        KeyCode::BracketRight => ']',
        KeyCode::Backslash => '\\',
        KeyCode::Semicolon => ';',
        KeyCode::Quote => '\'',
        KeyCode::Comma => ',',
        KeyCode::Period => '.',
        KeyCode::Slash => '/',
        KeyCode::Backquote => '`',
        _ => return None,
    };
    Some(c)
}

/// Kitty codepoints for keypad keys (reported only with "report all keys as escape codes").
fn keypad_code(key_code: KeyCode) -> Option<u32> {
    Some(match key_code {
        KeyCode::Numpad0 => 57399,
        KeyCode::Numpad1 => 57400,
        KeyCode::Numpad2 => 57401,
        KeyCode::Numpad3 => 57402,
        KeyCode::Numpad4 => 57403,
        KeyCode::Numpad5 => 57404,
        KeyCode::Numpad6 => 57405,
        KeyCode::Numpad7 => 57406,
        KeyCode::Numpad8 => 57407,
        KeyCode::Numpad9 => 57408,
        KeyCode::NumpadDecimal => 57409,
        KeyCode::NumpadDivide => 57410,
        KeyCode::NumpadMultiply => 57411,
        KeyCode::NumpadSubtract => 57412,
        KeyCode::NumpadAdd => 57413,
        KeyCode::NumpadEnter => 57414,
        KeyCode::NumpadEqual => 57415,
        _ => return None,
    })
}

/// Functional keys that keep their legacy CSI/SS3 shape in the kitty protocol.
enum FuncKey {
    Cursor(char),
    Tilde(u8),
    Ss3(char),
}

fn func_key(named: &NamedKey) -> Option<FuncKey> {
    Some(match named {
        NamedKey::ArrowUp => FuncKey::Cursor('A'),
        NamedKey::ArrowDown => FuncKey::Cursor('B'),
        NamedKey::ArrowRight => FuncKey::Cursor('C'),
        NamedKey::ArrowLeft => FuncKey::Cursor('D'),
        NamedKey::Home => FuncKey::Cursor('H'),
        NamedKey::End => FuncKey::Cursor('F'),
        NamedKey::Insert => FuncKey::Tilde(2),
        NamedKey::Delete => FuncKey::Tilde(3),
        NamedKey::PageUp => FuncKey::Tilde(5),
        NamedKey::PageDown => FuncKey::Tilde(6),
        NamedKey::F1 => FuncKey::Ss3('P'),
        NamedKey::F2 => FuncKey::Ss3('Q'),
        NamedKey::F3 => FuncKey::Tilde(13),
        NamedKey::F4 => FuncKey::Ss3('S'),
        NamedKey::F5 => FuncKey::Tilde(15),
        NamedKey::F6 => FuncKey::Tilde(17),
        NamedKey::F7 => FuncKey::Tilde(18),
        NamedKey::F8 => FuncKey::Tilde(19),
        NamedKey::F9 => FuncKey::Tilde(20),
        NamedKey::F10 => FuncKey::Tilde(21),
        NamedKey::F11 => FuncKey::Tilde(23),
        NamedKey::F12 => FuncKey::Tilde(24),
        _ => return None,
    })
}

/// Fields of a kitty `CSI code[:shifted[:base]] ; mods[:event] ; text u` sequence.
struct KittyKey {
    code: u32,
    shifted: Option<u32>,
    base: Option<u32>,
    mods: u8,
    /// Event type (2 = repeat, 3 = release); `None` means press / not reported.
    event: Option<u8>,
    text: Option<String>,
}

impl KittyKey {
    fn encode(&self) -> Vec<u8> {
        let mut out = format!("\x1b[{}", self.code);
        if self.shifted.is_some() || self.base.is_some() {
            out.push(':');
            if let Some(s) = self.shifted {
                out.push_str(&s.to_string());
            }
            if let Some(b) = self.base {
                out.push_str(&format!(":{b}"));
            }
        }
        if self.mods > 1 || self.event.is_some() || self.text.is_some() {
            out.push(';');
            if self.mods > 1 || self.event.is_some() {
                out.push_str(&self.mods.to_string());
            }
            if let Some(e) = self.event {
                out.push_str(&format!(":{e}"));
            }
        }
        if let Some(t) = &self.text {
            let cps: Vec<String> = t.chars().map(|c| (c as u32).to_string()).collect();
            out.push(';');
            out.push_str(&cps.join(":"));
        }
        out.push('u');
        out.into_bytes()
    }
}

/// Encode a functional key; unmodified presses keep the legacy form.
fn kitty_func_key(key: FuncKey, mods: u8, event: Option<u8>, app_cursor: bool) -> Vec<u8> {
    if mods == 1 && event.is_none() {
        return match key {
            FuncKey::Cursor(c) => cursor_key(c, 1, app_cursor),
            FuncKey::Tilde(n) => tilde_key(n, 1),
            FuncKey::Ss3(c) => ss3_function_key(c, 1),
        };
    }
    let field = match event {
        Some(e) => format!("{mods}:{e}"),
        None => mods.to_string(),
    };
    match key {
        FuncKey::Cursor(c) | FuncKey::Ss3(c) => format!("\x1b[1;{field}{c}"),
        FuncKey::Tilde(n) => format!("\x1b[{n};{field}~"),
    }
    .into_bytes()
}

/// Encode a key under the kitty keyboard protocol. `None` means "not handled here": the caller
/// falls back to the legacy encoding for presses.
#[allow(clippy::too_many_arguments)]
fn encode_kitty(
    physical: Option<KeyCode>,
    logical: &Key,
    unshifted: Option<&Key>,
    text: Option<&str>,
    state: ElementState,
    repeat: bool,
    modifiers: ModifiersState,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let disambiguate = mode.contains(TermMode::DISAMBIGUATE_ESC_CODES);
    let report_events = mode.contains(TermMode::REPORT_EVENT_TYPES);
    let alternates = mode.contains(TermMode::REPORT_ALTERNATE_KEYS);
    let all_keys = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let associated = mode.contains(TermMode::REPORT_ASSOCIATED_TEXT);

    let pressed = state.is_pressed();
    let event = match (pressed, repeat) {
        (false, _) => Some(3),
        (true, true) => Some(2),
        (true, false) => None,
    }
    .filter(|_| report_events);
    let mods = kitty_modifier_param(modifiers);
    let (ctrl, alt) = (modifiers.control_key(), modifiers.alt_key());

    let simple = |code: u32| KittyKey {
        code,
        shifted: None,
        base: None,
        mods,
        event,
        text: None,
    };

    if let Key::Named(named) = logical {
        if let Some(key) = func_key(named) {
            return Some(kitty_func_key(
                key,
                mods,
                event,
                mode.contains(TermMode::APP_CURSOR),
            ));
        }
        match named {
            // Enter/Tab/Backspace stay legacy unless modified; no release events without flag 8.
            NamedKey::Enter | NamedKey::Tab | NamedKey::Backspace => {
                if !all_keys && (mods == 1 || !pressed) {
                    return None;
                }
                let code = match named {
                    NamedKey::Enter => 13,
                    NamedKey::Tab => 9,
                    _ => 127,
                };
                return Some(simple(code).encode());
            }
            NamedKey::Escape if disambiguate || all_keys => return Some(simple(27).encode()),
            NamedKey::Space => {}
            _ => return None,
        }
    }

    // Keypad keys have dedicated codepoints, but only when everything is reported as escapes.
    if all_keys {
        if let Some(code) = physical.and_then(keypad_code) {
            return Some(simple(code).encode());
        }
    }

    // Text keys: CSI u when forced, or when Ctrl/Alt/Super would make the legacy form ambiguous.
    if !(all_keys || (disambiguate && (ctrl || alt || modifiers.super_key()))) {
        return None;
    }
    let first_char = |k: &Key| match k {
        Key::Character(s) => s.chars().next(),
        Key::Named(NamedKey::Space) => Some(' '),
        _ => None,
    };
    let lower = |c: char| c.to_lowercase().next().unwrap_or(c);
    let base_char = physical.and_then(physical_base_char);
    let code_char = unshifted
        .and_then(first_char)
        .or_else(|| first_char(logical))
        .map(lower)
        .or(base_char)?;
    let code = code_char as u32;

    let (mut shifted, mut base) = (None, None);
    if alternates {
        if modifiers.shift_key() {
            shifted = first_char(logical).map(|c| c as u32).filter(|&c| c != code);
        }
        base = base_char.map(|c| c as u32).filter(|&c| c != code);
    }
    let text = text
        .filter(|t| {
            associated
                && all_keys
                && pressed
                && !ctrl
                && !modifiers.super_key()
                && !t.is_empty()
                && t.chars().all(|c| !c.is_control())
        })
        .map(str::to_owned);
    Some(
        KittyKey {
            shifted,
            base,
            text,
            ..simple(code)
        }
        .encode(),
    )
}

/// Key handling aware of the terminal's keyboard mode. Without any kitty protocol flag active
/// this is identical to [`handle_key_raw`]; otherwise keys are encoded per the kitty keyboard
/// protocol, with the legacy encoding as a fallback for presses.
#[allow(clippy::too_many_arguments)]
pub fn handle_key_mode_raw(
    physical: Option<KeyCode>,
    logical: &Key,
    unshifted: Option<&Key>,
    text: Option<&str>,
    state: ElementState,
    repeat: bool,
    modifiers: ModifiersState,
    mode: TermMode,
) -> Option<InputAction> {
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    if !mode.intersects(TermMode::KITTY_KEYBOARD_PROTOCOL) {
        return handle_key_raw(physical, logical, text, state, modifiers, app_cursor);
    }

    // Releases are only reported when the application asked for event types.
    if !state.is_pressed() && !mode.contains(TermMode::REPORT_EVENT_TYPES) {
        return None;
    }

    if state.is_pressed() {
        if let Some(action) =
            shortcut_key(physical, logical).and_then(|c| app_shortcut(c, modifiers))
        {
            return Some(action);
        }
        // AltGr arrives as Ctrl+Alt on Windows: plain symbols pass through untouched.
        if modifiers.control_key() && modifiers.alt_key() {
            if let Some(t) = text {
                if !t.is_empty()
                    && t.chars()
                        .all(|c| !c.is_control() && !c.is_ascii_alphanumeric())
                {
                    return Some(InputAction::Bytes(t.as_bytes().to_vec()));
                }
            }
        }
    }

    match encode_kitty(
        physical, logical, unshifted, text, state, repeat, modifiers, mode,
    ) {
        Some(bytes) => Some(InputAction::Bytes(bytes)),
        None => handle_key_raw(physical, logical, text, state, modifiers, app_cursor),
    }
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
