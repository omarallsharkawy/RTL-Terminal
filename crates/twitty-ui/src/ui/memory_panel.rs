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

use crate::layout::Rect;
use crate::ui::chrome::UiLanguage;
use crate::ui::settings_panel::FocusZone;

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

// ---- Physical-pixel geometry shared by hit testing and rendering ----

#[derive(Clone, Debug)]
pub struct RulesFileRowLayout {
    pub index: usize,
    pub rect: Rect,
    pub title_rect: Rect,
    pub hash_rect: Rect,
    pub badge_rect: Rect,
    pub approve_btn: Option<Rect>,
    pub revoke_btn: Option<Rect>,
}

#[derive(Clone, Debug)]
pub struct MemoryEntryRowLayout {
    pub id: i64,
    pub list_index: usize,
    pub rect: Rect,
    pub text_rect: Rect,
    pub meta_rect: Rect,
    pub badge_rect: Option<Rect>,
    pub approve_btn: Option<Rect>,
    pub edit_btn: Option<Rect>,
    pub delete_btn: Option<Rect>,
    pub edit_input: Option<Rect>,
    pub save_btn: Option<Rect>,
    pub cancel_btn: Option<Rect>,
}

#[derive(Clone, Debug)]
pub struct ExclusionRowLayout {
    pub index: usize,
    pub rect: Rect,
    pub text_rect: Rect,
    pub remove_btn: Rect,
}

#[derive(Clone, Debug)]
pub struct MemoryLayout {
    pub mirrored: bool,
    pub panel: Rect,
    pub header: Rect,
    pub header_title: Rect,
    pub header_subtitle: Rect,
    pub excluded_badge: Option<Rect>,
    pub forget_button: Option<Rect>,
    pub close_button: Rect,
    pub sidebar: Rect,
    pub divider: Rect,
    pub tabs: Vec<(MemoryTab, Rect)>,
    pub viewport: Rect,
    pub add_input: Option<Rect>,
    pub add_submit: Option<Rect>,
    pub refresh_button: Option<Rect>,
    pub exclude_current_btn: Option<Rect>,
    pub excluded_banner: Option<Rect>,
    pub rules_heading: Option<Rect>,
    pub rules_rows: Vec<RulesFileRowLayout>,
    pub user_rules_heading: Option<Rect>,
    pub entry_rows: Vec<MemoryEntryRowLayout>,
    pub exclusion_rows: Vec<ExclusionRowLayout>,
    pub max_scroll: f32,
    pub scrollbar: Option<Rect>,
}

impl MemoryLayout {
    pub fn build(surface: (f32, f32), scale: f32, state: &MemoryPanelState) -> Self {
        let px = |v: f32| (v * scale).round();
        let mirrored = state.language == UiLanguage::Arabic;
        let (sw, sh) = surface;

        let margin = px(24.0);
        let max_w = (sw - 2.0 * margin).max(0.0);
        let max_h = (sh - 2.0 * margin).max(0.0);
        let panel_w = max_w.min(px(840.0)).max(max_w.min(px(360.0)));
        let panel_h = max_h.min(px(600.0)).max(max_h.min(px(300.0)));
        let panel_x = ((sw - panel_w) / 2.0).round().max(0.0);
        let panel_y = ((sh - panel_h) / 2.0).round().max(0.0);
        let panel = Rect::new(panel_x, panel_y, panel_w, panel_h);

        let header_h = px(56.0).min(panel_h);
        let header = Rect::new(panel.x, panel.y, panel.w, header_h);
        let pad_x = px(16.0);
        let close_side = px(28.0);
        let btn_h = px(28.0);
        let close_y = header.y + ((header.h - close_side) / 2.0).round();
        let forget_w = if state.project_root.is_some() {
            px(148.0)
        } else {
            0.0
        };
        let gap = px(8.0);

        let (close_button, forget_button, title_x, title_w) = if mirrored {
            let cb = Rect::new(header.x + pad_x, close_y, close_side, close_side);
            let fb =
                (forget_w > 0.0).then(|| Rect::new(cb.right() + gap, close_y, forget_w, btn_h));
            let left_bound = fb.map_or(cb.right(), |r| r.right()) + px(12.0);
            let w = (header.right() - pad_x - left_bound).max(0.0);
            (cb, fb, left_bound, w)
        } else {
            let cb = Rect::new(
                header.right() - pad_x - close_side,
                close_y,
                close_side,
                close_side,
            );
            let fb = (forget_w > 0.0)
                .then(|| Rect::new(cb.x - gap - forget_w, close_y, forget_w, btn_h));
            let right_bound = fb.map_or(cb.x, |r| r.x) - px(12.0);
            let tx = header.x + pad_x;
            let w = (right_bound - tx).max(0.0);
            (cb, fb, tx, w)
        };

        let header_title = Rect::new(title_x, header.y + px(8.0), title_w, px(22.0));
        let badge_w = px(74.0);
        let (header_subtitle, excluded_badge) = if state.excluded && title_w > badge_w + px(40.0) {
            let sy = header_title.bottom() + px(2.0);
            let sh_row = px(18.0);
            if mirrored {
                let badge = Rect::new(title_x, sy, badge_w, sh_row);
                let sub = Rect::new(
                    badge.right() + px(8.0),
                    sy,
                    (title_w - badge_w - px(8.0)).max(0.0),
                    sh_row,
                );
                (sub, Some(badge))
            } else {
                let badge = Rect::new(title_x + title_w - badge_w, sy, badge_w, sh_row);
                let sub = Rect::new(title_x, sy, (title_w - badge_w - px(8.0)).max(0.0), sh_row);
                (sub, Some(badge))
            }
        } else {
            (
                Rect::new(title_x, header_title.bottom() + px(2.0), title_w, px(18.0)),
                None,
            )
        };

        let body_y = header.bottom();
        let body_h = (panel.bottom() - body_y).max(0.0);
        let sidebar_w = px(184.0).min((panel.w * 0.30).round());
        let div_w = px(1.0).max(1.0);

        let (sidebar, divider, content_area) = if mirrored {
            let sb = Rect::new(panel.right() - sidebar_w, body_y, sidebar_w, body_h);
            let dv = Rect::new(sb.x - div_w, body_y, div_w, body_h);
            let ca = Rect::new(panel.x, body_y, (dv.x - panel.x).max(0.0), body_h);
            (sb, dv, ca)
        } else {
            let sb = Rect::new(panel.x, body_y, sidebar_w, body_h);
            let dv = Rect::new(sb.right(), body_y, div_w, body_h);
            let ca = Rect::new(
                dv.right(),
                body_y,
                (panel.right() - dv.right()).max(0.0),
                body_h,
            );
            (sb, dv, ca)
        };

        // Sidebar tabs.
        let mut tabs = Vec::with_capacity(MemoryTab::ALL.len());
        let tab_pad_x = px(10.0);
        let tab_h = px(36.0);
        let tab_gap = px(4.0);
        let mut ty = sidebar.y + px(12.0);
        for tab in MemoryTab::ALL {
            let r = Rect::new(
                sidebar.x + tab_pad_x,
                ty,
                (sidebar.w - 2.0 * tab_pad_x).max(0.0),
                tab_h,
            );
            tabs.push((tab, r));
            ty += tab_h + tab_gap;
        }

        // Content top bar (Add input / Refresh / Excluded banner) + scrollable viewport.
        let inner_pad_x = px(16.0);
        let inner_pad_y = px(12.0);
        let scrollbar_reserve = px(10.0);
        let content_x = content_area.x + inner_pad_x;
        let content_w = (content_area.w - 2.0 * inner_pad_x - scrollbar_reserve).max(0.0);
        let mut top_cursor = content_area.y + inner_pad_y;

        let mut add_input = None;
        let mut add_submit = None;
        let mut refresh_button = None;
        let mut exclude_current_btn = None;
        let mut excluded_banner = None;

        if state.tab == MemoryTab::Exclusions {
            let bar_h = px(34.0);
            let add_w = px(78.0);
            let (inp, btn) = split_bar(
                content_x, top_cursor, content_w, bar_h, add_w, gap, mirrored,
            );
            add_input = Some(inp);
            add_submit = Some(btn);
            top_cursor += bar_h + px(8.0);

            if state.project_root.is_some() && !state.excluded {
                let ex_h = px(28.0);
                let ex_w = px(176.0).min(content_w);
                let ex_x = if mirrored {
                    content_x + content_w - ex_w
                } else {
                    content_x
                };
                exclude_current_btn = Some(Rect::new(ex_x, top_cursor, ex_w, ex_h));
                top_cursor += ex_h + px(10.0);
            }
        } else if state.excluded {
            let banner_h = px(40.0);
            excluded_banner = Some(Rect::new(content_x, top_cursor, content_w, banner_h));
            top_cursor += banner_h + px(10.0);
        } else if state.project_root.is_some() {
            let bar_h = px(34.0);
            let add_w = px(78.0);
            let ref_w = if state.tab == MemoryTab::Env {
                px(86.0)
            } else {
                0.0
            };
            if ref_w > 0.0 {
                let total_btns = add_w + gap + ref_w;
                let (inp, btns_rect) = split_bar(
                    content_x, top_cursor, content_w, bar_h, total_btns, gap, mirrored,
                );
                let (a_btn, r_btn) = if mirrored {
                    let r = Rect::new(btns_rect.x, top_cursor, ref_w, bar_h);
                    let a = Rect::new(r.right() + gap, top_cursor, add_w, bar_h);
                    (a, r)
                } else {
                    let a = Rect::new(btns_rect.x, top_cursor, add_w, bar_h);
                    let r = Rect::new(a.right() + gap, top_cursor, ref_w, bar_h);
                    (a, r)
                };
                add_input = Some(inp);
                add_submit = Some(a_btn);
                refresh_button = Some(r_btn);
            } else {
                let (inp, btn) = split_bar(
                    content_x, top_cursor, content_w, bar_h, add_w, gap, mirrored,
                );
                add_input = Some(inp);
                add_submit = Some(btn);
            }
            top_cursor += bar_h + px(10.0);
        }

        let viewport = Rect::new(
            content_x,
            top_cursor,
            content_w,
            (content_area.bottom() - inner_pad_y - top_cursor).max(0.0),
        );

        let scroll_px = (state.scroll_y * scale).round();
        let mut cursor_y = viewport.y - scroll_px;
        let row_gap = px(8.0);
        let heading_h = px(20.0);

        let mut rules_heading = None;
        let mut rules_rows = Vec::new();
        let mut user_rules_heading = None;
        let mut entry_rows = Vec::new();
        let mut exclusion_rows = Vec::new();

        match state.tab {
            MemoryTab::Rules => {
                rules_heading = Some(Rect::new(viewport.x, cursor_y, viewport.w, heading_h));
                cursor_y += heading_h + px(6.0);

                for (idx, file) in state.rules_files.iter().enumerate() {
                    let h = px(56.0);
                    let rect = Rect::new(viewport.x, cursor_y, viewport.w, h);
                    rules_rows.push(layout_rules_file_row(idx, rect, file, scale, mirrored));
                    cursor_y += h + row_gap;
                }
                if state.rules_files.is_empty() {
                    cursor_y += px(24.0) + row_gap;
                }

                cursor_y += px(4.0);
                user_rules_heading = Some(Rect::new(viewport.x, cursor_y, viewport.w, heading_h));
                cursor_y += heading_h + px(6.0);

                let offset = state.rules_files.len();
                for (i, entry) in state.entries_for_tab(MemoryTab::Rules).iter().enumerate() {
                    let editing =
                        state.input_target == Some(MemoryInputTarget::EditEntry(entry.id));
                    let h = px(if editing { 64.0 } else { 58.0 });
                    let rect = Rect::new(viewport.x, cursor_y, viewport.w, h);
                    entry_rows.push(layout_entry_row(
                        entry,
                        offset + i,
                        rect,
                        editing,
                        false,
                        scale,
                        mirrored,
                    ));
                    cursor_y += h + row_gap;
                }
            }
            MemoryTab::Facts | MemoryTab::Commands | MemoryTab::Env => {
                for (i, entry) in state.entries_for_tab(state.tab).iter().enumerate() {
                    let editing =
                        state.input_target == Some(MemoryInputTarget::EditEntry(entry.id));
                    let show_approve = state.tab == MemoryTab::Facts && !entry.approved;
                    let h = px(if editing { 64.0 } else { 58.0 });
                    let rect = Rect::new(viewport.x, cursor_y, viewport.w, h);
                    entry_rows.push(layout_entry_row(
                        entry,
                        i,
                        rect,
                        editing,
                        show_approve,
                        scale,
                        mirrored,
                    ));
                    cursor_y += h + row_gap;
                }
            }
            MemoryTab::Exclusions => {
                for (index, _) in state.exclusions.iter().enumerate() {
                    let h = px(44.0);
                    let rect = Rect::new(viewport.x, cursor_y, viewport.w, h);
                    let pad = px(12.0);
                    let rm_w = px(74.0);
                    let rm_h = px(26.0);
                    let rm_y = rect.y + ((rect.h - rm_h) / 2.0).round();
                    let (text_rect, remove_btn) = if mirrored {
                        let rb = Rect::new(rect.x + pad, rm_y, rm_w, rm_h);
                        let tr = Rect::new(
                            rb.right() + px(10.0),
                            rect.y,
                            (rect.right() - pad - (rb.right() + px(10.0))).max(0.0),
                            rect.h,
                        );
                        (tr, rb)
                    } else {
                        let rb = Rect::new(rect.right() - pad - rm_w, rm_y, rm_w, rm_h);
                        let tr = Rect::new(
                            rect.x + pad,
                            rect.y,
                            (rb.x - px(10.0) - (rect.x + pad)).max(0.0),
                            rect.h,
                        );
                        (tr, rb)
                    };
                    exclusion_rows.push(ExclusionRowLayout {
                        index,
                        rect,
                        text_rect,
                        remove_btn,
                    });
                    cursor_y += h + row_gap;
                }
            }
        }

        let content_total_h = (cursor_y + scroll_px - viewport.y).max(0.0);
        let max_scroll_px = (content_total_h - viewport.h).max(0.0);
        let max_scroll = if scale > 0.0 {
            max_scroll_px / scale
        } else {
            0.0
        };

        let scrollbar = (max_scroll_px > 1.0 && viewport.h > px(24.0)).then(|| {
            let sb_w = px(4.0).max(2.0);
            let sb_x = content_area.right() - px(8.0) - sb_w;
            let thumb_h = ((viewport.h / content_total_h) * viewport.h)
                .max(px(24.0))
                .min(viewport.h);
            let ratio = (scroll_px / max_scroll_px).clamp(0.0, 1.0);
            let thumb_y = viewport.y + ratio * (viewport.h - thumb_h);
            Rect::new(sb_x, thumb_y, sb_w, thumb_h)
        });

        Self {
            mirrored,
            panel,
            header,
            header_title,
            header_subtitle,
            excluded_badge,
            forget_button,
            close_button,
            sidebar,
            divider,
            tabs,
            viewport,
            add_input,
            add_submit,
            refresh_button,
            exclude_current_btn,
            excluded_banner,
            rules_heading,
            rules_rows,
            user_rules_heading,
            entry_rows,
            exclusion_rows,
            max_scroll,
            scrollbar,
        }
    }

    pub fn selected_rect(&self, selected: usize) -> Option<Rect> {
        if let Some(row) = self.rules_rows.iter().find(|r| r.index == selected) {
            return Some(row.rect);
        }
        if let Some(row) = self.entry_rows.iter().find(|r| r.list_index == selected) {
            return Some(row.rect);
        }
        self.exclusion_rows
            .iter()
            .find(|r| r.index == selected)
            .map(|r| r.rect)
    }

    /// Hit-test physical pixel `(x, y)` against all interactive elements.
    pub fn hit(&self, x: f64, y: f64) -> MemoryHit {
        if !self.panel.contains(x, y) {
            return MemoryHit::Outside;
        }
        if self.close_button.contains(x, y) {
            return MemoryHit::CloseButton;
        }
        if let Some(fb) = self.forget_button {
            if fb.contains(x, y) {
                return MemoryHit::ForgetProject;
            }
        }
        for &(tab, rect) in &self.tabs {
            if rect.contains(x, y) {
                return MemoryHit::Tab(tab);
            }
        }
        if let Some(inp) = self.add_input {
            if inp.contains(x, y) {
                return MemoryHit::AddInput;
            }
        }
        if let Some(btn) = self.add_submit {
            if btn.contains(x, y) {
                return MemoryHit::AddSubmit;
            }
        }
        if let Some(btn) = self.refresh_button {
            if btn.contains(x, y) {
                return MemoryHit::RefreshEnv;
            }
        }
        if let Some(btn) = self.exclude_current_btn {
            if btn.contains(x, y) {
                return MemoryHit::ExcludeCurrentProject;
            }
        }

        if self.viewport.contains(x, y) {
            for row in &self.rules_rows {
                if !row.rect.contains(x, y) {
                    continue;
                }
                if let Some(btn) = row.approve_btn {
                    if btn.contains(x, y) {
                        return MemoryHit::RulesApprove(row.index);
                    }
                }
                if let Some(btn) = row.revoke_btn {
                    if btn.contains(x, y) {
                        return MemoryHit::RulesRevoke(row.index);
                    }
                }
            }
            for row in &self.entry_rows {
                if !row.rect.contains(x, y) {
                    continue;
                }
                if let Some(btn) = row.save_btn {
                    if btn.contains(x, y) {
                        return MemoryHit::EditSave(row.id);
                    }
                }
                if let Some(btn) = row.cancel_btn {
                    if btn.contains(x, y) {
                        return MemoryHit::EditCancel;
                    }
                }
                if let Some(inp) = row.edit_input {
                    if inp.contains(x, y) {
                        return MemoryHit::EditInput(row.id);
                    }
                }
                if let Some(btn) = row.approve_btn {
                    if btn.contains(x, y) {
                        return MemoryHit::EntryApprove(row.id);
                    }
                }
                if let Some(btn) = row.edit_btn {
                    if btn.contains(x, y) {
                        return MemoryHit::EntryEdit(row.id);
                    }
                }
                if let Some(btn) = row.delete_btn {
                    if btn.contains(x, y) {
                        return MemoryHit::EntryDelete(row.id);
                    }
                }
                return MemoryHit::EntryRow(row.id);
            }
            for row in &self.exclusion_rows {
                if row.remove_btn.contains(x, y) {
                    return MemoryHit::ExclusionRemove(row.index);
                }
            }
        }

        MemoryHit::Panel
    }
}

fn split_bar(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    trailing_w: f32,
    gap: f32,
    mirrored: bool,
) -> (Rect, Rect) {
    let tw = trailing_w.min(w);
    let inp_w = (w - tw - gap).max(0.0);
    if mirrored {
        (Rect::new(x + tw + gap, y, inp_w, h), Rect::new(x, y, tw, h))
    } else {
        (Rect::new(x, y, inp_w, h), Rect::new(x + w - tw, y, tw, h))
    }
}

fn layout_rules_file_row(
    index: usize,
    rect: Rect,
    file: &RulesFileInfo,
    scale: f32,
    mirrored: bool,
) -> RulesFileRowLayout {
    let px = |v: f32| (v * scale).round();
    let pad = px(12.0);
    let gap = px(6.0);
    let btn_h = px(26.0);
    let btn_y = rect.y + ((rect.h - btn_h) / 2.0).round();
    let badge_w = px(88.0);
    let badge_h = px(22.0);
    let badge_y = rect.y + ((rect.h - badge_h) / 2.0).round();

    let (approve_w, revoke_w) = match file.state {
        RulesFileState::Approved => (0.0, px(70.0)),
        RulesFileState::Unapproved => (px(78.0), 0.0),
        RulesFileState::Changed => (px(94.0), px(68.0)),
    };

    let (title_x, title_w, badge_rect, approve_btn, revoke_btn) = if mirrored {
        let mut cursor = rect.x + pad;
        let rev = (revoke_w > 0.0).then(|| {
            let r = Rect::new(cursor, btn_y, revoke_w, btn_h);
            cursor = r.right() + gap;
            r
        });
        let app = (approve_w > 0.0).then(|| {
            let r = Rect::new(cursor, btn_y, approve_w, btn_h);
            cursor = r.right() + gap;
            r
        });
        let badge = Rect::new(cursor, badge_y, badge_w, badge_h);
        cursor = badge.right() + px(10.0);
        let tw = (rect.right() - pad - cursor).max(0.0);
        (cursor, tw, badge, app, rev)
    } else {
        let mut cursor = rect.right() - pad;
        let rev = (revoke_w > 0.0).then(|| {
            cursor -= revoke_w;
            let r = Rect::new(cursor, btn_y, revoke_w, btn_h);
            cursor -= gap;
            r
        });
        let app = (approve_w > 0.0).then(|| {
            cursor -= approve_w;
            let r = Rect::new(cursor, btn_y, approve_w, btn_h);
            cursor -= gap;
            r
        });
        cursor -= badge_w;
        let badge = Rect::new(cursor, badge_y, badge_w, badge_h);
        let tx = rect.x + pad;
        let tw = (badge.x - px(10.0) - tx).max(0.0);
        (tx, tw, badge, app, rev)
    };

    let title_rect = Rect::new(title_x, rect.y + px(8.0), title_w, px(20.0));
    let hash_rect = Rect::new(title_x, title_rect.bottom() + px(2.0), title_w, px(16.0));

    RulesFileRowLayout {
        index,
        rect,
        title_rect,
        hash_rect,
        badge_rect,
        approve_btn,
        revoke_btn,
    }
}

fn layout_entry_row(
    entry: &MemoryEntry,
    list_index: usize,
    rect: Rect,
    editing: bool,
    show_approve: bool,
    scale: f32,
    mirrored: bool,
) -> MemoryEntryRowLayout {
    let px = |v: f32| (v * scale).round();
    let pad = px(12.0);
    let gap = px(6.0);
    let btn_h = px(26.0);
    let btn_y = rect.y + ((rect.h - btn_h) / 2.0).round();

    if editing {
        let save_w = px(66.0);
        let cancel_w = px(68.0);
        let inp_h = px(32.0);
        let inp_y = rect.y + ((rect.h - inp_h) / 2.0).round();
        let (edit_input, save_btn, cancel_btn) = if mirrored {
            let cb = Rect::new(rect.x + pad, btn_y, cancel_w, btn_h);
            let sb = Rect::new(cb.right() + gap, btn_y, save_w, btn_h);
            let ix = sb.right() + px(8.0);
            let iw = (rect.right() - pad - ix).max(0.0);
            (Rect::new(ix, inp_y, iw, inp_h), sb, cb)
        } else {
            let cb = Rect::new(rect.right() - pad - cancel_w, btn_y, cancel_w, btn_h);
            let sb = Rect::new(cb.x - gap - save_w, btn_y, save_w, btn_h);
            let ix = rect.x + pad;
            let iw = (sb.x - px(8.0) - ix).max(0.0);
            (Rect::new(ix, inp_y, iw, inp_h), sb, cb)
        };
        return MemoryEntryRowLayout {
            id: entry.id,
            list_index,
            rect,
            text_rect: edit_input,
            meta_rect: Rect::new(0.0, 0.0, 0.0, 0.0),
            badge_rect: None,
            approve_btn: None,
            edit_btn: None,
            delete_btn: None,
            edit_input: Some(edit_input),
            save_btn: Some(save_btn),
            cancel_btn: Some(cancel_btn),
        };
    }

    let del_w = px(60.0);
    let edit_w = px(56.0);
    let app_w = if show_approve { px(74.0) } else { 0.0 };
    let badge_w = if show_approve || entry.kind == MemoryKind::Command {
        px(78.0)
    } else {
        0.0
    };
    let badge_h = px(20.0);
    let badge_y = rect.y + ((rect.h - badge_h) / 2.0).round();

    let (text_x, text_w, badge_rect, approve_btn, edit_btn, delete_btn) = if mirrored {
        let mut cursor = rect.x + pad;
        let del = Rect::new(cursor, btn_y, del_w, btn_h);
        cursor = del.right() + gap;
        let ed = Rect::new(cursor, btn_y, edit_w, btn_h);
        cursor = ed.right() + gap;
        let app = (app_w > 0.0).then(|| {
            let r = Rect::new(cursor, btn_y, app_w, btn_h);
            cursor = r.right() + gap;
            r
        });
        let badge = (badge_w > 0.0).then(|| {
            let r = Rect::new(cursor, badge_y, badge_w, badge_h);
            cursor = r.right() + px(8.0);
            r
        });
        let tx = cursor + px(4.0);
        let tw = (rect.right() - pad - tx).max(0.0);
        (tx, tw, badge, app, Some(ed), Some(del))
    } else {
        let mut cursor = rect.right() - pad;
        cursor -= del_w;
        let del = Rect::new(cursor, btn_y, del_w, btn_h);
        cursor -= gap + edit_w;
        let ed = Rect::new(cursor, btn_y, edit_w, btn_h);
        let app = (app_w > 0.0).then(|| {
            cursor -= gap + app_w;
            Rect::new(cursor, btn_y, app_w, btn_h)
        });
        let badge = (badge_w > 0.0).then(|| {
            cursor -= gap + badge_w;
            Rect::new(cursor, badge_y, badge_w, badge_h)
        });
        let tx = rect.x + pad;
        let tw = (cursor - px(10.0) - tx).max(0.0);
        (tx, tw, badge, app, Some(ed), Some(del))
    };

    let text_rect = Rect::new(text_x, rect.y + px(8.0), text_w, px(22.0));
    let meta_rect = Rect::new(text_x, text_rect.bottom() + px(2.0), text_w, px(16.0));

    MemoryEntryRowLayout {
        id: entry.id,
        list_index,
        rect,
        text_rect,
        meta_rect,
        badge_rect,
        approve_btn,
        edit_btn,
        delete_btn,
        edit_input: None,
        save_btn: None,
        cancel_btn: None,
    }
}
