//! Window-free state, layout, keyboard navigation, Arabic IME text editing, and pointer hit
//! testing for the Project Memory screen (`memory.open` in the palette, or Settings -> AI ->
//! "Project memory").
//!
//! Drawing lives in `render::scene::memory`; off-thread store & config hooks live in
//! `app::memory`.

use winit::keyboard::{Key, ModifiersState, NamedKey};

use twitty_store::{
    FrequentCommand, MemoryEntry, MemoryKind, ProjectMemorySnapshot, RulesFileInfo, RulesFileState,
    AUTO_COMMAND_SOURCE, AUTO_ENV_SOURCE,
};

use crate::ui::chrome::UiLanguage;
use crate::ui::settings_panel::FocusZone;

mod layout;
pub use layout::{ExclusionRowLayout, MemoryEntryRowLayout, MemoryLayout, RulesFileRowLayout};
/// Maximum characters accepted in inline memory / exclusion input fields.
const MAX_INPUT_CHARS: usize = 512;

/// Navigation tabs in the Project Memory sidebar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MemoryTab {
    #[default]
    Rules,
    Facts,
    Commands,
    Env,
    Exclusions,
}

impl MemoryTab {
    pub const ALL: [Self; 5] = [
        Self::Rules,
        Self::Facts,
        Self::Commands,
        Self::Env,
        Self::Exclusions,
    ];

    pub fn index(self) -> usize {
        match self {
            Self::Rules => 0,
            Self::Facts => 1,
            Self::Commands => 2,
            Self::Env => 3,
            Self::Exclusions => 4,
        }
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL[index % Self::ALL.len()]
    }

    pub fn label(self, language: UiLanguage) -> &'static str {
        match self {
            Self::Rules => language.t("Rules"),
            Self::Facts => language.t("Facts"),
            Self::Commands => language.t("Commands"),
            Self::Env => language.t("Environment"),
            Self::Exclusions => language.t("Exclusions"),
        }
    }

    pub fn memory_kind(self) -> Option<MemoryKind> {
        match self {
            Self::Rules => Some(MemoryKind::Rule),
            Self::Facts => Some(MemoryKind::Fact),
            Self::Commands => Some(MemoryKind::Command),
            Self::Env => Some(MemoryKind::Env),
            Self::Exclusions => None,
        }
    }

    pub fn add_placeholder(self, language: UiLanguage) -> &'static str {
        match self {
            Self::Rules => language.t("Add a project rule…"),
            Self::Facts => language.t("Add a project fact…"),
            Self::Commands => language.t("Add a frequent command…"),
            Self::Env => language.t("Add an environment note…"),
            Self::Exclusions => language.t("Exclude glob or path (e.g. ~/secrets, **/private/**)…"),
        }
    }
}

/// Which text input inside the Memory screen is actively accepting typing/IME.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoryInputTarget {
    /// The top "Add ..." input bar of the active tab.
    AddBar,
    /// Inline editor for an existing `memory` row id.
    EditEntry(i64),
}

/// Side-effect action requested by the Memory screen for `app::memory` to execute off-thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoryAction {
    AddMemory { kind: MemoryKind, text: String },
    UpdateMemory { id: i64, text: String },
    ApproveMemory { id: i64 },
    DeleteMemory { id: i64 },
    ApproveRulesFile { rel_path: String, sha256: String },
    RevokeRulesFile { rel_path: String },
    RefreshEnv,
    AddExclusion { pattern: String },
    RemoveExclusion { index: usize },
    ForgetProject,
}

/// Result of handling one input event on [`MemoryPanelState`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryOutcome {
    pub close: bool,
    pub redraw: bool,
    pub actions: Vec<MemoryAction>,
}

/// Interactive hit targets in the Memory screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoryHit {
    Outside,
    Panel,
    CloseButton,
    ForgetProject,
    Tab(MemoryTab),
    AddInput,
    AddSubmit,
    RefreshEnv,
    ExcludeCurrentProject,
    RulesApprove(usize),
    RulesRevoke(usize),
    EntryRow(i64),
    EntryApprove(i64),
    EntryEdit(i64),
    EntryDelete(i64),
    EditInput(i64),
    EditSave(i64),
    EditCancel,
    ExclusionRemove(usize),
}

impl MemoryHit {
    pub fn is_text_input(&self) -> bool {
        matches!(self, Self::AddInput | Self::EditInput(_))
    }

    pub fn is_interactive(&self) -> bool {
        !matches!(self, Self::Outside | Self::Panel)
    }
}

/// Window-free state of the Project Memory overlay.
#[derive(Clone, Debug)]
pub struct MemoryPanelState {
    pub language: UiLanguage,
    pub tab: MemoryTab,
    pub focus: FocusZone,
    /// Focused item index within the active tab's content list.
    pub selected: usize,
    /// Vertical scroll offset in logical pixels.
    pub scroll_y: f32,
    /// Current active project root path (None if focused pane has no local cwd).
    pub project_root: Option<String>,
    /// True when the active project matches `exclusions`.
    pub excluded: bool,
    /// Loading indicator while waiting for the first off-thread snapshot.
    pub loading: bool,
    /// Inspected candidate rules files in the project root.
    pub rules_files: Vec<RulesFileInfo>,
    /// All stored memory rows for the current project.
    pub entries: Vec<MemoryEntry>,
    /// Computed frequent commands (used for run/success badges on the Commands tab).
    pub frequent_commands: Vec<FrequentCommand>,
    /// Live copy of `config.memory_exclude`.
    pub exclusions: Vec<String>,
    /// Active text input target (Add bar or inline edit of a memory entry).
    pub input_target: Option<MemoryInputTarget>,
    /// Text buffer for `input_target`.
    pub input_text: String,
    /// Two-step confirmation arm for "Forget this project".
    pub confirm_forget: bool,
    /// Last hovered hit target for hover highlights.
    pub hover: Option<MemoryHit>,
}

impl MemoryPanelState {
    pub fn new(
        project_root: Option<String>,
        exclusions: Vec<String>,
        language: UiLanguage,
    ) -> Self {
        let excluded = project_root.as_deref().is_some_and(|root| {
            twitty_store::is_path_excluded(std::path::Path::new(root), &exclusions)
        });
        Self {
            language,
            tab: MemoryTab::Rules,
            focus: FocusZone::Content,
            selected: 0,
            scroll_y: 0.0,
            project_root,
            excluded,
            loading: true,
            rules_files: Vec::new(),
            entries: Vec::new(),
            frequent_commands: Vec::new(),
            exclusions,
            input_target: None,
            input_text: String::new(),
            confirm_forget: false,
            hover: None,
        }
    }

    /// Apply a freshly loaded [`ProjectMemorySnapshot`] from the store worker.
    pub fn apply_snapshot(&mut self, snapshot: ProjectMemorySnapshot) {
        self.project_root = (!snapshot.project_root.is_empty()).then_some(snapshot.project_root);
        self.excluded = snapshot.excluded;
        self.rules_files = snapshot.rules_files;
        self.entries = snapshot.entries;
        self.frequent_commands = snapshot.frequent_commands;
        self.loading = false;
        self.clamp_selection();
    }

    /// True when an inline text field is focused and should receive typing / IME / Paste.
    pub fn accepts_text_input(&self) -> bool {
        self.input_target.is_some()
    }

    /// Entries belonging to `tab` in display order (pending facts first, then newest updated).
    pub fn entries_for_tab(&self, tab: MemoryTab) -> Vec<&MemoryEntry> {
        let Some(kind) = tab.memory_kind() else {
            return Vec::new();
        };
        let mut list: Vec<&MemoryEntry> = self.entries.iter().filter(|e| e.kind == kind).collect();
        list.sort_by(|a, b| {
            a.approved
                .cmp(&b.approved)
                .then_with(|| b.updated_at.cmp(&a.updated_at))
                .then_with(|| b.id.cmp(&a.id))
        });
        list
    }

    /// Count shown next to a sidebar tab.
    pub fn tab_count(&self, tab: MemoryTab) -> usize {
        match tab {
            MemoryTab::Rules => {
                self.rules_files.len()
                    + self
                        .entries
                        .iter()
                        .filter(|e| e.kind == MemoryKind::Rule)
                        .count()
            }
            MemoryTab::Facts => self
                .entries
                .iter()
                .filter(|e| e.kind == MemoryKind::Fact)
                .count(),
            MemoryTab::Commands => self
                .entries
                .iter()
                .filter(|e| e.kind == MemoryKind::Command)
                .count(),
            MemoryTab::Env => self
                .entries
                .iter()
                .filter(|e| e.kind == MemoryKind::Env)
                .count(),
            MemoryTab::Exclusions => self.exclusions.len(),
        }
    }

    /// True if a tab has items needing user attention (changed rules file or pending facts).
    pub fn tab_has_attention(&self, tab: MemoryTab) -> bool {
        match tab {
            MemoryTab::Rules => self
                .rules_files
                .iter()
                .any(|f| f.state == RulesFileState::Changed),
            MemoryTab::Facts => self
                .entries
                .iter()
                .any(|e| e.kind == MemoryKind::Fact && !e.approved),
            _ => false,
        }
    }

    /// Number of selectable content rows in the active tab.
    pub fn content_item_count(&self) -> usize {
        match self.tab {
            MemoryTab::Rules => {
                self.rules_files.len() + self.entries_for_tab(MemoryTab::Rules).len()
            }
            MemoryTab::Facts | MemoryTab::Commands | MemoryTab::Env => {
                self.entries_for_tab(self.tab).len()
            }
            MemoryTab::Exclusions => self.exclusions.len(),
        }
    }

    /// Switch to `tab`, resetting scroll and inline editing cleanly.
    pub fn select_tab(&mut self, tab: MemoryTab) {
        if self.tab != tab {
            self.tab = tab;
            self.selected = 0;
            self.scroll_y = 0.0;
            self.input_target = None;
            self.input_text.clear();
            self.confirm_forget = false;
        }
    }

    fn clamp_selection(&mut self) {
        let count = self.content_item_count();
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
    }

    /// Handle committed text (keyboard text or Arabic IME commit).
    pub fn on_text(&mut self, text: &str) -> MemoryOutcome {
        let mut outcome = MemoryOutcome::default();
        if self.input_target.is_none() {
            return outcome;
        }
        let mut changed = false;
        for ch in text.chars() {
            if ch.is_control() {
                continue;
            }
            if self.input_text.chars().count() >= MAX_INPUT_CHARS {
                break;
            }
            self.input_text.push(ch);
            changed = true;
        }
        if changed {
            outcome.redraw = true;
        }
        outcome
    }

    /// Handle a key press while the Memory screen is open.
    pub fn on_key(
        &mut self,
        key: &Key,
        text: Option<&str>,
        modifiers: ModifiersState,
        surface: (f32, f32),
        scale: f32,
    ) -> MemoryOutcome {
        let mut outcome = MemoryOutcome::default();
        let ctrl = modifiers.control_key();
        let alt = modifiers.alt_key();
        let super_key = modifiers.super_key();

        // Active text input handles typing, backspace, Enter (submit), and Escape (cancel).
        if let Some(target) = self.input_target.clone() {
            match key {
                Key::Named(NamedKey::Escape) => {
                    self.input_target = None;
                    self.input_text.clear();
                    outcome.redraw = true;
                    return outcome;
                }
                Key::Named(NamedKey::Enter) => {
                    self.submit_input(&target, &mut outcome);
                    return outcome;
                }
                Key::Named(NamedKey::Backspace) => {
                    if ctrl {
                        pop_last_word(&mut self.input_text);
                    } else {
                        self.input_text.pop();
                    }
                    outcome.redraw = true;
                    return outcome;
                }
                Key::Named(NamedKey::Space) if !ctrl && !alt && !super_key => {
                    return self.on_text(" ");
                }
                _ => {
                    if !ctrl && !alt && !super_key {
                        if let Some(s) = text {
                            return self.on_text(s);
                        }
                        if let Key::Character(c) = key {
                            return self.on_text(c.as_str());
                        }
                    }
                    return outcome;
                }
            }
        }

        // Escape cancels forget confirmation first, otherwise closes the Memory screen.
        if *key == Key::Named(NamedKey::Escape) {
            if self.confirm_forget {
                self.confirm_forget = false;
                outcome.redraw = true;
            } else {
                outcome.close = true;
            }
            return outcome;
        }

        // Any other key cancels forget confirmation.
        if self.confirm_forget {
            self.confirm_forget = false;
            outcome.redraw = true;
        }

        match key {
            Key::Named(NamedKey::Tab) => {
                self.focus = match self.focus {
                    FocusZone::Sidebar => FocusZone::Content,
                    FocusZone::Content => FocusZone::Sidebar,
                };
                outcome.redraw = true;
            }
            Key::Named(NamedKey::ArrowUp) => {
                if self.focus == FocusZone::Sidebar {
                    let prev = (self.tab.index() + MemoryTab::ALL.len() - 1) % MemoryTab::ALL.len();
                    self.select_tab(MemoryTab::from_index(prev));
                    outcome.redraw = true;
                } else if self.selected > 0 {
                    self.selected -= 1;
                    self.ensure_selected_visible(surface, scale);
                    outcome.redraw = true;
                }
            }
            Key::Named(NamedKey::ArrowDown) => {
                if self.focus == FocusZone::Sidebar {
                    let next = (self.tab.index() + 1) % MemoryTab::ALL.len();
                    self.select_tab(MemoryTab::from_index(next));
                    outcome.redraw = true;
                } else {
                    let count = self.content_item_count();
                    if count > 0 && self.selected + 1 < count {
                        self.selected += 1;
                        self.ensure_selected_visible(surface, scale);
                        outcome.redraw = true;
                    }
                }
            }
            Key::Named(NamedKey::ArrowLeft) | Key::Named(NamedKey::ArrowRight) => {
                let mirrored = self.language == UiLanguage::Arabic;
                let toward_content = (*key == Key::Named(NamedKey::ArrowRight)) ^ mirrored;
                self.focus = if toward_content {
                    FocusZone::Content
                } else {
                    FocusZone::Sidebar
                };
                outcome.redraw = true;
            }
            Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space) => {
                if self.focus == FocusZone::Sidebar {
                    self.focus = FocusZone::Content;
                    outcome.redraw = true;
                } else {
                    self.activate_selected(&mut outcome);
                }
            }
            Key::Named(NamedKey::Delete) | Key::Named(NamedKey::Backspace) => {
                if self.focus == FocusZone::Content {
                    self.delete_selected(&mut outcome);
                }
            }
            Key::Character(c) if !ctrl && !alt && !super_key => match c.as_str() {
                "a" | "A" | "+" => {
                    if self.can_add_in_active_tab() {
                        self.input_target = Some(MemoryInputTarget::AddBar);
                        self.input_text.clear();
                        outcome.redraw = true;
                    }
                }
                "e" | "E" => {
                    if self.focus == FocusZone::Content {
                        self.begin_edit_selected(&mut outcome);
                    }
                }
                "r" | "R" if self.tab == MemoryTab::Env && self.can_mutate_project() => {
                    outcome.actions.push(MemoryAction::RefreshEnv);
                    outcome.redraw = true;
                }
                _ => {}
            },
            _ => {}
        }

        outcome
    }

    fn can_mutate_project(&self) -> bool {
        self.project_root.is_some() && !self.excluded
    }

    fn can_add_in_active_tab(&self) -> bool {
        match self.tab {
            MemoryTab::Exclusions => true,
            _ => self.can_mutate_project(),
        }
    }

    fn submit_input(&mut self, target: &MemoryInputTarget, outcome: &mut MemoryOutcome) {
        let trimmed = self.input_text.trim().to_owned();
        if trimmed.is_empty() {
            self.input_target = None;
            self.input_text.clear();
            outcome.redraw = true;
            return;
        }
        match target {
            MemoryInputTarget::AddBar => {
                if self.tab == MemoryTab::Exclusions {
                    if !self.exclusions.iter().any(|p| p == &trimmed) {
                        self.exclusions.push(trimmed.clone());
                        if let Some(ref root) = self.project_root {
                            self.excluded = twitty_store::is_path_excluded(
                                std::path::Path::new(root),
                                &self.exclusions,
                            );
                        }
                        outcome
                            .actions
                            .push(MemoryAction::AddExclusion { pattern: trimmed });
                    }
                } else if let Some(kind) = self.tab.memory_kind() {
                    if self.can_mutate_project() {
                        outcome.actions.push(MemoryAction::AddMemory {
                            kind,
                            text: trimmed,
                        });
                    }
                }
            }
            MemoryInputTarget::EditEntry(id) => {
                if let Some(entry) = self.entries.iter_mut().find(|e| e.id == *id) {
                    entry.text = trimmed.clone();
                }
                outcome.actions.push(MemoryAction::UpdateMemory {
                    id: *id,
                    text: trimmed,
                });
            }
        }
        self.input_target = None;
        self.input_text.clear();
        outcome.redraw = true;
    }

    fn activate_selected(&mut self, outcome: &mut MemoryOutcome) {
        match self.tab {
            MemoryTab::Rules => {
                if self.selected < self.rules_files.len() {
                    let file = &self.rules_files[self.selected];
                    match file.state {
                        RulesFileState::Approved => {
                            outcome.actions.push(MemoryAction::RevokeRulesFile {
                                rel_path: file.rel_path.clone(),
                            });
                        }
                        RulesFileState::Unapproved | RulesFileState::Changed => {
                            outcome.actions.push(MemoryAction::ApproveRulesFile {
                                rel_path: file.rel_path.clone(),
                                sha256: file.current_sha256.clone(),
                            });
                        }
                    }
                    outcome.redraw = true;
                } else {
                    let idx = self.selected - self.rules_files.len();
                    let found = self
                        .entries_for_tab(MemoryTab::Rules)
                        .get(idx)
                        .map(|e| (e.id, e.text.clone()));
                    if let Some((id, text)) = found {
                        self.input_target = Some(MemoryInputTarget::EditEntry(id));
                        self.input_text = text;
                        outcome.redraw = true;
                    }
                }
            }
            MemoryTab::Facts => {
                let found = self
                    .entries_for_tab(MemoryTab::Facts)
                    .get(self.selected)
                    .map(|e| (e.id, e.approved, e.text.clone()));
                if let Some((id, approved, text)) = found {
                    if !approved {
                        if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
                            e.approved = true;
                        }
                        outcome.actions.push(MemoryAction::ApproveMemory { id });
                    } else {
                        self.input_target = Some(MemoryInputTarget::EditEntry(id));
                        self.input_text = text;
                    }
                    outcome.redraw = true;
                }
            }
            MemoryTab::Commands | MemoryTab::Env => {
                let found = self
                    .entries_for_tab(self.tab)
                    .get(self.selected)
                    .map(|e| (e.id, e.text.clone()));
                if let Some((id, text)) = found {
                    self.input_target = Some(MemoryInputTarget::EditEntry(id));
                    self.input_text = text;
                    outcome.redraw = true;
                }
            }
            MemoryTab::Exclusions => {
                // Enter on an exclusion item does nothing destructive; use Delete or Remove button.
            }
        }
    }

    fn begin_edit_selected(&mut self, outcome: &mut MemoryOutcome) {
        let entry = match self.tab {
            MemoryTab::Rules => {
                if self.selected < self.rules_files.len() {
                    None
                } else {
                    self.entries_for_tab(MemoryTab::Rules)
                        .get(self.selected - self.rules_files.len())
                        .map(|e| (e.id, e.text.clone()))
                }
            }
            MemoryTab::Facts | MemoryTab::Commands | MemoryTab::Env => self
                .entries_for_tab(self.tab)
                .get(self.selected)
                .map(|e| (e.id, e.text.clone())),
            MemoryTab::Exclusions => None,
        };
        if let Some((id, text)) = entry {
            self.input_target = Some(MemoryInputTarget::EditEntry(id));
            self.input_text = text;
            outcome.redraw = true;
        }
    }

    fn delete_selected(&mut self, outcome: &mut MemoryOutcome) {
        match self.tab {
            MemoryTab::Rules => {
                if self.selected < self.rules_files.len() {
                    let file = &self.rules_files[self.selected];
                    if file.state == RulesFileState::Approved {
                        outcome.actions.push(MemoryAction::RevokeRulesFile {
                            rel_path: file.rel_path.clone(),
                        });
                        outcome.redraw = true;
                    }
                } else {
                    let idx = self.selected - self.rules_files.len();
                    if let Some(id) = self
                        .entries_for_tab(MemoryTab::Rules)
                        .get(idx)
                        .map(|e| e.id)
                    {
                        self.entries.retain(|e| e.id != id);
                        self.clamp_selection();
                        outcome.actions.push(MemoryAction::DeleteMemory { id });
                        outcome.redraw = true;
                    }
                }
            }
            MemoryTab::Facts | MemoryTab::Commands | MemoryTab::Env => {
                if let Some(id) = self
                    .entries_for_tab(self.tab)
                    .get(self.selected)
                    .map(|e| e.id)
                {
                    self.entries.retain(|e| e.id != id);
                    self.clamp_selection();
                    outcome.actions.push(MemoryAction::DeleteMemory { id });
                    outcome.redraw = true;
                }
            }
            MemoryTab::Exclusions => {
                if self.selected < self.exclusions.len() {
                    let index = self.selected;
                    self.exclusions.remove(index);
                    if let Some(ref root) = self.project_root {
                        self.excluded = twitty_store::is_path_excluded(
                            std::path::Path::new(root),
                            &self.exclusions,
                        );
                    }
                    self.clamp_selection();
                    outcome
                        .actions
                        .push(MemoryAction::RemoveExclusion { index });
                    outcome.redraw = true;
                }
            }
        }
    }

    fn ensure_selected_visible(&mut self, surface: (f32, f32), scale: f32) {
        let layout = MemoryLayout::build(surface, scale, self);
        let vp = layout.viewport;
        if let Some(rect) = layout.selected_rect(self.selected) {
            if rect.y < vp.y {
                self.scroll_y = (self.scroll_y - (vp.y - rect.y) / scale).max(0.0);
            } else if rect.bottom() > vp.bottom() {
                self.scroll_y =
                    (self.scroll_y + (rect.bottom() - vp.bottom()) / scale).min(layout.max_scroll);
            }
        }
    }

    pub fn on_mouse_move(
        &mut self,
        x: f64,
        y: f64,
        surface: (f32, f32),
        scale: f32,
    ) -> MemoryOutcome {
        let mut outcome = MemoryOutcome::default();
        let hit = MemoryLayout::build(surface, scale, self).hit(x, y);
        let next = (!matches!(hit, MemoryHit::Outside | MemoryHit::Panel)).then_some(hit);
        if self.hover != next {
            self.hover = next;
            outcome.redraw = true;
        }
        outcome
    }

    pub fn on_mouse_leave(&mut self) -> MemoryOutcome {
        let mut outcome = MemoryOutcome::default();
        if self.hover.take().is_some() {
            outcome.redraw = true;
        }
        outcome
    }

    pub fn on_mouse_down(
        &mut self,
        x: f64,
        y: f64,
        surface: (f32, f32),
        scale: f32,
    ) -> MemoryOutcome {
        let mut outcome = MemoryOutcome::default();
        let layout = MemoryLayout::build(surface, scale, self);
        let hit = layout.hit(x, y);

        if !matches!(hit, MemoryHit::ForgetProject) && self.confirm_forget {
            self.confirm_forget = false;
            outcome.redraw = true;
        }

        match hit {
            MemoryHit::Outside | MemoryHit::CloseButton => {
                outcome.close = true;
            }
            MemoryHit::Panel => {}
            MemoryHit::ForgetProject => {
                if self.project_root.is_some() {
                    if self.confirm_forget {
                        self.confirm_forget = false;
                        self.entries.clear();
                        for file in &mut self.rules_files {
                            file.approved_sha256 = None;
                            file.approved_content = None;
                            file.state = RulesFileState::Unapproved;
                        }
                        self.frequent_commands.clear();
                        self.selected = 0;
                        self.scroll_y = 0.0;
                        self.input_target = None;
                        self.input_text.clear();
                        outcome.actions.push(MemoryAction::ForgetProject);
                        outcome.redraw = true;
                    } else {
                        self.confirm_forget = true;
                        outcome.redraw = true;
                    }
                }
            }
            MemoryHit::Tab(tab) => {
                self.focus = FocusZone::Sidebar;
                self.select_tab(tab);
                outcome.redraw = true;
            }
            MemoryHit::AddInput => {
                if self.can_add_in_active_tab() {
                    if self.input_target != Some(MemoryInputTarget::AddBar) {
                        self.input_target = Some(MemoryInputTarget::AddBar);
                        self.input_text.clear();
                    }
                    self.focus = FocusZone::Content;
                    outcome.redraw = true;
                }
            }
            MemoryHit::AddSubmit => {
                if self.input_target == Some(MemoryInputTarget::AddBar) {
                    self.submit_input(&MemoryInputTarget::AddBar, &mut outcome);
                } else if self.can_add_in_active_tab() {
                    self.input_target = Some(MemoryInputTarget::AddBar);
                    self.input_text.clear();
                    outcome.redraw = true;
                }
            }
            MemoryHit::RefreshEnv => {
                if self.can_mutate_project() {
                    outcome.actions.push(MemoryAction::RefreshEnv);
                    outcome.redraw = true;
                }
            }
            MemoryHit::ExcludeCurrentProject => {
                if let Some(root) = self.project_root.clone() {
                    if !self.exclusions.iter().any(|p| p == &root) {
                        self.exclusions.push(root.clone());
                        self.excluded = true;
                        outcome
                            .actions
                            .push(MemoryAction::AddExclusion { pattern: root });
                        outcome.redraw = true;
                    }
                }
            }
            MemoryHit::RulesApprove(idx) => {
                if let Some(file) = self.rules_files.get_mut(idx) {
                    let rel_path = file.rel_path.clone();
                    let sha256 = file.current_sha256.clone();
                    file.approved_sha256 = Some(sha256.clone());
                    file.state = RulesFileState::Approved;
                    outcome
                        .actions
                        .push(MemoryAction::ApproveRulesFile { rel_path, sha256 });
                    outcome.redraw = true;
                }
            }
            MemoryHit::RulesRevoke(idx) => {
                if let Some(file) = self.rules_files.get_mut(idx) {
                    let rel_path = file.rel_path.clone();
                    file.approved_sha256 = None;
                    file.approved_content = None;
                    file.state = RulesFileState::Unapproved;
                    outcome
                        .actions
                        .push(MemoryAction::RevokeRulesFile { rel_path });
                    outcome.redraw = true;
                }
            }
            MemoryHit::EntryRow(id) => {
                self.focus = FocusZone::Content;
                if let Some(idx) = self.row_index_for_entry_id(id) {
                    self.selected = idx;
                }
                outcome.redraw = true;
            }
            MemoryHit::EntryApprove(id) => {
                if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
                    entry.approved = true;
                }
                outcome.actions.push(MemoryAction::ApproveMemory { id });
                outcome.redraw = true;
            }
            MemoryHit::EntryEdit(id) => {
                if let Some(entry) = self.entries.iter().find(|e| e.id == id) {
                    self.input_target = Some(MemoryInputTarget::EditEntry(id));
                    self.input_text = entry.text.clone();
                    self.focus = FocusZone::Content;
                    outcome.redraw = true;
                }
            }
            MemoryHit::EntryDelete(id) => {
                self.entries.retain(|e| e.id != id);
                if self.input_target == Some(MemoryInputTarget::EditEntry(id)) {
                    self.input_target = None;
                    self.input_text.clear();
                }
                self.clamp_selection();
                outcome.actions.push(MemoryAction::DeleteMemory { id });
                outcome.redraw = true;
            }
            MemoryHit::EditInput(_) => {
                outcome.redraw = true;
            }
            MemoryHit::EditSave(id) => {
                self.submit_input(&MemoryInputTarget::EditEntry(id), &mut outcome);
            }
            MemoryHit::EditCancel => {
                self.input_target = None;
                self.input_text.clear();
                outcome.redraw = true;
            }
            MemoryHit::ExclusionRemove(index) => {
                if index < self.exclusions.len() {
                    self.exclusions.remove(index);
                    if let Some(ref root) = self.project_root {
                        self.excluded = twitty_store::is_path_excluded(
                            std::path::Path::new(root),
                            &self.exclusions,
                        );
                    }
                    self.clamp_selection();
                    outcome
                        .actions
                        .push(MemoryAction::RemoveExclusion { index });
                    outcome.redraw = true;
                }
            }
        }
        outcome
    }

    pub fn on_wheel(
        &mut self,
        x: f64,
        y: f64,
        lines: f64,
        surface: (f32, f32),
        scale: f32,
    ) -> MemoryOutcome {
        let mut outcome = MemoryOutcome::default();
        let layout = MemoryLayout::build(surface, scale, self);
        if !layout.panel.contains(x, y) || layout.max_scroll <= 0.0 {
            return outcome;
        }
        let next = (self.scroll_y - lines as f32 * 28.0).clamp(0.0, layout.max_scroll);
        if (next - self.scroll_y).abs() > 0.5 {
            self.scroll_y = next;
            outcome.redraw = true;
        }
        outcome
    }

    fn row_index_for_entry_id(&self, id: i64) -> Option<usize> {
        let offset = if self.tab == MemoryTab::Rules {
            self.rules_files.len()
        } else {
            0
        };
        self.entries_for_tab(self.tab)
            .iter()
            .position(|e| e.id == id)
            .map(|i| i + offset)
    }

    /// Look up frequent-command run stats matching a command memory entry, if any.
    pub fn frequent_stats_for<'a>(&'a self, entry: &MemoryEntry) -> Option<&'a FrequentCommand> {
        if entry.kind != MemoryKind::Command {
            return None;
        }
        self.frequent_commands
            .iter()
            .find(|fc| entry.text.contains(&fc.command))
    }

    /// Short human-friendly source label for a memory entry.
    pub fn source_badge(&self, entry: &MemoryEntry) -> &'static str {
        if entry.source == AUTO_ENV_SOURCE {
            self.language.t("Detected")
        } else if entry.source == AUTO_COMMAND_SOURCE {
            self.language.t("History")
        } else if entry.source == "agent" {
            self.language.t("Agent")
        } else {
            self.language.t("User")
        }
    }
}

fn pop_last_word(s: &mut String) {
    while s.ends_with(char::is_whitespace) {
        s.pop();
    }
    while let Some(ch) = s.chars().next_back() {
        if ch.is_whitespace() {
            break;
        }
        s.pop();
    }
}
