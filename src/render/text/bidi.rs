//! Bidi helpers for shaping terminal rows: direction marks and right-to-left detection.

use unicode_bidi::{bidi_class, BidiClass};

use crate::terminal::CellData;

/// Left-to-right mark prepended to RTL segments so prompts never flip direction.
pub(crate) const LRM: &str = "\u{200E}";
/// Right-to-left mark appended after spaces typed at the end of Arabic, so they stay on its left.
pub(crate) const RLM: &str = "\u{200F}";
/// Left-to-right isolate / pop directional isolate: wrap Latin words and numbers inside Arabic so
/// hashes like `7abaf40` are never torn into `7` … `abaf40`.
pub(crate) const LRI: &str = "\u{2066}";
pub(crate) const PDI: &str = "\u{2069}";

/// Strongly right-to-left (Arabic, Hebrew…).
pub(crate) fn is_rtl_char(c: char) -> bool {
    matches!(bidi_class(c), BidiClass::R | BidiClass::AL)
}

/// True when the last strong character is Arabic/Hebrew and only neutrals (spaces, punctuation,
/// symbols) follow it. Digits are excluded: numbers are isolated and keep their own side.
pub(crate) fn trails_rtl_neutrals(cells: &[CellData]) -> bool {
    let mut trailing = 0;
    for cell in cells.iter().rev() {
        match bidi_class(cell.c) {
            BidiClass::R | BidiClass::AL => return trailing > 0,
            BidiClass::L | BidiClass::EN | BidiClass::AN => return false,
            _ => trailing += 1,
        }
    }
    false
}
