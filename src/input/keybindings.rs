//! Customizable keyboard shortcuts: chord parsing, plain-typing guard, config
//! serialization (preserving JSON map order so last-entry-wins conflict resolution
//! works deterministically), default bindings, overrides, unbinds, and conflict tracking.

use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use winit::keyboard::{KeyCode, ModifiersState};

use crate::commands;
use crate::input::InputAction;
use crate::shell_integration::clean_text;

/// Maximum number of user keybinding entries kept from `config.json`.
const MAX_KEYBINDING_ENTRIES: usize = 256;

/// Maximum number of chords allowed per command entry.
const MAX_CHORDS_PER_COMMAND: usize = 16;

/// Maximum characters accepted in one raw keybinding string.
const MAX_CHORD_STR_CHARS: usize = 64;

/// A single key chord: modifiers plus one key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub super_key: bool,
    pub key: KeyCode,
}

/// Why a chord string failed to parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChordParseError {
    Empty,
    MissingKey,
    DuplicateModifier(&'static str),
    MultipleKeys,
    UnknownToken(String),
}

impl ChordParseError {
    pub fn message_en(&self) -> String {
        match self {
            Self::Empty => "empty chord".to_owned(),
            Self::MissingKey => "missing key after modifier".to_owned(),
            Self::DuplicateModifier(m) => format!("duplicate modifier {m}"),
            Self::MultipleKeys => "multiple keys in a single chord".to_owned(),
            Self::UnknownToken(tok) => format!("unknown key or modifier {tok:?}"),
        }
    }

    pub fn message_ar(&self) -> String {
        match self {
            Self::Empty => "اختصار فارغ".to_owned(),
            Self::MissingKey => "ينقص مفتاح بعد مفتاح التعديل".to_owned(),
            Self::DuplicateModifier(m) => format!("مفتاح تعديل مكرر {m}"),
            Self::MultipleKeys => "أكثر من مفتاح في اختصار واحد".to_owned(),
            Self::UnknownToken(tok) => format!("مفتاح غير معروف {tok:?}"),
        }
    }
}

impl fmt::Display for ChordParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message_en())
    }
}

impl Chord {
    pub const fn new(ctrl: bool, shift: bool, alt: bool, super_key: bool, key: KeyCode) -> Self {
        Self {
            ctrl,
            shift,
            alt,
            super_key,
            key,
        }
    }

    pub fn from_modifiers(modifiers: ModifiersState, key: KeyCode) -> Self {
        Self {
            ctrl: modifiers.control_key(),
            shift: modifiers.shift_key(),
            alt: modifiers.alt_key(),
            super_key: modifiers.super_key(),
            key,
        }
    }

    /// True if binding this chord would swallow ordinary typing in the terminal.
    ///
    /// Chords with Ctrl, Alt, or Super are always allowed. Unmodified or Shift-only chords
    /// are only allowed on function (`F1`–`F12`) and navigation keys.
    pub fn swallows_plain_typing(self) -> bool {
        !self.ctrl && !self.alt && !self.super_key && !is_function_or_navigation_key(self.key)
    }

    /// Parse a single chord string such as `"Ctrl+Shift+T"`, `"Alt+Enter"`, `"Super+F5"`,
    /// `"Ctrl+,"`, or `"Ctrl+Plus"`.
    ///
    /// Multi-chord sequences (e.g. `"Ctrl+K Ctrl+S"`), modifier-only strings, duplicate
    /// modifiers, and unknown tokens are rejected.
    pub fn parse(input: &str) -> Result<Self, ChordParseError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(ChordParseError::Empty);
        }

        let tokens = split_chord_tokens(trimmed)?;
        let mut ctrl = false;
        let mut shift = false;
        let mut alt = false;
        let mut super_key = false;
        let mut key: Option<KeyCode> = None;

        for raw in tokens {
            let tok = raw.trim();
            if tok.is_empty() {
                return Err(ChordParseError::UnknownToken(raw.to_owned()));
            }
            // Reject whitespace inside a token (e.g. "Ctrl+K Ctrl+S" -> "K Ctrl").
            if tok.chars().any(char::is_whitespace) {
                return Err(ChordParseError::UnknownToken(tok.to_owned()));
            }
            let lower = tok.to_ascii_lowercase();
            match lower.as_str() {
                "ctrl" | "control" => {
                    if ctrl {
                        return Err(ChordParseError::DuplicateModifier("Ctrl"));
                    }
                    ctrl = true;
                }
                "shift" => {
                    if shift {
                        return Err(ChordParseError::DuplicateModifier("Shift"));
                    }
                    shift = true;
                }
                "alt" | "option" | "opt" => {
                    if alt {
                        return Err(ChordParseError::DuplicateModifier("Alt"));
                    }
                    alt = true;
                }
                "super" | "meta" | "cmd" | "command" | "win" | "logo" => {
                    if super_key {
                        return Err(ChordParseError::DuplicateModifier("Super"));
                    }
                    super_key = true;
                }
                _ => {
                    let parsed = parse_key_token(&lower)
                        .ok_or_else(|| ChordParseError::UnknownToken(tok.to_owned()))?;
                    if key.is_some() {
                        return Err(ChordParseError::MultipleKeys);
                    }
                    key = Some(parsed);
                }
            }
        }

        let key = key.ok_or(ChordParseError::MissingKey)?;
        Ok(Self {
            ctrl,
            shift,
            alt,
            super_key,
            key,
        })
    }

    /// Canonical display form, e.g. `"Ctrl+Shift+T"`, `"Alt+Enter"`, `"Ctrl+,"`.
    /// Never contains `'+'` inside the key name itself, so UI keycap splitters work cleanly.
    pub fn canonical_string(self) -> String {
        let mut out = String::new();
        if self.ctrl {
            out.push_str("Ctrl+");
        }
        if self.shift {
            out.push_str("Shift+");
        }
        if self.alt {
            out.push_str("Alt+");
        }
        if self.super_key {
            out.push_str("Super+");
        }
        out.push_str(key_code_name(self.key).unwrap_or("?"));
        out
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical_string())
    }
}

/// Split a chord string on `'+'`, treating a trailing `"++"` (like `"Ctrl++"`) as separator + `"+"`.
fn split_chord_tokens(s: &str) -> Result<Vec<&str>, ChordParseError> {
    if s == "+" {
        return Ok(vec!["+"]);
    }
    if s.starts_with('+') {
        return Err(ChordParseError::UnknownToken(s.to_owned()));
    }
    let (head, trailing_plus) = if let Some(prefix) = s.strip_suffix("++") {
        (prefix, true)
    } else if s.ends_with('+') {
        return Err(ChordParseError::MissingKey);
    } else {
        (s, false)
    };
    let mut parts: Vec<&str> = if head.is_empty() {
        Vec::new()
    } else {
        head.split('+').collect()
    };
    if trailing_plus {
        parts.push("+");
    }
    if parts.is_empty() {
        return Err(ChordParseError::Empty);
    }
    Ok(parts)
}

/// Keys permitted in a chord without Ctrl, Alt, or Super (function and navigation keys).
pub fn is_function_or_navigation_key(key: KeyCode) -> bool {
    matches!(
        key,
        KeyCode::F1
            | KeyCode::F2
            | KeyCode::F3
            | KeyCode::F4
            | KeyCode::F5
            | KeyCode::F6
            | KeyCode::F7
            | KeyCode::F8
            | KeyCode::F9
            | KeyCode::F10
            | KeyCode::F11
            | KeyCode::F12
            | KeyCode::ArrowUp
            | KeyCode::ArrowDown
            | KeyCode::ArrowLeft
            | KeyCode::ArrowRight
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Insert
            | KeyCode::Delete
    )
}

/// True when `code` is a lone modifier key (ignored while capturing a shortcut).
pub fn is_modifier_key_code(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::ShiftLeft
            | KeyCode::ShiftRight
            | KeyCode::ControlLeft
            | KeyCode::ControlRight
            | KeyCode::AltLeft
            | KeyCode::AltRight
            | KeyCode::SuperLeft
            | KeyCode::SuperRight
            | KeyCode::CapsLock
            | KeyCode::NumLock
            | KeyCode::ScrollLock
    )
}

/// True when `code` is supported as a bindable shortcut key.
pub fn is_bindable_key_code(code: KeyCode) -> bool {
    key_code_name(code).is_some()
}

fn parse_key_token(lower: &str) -> Option<KeyCode> {
    if lower.len() == 1 {
        let b = lower.as_bytes()[0];
        if b.is_ascii_lowercase() {
            return Some(LETTER_CODES[(b - b'a') as usize]);
        }
        if b.is_ascii_digit() {
            return Some(DIGIT_CODES[(b - b'0') as usize]);
        }
    }
    Some(match lower {
        // Punctuation & symbols
        "," | "comma" => KeyCode::Comma,
        "." | "period" | "dot" => KeyCode::Period,
        "/" | "slash" => KeyCode::Slash,
        ";" | "semicolon" => KeyCode::Semicolon,
        "'" | "quote" | "apostrophe" => KeyCode::Quote,
        "[" | "bracketleft" | "lbracket" => KeyCode::BracketLeft,
        "]" | "bracketright" | "rbracket" => KeyCode::BracketRight,
        "\\" | "backslash" => KeyCode::Backslash,
        "-" | "minus" | "dash" => KeyCode::Minus,
        "=" | "equal" | "equals" | "+" | "plus" => KeyCode::Equal,
        "`" | "backquote" | "grave" | "backtick" => KeyCode::Backquote,
        // Named keys
        "enter" | "return" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "space" | "spacebar" => KeyCode::Space,
        "escape" | "esc" => KeyCode::Escape,
        "backspace" | "bs" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" | "page_up" => KeyCode::PageUp,
        "pagedown" | "pgdn" | "pgdown" | "page_down" => KeyCode::PageDown,
        "up" | "arrowup" | "arrow_up" => KeyCode::ArrowUp,
        "down" | "arrowdown" | "arrow_down" => KeyCode::ArrowDown,
        "left" | "arrowleft" | "arrow_left" => KeyCode::ArrowLeft,
        "right" | "arrowright" | "arrow_right" => KeyCode::ArrowRight,
        // Function keys
        "f1" => KeyCode::F1,
        "f2" => KeyCode::F2,
        "f3" => KeyCode::F3,
        "f4" => KeyCode::F4,
        "f5" => KeyCode::F5,
        "f6" => KeyCode::F6,
        "f7" => KeyCode::F7,
        "f8" => KeyCode::F8,
        "f9" => KeyCode::F9,
        "f10" => KeyCode::F10,
        "f11" => KeyCode::F11,
        "f12" => KeyCode::F12,
        // Numpad keys
        "numpad0" | "kp_0" | "kp0" => KeyCode::Numpad0,
        "numpad1" | "kp_1" | "kp1" => KeyCode::Numpad1,
        "numpad2" | "kp_2" | "kp2" => KeyCode::Numpad2,
        "numpad3" | "kp_3" | "kp3" => KeyCode::Numpad3,
        "numpad4" | "kp_4" | "kp4" => KeyCode::Numpad4,
        "numpad5" | "kp_5" | "kp5" => KeyCode::Numpad5,
        "numpad6" | "kp_6" | "kp6" => KeyCode::Numpad6,
        "numpad7" | "kp_7" | "kp7" => KeyCode::Numpad7,
        "numpad8" | "kp_8" | "kp8" => KeyCode::Numpad8,
        "numpad9" | "kp_9" | "kp9" => KeyCode::Numpad9,
        "numpadadd" | "kp_add" => KeyCode::NumpadAdd,
        "numpadsubtract" | "kp_subtract" => KeyCode::NumpadSubtract,
        "numpadmultiply" | "kp_multiply" => KeyCode::NumpadMultiply,
        "numpaddivide" | "kp_divide" => KeyCode::NumpadDivide,
        "numpaddecimal" | "kp_decimal" => KeyCode::NumpadDecimal,
        "numpadenter" | "kp_enter" => KeyCode::NumpadEnter,
        "numpadequal" | "kp_equal" => KeyCode::NumpadEqual,
        _ => return None,
    })
}

/// Canonical display name of a bindable `KeyCode`.
pub fn key_code_name(code: KeyCode) -> Option<&'static str> {
    Some(match code {
        KeyCode::KeyA => "A",
        KeyCode::KeyB => "B",
        KeyCode::KeyC => "C",
        KeyCode::KeyD => "D",
        KeyCode::KeyE => "E",
        KeyCode::KeyF => "F",
        KeyCode::KeyG => "G",
        KeyCode::KeyH => "H",
        KeyCode::KeyI => "I",
        KeyCode::KeyJ => "J",
        KeyCode::KeyK => "K",
        KeyCode::KeyL => "L",
        KeyCode::KeyM => "M",
        KeyCode::KeyN => "N",
        KeyCode::KeyO => "O",
        KeyCode::KeyP => "P",
        KeyCode::KeyQ => "Q",
        KeyCode::KeyR => "R",
        KeyCode::KeyS => "S",
        KeyCode::KeyT => "T",
        KeyCode::KeyU => "U",
        KeyCode::KeyV => "V",
        KeyCode::KeyW => "W",
        KeyCode::KeyX => "X",
        KeyCode::KeyY => "Y",
        KeyCode::KeyZ => "Z",
        KeyCode::Digit0 => "0",
        KeyCode::Digit1 => "1",
        KeyCode::Digit2 => "2",
        KeyCode::Digit3 => "3",
        KeyCode::Digit4 => "4",
        KeyCode::Digit5 => "5",
        KeyCode::Digit6 => "6",
        KeyCode::Digit7 => "7",
        KeyCode::Digit8 => "8",
        KeyCode::Digit9 => "9",
        KeyCode::Comma => ",",
        KeyCode::Period => ".",
        KeyCode::Slash => "/",
        KeyCode::Semicolon => ";",
        KeyCode::Quote => "'",
        KeyCode::BracketLeft => "[",
        KeyCode::BracketRight => "]",
        KeyCode::Backslash => "\\",
        KeyCode::Minus => "-",
        KeyCode::Equal => "=",
        KeyCode::Backquote => "`",
        KeyCode::Enter => "Enter",
        KeyCode::Tab => "Tab",
        KeyCode::Space => "Space",
        KeyCode::Escape => "Escape",
        KeyCode::Backspace => "Backspace",
        KeyCode::Delete => "Delete",
        KeyCode::Insert => "Insert",
        KeyCode::Home => "Home",
        KeyCode::End => "End",
        KeyCode::PageUp => "PageUp",
        KeyCode::PageDown => "PageDown",
        KeyCode::ArrowUp => "Up",
        KeyCode::ArrowDown => "Down",
        KeyCode::ArrowLeft => "Left",
        KeyCode::ArrowRight => "Right",
        KeyCode::F1 => "F1",
        KeyCode::F2 => "F2",
        KeyCode::F3 => "F3",
        KeyCode::F4 => "F4",
        KeyCode::F5 => "F5",
        KeyCode::F6 => "F6",
        KeyCode::F7 => "F7",
        KeyCode::F8 => "F8",
        KeyCode::F9 => "F9",
        KeyCode::F10 => "F10",
        KeyCode::F11 => "F11",
        KeyCode::F12 => "F12",
        KeyCode::Numpad0 => "Numpad0",
        KeyCode::Numpad1 => "Numpad1",
        KeyCode::Numpad2 => "Numpad2",
        KeyCode::Numpad3 => "Numpad3",
        KeyCode::Numpad4 => "Numpad4",
        KeyCode::Numpad5 => "Numpad5",
        KeyCode::Numpad6 => "Numpad6",
        KeyCode::Numpad7 => "Numpad7",
        KeyCode::Numpad8 => "Numpad8",
        KeyCode::Numpad9 => "Numpad9",
        KeyCode::NumpadAdd => "NumpadAdd",
        KeyCode::NumpadSubtract => "NumpadSubtract",
        KeyCode::NumpadMultiply => "NumpadMultiply",
        KeyCode::NumpadDivide => "NumpadDivide",
        KeyCode::NumpadDecimal => "NumpadDecimal",
        KeyCode::NumpadEnter => "NumpadEnter",
        KeyCode::NumpadEqual => "NumpadEqual",
        _ => return None,
    })
}

const LETTER_CODES: [KeyCode; 26] = [
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

const DIGIT_CODES: [KeyCode; 10] = [
    KeyCode::Digit0,
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
];

// ---- Config representation (preserves JSON object insertion order) ----

/// A single user override value inside `config.json`'s `"keybindings"` object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeybindingValue {
    /// Explicitly unbound (`null`, `""`, or `[]`).
    Unbind,
    /// One key chord string (e.g. `"Alt+Enter"`).
    Single(String),
    /// Multiple key chord strings (e.g. `["Ctrl+Tab", "Ctrl+PageDown"]`).
    Multiple(Vec<String>),
}

/// User keybinding overrides from `config.json`, preserving JSON key order so that when two
/// commands share a chord, the later entry in the user's map wins deterministically.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeybindingsConfig {
    entries: Vec<(String, KeybindingValue)>,
}

impl KeybindingsConfig {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[(String, KeybindingValue)] {
        &self.entries
    }

    pub fn get(&self, command: &str) -> Option<&KeybindingValue> {
        self.entries
            .iter()
            .rev()
            .find(|(k, _)| k == command)
            .map(|(_, v)| v)
    }

    pub fn contains(&self, command: &str) -> bool {
        self.entries.iter().any(|(k, _)| k == command)
    }

    /// Set or update an override for `command`, placing it at the end so the most recent edit
    /// wins any conflict.
    pub fn set(&mut self, command: impl Into<String>, value: KeybindingValue) {
        let cmd = command.into();
        self.entries.retain(|(k, _)| k != &cmd);
        self.entries.push((cmd, value));
    }

    /// Remove an override for `command` (returning it to its default). Returns `true` if removed.
    pub fn remove(&mut self, command: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|(k, _)| k != command);
        self.entries.len() != before
    }

    /// Clear all user overrides (reset all to defaults). Returns `true` if any existed.
    pub fn clear(&mut self) -> bool {
        let had = !self.entries.is_empty();
        self.entries.clear();
        had
    }
}

impl Serialize for KeybindingsConfig {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.entries.len()))?;
        for (cmd, value) in &self.entries {
            match value {
                KeybindingValue::Unbind => map.serialize_entry(cmd, &Option::<&str>::None)?,
                KeybindingValue::Single(s) => map.serialize_entry(cmd, s)?,
                KeybindingValue::Multiple(list) => map.serialize_entry(cmd, list)?,
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for KeybindingsConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_keybindings(deserializer)
    }
}

/// Serde deserializer for `TwittyConfig::keybindings`. Preserves JSON key order, never rejects
/// the whole `config.json` on bad values, and emits bilingual `config::warn` messages.
pub fn deserialize_keybindings<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<KeybindingsConfig, D::Error> {
    struct KeybindingsVisitor;

    impl<'de> Visitor<'de> for KeybindingsVisitor {
        type Value = KeybindingsConfig;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a JSON object mapping command ids to key chords")
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E> {
            Ok(KeybindingsConfig::default())
        }

        fn visit_none<E>(self) -> Result<Self::Value, E> {
            Ok(KeybindingsConfig::default())
        }

        fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
            let mut out = KeybindingsConfig::default();
            let mut count = 0usize;
            while let Some((raw_key, raw_val)) = access.next_entry::<String, Value>()? {
                count += 1;
                if count > MAX_KEYBINDING_ENTRIES {
                    if count == MAX_KEYBINDING_ENTRIES + 1 {
                        crate::config::warn(format!(
                            "twitty: keybindings has more than {MAX_KEYBINDING_ENTRIES} entries; ignoring the rest / يحتوي keybindings على أكثر من {MAX_KEYBINDING_ENTRIES} عنصر؛ تم تجاهل الباقي"
                        ));
                    }
                    continue;
                }
                let cmd = clean_text(&raw_key, MAX_CHORD_STR_CHARS);
                match parse_json_binding_value(&cmd, raw_val) {
                    Ok(val) => out.set(cmd, val),
                    Err(msg) => crate::config::warn(msg),
                }
            }
            Ok(out)
        }

        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            while seq.next_element::<Value>()?.is_some() {}
            crate::config::warn(
                "twitty: ignoring invalid keybindings (expected a JSON object) / تم تجاهل keybindings غير صالحة (المتوقع كائن JSON)"
                    .into(),
            );
            Ok(KeybindingsConfig::default())
        }

        fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
            crate::config::warn(
                "twitty: ignoring invalid keybindings (expected a JSON object) / تم تجاهل keybindings غير صالحة (المتوقع كائن JSON)"
                    .into(),
            );
            Ok(KeybindingsConfig::default())
        }

        fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
            crate::config::warn(
                "twitty: ignoring invalid keybindings (expected a JSON object) / تم تجاهل keybindings غير صالحة (المتوقع كائن JSON)"
                    .into(),
            );
            Ok(KeybindingsConfig::default())
        }

        fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
            crate::config::warn(
                "twitty: ignoring invalid keybindings (expected a JSON object) / تم تجاهل keybindings غير صالحة (المتوقع كائن JSON)"
                    .into(),
            );
            Ok(KeybindingsConfig::default())
        }

        fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
            crate::config::warn(
                "twitty: ignoring invalid keybindings (expected a JSON object) / تم تجاهل keybindings غير صالحة (المتوقع كائن JSON)"
                    .into(),
            );
            Ok(KeybindingsConfig::default())
        }

        fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
            crate::config::warn(
                "twitty: ignoring invalid keybindings (expected a JSON object) / تم تجاهل keybindings غير صالحة (المتوقع كائن JSON)"
                    .into(),
            );
            Ok(KeybindingsConfig::default())
        }
    }

    deserializer.deserialize_any(KeybindingsVisitor)
}

fn parse_json_binding_value(cmd: &str, value: Value) -> Result<KeybindingValue, String> {
    match value {
        Value::Null => Ok(KeybindingValue::Unbind),
        Value::String(s) => {
            let cleaned = clean_text(&s, MAX_CHORD_STR_CHARS);
            if cleaned.trim().is_empty() {
                Ok(KeybindingValue::Unbind)
            } else {
                Ok(KeybindingValue::Single(cleaned))
            }
        }
        Value::Array(items) => {
            if items.is_empty() {
                return Ok(KeybindingValue::Unbind);
            }
            let mut chords = Vec::with_capacity(items.len().min(MAX_CHORDS_PER_COMMAND));
            for item in items.into_iter().take(MAX_CHORDS_PER_COMMAND) {
                match item {
                    Value::String(s) => {
                        let cleaned = clean_text(&s, MAX_CHORD_STR_CHARS);
                        if !cleaned.trim().is_empty() {
                            chords.push(cleaned);
                        }
                    }
                    _ => {
                        crate::config::warn(format!(
                            "twitty: ignoring non-string chord in keybindings.{cmd} / تم تجاهل اختصار غير نصي في keybindings.{cmd}"
                        ));
                    }
                }
            }
            if chords.is_empty() {
                Err(format!(
                    "twitty: ignoring invalid keybindings.{cmd} (no valid chord strings) / تم تجاهل keybindings.{cmd} (لا توجد اختصارات نصية صالحة)"
                ))
            } else {
                Ok(KeybindingValue::Multiple(chords))
            }
        }
        _ => Err(format!(
            "twitty: ignoring invalid keybindings.{cmd} (expected string, array, or null) / تم تجاهل keybindings.{cmd} (المتوقع نص أو مصفوفة أو null)"
        )),
    }
}

// ---- Default bindings table ----

/// Internal default-only bindings for `Ctrl+Shift+1..9` (not in `commands::all()`).
const SELECT_TAB_IDS: [&str; 9] = [
    "tab.select-1",
    "tab.select-2",
    "tab.select-3",
    "tab.select-4",
    "tab.select-5",
    "tab.select-6",
    "tab.select-7",
    "tab.select-8",
    "tab.select-9",
];

/// Default chords for a built-in command id (from `commands::all()`).
pub fn default_chords_for(command_name: &str) -> Vec<Chord> {
    use KeyCode::*;
    let cs = |k| Chord::new(true, true, false, false, k);
    let c = |k| Chord::new(true, false, false, false, k);
    let a = |k| Chord::new(false, false, true, false, k);
    match command_name {
        "palette.open" => vec![cs(KeyP)],
        "composer.open" => vec![cs(Space)],
        "settings.open" => vec![c(Comma)],
        "tab.new" => vec![cs(KeyT)],
        "tab.next" => vec![c(Tab), c(PageDown)],
        "tab.prev" => vec![cs(Tab), c(PageUp)],
        "tab.move-left" => vec![cs(PageUp)],
        "tab.move-right" => vec![cs(PageDown)],
        "tab.reopen" => vec![Chord::new(true, true, true, false, KeyT)],
        "tab.rename" => vec![cs(KeyN)],
        "project.open" => vec![cs(KeyL)],
        "pane.zoom" => vec![cs(KeyZ)],
        "pane.flash" => vec![cs(KeyH)],
        "pane.close-others" => vec![cs(KeyK)],
        "pane.close" => vec![cs(KeyW)],
        "split.right" => vec![cs(KeyE)],
        "split.down" => vec![cs(KeyO)],
        "focus.left" => vec![a(ArrowLeft)],
        "focus.right" => vec![a(ArrowRight)],
        "focus.up" => vec![a(ArrowUp)],
        "focus.down" => vec![a(ArrowDown)],
        "sidebar.toggle" => vec![cs(KeyB)],
        "attention.next" => vec![cs(KeyU)],
        "notifications.list" => vec![cs(KeyI)],
        "notifications.jump-unread" => vec![cs(KeyJ)],
        "block.prev" => vec![cs(ArrowUp)],
        "block.next" => vec![cs(ArrowDown)],
        "block.copy-output" => vec![cs(KeyY)],
        "agent.ask" => vec![cs(KeyA)],
        "review.toggle" => vec![cs(KeyR)],
        "edit.copy" => vec![cs(KeyC)],
        "edit.paste" => vec![cs(KeyV)],
        "search.open" => vec![cs(KeyF)],
        "search.all" => vec![cs(KeyG)],
        "zoom.in" => vec![c(Equal), c(NumpadAdd)],
        "zoom.out" => vec![c(Minus), c(NumpadSubtract)],
        "zoom.reset" => vec![c(Digit0), c(Numpad0)],
        _ => Vec::new(),
    }
}

/// Look up the English and Arabic display title for any binding owner id (including internal
/// `tab.select-1..9`).
pub fn command_titles(id: &str) -> (&'static str, &'static str) {
    if let Some(cmd) = commands::all().into_iter().find(|c| c.name == id) {
        return (cmd.title_en, cmd.title_ar);
    }
    match id {
        "tab.select-1" => ("Switch to tab 1", "التاب 1"),
        "tab.select-2" => ("Switch to tab 2", "التاب 2"),
        "tab.select-3" => ("Switch to tab 3", "التاب 3"),
        "tab.select-4" => ("Switch to tab 4", "التاب 4"),
        "tab.select-5" => ("Switch to tab 5", "التاب 5"),
        "tab.select-6" => ("Switch to tab 6", "التاب 6"),
        "tab.select-7" => ("Switch to tab 7", "التاب 7"),
        "tab.select-8" => ("Switch to tab 8", "التاب 8"),
        "tab.select-9" => ("Switch to tab 9", "التاب 9"),
        _ => ("Unknown command", "أمر غير معروف"),
    }
}

// ---- Conflict & resolution ----

/// A chord claimed by more than one command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeybindingConflict {
    pub chord: Chord,
    /// Stable id of the command that won the chord.
    pub winner: &'static str,
    /// Stable ids of commands whose claim on `chord` was shadowed by `winner`.
    pub shadowed: Vec<&'static str>,
}

/// Inline conflict detail for one command row in the Settings -> Keys UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandConflictInfo {
    pub chord: Chord,
    /// True if this command is the active winner; false if it was shadowed by `other`.
    pub is_winner: bool,
    /// The other command id involved in the conflict.
    pub other: &'static str,
}

/// Output of resolving user `KeybindingsConfig` on top of built-in defaults.
#[derive(Clone, Debug)]
pub struct ResolvedKeybindings {
    pub bindings: KeyBindings,
    /// Bilingual warnings (unknown ids, invalid chords, plain-typing guard, conflicts).
    pub warnings: Vec<String>,
}

/// Compiled runtime keybinding table.
#[derive(Clone, Debug)]
pub struct KeyBindings {
    /// Active chord -> action lookup table used on every key press.
    by_chord: HashMap<Chord, InputAction>,
    /// Active (winning) chords per command id, in display order.
    effective: HashMap<&'static str, Vec<Chord>>,
    /// Configured (desired) chords per command id before conflict shadowing (for Settings UI).
    configured: HashMap<&'static str, Vec<Chord>>,
    /// Commands that were explicitly modified by the user (rebound or unbound).
    customized: HashMap<&'static str, bool>,
    /// All detected chord conflicts.
    conflicts: Vec<KeybindingConflict>,
}

impl Default for KeyBindings {
    fn default() -> Self {
        Self::defaults().clone()
    }
}

impl KeyBindings {
    /// Shared built-in default keybinding table (zero user overrides).
    pub fn defaults() -> &'static Self {
        static DEFAULTS: OnceLock<KeyBindings> = OnceLock::new();
        DEFAULTS.get_or_init(|| Self::resolve(&KeybindingsConfig::default()).bindings)
    }

    /// Resolve user overrides on top of defaults, returning both the compiled table and any
    /// bilingual config warnings.
    pub fn resolve(config: &KeybindingsConfig) -> ResolvedKeybindings {
        let all_cmds = commands::all();
        let mut by_name: HashMap<&'static str, InputAction> =
            HashMap::with_capacity(all_cmds.len() + 9);
        let mut desired: Vec<(&'static str, InputAction, Vec<Chord>)> =
            Vec::with_capacity(all_cmds.len() + 9);

        for cmd in &all_cmds {
            if let Some(action) = commands::action(cmd.name) {
                by_name.insert(cmd.name, action.clone());
                desired.push((cmd.name, action, default_chords_for(cmd.name)));
            }
        }
        for (i, &id) in SELECT_TAB_IDS.iter().enumerate() {
            let chord = Chord::new(true, true, false, false, DIGIT_CODES[i + 1]);
            desired.push((id, InputAction::SelectTab(i), vec![chord]));
        }

        let mut warnings = Vec::new();
        let mut user_order: HashMap<&'static str, usize> = HashMap::new();
        let mut customized: HashMap<&'static str, bool> = HashMap::new();

        for (entry_idx, (raw_cmd, raw_val)) in config.entries().iter().enumerate() {
            let Some((&cmd_id, _)) = by_name.get_key_value(raw_cmd.as_str()) else {
                warnings.push(format!(
                    "twitty: unknown keybinding command \"{raw_cmd}\" / أمر اختصار غير معروف: \"{raw_cmd}\""
                ));
                continue;
            };

            match raw_val {
                KeybindingValue::Unbind => {
                    if let Some(slot) = desired.iter_mut().find(|(id, _, _)| *id == cmd_id) {
                        slot.2.clear();
                    }
                    user_order.insert(cmd_id, entry_idx);
                    customized.insert(cmd_id, true);
                }
                KeybindingValue::Single(s) => {
                    if let Some(chord) = parse_user_chord(cmd_id, s, &mut warnings) {
                        if let Some(slot) = desired.iter_mut().find(|(id, _, _)| *id == cmd_id) {
                            slot.2 = vec![chord];
                        }
                        user_order.insert(cmd_id, entry_idx);
                        customized.insert(cmd_id, true);
                    }
                }
                KeybindingValue::Multiple(list) => {
                    let mut parsed = Vec::new();
                    for s in list {
                        if let Some(chord) = parse_user_chord(cmd_id, s, &mut warnings) {
                            if !parsed.contains(&chord) {
                                parsed.push(chord);
                            }
                        }
                    }
                    if !parsed.is_empty() {
                        if let Some(slot) = desired.iter_mut().find(|(id, _, _)| *id == cmd_id) {
                            slot.2 = parsed;
                        }
                        user_order.insert(cmd_id, entry_idx);
                        customized.insert(cmd_id, true);
                    }
                }
            }
        }

        // Group every claim by chord (keeping deterministic insertion order).
        struct Claim {
            cmd: &'static str,
            action: InputAction,
            /// `(1, user_entry_idx)` for user bindings, `(0, default_idx)` for defaults.
            priority: (u8, usize),
        }

        let mut claim_order: Vec<Chord> = Vec::new();
        let mut claims_by_chord: HashMap<Chord, Vec<Claim>> = HashMap::new();
        let mut configured: HashMap<&'static str, Vec<Chord>> =
            HashMap::with_capacity(desired.len());

        for (default_idx, (cmd_id, action, chords)) in desired.into_iter().enumerate() {
            let priority = match user_order.get(cmd_id) {
                Some(&idx) => (1u8, idx),
                None => (0u8, default_idx),
            };
            configured.insert(cmd_id, chords.clone());
            for chord in chords {
                let list = claims_by_chord.entry(chord).or_insert_with(|| {
                    claim_order.push(chord);
                    Vec::new()
                });
                list.push(Claim {
                    cmd: cmd_id,
                    action: action.clone(),
                    priority,
                });
            }
        }

        let mut by_chord: HashMap<Chord, InputAction> = HashMap::with_capacity(claim_order.len());
        let mut effective: HashMap<&'static str, Vec<Chord>> = HashMap::new();
        let mut conflicts = Vec::new();

        for chord in claim_order {
            let claims = &claims_by_chord[&chord];
            let winner_idx = claims
                .iter()
                .enumerate()
                .max_by_key(|(_, c)| c.priority)
                .map(|(i, _)| i)
                .unwrap_or(0);
            let winner = &claims[winner_idx];
            by_chord.insert(chord, winner.action.clone());
            effective.entry(winner.cmd).or_default().push(chord);

            if claims.len() > 1 {
                let shadowed: Vec<&'static str> = claims
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != winner_idx)
                    .map(|(_, c)| c.cmd)
                    .collect();
                let shadowed_list = shadowed.join(", ");
                warnings.push(format!(
                    "twitty: shortcut conflict on {chord}: \"{}\" overrides \"{shadowed_list}\" / تعارض اختصار على {chord}: \"{}\" يلغي \"{shadowed_list}\"",
                    winner.cmd, winner.cmd
                ));
                conflicts.push(KeybindingConflict {
                    chord,
                    winner: winner.cmd,
                    shadowed,
                });
            }
        }

        // Preserve each command's configured chord ordering inside `effective`.
        for (cmd_id, eff_list) in &mut effective {
            if let Some(cfg_list) = configured.get(cmd_id) {
                eff_list
                    .sort_by_key(|ch| cfg_list.iter().position(|c| c == ch).unwrap_or(usize::MAX));
            }
        }

        ResolvedKeybindings {
            bindings: KeyBindings {
                by_chord,
                effective,
                configured,
                customized,
                conflicts,
            },
            warnings,
        }
    }

    /// Build compiled bindings from `config` without returning warnings.
    pub fn from_config(config: &KeybindingsConfig) -> Self {
        Self::resolve(config).bindings
    }

    /// Look up the action bound to `(key_code, modifiers)`, if any.
    pub fn lookup(&self, key_code: KeyCode, modifiers: ModifiersState) -> Option<InputAction> {
        let chord = Chord::from_modifiers(modifiers, key_code);
        self.by_chord.get(&chord).cloned()
    }

    /// Active (winning) chords bound to `command_name`.
    pub fn effective_chords(&self, command_name: &str) -> &[Chord] {
        self.effective
            .get(command_name)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Configured chords for `command_name` (including any shadowed by a conflict).
    pub fn configured_chords(&self, command_name: &str) -> &[Chord] {
        self.configured
            .get(command_name)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Primary effective shortcut string shown in the Command Palette / control API.
    pub fn display_shortcut(&self, command_name: &str) -> Option<String> {
        self.effective_chords(command_name)
            .first()
            .map(|c| c.canonical_string())
    }

    /// Primary configured shortcut string shown in the Settings -> Keys screen (shows even a
    /// shadowed chord so the user can see what conflicted).
    pub fn settings_display_shortcut(&self, command_name: &str) -> Option<String> {
        self.configured_chords(command_name)
            .first()
            .map(|c| c.canonical_string())
    }

    /// True if `command_name` was explicitly modified in the user's `keybindings` config.
    pub fn is_customized(&self, command_name: &str) -> bool {
        self.customized.get(command_name).copied().unwrap_or(false)
    }

    /// All conflicts detected while resolving this table.
    pub fn conflicts(&self) -> &[KeybindingConflict] {
        &self.conflicts
    }

    /// First conflict involving `command_name` (either as winner or as shadowed), for inline UI.
    pub fn conflict_for(&self, command_name: &str) -> Option<CommandConflictInfo> {
        for conflict in &self.conflicts {
            if conflict.winner == command_name {
                if let Some(&other) = conflict.shadowed.first() {
                    return Some(CommandConflictInfo {
                        chord: conflict.chord,
                        is_winner: true,
                        other,
                    });
                }
            } else if conflict.shadowed.contains(&command_name) {
                return Some(CommandConflictInfo {
                    chord: conflict.chord,
                    is_winner: false,
                    other: conflict.winner,
                });
            }
        }
        None
    }

    /// Every active `(Chord, InputAction)` pair in the compiled table.
    pub fn len(&self) -> usize {
        self.by_chord.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_chord.is_empty()
    }

    /// Render a markdown shortcut table of all built-in commands with their effective chords.
    pub fn markdown_shortcut_table(&self) -> String {
        let mut out = String::from("| Command | Action | Keys |\n| --- | --- | --- |\n");
        for cmd in commands::all() {
            let keys = self
                .effective_chords(cmd.name)
                .iter()
                .map(|c| c.canonical_string())
                .collect::<Vec<_>>()
                .join(", ");
            let keys_cell = if keys.is_empty() { "—" } else { &keys };
            out.push_str(&format!(
                "| `{}` | {} | {} |\n",
                cmd.name, cmd.title_en, keys_cell
            ));
        }
        out
    }
}

fn parse_user_chord(cmd_id: &str, raw: &str, warnings: &mut Vec<String>) -> Option<Chord> {
    match Chord::parse(raw) {
        Err(err) => {
            warnings.push(format!(
                "twitty: invalid shortcut \"{raw}\" for \"{cmd_id}\" ({}) / اختصار غير صالح \"{raw}\" للأمر \"{cmd_id}\" ({})",
                err.message_en(),
                err.message_ar()
            ));
            None
        }
        Ok(chord) if chord.swallows_plain_typing() => {
            warnings.push(format!(
                "twitty: ignoring shortcut \"{chord}\" for \"{cmd_id}\" (would swallow plain typing) / تم تجاهل الاختصار \"{chord}\" للأمر \"{cmd_id}\" (سيمنع الكتابة العادية)"
            ));
            None
        }
        Ok(chord) => Some(chord),
    }
}

/// Convenience helper for applying a newly captured chord to `config.keybindings`.
/// If `chord` is the command's exact single default AND no other user entry claims `chord`,
/// removes the redundant override; otherwise stores it at the end of `config.keybindings`.
pub fn set_captured_chord(config: &mut KeybindingsConfig, command: &str, chord: Chord) {
    let defaults = default_chords_for(command);
    let chord_claimed_by_other = config.entries().iter().any(|(other, val)| {
        other != command
            && match val {
                KeybindingValue::Unbind => false,
                KeybindingValue::Single(s) => Chord::parse(s).ok() == Some(chord),
                KeybindingValue::Multiple(list) => {
                    list.iter().any(|s| Chord::parse(s).ok() == Some(chord))
                }
            }
    });
    if defaults.len() == 1 && defaults[0] == chord && !chord_claimed_by_other {
        config.remove(command);
    } else {
        config.set(command, KeybindingValue::Single(chord.canonical_string()));
    }
}
