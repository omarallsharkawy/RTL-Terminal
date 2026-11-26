//! Multi-line bidirectional text editor for the command composer.
//!
//! Logical storage is a UTF-8 `String` with byte offsets on grapheme boundaries. Left/Right and
//! Ctrl+Left/Right move logically in the visual direction of the bidi run the caret sits in
//! (using the same RTL phrase / LRI-isolate rules as `render::text::shaper`).

use unicode_bidi::{bidi_class, BidiClass};
use unicode_segmentation::UnicodeSegmentation;

/// Maximum editor buffer size in bytes (64 KiB).
pub const MAX_COMPOSER_BYTES: usize = 64 * 1024;
/// Maximum undo history entries kept in memory.
pub const MAX_UNDO_STEPS: usize = 200;

/// Active IME preedit composition inside the composer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preedit {
    pub text: String,
    pub cursor: Option<(usize, usize)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    text: String,
    caret: usize,
    anchor: Option<usize>,
}

/// Multi-line text editor with selection, bidi-aware caret movement, undo/redo, and inline IME
/// preedit.
#[derive(Clone, Debug, Default)]
pub struct TextEditor {
    text: String,
    /// Logical byte offset of the caret (always on a UTF-8 and grapheme boundary).
    caret: usize,
    /// Selection anchor byte offset, when a selection is active.
    anchor: Option<usize>,
    /// Target grapheme column preserved across consecutive Up/Down moves.
    preferred_col: Option<usize>,
    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    preedit: Option<Preedit>,
}

impl TextEditor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_text(text: impl Into<String>) -> Self {
        let mut ed = Self::new();
        ed.set_text_without_undo(text.into());
        ed
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.preedit.is_none()
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    pub fn anchor(&self) -> Option<usize> {
        self.anchor
    }

    pub fn preedit(&self) -> Option<&Preedit> {
        self.preedit.as_ref()
    }

    pub fn set_preedit(&mut self, preedit: Option<(String, Option<(usize, usize)>)>) {
        self.preedit = preedit.and_then(|(text, cursor)| {
            if text.is_empty() {
                None
            } else {
                Some(Preedit { text, cursor })
            }
        });
    }

    /// Ordered `(start, end)` byte range of the active selection, if non-empty.
    pub fn selection_range(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        if anchor == self.caret {
            None
        } else if anchor < self.caret {
            Some((anchor, self.caret))
        } else {
            Some((self.caret, anchor))
        }
    }

    pub fn selected_text(&self) -> Option<&str> {
        let (start, end) = self.selection_range()?;
        self.text.get(start..end)
    }

    pub fn clear(&mut self) {
        if !self.text.is_empty() || self.caret != 0 || self.anchor.is_some() {
            self.push_undo();
            self.text.clear();
            self.caret = 0;
            self.anchor = None;
            self.preferred_col = None;
            self.preedit = None;
        }
    }

    /// Replace the entire buffer (e.g. when browsing history), placing the caret at the end.
    pub fn set_text(&mut self, text: String) {
        let cleaned = sanitize_insert(&text);
        if cleaned == self.text && self.caret == self.text.len() && self.anchor.is_none() {
            return;
        }
        self.push_undo();
        self.text = truncate_to_limit(cleaned, MAX_COMPOSER_BYTES);
        self.caret = self.text.len();
        self.anchor = None;
        self.preferred_col = None;
    }

    /// Replace buffer without recording undo (used during initialization or history browsing).
    pub fn set_text_without_undo(&mut self, text: String) {
        let cleaned = truncate_to_limit(sanitize_insert(&text), MAX_COMPOSER_BYTES);
        self.text = cleaned;
        self.caret = self.text.len();
        self.anchor = None;
        self.preferred_col = None;
    }

    /// Set the caret to a specific byte offset (clamped to nearest valid grapheme boundary).
    pub fn set_caret(&mut self, byte: usize, extend: bool) {
        let clamped = clamp_grapheme_boundary(&self.text, byte);
        self.move_to(clamped, extend);
        self.preferred_col = None;
    }

    /// Logical lines of the buffer (always at least one, even when empty).
    pub fn lines(&self) -> Vec<&str> {
        self.text.split('\n').collect()
    }

    pub fn line_count(&self) -> usize {
        self.text.bytes().filter(|&b| b == b'\n').count() + 1
    }

    /// Byte range `(start, end)` of `line_idx` (excluding its `\n`), if in range.
    pub fn line_byte_range(&self, line_idx: usize) -> Option<(usize, usize)> {
        let mut start = 0;
        for (idx, line) in self.text.split('\n').enumerate() {
            let end = start + line.len();
            if idx == line_idx {
                return Some((start, end));
            }
            start = end + 1;
        }
        None
    }

    /// `(line_idx, byte_offset_within_line)` for any byte offset in `self.text`.
    pub fn byte_to_line_col(&self, byte: usize) -> (usize, usize) {
        let byte = byte.min(self.text.len());
        let mut line_idx = 0;
        let mut line_start = 0;
        for (i, b) in self.text.bytes().enumerate() {
            if i == byte {
                return (line_idx, byte - line_start);
            }
            if b == b'\n' {
                line_idx += 1;
                line_start = i + 1;
            }
        }
        (line_idx, byte - line_start)
    }

    /// `(line_idx, byte_offset_within_line)` of the caret.
    pub fn caret_line_col(&self) -> (usize, usize) {
        self.byte_to_line_col(self.caret)
    }

    /// Selection as `((start_line, start_byte), (end_line, end_byte))`, if any.
    pub fn selection_line_cols(&self) -> Option<((usize, usize), (usize, usize))> {
        let (s, e) = self.selection_range()?;
        Some((self.byte_to_line_col(s), self.byte_to_line_col(e)))
    }

    pub fn is_on_first_line(&self) -> bool {
        !self.text[..self.caret].contains('\n')
    }

    pub fn is_on_last_line(&self) -> bool {
        !self.text[self.caret..].contains('\n')
    }

    pub fn is_at_end_of_line(&self) -> bool {
        let (line_idx, col) = self.caret_line_col();
        self.line_byte_range(line_idx)
            .is_some_and(|(s, e)| col == e - s)
    }

    pub fn is_at_end_of_buffer(&self) -> bool {
        self.caret == self.text.len()
    }

    /// True when the caret currently sits in a right-to-left bidi run on its line.
    pub fn caret_is_rtl(&self) -> bool {
        let (line_idx, col) = self.caret_line_col();
        let Some((s, e)) = self.line_byte_range(line_idx) else {
            return false;
        };
        caret_is_rtl(&self.text[s..e], col)
    }

    /// Insert text at the caret (replacing any active selection).
    pub fn insert_text(&mut self, input: &str) -> bool {
        let clean = sanitize_insert(input);
        if clean.is_empty() && self.selection_range().is_none() {
            return false;
        }
        self.push_undo();
        self.delete_selection_inner();
        let room = MAX_COMPOSER_BYTES.saturating_sub(self.text.len());
        let to_insert = truncate_to_limit(clean, room);
        if to_insert.is_empty() {
            return false;
        }
        self.text.insert_str(self.caret, &to_insert);
        self.caret += to_insert.len();
        self.anchor = None;
        self.preferred_col = None;
        true
    }

    /// Insert a newline at the caret (Shift+Enter).
    pub fn insert_newline(&mut self) -> bool {
        self.insert_text("\n")
    }

    /// Replace byte range `start..end` with `replacement`, leaving the caret at the end of the
    /// inserted text.
    pub fn replace_range(&mut self, start: usize, end: usize, replacement: &str) -> bool {
        let start = clamp_grapheme_boundary(&self.text, start);
        let end = clamp_grapheme_boundary(&self.text, end).max(start);
        let clean = sanitize_insert(replacement);
        if self.text.get(start..end) == Some(clean.as_str()) && self.caret == end {
            return false;
        }
        self.push_undo();
        let removed = end - start;
        let room = MAX_COMPOSER_BYTES.saturating_sub(self.text.len() - removed);
        let to_insert = truncate_to_limit(clean, room);
        self.text.replace_range(start..end, &to_insert);
        self.caret = start + to_insert.len();
        self.anchor = None;
        self.preferred_col = None;
        true
    }

    pub fn backspace(&mut self) -> bool {
        if self.selection_range().is_some() {
            self.push_undo();
            self.delete_selection_inner();
            self.preferred_col = None;
            return true;
        }
        let prev = prev_grapheme(&self.text, self.caret);
        if prev == self.caret {
            return false;
        }
        self.push_undo();
        self.text.replace_range(prev..self.caret, "");
        self.caret = prev;
        self.preferred_col = None;
        true
    }

    pub fn delete_forward(&mut self) -> bool {
        if self.selection_range().is_some() {
            self.push_undo();
            self.delete_selection_inner();
            self.preferred_col = None;
            return true;
        }
        let next = next_grapheme(&self.text, self.caret);
        if next == self.caret {
            return false;
        }
        self.push_undo();
        self.text.replace_range(self.caret..next, "");
        self.preferred_col = None;
        true
    }

    pub fn delete_word_backward(&mut self) -> bool {
        if self.selection_range().is_some() {
            self.push_undo();
            self.delete_selection_inner();
            self.preferred_col = None;
            return true;
        }
        let prev = prev_word_boundary(&self.text, self.caret);
        if prev == self.caret {
            return false;
        }
        self.push_undo();
        self.text.replace_range(prev..self.caret, "");
        self.caret = prev;
        self.preferred_col = None;
        true
    }

    pub fn delete_word_forward(&mut self) -> bool {
        if self.selection_range().is_some() {
            self.push_undo();
            self.delete_selection_inner();
            self.preferred_col = None;
            return true;
        }
        let next = next_word_boundary(&self.text, self.caret);
        if next == self.caret {
            return false;
        }
        self.push_undo();
        self.text.replace_range(self.caret..next, "");
        self.preferred_col = None;
        true
    }

    /// Move logically backward (toward index 0) by one grapheme.
    pub fn move_logical_backward(&mut self, extend: bool) {
        if !extend {
            if let Some((start, _)) = self.selection_range() {
                self.caret = start;
                self.anchor = None;
                self.preferred_col = None;
                return;
            }
        }
        let target = prev_grapheme(&self.text, self.caret);
        self.move_to(target, extend);
        self.preferred_col = None;
    }

    /// Move logically forward (toward end of buffer) by one grapheme.
    pub fn move_logical_forward(&mut self, extend: bool) {
        if !extend {
            if let Some((_, end)) = self.selection_range() {
                self.caret = end;
                self.anchor = None;
                self.preferred_col = None;
                return;
            }
        }
        let target = next_grapheme(&self.text, self.caret);
        self.move_to(target, extend);
        self.preferred_col = None;
    }

    /// Move Left in the visual direction of the text the caret sits in:
    /// in RTL text this advances logically forward; in LTR text it moves logically backward.
    pub fn move_visual_left(&mut self, extend: bool) {
        if self.caret_is_rtl() {
            self.move_logical_forward(extend);
        } else {
            self.move_logical_backward(extend);
        }
    }

    /// Move Right in the visual direction of the text the caret sits in:
    /// in RTL text this moves logically backward; in LTR text it advances logically forward.
    pub fn move_visual_right(&mut self, extend: bool) {
        if self.caret_is_rtl() {
            self.move_logical_backward(extend);
        } else {
            self.move_logical_forward(extend);
        }
    }

    /// Move by one word logically backward.
    pub fn move_word_logical_backward(&mut self, extend: bool) {
        let target = prev_word_boundary(&self.text, self.caret);
        self.move_to(target, extend);
        self.preferred_col = None;
    }

    /// Move by one word logically forward.
    pub fn move_word_logical_forward(&mut self, extend: bool) {
        let target = next_word_boundary(&self.text, self.caret);
        self.move_to(target, extend);
        self.preferred_col = None;
    }

    /// Move by one word visually left (logical forward in RTL, logical backward in LTR).
    pub fn move_word_visual_left(&mut self, extend: bool) {
        if self.caret_is_rtl() {
            self.move_word_logical_forward(extend);
        } else {
            self.move_word_logical_backward(extend);
        }
    }

    /// Move by one word visually right (logical backward in RTL, logical forward in LTR).
    pub fn move_word_visual_right(&mut self, extend: bool) {
        if self.caret_is_rtl() {
            self.move_word_logical_backward(extend);
        } else {
            self.move_word_logical_forward(extend);
        }
    }

    /// Move to start of current line (or start of buffer when `document` is true).
    pub fn move_home(&mut self, document: bool, extend: bool) {
        let target = if document {
            0
        } else {
            let (line_idx, _) = self.caret_line_col();
            self.line_byte_range(line_idx).map_or(0, |(s, _)| s)
        };
        self.move_to(target, extend);
        self.preferred_col = None;
    }

    /// Move to end of current line (or end of buffer when `document` is true).
    pub fn move_end(&mut self, document: bool, extend: bool) {
        let target = if document {
            self.text.len()
        } else {
            let (line_idx, _) = self.caret_line_col();
            self.line_byte_range(line_idx)
                .map_or(self.text.len(), |(_, e)| e)
        };
        self.move_to(target, extend);
        self.preferred_col = None;
    }

    /// Move caret one line up. Returns `false` if already on the first line (so the caller can
    /// browse older history).
    pub fn move_up(&mut self, extend: bool) -> bool {
        let (line_idx, col_byte) = self.caret_line_col();
        if line_idx == 0 {
            return false;
        }
        let Some((cur_start, _)) = self.line_byte_range(line_idx) else {
            return false;
        };
        let grapheme_col = self.preferred_col.unwrap_or_else(|| {
            self.text[cur_start..cur_start + col_byte]
                .graphemes(true)
                .count()
        });
        let Some((prev_start, prev_end)) = self.line_byte_range(line_idx - 1) else {
            return false;
        };
        let target = prev_start + nth_grapheme_byte(&self.text[prev_start..prev_end], grapheme_col);
        self.move_to(target, extend);
        self.preferred_col = Some(grapheme_col);
        true
    }

    /// Move caret one line down. Returns `false` if already on the last line (so the caller can
    /// browse newer history).
    pub fn move_down(&mut self, extend: bool) -> bool {
        let (line_idx, col_byte) = self.caret_line_col();
        if line_idx + 1 >= self.line_count() {
            return false;
        }
        let Some((cur_start, _)) = self.line_byte_range(line_idx) else {
            return false;
        };
        let grapheme_col = self.preferred_col.unwrap_or_else(|| {
            self.text[cur_start..cur_start + col_byte]
                .graphemes(true)
                .count()
        });
        let Some((next_start, next_end)) = self.line_byte_range(line_idx + 1) else {
            return false;
        };
        let target = next_start + nth_grapheme_byte(&self.text[next_start..next_end], grapheme_col);
        self.move_to(target, extend);
        self.preferred_col = Some(grapheme_col);
        true
    }

    pub fn select_all(&mut self) {
        if self.text.is_empty() {
            self.anchor = None;
            self.caret = 0;
        } else {
            self.anchor = Some(0);
            self.caret = self.text.len();
        }
        self.preferred_col = None;
    }

    pub fn copy(&self) -> Option<String> {
        self.selected_text().map(str::to_owned)
    }

    pub fn cut(&mut self) -> Option<String> {
        let text = self.copy()?;
        self.push_undo();
        self.delete_selection_inner();
        self.preferred_col = None;
        Some(text)
    }

    pub fn paste(&mut self, text: &str) -> bool {
        self.insert_text(text)
    }

    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo_stack.pop() else {
            return false;
        };
        self.redo_stack.push(self.snapshot());
        if self.redo_stack.len() > MAX_UNDO_STEPS {
            self.redo_stack.remove(0);
        }
        self.restore(prev);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo_stack.pop() else {
            return false;
        };
        self.undo_stack.push(self.snapshot());
        if self.undo_stack.len() > MAX_UNDO_STEPS {
            self.undo_stack.remove(0);
        }
        self.restore(next);
        true
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            caret: self.caret,
            anchor: self.anchor,
        }
    }

    fn restore(&mut self, snap: Snapshot) {
        self.text = snap.text;
        self.caret = snap.caret.min(self.text.len());
        self.anchor = snap.anchor.map(|a| a.min(self.text.len()));
        self.preferred_col = None;
    }

    fn push_undo(&mut self) {
        let snap = self.snapshot();
        if self.undo_stack.last() != Some(&snap) {
            if self.undo_stack.len() >= MAX_UNDO_STEPS {
                self.undo_stack.remove(0);
            }
            self.undo_stack.push(snap);
        }
        self.redo_stack.clear();
    }

    fn delete_selection_inner(&mut self) {
        if let Some((start, end)) = self.selection_range() {
            self.text.replace_range(start..end, "");
            self.caret = start;
            self.anchor = None;
        }
    }

    fn move_to(&mut self, target: usize, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.caret);
            }
            self.caret = target;
            if self.anchor == Some(self.caret) {
                self.anchor = None;
            }
        } else {
            self.caret = target;
            self.anchor = None;
        }
    }
}

/// Strongly right-to-left character (Arabic, Hebrew…), matching `render::text::bidi::is_rtl_char`.
pub fn is_rtl_char(c: char) -> bool {
    matches!(bidi_class(c), BidiClass::R | BidiClass::AL)
}

/// Compute per-character RTL flags for one line using the exact same typing-mode bidi rules as
/// `RowShaper` (`src/render/text/shaper.rs` + `src/render/text/bidi.rs`):
/// - No RTL char on the line => all LTR.
/// - Otherwise the RTL phrase starts at the first whitespace-delimited word containing an RTL char.
/// - Inside that phrase, contiguous ASCII-alphanumeric words with no RTL char (and spaces between
///   neighbouring such words) are LRI-isolated (LTR); everything else is RTL.
pub fn line_char_rtl_flags(line: &str) -> Vec<(usize, bool)> {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let n = chars.len();
    if n == 0 {
        return Vec::new();
    }
    let Some(first_rtl) = chars.iter().position(|&(_, c)| is_rtl_char(c)) else {
        return chars.into_iter().map(|(b, _)| (b, false)).collect();
    };
    let phrase_start = chars[..first_rtl]
        .iter()
        .rposition(|&(_, c)| c.is_whitespace())
        .map_or(0, |b| b + 1);

    let mut is_rtl = vec![false; n];
    for flag in &mut is_rtl[phrase_start..] {
        *flag = true;
    }

    // Mark LRI-isolated runs inside the RTL phrase (contiguous words with ASCII alphanumeric and
    // no RTL char, plus spaces between neighbouring such words).
    let mut open: Option<usize> = None;
    let mut last_end = phrase_start;
    let mut i = phrase_start;
    while i < n {
        if chars[i].1.is_whitespace() {
            i += 1;
            continue;
        }
        let word_start = i;
        while i < n && !chars[i].1.is_whitespace() {
            i += 1;
        }
        let word = &chars[word_start..i];
        if word.iter().any(|&(_, c)| c.is_ascii_alphanumeric())
            && !word.iter().any(|&(_, c)| is_rtl_char(c))
        {
            open.get_or_insert(word_start);
            last_end = i;
        } else if let Some(o) = open.take() {
            for flag in &mut is_rtl[o..last_end] {
                *flag = false;
            }
        }
    }
    if let Some(o) = open {
        for flag in &mut is_rtl[o..last_end] {
            *flag = false;
        }
    }

    chars
        .into_iter()
        .zip(is_rtl)
        .map(|((byte, _), rtl)| (byte, rtl))
        .collect()
}

/// Whether the caret at `col_byte` inside `line` sits in a right-to-left run.
///
/// Inside a line, uses the direction of the character at the caret (matching the glyph the caret
/// is visually anchored to); at the end of a line, uses the direction of the character immediately
/// before the caret.
pub fn caret_is_rtl(line: &str, col_byte: usize) -> bool {
    let flags = line_char_rtl_flags(line);
    if flags.is_empty() {
        return false;
    }
    if col_byte < line.len() {
        let idx = flags.partition_point(|&(b, _)| b < col_byte);
        flags
            .get(idx)
            .or_else(|| flags.last())
            .is_some_and(|&(_, rtl)| rtl)
    } else {
        flags.last().is_some_and(|&(_, rtl)| rtl)
    }
}

fn sanitize_insert(input: &str) -> String {
    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    normalized
        .chars()
        .filter(|&c| {
            (c == '\n' || c == '\t' || !c.is_control())
                && !crate::terminal::is_bidi_control(c)
                && !matches!(c, '\u{2028}' | '\u{2029}')
        })
        .collect()
}

fn truncate_to_limit(mut text: String, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text;
    }
    let end = text
        .grapheme_indices(true)
        .map(|(i, _)| i)
        .take_while(|&i| i <= max_bytes)
        .last()
        .unwrap_or(0);
    text.truncate(end);
    text
}

fn clamp_grapheme_boundary(text: &str, byte: usize) -> usize {
    if byte >= text.len() {
        return text.len();
    }
    text.grapheme_indices(true)
        .map(|(i, _)| i)
        .take_while(|&i| i <= byte)
        .last()
        .unwrap_or(0)
}

fn prev_grapheme(text: &str, byte: usize) -> usize {
    let byte = byte.min(text.len());
    text[..byte]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(i, _)| i)
}

fn next_grapheme(text: &str, byte: usize) -> usize {
    let byte = byte.min(text.len());
    text[byte..]
        .graphemes(true)
        .next()
        .map_or(text.len(), |g| byte + g.len())
}

fn nth_grapheme_byte(line: &str, n: usize) -> usize {
    line.grapheme_indices(true)
        .nth(n)
        .map_or(line.len(), |(i, _)| i)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Space,
    Word,
    Punct,
}

fn char_class(g: &str) -> CharClass {
    if g.chars().all(char::is_whitespace) {
        CharClass::Space
    } else if g.chars().any(|c| c.is_alphanumeric() || c == '_') {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

fn prev_word_boundary(text: &str, byte: usize) -> usize {
    let byte = byte.min(text.len());
    let mut items: Vec<(usize, &str)> = text[..byte].grapheme_indices(true).collect();
    // Skip trailing whitespace (and stop at newline boundary if crossing into another line).
    while let Some(&(_, g)) = items.last() {
        if char_class(g) == CharClass::Space {
            items.pop();
        } else {
            break;
        }
    }
    let Some(&(mut pos, last_g)) = items.last() else {
        return 0;
    };
    let cls = char_class(last_g);
    while let Some(&(i, g)) = items.last() {
        if char_class(g) == cls {
            pos = i;
            items.pop();
        } else {
            break;
        }
    }
    pos
}

fn next_word_boundary(text: &str, byte: usize) -> usize {
    let byte = byte.min(text.len());
    let items: Vec<(usize, &str)> = text[byte..]
        .grapheme_indices(true)
        .map(|(i, g)| (byte + i, g))
        .collect();
    let mut idx = 0;
    while idx < items.len() && char_class(items[idx].1) == CharClass::Space {
        idx += 1;
    }
    if idx >= items.len() {
        return text.len();
    }
    let cls = char_class(items[idx].1);
    while idx < items.len() && char_class(items[idx].1) == cls {
        idx += 1;
    }
    if idx < items.len() {
        items[idx].0
    } else {
        text.len()
    }
}
