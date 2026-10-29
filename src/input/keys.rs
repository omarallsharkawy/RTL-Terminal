//! Key, paste and mouse encoders: the bytes an input event sends to the program.
//!
//! A key press is looked up in three layers, first match wins:
//! 1. Twitty's own shortcuts ([`InputAction`] other than `Bytes`).
//! 2. The kitty keyboard protocol, when the program enabled any of its flags.
//! 3. The legacy xterm encoding (also the fallback for keys kitty leaves alone).

use crate::layout::FocusDir;
use alacritty_terminal::term::TermMode;
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

/// What a key press asks for: bytes for the program, or one of Twitty's own actions.
#[derive(Debug, PartialEq, Eq)]
pub enum InputAction {
    /// Send these bytes to the program.
    Bytes(Vec<u8>),
    /// Make the font larger.
    ZoomIn,
    /// Make the font smaller.
    ZoomOut,
    /// Go back to the configured font size.
    ZoomReset,
    /// Copy the selection to the clipboard.
    Copy,
    /// Paste the clipboard into the program.
    Paste,
    /// Open interactive scrollback search.
    Search,
    /// Open a new tab.
    NewTab,
    /// Close the focused pane (its tab, or the window, when it is the last one).
    ClosePane,
    /// Switch to the next tab.
    NextTab,
    /// Switch to the previous tab.
    PrevTab,
    /// Split the focused pane side by side.
    SplitRight,
    /// Split the focused pane top and bottom.
    SplitDown,
    /// Move focus to the neighbouring pane.
    Focus(FocusDir),
}

/// One key event with everything the encoders look at. Window-free, so the `*_raw` entry
/// points can build it in tests.
#[derive(Clone, Copy)]
struct KeyInput<'a> {
    physical: Option<KeyCode>,
    logical: &'a Key,
    /// The key with no modifiers applied (kitty reports it as the key code).
    unshifted: Option<&'a Key>,
    text: Option<&'a str>,
    state: ElementState,
    repeat: bool,
    modifiers: ModifiersState,
}

impl<'a> KeyInput<'a> {
    fn from_event(event: &'a KeyEvent, unshifted: &'a Key, modifiers: ModifiersState) -> Self {
        let physical = match event.physical_key {
            PhysicalKey::Code(c) => Some(c),
            _ => None,
        };
        Self {
            physical,
            logical: &event.logical_key,
            unshifted: Some(unshifted),
            text: event.text.as_deref(),
            state: event.state,
            repeat: event.repeat,
            modifiers,
        }
    }

    fn pressed(&self) -> bool {
        self.state.is_pressed()
    }
}

/// What a key event does in a terminal with keyboard mode `mode`: a Twitty shortcut, or the
/// bytes to send. `None` when the key sends nothing (a release, a lone modifier...).
pub(crate) fn handle_key(
    event: &KeyEvent,
    modifiers: ModifiersState,
    mode: TermMode,
) -> Option<InputAction> {
    let unshifted = event.key_without_modifiers();
    key_with_mode(
        &KeyInput::from_event(event, &unshifted, modifiers),
        mode,
        true,
    )
}

/// The Twitty shortcut this key (pressed or released) belongs to, if any.
pub(crate) fn key_shortcut(event: &KeyEvent, modifiers: ModifiersState) -> Option<InputAction> {
    let physical = match event.physical_key {
        PhysicalKey::Code(c) => Some(c),
        _ => None,
    };
    shortcut_key(physical, &event.logical_key).and_then(|k| app_shortcut(k, modifiers))
}

/// The bytes a key sends to the program, ignoring Twitty's own shortcuts. Used when a
/// shortcut has nothing to act on (Alt+Left with a single pane), so the program gets the key.
pub(crate) fn terminal_key(
    event: &KeyEvent,
    modifiers: ModifiersState,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let unshifted = event.key_without_modifiers();
    into_bytes(key_with_mode(
        &KeyInput::from_event(event, &unshifted, modifiers),
        mode,
        false,
    ))
}

/// Legacy (non-kitty) key handling, with Twitty's shortcuts. `app_cursor` is DECCKM.
pub fn handle_key_raw(
    physical: Option<KeyCode>,
    logical: &Key,
    text: Option<&str>,
    state: ElementState,
    modifiers: ModifiersState,
    app_cursor: bool,
) -> Option<InputAction> {
    let key = KeyInput {
        physical,
        logical,
        unshifted: None,
        text,
        state,
        repeat: false,
        modifiers,
    };
    let mode = if app_cursor {
        TermMode::APP_CURSOR
    } else {
        TermMode::NONE
    };
    legacy_key(&key, mode, true)
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
    let key = KeyInput {
        physical,
        logical,
        unshifted,
        text,
        state,
        repeat,
        modifiers,
    };
    key_with_mode(&key, mode, true)
}

/// Like [`handle_key_mode_raw`], but Twitty's shortcuts are not looked up: the bytes the
/// program would receive for this key.
#[allow(clippy::too_many_arguments)]
pub fn terminal_key_raw(
    physical: Option<KeyCode>,
    logical: &Key,
    unshifted: Option<&Key>,
    text: Option<&str>,
    state: ElementState,
    repeat: bool,
    modifiers: ModifiersState,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let key = KeyInput {
        physical,
        logical,
        unshifted,
        text,
        state,
        repeat,
        modifiers,
    };
    into_bytes(key_with_mode(&key, mode, false))
}

fn into_bytes(action: Option<InputAction>) -> Option<Vec<u8>> {
    match action {
        Some(InputAction::Bytes(bytes)) => Some(bytes),
        _ => None,
    }
}

// ---- shortcuts ----

/// Letter keys in alphabetical order: index `i` is the key of `'a' + i`.
const LETTER_KEYS: [KeyCode; 26] = [
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

/// The lowercase letter `'a'..='z'` a physical letter key carries on a US layout.
fn key_letter(key_code: KeyCode) -> Option<u8> {
    LETTER_KEYS
        .iter()
        .position(|&k| k == key_code)
        .map(|i| b'a' + i as u8)
}

/// Named keys that shortcuts can be bound to, and the physical key each stands for.
const SHORTCUT_NAMED_KEYS: [(NamedKey, KeyCode); 7] = [
    (NamedKey::Tab, KeyCode::Tab),
    (NamedKey::PageUp, KeyCode::PageUp),
    (NamedKey::PageDown, KeyCode::PageDown),
    (NamedKey::ArrowLeft, KeyCode::ArrowLeft),
    (NamedKey::ArrowRight, KeyCode::ArrowRight),
    (NamedKey::ArrowUp, KeyCode::ArrowUp),
    (NamedKey::ArrowDown, KeyCode::ArrowDown),
];

/// The key a shortcut refers to: the Latin letter or named key that was typed when there is
/// one (so shortcuts follow the key labels on AZERTY and work with virtual keyboards such as
/// `wtype`), otherwise the physical key (non-Latin layouts like Arabic).
fn shortcut_key(physical: Option<KeyCode>, logical: &Key) -> Option<KeyCode> {
    let from_logical = match logical {
        Key::Character(s) => {
            let mut chars = s.chars();
            match (chars.next().map(|c| c.to_ascii_lowercase()), chars.next()) {
                (Some(c @ 'a'..='z'), None) => Some(LETTER_KEYS[(c as u8 - b'a') as usize]),
                _ => None,
            }
        }
        Key::Named(named) => SHORTCUT_NAMED_KEYS
            .iter()
            .find(|(n, _)| n == named)
            .map(|&(_, code)| code),
        _ => None,
    };
    from_logical.or(physical)
}

/// Shortcuts handled by Twitty itself, independent of the terminal's keyboard mode.
fn app_shortcut(key_code: KeyCode, modifiers: ModifiersState) -> Option<InputAction> {
    let (ctrl, shift, alt) = (
        modifiers.control_key(),
        modifiers.shift_key(),
        modifiers.alt_key(),
    );
    match (ctrl, shift, alt) {
        (true, true, false) => {
            ctrl_shift_shortcut(key_code).or_else(|| tab_shortcut(key_code, true))
        }
        (true, false, false) => tab_shortcut(key_code, false).or_else(|| zoom_shortcut(key_code)),
        (false, false, true) => focus_shortcut(key_code),
        _ => None,
    }
}

/// Ctrl+Shift+letter: clipboard, search, tabs and splits.
fn ctrl_shift_shortcut(key_code: KeyCode) -> Option<InputAction> {
    Some(match key_code {
        KeyCode::KeyC => InputAction::Copy,
        KeyCode::KeyV => InputAction::Paste,
        KeyCode::KeyF => InputAction::Search,
        KeyCode::KeyT => InputAction::NewTab,
        KeyCode::KeyW => InputAction::ClosePane,
        KeyCode::KeyE => InputAction::SplitRight,
        KeyCode::KeyO => InputAction::SplitDown,
        _ => return None,
    })
}

/// Ctrl+Tab / Ctrl+Shift+Tab and Ctrl+PageDown / Ctrl+PageUp cycle tabs.
fn tab_shortcut(key_code: KeyCode, shift: bool) -> Option<InputAction> {
    match key_code {
        KeyCode::Tab if shift => Some(InputAction::PrevTab),
        KeyCode::Tab => Some(InputAction::NextTab),
        KeyCode::PageDown if !shift => Some(InputAction::NextTab),
        KeyCode::PageUp if !shift => Some(InputAction::PrevTab),
        _ => None,
    }
}

/// Ctrl+= / Ctrl+- / Ctrl+0 (main row or keypad) zoom.
fn zoom_shortcut(key_code: KeyCode) -> Option<InputAction> {
    match key_code {
        KeyCode::Equal | KeyCode::NumpadAdd => Some(InputAction::ZoomIn),
        KeyCode::Minus | KeyCode::NumpadSubtract => Some(InputAction::ZoomOut),
        KeyCode::Digit0 | KeyCode::Numpad0 => Some(InputAction::ZoomReset),
        _ => None,
    }
}

/// Alt+arrow moves focus between panes.
fn focus_shortcut(key_code: KeyCode) -> Option<InputAction> {
    let dir = match key_code {
        KeyCode::ArrowLeft => FocusDir::Left,
        KeyCode::ArrowRight => FocusDir::Right,
        KeyCode::ArrowUp => FocusDir::Up,
        KeyCode::ArrowDown => FocusDir::Down,
        _ => return None,
    };
    Some(InputAction::Focus(dir))
}

// ---- legacy (xterm) encoding ----

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

/// Prefix `bytes` with ESC when Alt is held (Meta sends ESC).
fn with_alt(alt: bool, mut bytes: Vec<u8>) -> Vec<u8> {
    if alt {
        bytes.insert(0, 0x1b);
    }
    bytes
}

/// Control byte for Ctrl+<physical key>, independent of the active layout (works on Arabic layouts).
fn ctrl_byte(key_code: KeyCode) -> Option<u8> {
    if let Some(letter) = key_letter(key_code) {
        return Some(letter - b'a' + 1);
    }
    match key_code {
        KeyCode::Space | KeyCode::Digit2 => Some(0x00),
        KeyCode::BracketLeft | KeyCode::Digit3 => Some(0x1b),
        KeyCode::Backslash | KeyCode::Digit4 => Some(0x1c),
        KeyCode::BracketRight | KeyCode::Digit5 => Some(0x1d),
        KeyCode::Digit6 => Some(0x1e),
        KeyCode::Slash | KeyCode::Digit7 => Some(0x1f),
        _ => None,
    }
}

/// AltGr arrives as Ctrl+Alt on Windows: symbols like @ € { } it types go through untouched.
fn altgr_text(key: &KeyInput) -> Option<Vec<u8>> {
    if !(key.modifiers.control_key() && key.modifiers.alt_key()) {
        return None;
    }
    key.text
        .filter(|t| {
            !t.is_empty()
                && t.chars()
                    .all(|c| !c.is_control() && !c.is_ascii_alphanumeric())
        })
        .map(|t| t.as_bytes().to_vec())
}

fn legacy_key(key: &KeyInput, mode: TermMode, shortcuts: bool) -> Option<InputAction> {
    if !key.pressed() {
        return None;
    }
    if let Some(bytes) = altgr_text(key) {
        return Some(InputAction::Bytes(bytes));
    }
    // App shortcuts and Ctrl combos go by the typed Latin letter, else the physical key
    // (so Ctrl+A is ^A on AZERTY, and Ctrl works on Arabic layouts).
    let key_code = shortcut_key(key.physical, key.logical);
    if let Some(action) = key_code
        .filter(|_| shortcuts)
        .and_then(|k| app_shortcut(k, key.modifiers))
    {
        return Some(action);
    }
    legacy_bytes(key, key_code, mode).map(InputAction::Bytes)
}

/// The legacy bytes of a pressed key that is not a shortcut.
fn legacy_bytes(key: &KeyInput, key_code: Option<KeyCode>, mode: TermMode) -> Option<Vec<u8>> {
    let (ctrl, alt) = (key.modifiers.control_key(), key.modifiers.alt_key());
    // Application keypad (DECKPAM): numpad keys send SS3 codes so programs can tell them apart.
    if mode.contains(TermMode::APP_KEYPAD) && !ctrl && !alt {
        if let Some(byte) = key.physical.and_then(app_keypad_byte) {
            return Some(vec![0x1b, b'O', byte]);
        }
    }
    if ctrl {
        if let Some(byte) = key_code.and_then(ctrl_byte) {
            return Some(with_alt(alt, vec![byte]));
        }
    }
    if let Key::Named(named) = key.logical {
        if let Some(bytes) = legacy_named(named, key.modifiers, mode.contains(TermMode::APP_CURSOR))
        {
            return Some(bytes);
        }
    }
    // Typed text (Arabic, Latin, numbers, symbols); Alt sends an ESC prefix (Meta).
    key.text
        .filter(|t| !ctrl && !t.is_empty())
        .map(|t| with_alt(alt, t.as_bytes().to_vec()))
}

/// Final byte of the SS3 sequence a numpad key sends in application keypad mode (xterm).
fn app_keypad_byte(code: KeyCode) -> Option<u8> {
    Some(match code {
        KeyCode::Numpad0 => b'p',
        KeyCode::Numpad1 => b'q',
        KeyCode::Numpad2 => b'r',
        KeyCode::Numpad3 => b's',
        KeyCode::Numpad4 => b't',
        KeyCode::Numpad5 => b'u',
        KeyCode::Numpad6 => b'v',
        KeyCode::Numpad7 => b'w',
        KeyCode::Numpad8 => b'x',
        KeyCode::Numpad9 => b'y',
        KeyCode::NumpadDecimal => b'n',
        KeyCode::NumpadDivide => b'o',
        KeyCode::NumpadMultiply => b'j',
        KeyCode::NumpadSubtract => b'm',
        KeyCode::NumpadAdd => b'k',
        KeyCode::NumpadEnter => b'M',
        KeyCode::NumpadEqual => b'X',
        _ => return None,
    })
}

/// Named navigation and control keys.
fn legacy_named(named: &NamedKey, modifiers: ModifiersState, app_cursor: bool) -> Option<Vec<u8>> {
    let ctrl = modifiers.control_key();
    let byte = match named {
        NamedKey::Enter => b'\r',
        // Ctrl+Backspace deletes a word in most shells (^H), plain Backspace is DEL.
        NamedKey::Backspace if ctrl => 0x08,
        NamedKey::Backspace => 0x7f,
        NamedKey::Tab if modifiers.shift_key() => return Some(b"\x1b[Z".to_vec()),
        NamedKey::Tab => b'\t',
        NamedKey::Space if ctrl => 0x00,
        NamedKey::Escape => 0x1b,
        _ => {
            let mods = modifier_param(modifiers);
            return FuncKey::legacy(named).map(|k| k.legacy_bytes(mods, app_cursor));
        }
    };
    Some(with_alt(modifiers.alt_key(), vec![byte]))
}

/// Functional keys, which keep their CSI/SS3 shape in both encodings.
#[derive(Clone, Copy)]
enum FuncKey {
    /// `CSI X` / `SS3 X` / `CSI 1;m X`.
    Cursor(char),
    /// `CSI n ~` / `CSI n;m ~`.
    Tilde(u8),
    /// `SS3 X` / `CSI 1;m X`.
    Ss3(char),
}

impl FuncKey {
    /// The kitty protocol's table (F3 is `CSI 13 ~` there).
    fn kitty(named: &NamedKey) -> Option<FuncKey> {
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

    /// The xterm table: like kitty's, except that F3 is `SS3 R`.
    fn legacy(named: &NamedKey) -> Option<FuncKey> {
        match named {
            NamedKey::F3 => Some(FuncKey::Ss3('R')),
            _ => FuncKey::kitty(named),
        }
    }

    /// The xterm encoding, with modifier parameter `mods` (1 = none).
    fn legacy_bytes(self, mods: u8, app_cursor: bool) -> Vec<u8> {
        match self {
            FuncKey::Cursor(c) => cursor_key(c, mods, app_cursor),
            FuncKey::Tilde(n) => tilde_key(n, mods),
            FuncKey::Ss3(c) => ss3_function_key(c, mods),
        }
    }
}

// ---- kitty keyboard protocol ----

/// Kitty modifier parameter: 1 + Shift(1) + Alt(2) + Ctrl(4) + Super(8).
fn kitty_modifier_param(modifiers: ModifiersState) -> u8 {
    modifier_param(modifiers) + 8 * modifiers.super_key() as u8
}

/// Digit and punctuation keys and the unshifted ASCII character each has on a US layout.
const BASE_LAYOUT_KEYS: [(KeyCode, char); 21] = [
    (KeyCode::Digit0, '0'),
    (KeyCode::Digit1, '1'),
    (KeyCode::Digit2, '2'),
    (KeyCode::Digit3, '3'),
    (KeyCode::Digit4, '4'),
    (KeyCode::Digit5, '5'),
    (KeyCode::Digit6, '6'),
    (KeyCode::Digit7, '7'),
    (KeyCode::Digit8, '8'),
    (KeyCode::Digit9, '9'),
    (KeyCode::Minus, '-'),
    (KeyCode::Equal, '='),
    (KeyCode::BracketLeft, '['),
    (KeyCode::BracketRight, ']'),
    (KeyCode::Backslash, '\\'),
    (KeyCode::Semicolon, ';'),
    (KeyCode::Quote, '\''),
    (KeyCode::Comma, ','),
    (KeyCode::Period, '.'),
    (KeyCode::Slash, '/'),
    (KeyCode::Backquote, '`'),
];

/// Unshifted ASCII character a physical key produces on a US layout (the "base layout key").
fn physical_base_char(key_code: KeyCode) -> Option<char> {
    key_letter(key_code).map(char::from).or_else(|| {
        BASE_LAYOUT_KEYS
            .iter()
            .find(|(k, _)| *k == key_code)
            .map(|&(_, c)| c)
    })
}

/// Keypad keys and their kitty codepoints (reported only with "report all keys as escape
/// codes").
const KEYPAD_KEYS: [(KeyCode, u32); 17] = [
    (KeyCode::Numpad0, 57399),
    (KeyCode::Numpad1, 57400),
    (KeyCode::Numpad2, 57401),
    (KeyCode::Numpad3, 57402),
    (KeyCode::Numpad4, 57403),
    (KeyCode::Numpad5, 57404),
    (KeyCode::Numpad6, 57405),
    (KeyCode::Numpad7, 57406),
    (KeyCode::Numpad8, 57407),
    (KeyCode::Numpad9, 57408),
    (KeyCode::NumpadDecimal, 57409),
    (KeyCode::NumpadDivide, 57410),
    (KeyCode::NumpadMultiply, 57411),
    (KeyCode::NumpadSubtract, 57412),
    (KeyCode::NumpadAdd, 57413),
    (KeyCode::NumpadEnter, 57414),
    (KeyCode::NumpadEqual, 57415),
];

fn keypad_code(key_code: KeyCode) -> Option<u32> {
    KEYPAD_KEYS
        .iter()
        .find(|(k, _)| *k == key_code)
        .map(|&(_, code)| code)
}

/// The progressive-enhancement flags a program pushed (`CSI > flags u`).
struct KittyFlags {
    disambiguate: bool,
    report_events: bool,
    alternates: bool,
    all_keys: bool,
    associated_text: bool,
}

impl KittyFlags {
    fn from_mode(mode: TermMode) -> Self {
        Self {
            disambiguate: mode.contains(TermMode::DISAMBIGUATE_ESC_CODES),
            report_events: mode.contains(TermMode::REPORT_EVENT_TYPES),
            alternates: mode.contains(TermMode::REPORT_ALTERNATE_KEYS),
            all_keys: mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC),
            associated_text: mode.contains(TermMode::REPORT_ASSOCIATED_TEXT),
        }
    }
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
    /// A key reported by its code alone.
    fn new(code: u32, mods: u8, event: Option<u8>) -> Self {
        Self {
            code,
            shifted: None,
            base: None,
            mods,
            event,
            text: None,
        }
    }

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
        return key.legacy_bytes(1, app_cursor);
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

/// How the kitty protocol treats a named key that is not a functional key.
enum KittyNamed {
    /// Report it as `CSI code u`.
    Code(u32),
    /// Leave it to the legacy encoding.
    Legacy,
    /// Encode it like a text key (Space).
    Text,
}

fn kitty_named(named: &NamedKey, flags: &KittyFlags, mods: u8, pressed: bool) -> KittyNamed {
    match named {
        // Enter/Tab/Backspace stay legacy unless modified; no release events without flag 8.
        NamedKey::Enter | NamedKey::Tab | NamedKey::Backspace => {
            if !flags.all_keys && (mods == 1 || !pressed) {
                return KittyNamed::Legacy;
            }
            KittyNamed::Code(match named {
                NamedKey::Enter => 13,
                NamedKey::Tab => 9,
                _ => 127,
            })
        }
        NamedKey::Escape if flags.disambiguate || flags.all_keys => KittyNamed::Code(27),
        NamedKey::Space => KittyNamed::Text,
        _ => KittyNamed::Legacy,
    }
}

/// The event type field: 2 for a repeat, 3 for a release, nothing for a press.
fn kitty_event(pressed: bool, repeat: bool) -> Option<u8> {
    match (pressed, repeat) {
        (false, _) => Some(3),
        (true, true) => Some(2),
        (true, false) => None,
    }
}

/// Encode a key under the kitty keyboard protocol. `None` means "not handled here": the caller
/// falls back to the legacy encoding for presses.
fn encode_kitty(key: &KeyInput, mode: TermMode) -> Option<Vec<u8>> {
    let flags = KittyFlags::from_mode(mode);
    let event = kitty_event(key.pressed(), key.repeat).filter(|_| flags.report_events);
    let mods = kitty_modifier_param(key.modifiers);

    if let Key::Named(named) = key.logical {
        if let Some(func) = FuncKey::kitty(named) {
            let app_cursor = mode.contains(TermMode::APP_CURSOR);
            return Some(kitty_func_key(func, mods, event, app_cursor));
        }
        match kitty_named(named, &flags, mods, key.pressed()) {
            KittyNamed::Code(code) => return Some(KittyKey::new(code, mods, event).encode()),
            KittyNamed::Legacy => return None,
            KittyNamed::Text => {}
        }
    }

    // Keypad keys have dedicated codepoints, but only when everything is reported as escapes.
    if flags.all_keys {
        if let Some(code) = key.physical.and_then(keypad_code) {
            return Some(KittyKey::new(code, mods, event).encode());
        }
    }
    kitty_text_key(key, &flags, mods, event).map(|k| k.encode())
}

/// A text-producing key: CSI u when forced, or when Ctrl/Alt/Super would make the legacy form
/// ambiguous.
fn kitty_text_key(
    key: &KeyInput,
    flags: &KittyFlags,
    mods: u8,
    event: Option<u8>,
) -> Option<KittyKey> {
    let m = key.modifiers;
    if !(flags.all_keys
        || (flags.disambiguate && (m.control_key() || m.alt_key() || m.super_key())))
    {
        return None;
    }
    let first_char = |k: &Key| match k {
        Key::Character(s) => s.chars().next(),
        Key::Named(NamedKey::Space) => Some(' '),
        _ => None,
    };
    let lower = |c: char| c.to_lowercase().next().unwrap_or(c);
    let base_char = key.physical.and_then(physical_base_char);
    let code_char = key
        .unshifted
        .and_then(first_char)
        .or_else(|| first_char(key.logical))
        .map(lower)
        .or(base_char)?;
    let code = code_char as u32;

    let (mut shifted, mut base) = (None, None);
    if flags.alternates {
        if m.shift_key() {
            shifted = first_char(key.logical)
                .map(|c| c as u32)
                .filter(|&c| c != code);
        }
        base = base_char.map(|c| c as u32).filter(|&c| c != code);
    }
    let text = key
        .text
        .filter(|t| {
            flags.associated_text
                && flags.all_keys
                && key.pressed()
                && !m.control_key()
                && !m.super_key()
                && !t.is_empty()
                && t.chars().all(|c| !c.is_control())
        })
        .map(str::to_owned);
    Some(KittyKey {
        shifted,
        base,
        text,
        ..KittyKey::new(code, mods, event)
    })
}

/// Shortcuts, then the kitty protocol when enabled, then the legacy encoding.
fn key_with_mode(key: &KeyInput, mode: TermMode, shortcuts: bool) -> Option<InputAction> {
    if !mode.intersects(TermMode::KITTY_KEYBOARD_PROTOCOL) {
        return legacy_key(key, mode, shortcuts);
    }

    // Releases are only reported when the application asked for event types.
    if !key.pressed() && !mode.contains(TermMode::REPORT_EVENT_TYPES) {
        return None;
    }

    let shortcut = shortcut_key(key.physical, key.logical)
        .filter(|_| shortcuts)
        .and_then(|c| app_shortcut(c, key.modifiers));
    if let Some(action) = shortcut {
        // Twitty consumed the press; a lone release would confuse the program.
        return key.pressed().then_some(action);
    }
    if key.pressed() {
        if let Some(bytes) = altgr_text(key) {
            return Some(InputAction::Bytes(bytes));
        }
    }

    match encode_kitty(key, mode) {
        Some(bytes) => Some(InputAction::Bytes(bytes)),
        None => legacy_key(key, mode, shortcuts),
    }
}

// ---- paste and mouse ----

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

/// Encode a mouse report in the format the program asked for: SGR (1006), UTF-8 (1005, for
/// coordinates past 223 on wide screens) or the legacy one.
pub fn encode_mouse_for_mode(
    button: u8,
    col: usize,
    row: usize,
    pressed: bool,
    modifiers: ModifiersState,
    mode: TermMode,
) -> Vec<u8> {
    if mode.contains(TermMode::SGR_MOUSE) || !mode.contains(TermMode::UTF8_MOUSE) {
        return encode_mouse(
            button,
            col,
            row,
            pressed,
            modifiers,
            mode.contains(TermMode::SGR_MOUSE),
        );
    }
    let mut out = encode_mouse(button, 0, 0, pressed, modifiers, false);
    out.truncate(4);
    // Each coordinate is one UTF-8 character, up to 2015 (the largest 2-byte code point - 32).
    for v in [col, row] {
        let c = char::from_u32((v + 1).min(2015) as u32 + 32).unwrap_or(' ');
        let mut buf = [0u8; 4];
        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
    }
    out
}
