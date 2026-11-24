//! The command palette (Ctrl+Shift+P): a query, the items that match it, and the keys that
//! drive it. Window-free; drawing is `render::scene::palette`.
//!
//! The palette lists one source of items at a time ([`PaletteMode`]): the commands, or the
//! notification history (`ui::notifications`). Any [`Searchable`] list works the same way.
//!
//! Matching works in Arabic and English alike: every word of the query must appear in the
//! command's name, English title or Arabic title, after folding case and the Arabic letters
//! people type interchangeably (أ إ آ ٱ → ا, ة → ه, ى → ي, tashkeel and tatweel dropped).

use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::commands::Command;

/// Most results shown at once.
pub const MAX_RESULTS: usize = 10;

/// Longest query kept, in characters.
const MAX_QUERY_CHARS: usize = 200;

/// What the palette lists.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    #[default]
    Commands,
    /// The notification history, newest first.
    Notifications,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PaletteState {
    pub query: String,
    /// Index into the current results.
    pub selected: usize,
    pub mode: PaletteMode,
}

/// An item the palette can list: matched against the query through its fields.
pub trait Searchable {
    /// Texts the query is matched against (titles in each language, names…).
    fn search_fields(&self) -> Vec<&str>;
}

impl Searchable for Command {
    fn search_fields(&self) -> Vec<&str> {
        vec![self.title_en, self.title_ar, self.name]
    }
}

impl<T: Searchable + ?Sized> Searchable for &T {
    fn search_fields(&self) -> Vec<&str> {
        (**self).search_fields()
    }
}

/// What a key does to an open palette.
#[derive(Debug, PartialEq, Eq)]
pub enum PaletteCmd {
    None,
    Close,
    /// Run the command with this name and close.
    Run(&'static str),
    /// Item `index` of the listed items was chosen (modes other than commands).
    Choose(usize),
}

impl PaletteState {
    /// Add typed or committed (IME) text to the query.
    pub fn push_text(&mut self, text: &str) {
        for c in text.chars().filter(|c| !c.is_control()) {
            if self.query.chars().count() >= MAX_QUERY_CHARS {
                break;
            }
            self.query.push(c);
        }
        self.selected = 0;
    }

    /// Handle one key press while commands are listed. `text` is what the key typed, if
    /// anything.
    pub fn key(
        &mut self,
        commands: &[Command],
        key: &Key,
        text: Option<&str>,
        modifiers: ModifiersState,
    ) -> PaletteCmd {
        match self.key_in(commands, key, text, modifiers) {
            PaletteCmd::Choose(i) => PaletteCmd::Run(commands[i].name),
            cmd => cmd,
        }
    }

    /// Handle one key press over any list of items; Enter answers `Choose(index into items)`.
    pub fn key_in<T: Searchable>(
        &mut self,
        items: &[T],
        key: &Key,
        text: Option<&str>,
        modifiers: ModifiersState,
    ) -> PaletteCmd {
        let results = matches(items, &self.query);
        match key {
            Key::Named(NamedKey::Escape) => return PaletteCmd::Close,
            Key::Named(NamedKey::Enter) => {
                return match results.get(self.selected) {
                    Some(&i) => PaletteCmd::Choose(i),
                    None => PaletteCmd::None,
                };
            }
            Key::Named(NamedKey::ArrowDown) => {
                if !results.is_empty() {
                    let n = results.len();
                    self.selected = (self.selected.min(n - 1) + 1) % n;
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                if !results.is_empty() {
                    let n = results.len();
                    self.selected = (self.selected.min(n - 1) + n - 1) % n;
                }
            }
            Key::Named(NamedKey::Backspace) => {
                if modifiers.control_key() {
                    // Ctrl+Backspace drops the last word.
                    let trimmed = self.query.trim_end();
                    let cut = trimmed.rfind(char::is_whitespace).map_or(0, |i| i + 1);
                    self.query.truncate(cut);
                } else {
                    self.query.pop();
                }
                self.selected = 0;
            }
            _ => {
                if let Some(text) = text.filter(|_| !modifiers.control_key()) {
                    self.push_text(text);
                }
            }
        }
        PaletteCmd::None
    }
}

/// The results shown when `selected` is the selected one of `len`: at most [`MAX_RESULTS`],
/// scrolled so the selection stays visible.
pub fn visible_window(len: usize, selected: usize) -> std::ops::Range<usize> {
    let start = selected
        .min(len.saturating_sub(1))
        .saturating_sub(MAX_RESULTS - 1);
    start..len.min(start + MAX_RESULTS)
}

/// Indices of the items matching `query`, best first. An empty query lists them all in
/// their own order.
pub fn matches<T: Searchable>(items: &[T], query: &str) -> Vec<usize> {
    let words: Vec<String> = query.split_whitespace().map(fold).collect();
    if words.is_empty() {
        return (0..items.len()).collect();
    }
    let mut scored: Vec<(usize, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let fields: Vec<String> = c.search_fields().into_iter().map(fold).collect();
            let mut score = 0;
            for word in &words {
                let best = fields
                    .iter()
                    .filter_map(|f| f.find(word.as_str()).map(|at| word_score(f, at)))
                    .min()?;
                score += best;
            }
            Some((score, i))
        })
        .collect();
    scored.sort();
    scored.into_iter().map(|(_, i)| i).collect()
}

/// Lower is better: a match at the start of a word beats one inside a word.
fn word_score(field: &str, at: usize) -> usize {
    let word_start = at == 0
        || field[..at]
            .chars()
            .next_back()
            .is_some_and(|c| !c.is_alphanumeric());
    if word_start {
        at.min(50)
    } else {
        100 + at
    }
}

/// Case and Arabic spelling variants folded away, for matching.
pub fn fold(text: &str) -> String {
    text.chars()
        .filter(|&c| !is_arabic_mark(c))
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'أ' | 'إ' | 'آ' | 'ٱ' => 'ا',
            'ة' => 'ه',
            'ى' => 'ي',
            'ؤ' => 'و',
            'ئ' => 'ي',
            c => c,
        })
        .collect()
}

/// Tashkeel (harakat, shadda, sukun, superscript alef) and tatweel.
fn is_arabic_mark(c: char) -> bool {
    matches!(c, '\u{064B}'..='\u{065F}' | '\u{0670}' | '\u{0640}')
}
