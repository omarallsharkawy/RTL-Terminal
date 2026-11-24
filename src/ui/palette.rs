//! The command palette (Ctrl+Shift+P): a query, the commands that match it, and the keys that
//! drive it. Window-free; drawing is `render::scene::palette`.
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

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PaletteState {
    pub query: String,
    /// Index into the current results.
    pub selected: usize,
}

/// What a key does to an open palette.
#[derive(Debug, PartialEq, Eq)]
pub enum PaletteCmd {
    None,
    Close,
    /// Run the command with this name and close.
    Run(&'static str),
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

    /// Handle one key press. `text` is what the key typed, if anything.
    pub fn key(
        &mut self,
        commands: &[Command],
        key: &Key,
        text: Option<&str>,
        modifiers: ModifiersState,
    ) -> PaletteCmd {
        let results = matches(commands, &self.query);
        match key {
            Key::Named(NamedKey::Escape) => return PaletteCmd::Close,
            Key::Named(NamedKey::Enter) => {
                return match results.get(self.selected) {
                    Some(&i) => PaletteCmd::Run(commands[i].name),
                    None => PaletteCmd::None,
                };
            }
            Key::Named(NamedKey::ArrowDown) => {
                if !results.is_empty() {
                    self.selected = (self.selected + 1) % results.len().min(MAX_RESULTS);
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                if !results.is_empty() {
                    let n = results.len().min(MAX_RESULTS);
                    self.selected = (self.selected + n - 1) % n;
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

/// Indices of the commands matching `query`, best first. An empty query lists them all in
/// palette order.
pub fn matches(commands: &[Command], query: &str) -> Vec<usize> {
    let words: Vec<String> = query.split_whitespace().map(fold).collect();
    if words.is_empty() {
        return (0..commands.len()).collect();
    }
    let mut scored: Vec<(usize, usize)> = commands
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let fields = [fold(c.title_en), fold(c.title_ar), fold(c.name)];
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
