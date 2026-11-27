//! Key name and modifier encoding for cmux `surface.send_key`.

use alacritty_terminal::term::TermMode;
use winit::event::ElementState;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey};

use crate::input::terminal_key_raw;

pub(super) const MAX_KEY_MODS: usize = 16;

/// Encode a cmux `surface.send_key` request (`key`, `mods`) into terminal input bytes using
/// Twitty's key encoder and the target pane's current [`TermMode`].
pub fn encode_key_name(key: &str, mods: &[String], mode: TermMode) -> Result<Vec<u8>, String> {
    if key.is_empty() {
        return Err("key must not be empty".into());
    }
    let mut modifiers = ModifiersState::empty();
    let mut base_key = key;

    if mods.is_empty() && key.len() > 1 && key.contains('+') {
        let parts: Vec<&str> = key.split('+').collect();
        if parts.len() >= 2
            && parts[..parts.len() - 1]
                .iter()
                .all(|m| parse_mod(m).is_some())
        {
            for m in &parts[..parts.len() - 1] {
                if let Some(flag) = parse_mod(m) {
                    modifiers |= flag;
                }
            }
            base_key = parts[parts.len() - 1];
        }
    } else {
        for m in mods {
            let Some(flag) = parse_mod(m) else {
                return Err(format!("unknown modifier: {m:?}"));
            };
            modifiers |= flag;
        }
    }

    let (physical, logical, text_owned) = resolve_key(base_key)?;
    let text_adjusted = if modifiers.shift_key() && !modifiers.control_key() {
        text_owned.as_deref().map(|t| t.to_ascii_uppercase())
    } else {
        text_owned
    };
    let text = text_adjusted.as_deref();

    terminal_key_raw(
        physical,
        &logical,
        Some(&logical),
        text,
        ElementState::Pressed,
        false,
        modifiers,
        mode,
    )
    .ok_or_else(|| format!("key {key:?} produced no terminal input"))
}

fn parse_mod(raw: &str) -> Option<ModifiersState> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(ModifiersState::CONTROL),
        "shift" => Some(ModifiersState::SHIFT),
        "alt" | "opt" | "option" => Some(ModifiersState::ALT),
        "super" | "meta" | "cmd" | "command" => Some(ModifiersState::SUPER),
        _ => None,
    }
}

fn resolve_key(name: &str) -> Result<(Option<KeyCode>, Key, Option<String>), String> {
    let mut chars = name.chars();
    if let Some(ch) = chars.next() {
        if chars.next().is_none() {
            if ch.is_ascii_control() {
                return Err("control characters are not valid key names".into());
            }
            if ch == ' ' {
                return Ok((
                    Some(KeyCode::Space),
                    Key::Named(NamedKey::Space),
                    Some(" ".into()),
                ));
            }
            if ch.is_ascii_graphic() {
                let physical = ascii_keycode(ch);
                let s = ch.to_string();
                return Ok((physical, Key::Character(s.clone().into()), Some(s)));
            }
            return Err(format!("unsupported key name: {name:?}"));
        }
    }

    let (physical, named, text) = match name.to_ascii_lowercase().as_str() {
        "return" | "enter" => (Some(KeyCode::Enter), NamedKey::Enter, None),
        "escape" | "esc" => (Some(KeyCode::Escape), NamedKey::Escape, None),
        "tab" => (Some(KeyCode::Tab), NamedKey::Tab, None),
        "backspace" => (Some(KeyCode::Backspace), NamedKey::Backspace, None),
        "delete" | "del" => (Some(KeyCode::Delete), NamedKey::Delete, None),
        "space" => (Some(KeyCode::Space), NamedKey::Space, Some(" ".into())),
        "up" | "arrow_up" => (Some(KeyCode::ArrowUp), NamedKey::ArrowUp, None),
        "down" | "arrow_down" => (Some(KeyCode::ArrowDown), NamedKey::ArrowDown, None),
        "left" | "arrow_left" => (Some(KeyCode::ArrowLeft), NamedKey::ArrowLeft, None),
        "right" | "arrow_right" => (Some(KeyCode::ArrowRight), NamedKey::ArrowRight, None),
        "home" => (Some(KeyCode::Home), NamedKey::Home, None),
        "end" => (Some(KeyCode::End), NamedKey::End, None),
        "page_up" | "pageup" => (Some(KeyCode::PageUp), NamedKey::PageUp, None),
        "page_down" | "pagedown" => (Some(KeyCode::PageDown), NamedKey::PageDown, None),
        "insert" => (Some(KeyCode::Insert), NamedKey::Insert, None),
        "f1" => (Some(KeyCode::F1), NamedKey::F1, None),
        "f2" => (Some(KeyCode::F2), NamedKey::F2, None),
        "f3" => (Some(KeyCode::F3), NamedKey::F3, None),
        "f4" => (Some(KeyCode::F4), NamedKey::F4, None),
        "f5" => (Some(KeyCode::F5), NamedKey::F5, None),
        "f6" => (Some(KeyCode::F6), NamedKey::F6, None),
        "f7" => (Some(KeyCode::F7), NamedKey::F7, None),
        "f8" => (Some(KeyCode::F8), NamedKey::F8, None),
        "f9" => (Some(KeyCode::F9), NamedKey::F9, None),
        "f10" => (Some(KeyCode::F10), NamedKey::F10, None),
        "f11" => (Some(KeyCode::F11), NamedKey::F11, None),
        "f12" => (Some(KeyCode::F12), NamedKey::F12, None),
        _ => return Err(format!("unknown key name: {name:?}")),
    };
    Ok((physical, Key::Named(named), text))
}

fn ascii_keycode(ch: char) -> Option<KeyCode> {
    Some(match ch.to_ascii_lowercase() {
        'a' => KeyCode::KeyA,
        'b' => KeyCode::KeyB,
        'c' => KeyCode::KeyC,
        'd' => KeyCode::KeyD,
        'e' => KeyCode::KeyE,
        'f' => KeyCode::KeyF,
        'g' => KeyCode::KeyG,
        'h' => KeyCode::KeyH,
        'i' => KeyCode::KeyI,
        'j' => KeyCode::KeyJ,
        'k' => KeyCode::KeyK,
        'l' => KeyCode::KeyL,
        'm' => KeyCode::KeyM,
        'n' => KeyCode::KeyN,
        'o' => KeyCode::KeyO,
        'p' => KeyCode::KeyP,
        'q' => KeyCode::KeyQ,
        'r' => KeyCode::KeyR,
        's' => KeyCode::KeyS,
        't' => KeyCode::KeyT,
        'u' => KeyCode::KeyU,
        'v' => KeyCode::KeyV,
        'w' => KeyCode::KeyW,
        'x' => KeyCode::KeyX,
        'y' => KeyCode::KeyY,
        'z' => KeyCode::KeyZ,
        '0' => KeyCode::Digit0,
        '1' => KeyCode::Digit1,
        '2' => KeyCode::Digit2,
        '3' => KeyCode::Digit3,
        '4' => KeyCode::Digit4,
        '5' => KeyCode::Digit5,
        '6' => KeyCode::Digit6,
        '7' => KeyCode::Digit7,
        '8' => KeyCode::Digit8,
        '9' => KeyCode::Digit9,
        '[' => KeyCode::BracketLeft,
        ']' => KeyCode::BracketRight,
        '\\' => KeyCode::Backslash,
        '/' => KeyCode::Slash,
        _ => return None,
    })
}
