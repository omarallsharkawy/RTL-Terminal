//! The command palette (Ctrl+Shift+P): a query, the items that match it, and the keys that
//! drive it. Window-free; drawing is `render::scene::palette`.
//!
//! The palette lists one source of items at a time ([`PaletteMode`]): the commands (built-in
//! and custom, as [`PaletteEntry`]s), the notification history (`ui::notifications`), or the
//! lines of every terminal matching the query (`ui::search_all`). Any [`Searchable`] list
//! works the same way; a list already matched elsewhere uses [`PaletteState::key_list`].
//!
//! Matching works in Arabic and English alike: every word of the query must appear in the
//! command's name, English title or Arabic title, after folding case and the Arabic letters
//! people type interchangeably (أ إ آ ٱ → ا, ة → ه, ى → ي, tashkeel and tatweel dropped).

use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::commands::Command;
use crate::input::keybindings::KeyBindings;
use twitty_core::language::UiLanguage;

/// Most results shown at once.
pub const MAX_RESULTS: usize = 10;

/// Longest query kept, in characters.
const MAX_QUERY_CHARS: usize = 200;

pub const RENAME_TAB_PLACEHOLDER: &str = twitty_core::en!("Rename tab…");

/// What the palette lists.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    #[default]
    Commands,
    /// The notification history, newest first.
    Notifications,
    /// Lines of every pane matching the query (`search.all`).
    Search,
    RenameTab,
    CloseOtherPanes,
    /// Project switcher (recent cwds + git repositories under `project_roots`).
    Projects,
    /// Prompt for a name to save the active tab's layout.
    SaveLayout,
    /// Pick a saved layout to recreate in a new tab.
    OpenLayout,
    /// Pick a saved layout to delete.
    DeleteLayout,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PaletteState {
    pub query: String,
    /// Index into the current results.
    pub selected: usize,
    pub mode: PaletteMode,
}

/// Identifier of one command palette entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteId {
    Builtin(&'static str),
    /// Index into the window's custom commands (`workflows::WorkflowSet`).
    Custom(usize),
}

/// One searchable entry in the command palette.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteEntry {
    pub id: PaletteId,
    /// Extra search text (stable command name, command line, description…).
    pub name: String,
    pub title_en: String,
    pub title_ar: String,
    /// Shortcut or small tag shown beside the English title.
    pub shortcut: Option<String>,
}

impl PaletteEntry {
    pub fn from_command(command: &Command) -> Self {
        Self::from_command_with(command, KeyBindings::defaults())
    }

    pub fn from_command_with(command: &Command, bindings: &KeyBindings) -> Self {
        Self {
            id: PaletteId::Builtin(command.name),
            name: command.name.to_owned(),
            title_en: command.title_en.to_owned(),
            title_ar: command.title_ar.to_owned(),
            shortcut: bindings.display_shortcut(command.name),
        }
    }

    pub fn from_commands(commands: &[Command]) -> Vec<Self> {
        Self::from_commands_with(commands, KeyBindings::defaults())
    }

    pub fn from_commands_with(commands: &[Command], bindings: &KeyBindings) -> Vec<Self> {
        commands
            .iter()
            .map(|c| Self::from_command_with(c, bindings))
            .collect()
    }

    pub fn builtins() -> Vec<Self> {
        Self::builtins_with(KeyBindings::defaults())
    }

    pub fn builtins_with(bindings: &KeyBindings) -> Vec<Self> {
        Self::from_commands_with(&crate::commands::all(), bindings)
    }
}

impl Searchable for PaletteEntry {
    fn search_fields(&self) -> Vec<&str> {
        vec![&self.title_en, &self.title_ar, &self.name]
    }
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
    /// Activate the chosen command palette entry.
    Run(PaletteId),
    /// Item `index` of the listed items was chosen (modes other than commands).
    Choose(usize),
}

#[derive(Debug, PartialEq, Eq)]
pub enum RenameCmd {
    Edit,
    Cancel,
    Save(Option<String>),
}

impl PaletteState {
    pub fn rename_key(
        &mut self,
        key: &Key,
        text: Option<&str>,
        modifiers: ModifiersState,
    ) -> RenameCmd {
        match self.key_list(1, key, text, modifiers) {
            PaletteCmd::Choose(_) => RenameCmd::Save(self.tab_title()),
            PaletteCmd::Close => RenameCmd::Cancel,
            _ => RenameCmd::Edit,
        }
    }

    pub fn tab_title(&self) -> Option<String> {
        let title = crate::shell_integration::clean_text(&self.query, MAX_QUERY_CHARS);
        (!title.is_empty()).then_some(title)
    }

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
        entries: &[PaletteEntry],
        key: &Key,
        text: Option<&str>,
        modifiers: ModifiersState,
    ) -> PaletteCmd {
        match self.key_in(entries, key, text, modifiers) {
            PaletteCmd::Choose(i) => PaletteCmd::Run(entries[i].id.clone()),
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
        self.key_results(&results, key, text, modifiers)
    }

    /// Handle one key press over `len` items that are already the results (matched elsewhere,
    /// like the lines of `search.all`); Enter answers `Choose(index)`.
    pub fn key_list(
        &mut self,
        len: usize,
        key: &Key,
        text: Option<&str>,
        modifiers: ModifiersState,
    ) -> PaletteCmd {
        let results: Vec<usize> = (0..len).collect();
        self.key_results(&results, key, text, modifiers)
    }

    fn key_results(
        &mut self,
        results: &[usize],
        key: &Key,
        text: Option<&str>,
        modifiers: ModifiersState,
    ) -> PaletteCmd {
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
                    let cut = trimmed
                        .char_indices()
                        .rev()
                        .find(|&(_, c)| c.is_whitespace())
                        .map_or(0, |(i, c)| i + c.len_utf8());
                    self.query.truncate(cut);
                } else {
                    self.query.pop();
                }
                self.selected = 0;
            }
            _ => {
                if let Some(text) = text.filter(|_| {
                    !modifiers.control_key() && !modifiers.alt_key() && !modifiers.super_key()
                }) {
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
    visible_window_in(len, selected, MAX_RESULTS)
}

pub fn visible_window_in(len: usize, selected: usize, capacity: usize) -> std::ops::Range<usize> {
    let start = selected
        .min(len.saturating_sub(1))
        .saturating_sub(capacity.saturating_sub(1));
    start..len.min(start + capacity)
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
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        fold_into(c, &mut out);
    }
    out
}

/// `c` folded as [`fold`] does, appended to `out` (nothing for tashkeel and tatweel).
pub fn fold_into(c: char, out: &mut String) {
    if c.is_ascii() {
        out.push(c.to_ascii_lowercase());
        return;
    }
    if is_arabic_mark(c) {
        return;
    }
    for c in c.to_lowercase() {
        out.push(match c {
            'أ' | 'إ' | 'آ' | 'ٱ' => 'ا',
            'ة' => 'ه',
            'ى' => 'ي',
            'ؤ' => 'و',
            'ئ' => 'ي',
            c => c,
        });
    }
}

/// Tashkeel (harakat, shadda, sukun, superscript alef) and tatweel.
fn is_arabic_mark(c: char) -> bool {
    matches!(c, '\u{064B}'..='\u{065F}' | '\u{0670}' | '\u{0640}')
}

/// What the palette shows; drawn by `render::scene::palette`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PaletteView {
    pub query: String,
    /// Title (English), Arabic title, shortcut.
    pub items: Vec<PaletteItem>,
    /// Index into `items`.
    pub selected: usize,
    /// Shown while the query is empty; `None` for the commands' hint.
    pub placeholder: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PaletteItem {
    pub title_en: String,
    pub title_ar: String,
    pub shortcut: Option<String>,
    /// Listed but cannot be chosen (a notification from a closed pane).
    pub dimmed: bool,
}

impl PaletteItem {
    /// A row of interface text (English, see [`twitty_core::language`]) in both languages.
    pub fn text(english: &str, dimmed: bool) -> Self {
        Self {
            title_en: UiLanguage::English.t(english).to_owned(),
            title_ar: UiLanguage::Arabic.t(english).to_owned(),
            shortcut: None,
            dimmed,
        }
    }
}
