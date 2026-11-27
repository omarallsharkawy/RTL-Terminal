//! Command composer: Warp-style multi-line RTL-friendly input editor docked at the bottom of the
//! focused pane.
//!
//! Window-free state machine covering config modes (`off` | `on-demand` | `always`), OSC 133
//! prompt-state show/hide rules, editing, history browsing, spec-driven + dynamic completion with
//! Arabic-aware matching and inline ghost suggestions, syntax highlighting, pre-run linting, and
//! bracketed-paste submission.

pub mod complete;
pub mod editor;
pub mod generators;
pub mod highlight;
pub mod history;
pub mod lint;
pub mod slash;
pub mod specs;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey};

use crate::input::encode_paste;
use crate::shell_integration::MarkKind;
use crate::workflows::Workflow;
use complete::{
    complete_with, extract_path_query, ghost_suggestion, needed_generators, CompleteInputs,
    CompletionItem, CompletionKind, DirEntryInfo,
};
use editor::TextEditor;
use generators::{GeneratedItem, GeneratorKey, GeneratorKind};
use specs::default_registry;

/// Maximum rows shown at once in the completion popup.
pub const MAX_POPUP_ROWS: usize = 8;

/// Configured display policy for the command composer (`config.json` key `"composer"`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ComposerMode {
    /// Disabled: never opens.
    Off,
    /// Opens on `Ctrl+Shift+Space` or from the command palette; closes on `Esc` or `Enter`.
    #[default]
    OnDemand,
    /// Shown automatically whenever the focused pane is at a shell prompt (known from OSC 133
    /// marks) and hidden while a program runs or on the alternate screen.
    Always,
}

impl ComposerMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::OnDemand => "on-demand",
            Self::Always => "always",
        }
    }
}

impl FromStr for ComposerMode {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Ok(Self::Off),
            "on-demand" | "ondemand" => Ok(Self::OnDemand),
            "always" => Ok(Self::Always),
            _ => Err(()),
        }
    }
}

/// Shell prompt lifecycle tracked per pane from OSC 133 marks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PromptState {
    /// No OSC 133 mark has been seen yet on this pane.
    #[default]
    Unknown,
    /// OSC 133 A or B seen: shell is presenting a prompt waiting for input.
    AtPrompt,
    /// OSC 133 C seen: a command is executing.
    Running,
    /// OSC 133 D seen: command ended, awaiting next prompt mark A.
    CommandEnded,
}

impl PromptState {
    pub fn on_mark(&mut self, kind: MarkKind) {
        *self = match kind {
            MarkKind::PromptStart | MarkKind::CommandStart => Self::AtPrompt,
            MarkKind::OutputStart => Self::Running,
            MarkKind::CommandEnd { .. } => Self::CommandEnded,
        };
    }

    pub fn is_at_prompt(self) -> bool {
        self == Self::AtPrompt
    }
}

/// Pure show/hide rule for the composer.
///
/// - `Off`: always hidden.
/// - `OnDemand`: shown when manually opened, hidden on alternate screen.
/// - `Always`: shown automatically at an OSC 133 prompt (unless dismissed via `Esc` for the
///   current prompt), hidden while a command/program is running or on the alternate screen.
///   Can also be manually re-opened on the main screen when not running.
pub fn should_show_composer(
    mode: ComposerMode,
    manual_open: bool,
    dismissed_for_prompt: bool,
    prompt: PromptState,
    running: bool,
    alt_screen: bool,
) -> bool {
    match mode {
        ComposerMode::Off => false,
        ComposerMode::OnDemand => manual_open && !alt_screen,
        ComposerMode::Always => {
            if alt_screen || running {
                false
            } else if manual_open {
                true
            } else {
                prompt == PromptState::AtPrompt && !dismissed_for_prompt
            }
        }
    }
}

/// Encode `text` for sending to the pane when the user presses Enter in the composer:
/// bracketed paste (when enabled by the pane) plus a carriage return (`\r`).
/// An empty command sends just `\r`.
pub fn encode_submit(text: &str, bracketed: bool) -> Vec<u8> {
    if text.is_empty() {
        return vec![b'\r'];
    }
    let mut bytes = encode_paste(text, bracketed);
    bytes.push(b'\r');
    bytes
}

/// A one-line `/command [args]` that should reach an agent as typed keys, not as a paste.
pub fn is_typed_slash_command(text: &str) -> bool {
    text.starts_with('/') && text.len() > 1 && !text.chars().any(|c| c.is_control())
}

/// Active completion popup list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PopupState {
    pub items: Vec<CompletionItem>,
    pub selected: usize,
}

impl PopupState {
    pub fn select_next(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected + 1) % self.items.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected + self.items.len() - 1) % self.items.len();
        }
    }

    pub fn visible_range(&self) -> std::ops::Range<usize> {
        let len = self.items.len();
        let cap = MAX_POPUP_ROWS;
        let start = self
            .selected
            .min(len.saturating_sub(1))
            .saturating_sub(cap.saturating_sub(1));
        start..len.min(start + cap)
    }

    pub fn current(&self) -> Option<&CompletionItem> {
        self.items.get(self.selected)
    }
}

/// Side effect requested by the composer state machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerAction {
    None,
    Redraw,
    /// Esc closed the composer (return focus to terminal).
    Close,
    /// Enter submitted this command text to the pane.
    Submit(String),
    /// Ctrl+C / Ctrl+X copied this text to the clipboard.
    Copy(String),
    /// Ctrl+V / Ctrl+Shift+V requested clipboard text.
    RequestPaste,
    /// Path completion needs directory entries for `PathBuf` loaded off the UI thread.
    RequestDir(PathBuf),
    /// Dynamic completion needs one or more generators executed off the UI thread.
    RequestGenerators(Vec<GeneratorKey>),
    /// Natural-language request (`# ...`, Ctrl+Enter, or Generate button) to send to the AI agent.
    /// The reply is placed back into the editor for review and NEVER executed automatically.
    GenerateAi(String),
    /// Esc cancelled an in-flight AI command generation.
    CancelAi,
    /// Esc discarded a generated AI command under review.
    DiscardAi,
    /// A Twitty command or saved workflow picked from the slash menu, by name.
    RunCommand(String),
}

/// Extract a natural-language AI request when the composer buffer starts with `#`.
pub fn extract_hash_ai_request(text: &str) -> Option<String> {
    let trimmed = text.trim_start();
    let rest = trimmed.strip_prefix('#')?.trim();
    (!rest.is_empty()).then(|| rest.to_owned())
}

/// Extract a natural-language AI request for explicit triggers (Ctrl+Enter or the Generate button),
/// stripping an optional leading `#`.
pub fn extract_ai_request(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let rest = trimmed.strip_prefix('#').map_or(trimmed, str::trim);
    (!rest.is_empty()).then(|| rest.to_owned())
}

/// Full interactive state of the command composer (editing, history browsing, completion popup,
/// inline ghost suggestion, and built-in AI command generation review).
#[derive(Clone, Debug)]
pub struct ComposerState {
    pub editor: TextEditor,
    history_index: Option<usize>,
    draft_before_history: String,
    pub popup: Option<PopupState>,
    pub ghost: Option<String>,
    /// Mirrors `config.ai` so `#` and Ctrl+Enter only trigger AI when enabled.
    pub ai_enabled: bool,
    /// True while a non-interactive AI request is in flight for this composer.
    pub ai_loading: bool,
    /// True while the editor holds an AI-generated command awaiting user review.
    pub ai_reviewing: bool,
    /// Agent slash items found for the pane's project (filled off the UI thread).
    pub slash: std::sync::Arc<slash::SlashCatalog>,
    /// Effective keybindings used for live `/command` shortcut hints in the slash popup.
    pub keybindings: Option<std::sync::Arc<crate::input::keybindings::KeyBindings>>,
    /// The coding agent running in the pane, if any (decides what a slash pick inserts).
    pub slash_foreground: slash::Foreground,
    /// The open popup is the slash menu (picks of Twitty commands run instead of inserting).
    slash_popup: bool,
}

impl Default for ComposerState {
    fn default() -> Self {
        Self {
            editor: TextEditor::new(),
            history_index: None,
            draft_before_history: String::new(),
            popup: None,
            ghost: None,
            ai_enabled: true,
            ai_loading: false,
            ai_reviewing: false,
            slash: Default::default(),
            keybindings: None,
            slash_foreground: slash::Foreground::Shell,
            slash_popup: false,
        }
    }
}

impl ComposerState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.editor = TextEditor::new();
        self.history_index = None;
        self.draft_before_history.clear();
        self.popup = None;
        self.ghost = None;
        self.ai_loading = false;
        self.ai_reviewing = false;
    }

    /// Place a generated AI command into the editor for user review.
    ///
    /// Strictly NEVER submits or executes the command: the user must explicitly press Enter to
    /// run it, or Esc to discard it.
    pub fn apply_ai_command(&mut self, command: &str) {
        self.editor.set_text(command.to_owned());
        self.history_index = None;
        self.popup = None;
        self.ghost = None;
        self.ai_loading = false;
        self.ai_reviewing = true;
    }

    /// Discard an AI-generated command currently under review without running it.
    pub fn discard_ai_command(&mut self) {
        self.editor = TextEditor::new();
        self.history_index = None;
        self.popup = None;
        self.ghost = None;
        self.ai_loading = false;
        self.ai_reviewing = false;
    }

    /// Recompute inline ghost suggestion (and clear stale popup if needed).
    pub fn refresh_ghost(&mut self, history: &[String], workflows: &[Workflow]) {
        if self.editor.preedit().is_some() || self.editor.selection_range().is_some() {
            self.ghost = None;
            return;
        }
        self.ghost = ghost_suggestion(self.editor.text(), self.editor.caret(), history, workflows);
    }

    /// Apply text from IME commit or clipboard paste.
    pub fn insert_text(
        &mut self,
        text: &str,
        history: &[String],
        workflows: &[Workflow],
    ) -> ComposerAction {
        self.editor.set_preedit(None);
        if !self.editor.insert_text(text) {
            return ComposerAction::None;
        }
        self.on_buffer_edited(history, workflows);
        ComposerAction::Redraw
    }

    /// Accept the selected item from the completion popup (or item at `index`).
    pub fn accept_popup_index(
        &mut self,
        index: usize,
        history: &[String],
        workflows: &[Workflow],
    ) -> ComposerAction {
        let Some(item) = self
            .popup
            .as_ref()
            .and_then(|p| p.items.get(index))
            .cloned()
        else {
            return ComposerAction::None;
        };
        self.apply_completion(&item, history, workflows)
    }

    /// Apply a popup pick: splice its text in, or run it when it is a Twitty command or a
    /// saved workflow picked from the slash menu.
    fn apply_completion(
        &mut self,
        item: &CompletionItem,
        history: &[String],
        workflows: &[Workflow],
    ) -> ComposerAction {
        if self.slash_popup
            && matches!(item.kind, CompletionKind::Action | CompletionKind::Workflow)
        {
            self.reset();
            return ComposerAction::RunCommand(item.replacement.clone());
        }
        self.splice_completion(item, history, workflows);
        ComposerAction::Redraw
    }

    fn splice_completion(
        &mut self,
        item: &CompletionItem,
        history: &[String],
        workflows: &[Workflow],
    ) {
        self.editor.replace_range(
            item.replace_range.0,
            item.replace_range.1,
            &item.replacement,
        );
        self.popup = None;
        self.slash_popup = false;
        self.history_index = None;
        self.refresh_ghost(history, workflows);
    }

    fn on_buffer_edited(&mut self, history: &[String], workflows: &[Workflow]) {
        self.history_index = None;
        self.popup = None;
        self.slash_popup = false;
        if self.editor.text().is_empty() {
            self.ai_reviewing = false;
        }
        self.refresh_ghost(history, workflows);
        self.open_slash_menu(workflows);
    }

    /// True while the slash menu is open (the buffer is a `/query`, not a shell command).
    pub fn slash_menu_open(&self) -> bool {
        self.slash_popup && self.popup.is_some()
    }

    /// Open the slash menu while the buffer is `/query` (see [`slash::slash_query`]).
    pub fn open_slash_menu(&mut self, workflows: &[Workflow]) {
        if self.ai_reviewing || self.editor.preedit().is_some() {
            return;
        }
        let text = self.editor.text();
        let Some(query) = slash::slash_query(text, self.editor.caret()) else {
            return;
        };
        let inputs = slash::SlashInputs {
            catalog: &self.slash,
            workflows,
            foreground: self.slash_foreground,
        };
        let items = slash::suggestions_with_keybindings(
            query,
            text.len(),
            &inputs,
            self.keybindings.as_deref(),
        );
        if !items.is_empty() {
            self.ghost = None;
            self.popup = Some(PopupState { items, selected: 0 });
            self.slash_popup = true;
        }
    }

    /// Check if path completion at the current caret needs a directory listing loaded.
    pub fn needed_dir(&self, cwd: Option<&Path>) -> Option<PathBuf> {
        extract_path_query(self.editor.text(), self.editor.caret(), cwd).map(|q| q.dir_to_read)
    }

    /// Return any dynamic generators needed for the current caret position.
    pub fn needed_generators(&self, cwd: Option<&Path>) -> Vec<GeneratorKey> {
        needed_generators(
            self.editor.text(),
            self.editor.caret(),
            default_registry(),
            cwd,
        )
    }

    /// Refresh open completion popup when background directory entries arrive.
    pub fn refresh_popup_if_open(
        &mut self,
        history: &[String],
        workflows: &[Workflow],
        dir_cache: Option<(&Path, &[DirEntryInfo])>,
        cwd: Option<&Path>,
    ) -> bool {
        let empty = HashMap::new();
        self.refresh_popup_if_open_with(history, workflows, dir_cache, cwd, &empty, None)
    }

    /// Refresh open completion popup when directory entries or dynamic generator results arrive.
    pub fn refresh_popup_if_open_with(
        &mut self,
        history: &[String],
        workflows: &[Workflow],
        dir_cache: Option<(&Path, &[DirEntryInfo])>,
        cwd: Option<&Path>,
        generated: &HashMap<GeneratorKind, Vec<GeneratedItem>>,
        path_commands: Option<&HashSet<String>>,
    ) -> bool {
        if self.popup.is_none() {
            return false;
        }
        if self.slash_popup {
            // Late directory or generator results are for path completion, not the slash menu.
            self.popup = None;
            self.slash_popup = false;
            self.open_slash_menu(workflows);
            return true;
        }
        let items =
            self.build_completions(history, workflows, dir_cache, cwd, generated, path_commands);
        if items.is_empty() {
            self.popup = None;
        } else if let Some(popup) = self.popup.as_mut() {
            popup.items = items;
            popup.selected = popup.selected.min(popup.items.len() - 1);
        }
        true
    }

    fn build_completions(
        &self,
        history: &[String],
        workflows: &[Workflow],
        dir_cache: Option<(&Path, &[DirEntryInfo])>,
        cwd: Option<&Path>,
        generated: &HashMap<GeneratorKind, Vec<GeneratedItem>>,
        path_commands: Option<&HashSet<String>>,
    ) -> Vec<CompletionItem> {
        let pq = extract_path_query(self.editor.text(), self.editor.caret(), cwd);
        let entries: &[DirEntryInfo] = match (&pq, dir_cache) {
            (Some(q), Some((cached_dir, entries))) if cached_dir == q.dir_to_read => entries,
            _ => &[],
        };
        let inputs = CompleteInputs {
            history,
            workflows,
            path_query: pq.as_ref(),
            dir_entries: entries,
            specs: default_registry(),
            generated,
            path_commands,
        };
        complete_with(self.editor.text(), self.editor.caret(), &inputs)
    }

    /// Handle one key press inside the composer (without dynamic generator cache).
    #[allow(clippy::too_many_arguments)]
    pub fn key(
        &mut self,
        key: &Key,
        physical: Option<KeyCode>,
        text: Option<&str>,
        modifiers: ModifiersState,
        history: &[String],
        workflows: &[Workflow],
        dir_cache: Option<(&Path, &[DirEntryInfo])>,
        cwd: Option<&Path>,
    ) -> ComposerAction {
        let empty = HashMap::new();
        self.key_with_context(
            key, physical, text, modifiers, history, workflows, dir_cache, cwd, &empty, None,
        )
    }

    /// Handle one key press inside the composer with dynamic generator results and `$PATH` cache.
    #[allow(clippy::too_many_arguments)]
    pub fn key_with_context(
        &mut self,
        key: &Key,
        physical: Option<KeyCode>,
        text: Option<&str>,
        modifiers: ModifiersState,
        history: &[String],
        workflows: &[Workflow],
        dir_cache: Option<(&Path, &[DirEntryInfo])>,
        cwd: Option<&Path>,
        generated: &HashMap<GeneratorKind, Vec<GeneratedItem>>,
        path_commands: Option<&HashSet<String>>,
    ) -> ComposerAction {
        let ctrl = modifiers.control_key();
        let shift = modifiers.shift_key();
        let alt = modifiers.alt_key();
        let super_key = modifiers.super_key();

        // While waiting on an AI response, only Esc (cancel) and Copy are active.
        if self.ai_loading {
            if matches!(key, Key::Named(NamedKey::Escape)) {
                self.ai_loading = false;
                return ComposerAction::CancelAi;
            }
            if ctrl && !alt && !super_key {
                let letter =
                    logical_ascii_letter(key).or_else(|| physical.and_then(physical_letter));
                if letter == Some('c') {
                    return match self.editor.copy() {
                        Some(copied) => ComposerAction::Copy(copied),
                        None => ComposerAction::None,
                    };
                }
            }
            return ComposerAction::None;
        }

        // Ctrl shortcuts (layout-independent via physical key fallback so Arabic layout works).
        if ctrl && !alt && !super_key {
            let letter = logical_ascii_letter(key).or_else(|| physical.and_then(physical_letter));
            match (letter, shift) {
                (Some('a'), false) => {
                    self.editor.select_all();
                    self.popup = None;
                    self.refresh_ghost(history, workflows);
                    return ComposerAction::Redraw;
                }
                (Some('c'), false) | (Some('c'), true) => {
                    return match self.editor.copy() {
                        Some(copied) => ComposerAction::Copy(copied),
                        None => ComposerAction::None,
                    };
                }
                (Some('x'), false) => {
                    return match self.editor.cut() {
                        Some(cut) => {
                            self.on_buffer_edited(history, workflows);
                            ComposerAction::Copy(cut)
                        }
                        None => ComposerAction::None,
                    };
                }
                (Some('v'), false) | (Some('v'), true) => {
                    return ComposerAction::RequestPaste;
                }
                (Some('z'), false) => {
                    if self.editor.undo() {
                        self.on_buffer_edited(history, workflows);
                        return ComposerAction::Redraw;
                    }
                    return ComposerAction::None;
                }
                (Some('z'), true) | (Some('y'), false) => {
                    if self.editor.redo() {
                        self.on_buffer_edited(history, workflows);
                        return ComposerAction::Redraw;
                    }
                    return ComposerAction::None;
                }
                _ => {}
            }
        }

        match key {
            Key::Named(NamedKey::Escape) => {
                if self.popup.take().is_some() {
                    ComposerAction::Redraw
                } else if self.ai_reviewing {
                    self.discard_ai_command();
                    ComposerAction::DiscardAi
                } else {
                    ComposerAction::Close
                }
            }
            Key::Named(NamedKey::Enter) => {
                if ctrl && !shift && !alt && !super_key {
                    if self.ai_enabled {
                        if let Some(req) = extract_ai_request(self.editor.text()) {
                            self.popup = None;
                            return ComposerAction::GenerateAi(req);
                        }
                    }
                    ComposerAction::None
                } else if shift && !ctrl && !alt {
                    self.popup = None;
                    self.editor.insert_newline();
                    self.on_buffer_edited(history, workflows);
                    ComposerAction::Redraw
                } else if !ctrl && !alt && !super_key {
                    if let Some(item) = self.popup.as_ref().and_then(PopupState::current).cloned() {
                        self.apply_completion(&item, history, workflows)
                    } else if self.ai_enabled
                        && !self.ai_reviewing
                        && self.editor.text().trim_start().starts_with('#')
                    {
                        if let Some(req) = extract_hash_ai_request(self.editor.text()) {
                            ComposerAction::GenerateAi(req)
                        } else {
                            ComposerAction::None
                        }
                    } else {
                        let cmd = self.editor.text().to_owned();
                        self.reset();
                        ComposerAction::Submit(cmd)
                    }
                } else {
                    ComposerAction::None
                }
            }
            Key::Named(NamedKey::Tab) if !ctrl && !alt && !super_key => {
                if let Some(popup) = self.popup.as_mut() {
                    if shift {
                        popup.select_prev();
                    } else {
                        popup.select_next();
                    }
                    return ComposerAction::Redraw;
                }
                let pq = extract_path_query(self.editor.text(), self.editor.caret(), cwd);
                let cache_miss = pq
                    .as_ref()
                    .filter(|q| dir_cache.is_none_or(|(d, _)| d != q.dir_to_read))
                    .map(|q| q.dir_to_read.clone());
                let missing_gens: Vec<GeneratorKey> = self
                    .needed_generators(cwd)
                    .into_iter()
                    .filter(|k| !generated.contains_key(&k.kind))
                    .collect();
                let items = self.build_completions(
                    history,
                    workflows,
                    dir_cache,
                    cwd,
                    generated,
                    path_commands,
                );
                if items.len() == 1 && cache_miss.is_none() && missing_gens.is_empty() {
                    return self.apply_completion(&items[0], history, workflows);
                }
                if !items.is_empty() {
                    self.popup = Some(PopupState { items, selected: 0 });
                    if !missing_gens.is_empty() {
                        return ComposerAction::RequestGenerators(missing_gens);
                    }
                    if let Some(dir) = cache_miss {
                        return ComposerAction::RequestDir(dir);
                    }
                    return ComposerAction::Redraw;
                }
                if !missing_gens.is_empty() {
                    return ComposerAction::RequestGenerators(missing_gens);
                }
                if let Some(ghost) = self.ghost.clone() {
                    self.editor.insert_text(&ghost);
                    self.on_buffer_edited(history, workflows);
                    return ComposerAction::Redraw;
                }
                if let Some(dir) = cache_miss {
                    return ComposerAction::RequestDir(dir);
                }
                ComposerAction::None
            }
            Key::Named(NamedKey::ArrowLeft) if !alt && !super_key => {
                self.popup = None;
                if !ctrl
                    && !shift
                    && self.editor.caret_is_rtl()
                    && self.editor.is_at_end_of_buffer()
                    && self.editor.selection_range().is_none()
                {
                    if let Some(ghost) = self.ghost.clone() {
                        self.editor.insert_text(&ghost);
                        self.on_buffer_edited(history, workflows);
                        return ComposerAction::Redraw;
                    }
                }
                if ctrl {
                    self.editor.move_word_visual_left(shift);
                } else {
                    self.editor.move_visual_left(shift);
                }
                self.refresh_ghost(history, workflows);
                ComposerAction::Redraw
            }
            Key::Named(NamedKey::ArrowRight) if !alt && !super_key => {
                self.popup = None;
                if !ctrl
                    && !shift
                    && !self.editor.caret_is_rtl()
                    && self.editor.is_at_end_of_buffer()
                    && self.editor.selection_range().is_none()
                {
                    if let Some(ghost) = self.ghost.clone() {
                        self.editor.insert_text(&ghost);
                        self.on_buffer_edited(history, workflows);
                        return ComposerAction::Redraw;
                    }
                }
                if ctrl {
                    self.editor.move_word_visual_right(shift);
                } else {
                    self.editor.move_visual_right(shift);
                }
                self.refresh_ghost(history, workflows);
                ComposerAction::Redraw
            }
            Key::Named(NamedKey::ArrowUp) if !alt && !super_key => {
                if let Some(popup) = self.popup.as_mut() {
                    popup.select_prev();
                    return ComposerAction::Redraw;
                }
                if !shift && !ctrl && self.editor.is_on_first_line() {
                    if self.browse_history_older(history) {
                        self.refresh_ghost(history, workflows);
                        return ComposerAction::Redraw;
                    }
                    return ComposerAction::None;
                }
                if self.editor.move_up(shift) {
                    self.refresh_ghost(history, workflows);
                    ComposerAction::Redraw
                } else {
                    ComposerAction::None
                }
            }
            Key::Named(NamedKey::ArrowDown) if !alt && !super_key => {
                if let Some(popup) = self.popup.as_mut() {
                    popup.select_next();
                    return ComposerAction::Redraw;
                }
                if !shift && !ctrl && self.editor.is_on_last_line() {
                    if self.browse_history_newer(history) {
                        self.refresh_ghost(history, workflows);
                        return ComposerAction::Redraw;
                    }
                    return ComposerAction::None;
                }
                if self.editor.move_down(shift) {
                    self.refresh_ghost(history, workflows);
                    ComposerAction::Redraw
                } else {
                    ComposerAction::None
                }
            }
            Key::Named(NamedKey::Home) if !alt && !super_key => {
                self.popup = None;
                self.editor.move_home(ctrl, shift);
                self.refresh_ghost(history, workflows);
                ComposerAction::Redraw
            }
            Key::Named(NamedKey::End) if !alt && !super_key => {
                self.popup = None;
                self.editor.move_end(ctrl, shift);
                self.refresh_ghost(history, workflows);
                ComposerAction::Redraw
            }
            Key::Named(NamedKey::Backspace) if !alt && !super_key => {
                let changed = if ctrl {
                    self.editor.delete_word_backward()
                } else {
                    self.editor.backspace()
                };
                if changed {
                    self.on_buffer_edited(history, workflows);
                    ComposerAction::Redraw
                } else {
                    ComposerAction::None
                }
            }
            Key::Named(NamedKey::Delete) if !alt && !super_key => {
                let changed = if ctrl {
                    self.editor.delete_word_forward()
                } else {
                    self.editor.delete_forward()
                };
                if changed {
                    self.on_buffer_edited(history, workflows);
                    ComposerAction::Redraw
                } else {
                    ComposerAction::None
                }
            }
            Key::Named(NamedKey::Space) if !ctrl && !alt && !super_key => {
                self.editor.insert_text(" ");
                self.on_buffer_edited(history, workflows);
                ComposerAction::Redraw
            }
            _ => {
                let Some(text) = text.filter(|t| !t.is_empty() && !ctrl && !alt && !super_key)
                else {
                    return ComposerAction::None;
                };
                if self.editor.insert_text(text) {
                    self.on_buffer_edited(history, workflows);
                    ComposerAction::Redraw
                } else {
                    ComposerAction::None
                }
            }
        }
    }

    fn browse_history_older(&mut self, history: &[String]) -> bool {
        if history.is_empty() {
            return false;
        }
        let next_idx = match self.history_index {
            None => {
                self.draft_before_history = self.editor.text().to_owned();
                0
            }
            Some(i) if i + 1 < history.len() => i + 1,
            Some(_) => return false,
        };
        self.history_index = Some(next_idx);
        self.editor.set_text_without_undo(history[next_idx].clone());
        true
    }

    fn browse_history_newer(&mut self, history: &[String]) -> bool {
        let Some(cur) = self.history_index else {
            return false;
        };
        if cur == 0 {
            self.history_index = None;
            let draft = std::mem::take(&mut self.draft_before_history);
            self.editor.set_text_without_undo(draft);
        } else {
            let next_idx = cur - 1;
            self.history_index = Some(next_idx);
            self.editor.set_text_without_undo(history[next_idx].clone());
        }
        true
    }
}

fn logical_ascii_letter(key: &Key) -> Option<char> {
    if let Key::Character(s) = key {
        let mut chars = s.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            let low = c.to_ascii_lowercase();
            if low.is_ascii_lowercase() {
                return Some(low);
            }
        }
    }
    None
}

fn physical_letter(code: KeyCode) -> Option<char> {
    Some(match code {
        KeyCode::KeyA => 'a',
        KeyCode::KeyC => 'c',
        KeyCode::KeyV => 'v',
        KeyCode::KeyX => 'x',
        KeyCode::KeyY => 'y',
        KeyCode::KeyZ => 'z',
        _ => return None,
    })
}
