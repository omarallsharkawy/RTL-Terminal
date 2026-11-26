//! Bidi helpers for shaping terminal rows: direction marks and right-to-left detection.

use unicode_bidi::{bidi_class, BidiClass};

use crate::terminal::CellData;

/// Left-to-right mark prepended to RTL segments so prompts never flip direction.
#[doc(hidden)]
pub const LRM: &str = "\u{200E}";
/// Right-to-left mark appended after spaces typed at the end of Arabic, so they stay on its left.
pub(crate) const RLM: &str = "\u{200F}";
/// Left-to-right isolate / pop directional isolate: wrap Latin words and numbers inside Arabic so
/// hashes like `7abaf40` are never torn into `7` … `abaf40`.
pub(crate) const LRI: &str = "\u{2066}";
pub(crate) const PDI: &str = "\u{2069}";
/// Right-to-left isolate: wraps an Arabic phrase that follows a left-to-right lead (a prompt).
pub(crate) const RLI: &str = "\u{2067}";

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

/// Where the Arabic phrase of a segment starts (the first word holding an Arabic letter), when
/// that phrase reads right to left. Everything before it (a prompt, `echo `) stays left to right
/// on the left; inside the phrase Latin words sit to the left of the Arabic that precedes them,
/// as in any Arabic text.
///
/// The line being typed (`typing`) is always right to left from its first Arabic word, so
/// English typed after an Arabic word lands on its left. In finished output the phrase is right
/// to left when its Arabic words outnumber its Latin ones, or match them when nothing with
/// words comes before (the line starts with Arabic, or a `❯ ` prompt): `Error: ملف not found`,
/// `ok تحذير done` and `"ملف": No such file or directory` keep reading left to right.
pub(crate) fn arabic_phrase(cells: &[CellData], typing: bool) -> Option<usize> {
    let first = cells.iter().position(|c| is_rtl_char(c.c))?;
    let start = cells[..first]
        .iter()
        .rposition(CellData::is_word_break)
        .map_or(0, |b| b + 1);
    // While typing, Arabic starts a right-to-left phrase at once.
    if typing {
        return Some(start);
    }
    let (mut arabic, mut latin) = (0, 0);
    for word in cells[start..].split(CellData::is_word_break) {
        if word.iter().any(|c| is_rtl_char(c.c)) {
            arabic += 1;
        } else if word.iter().any(|c| c.c.is_alphabetic()) {
            latin += 1;
        }
    }
    // A finished line that starts with Arabic still reads left to right when most of its
    // words are Latin: `"ملف": No such file or directory` is an English message quoting an
    // Arabic name.
    let lead_has_words = cells[..start].iter().any(|c| c.c.is_alphabetic());
    (arabic > latin || (arabic == latin && !lead_has_words)).then_some(start)
}
