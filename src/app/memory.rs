//! App hooks for the Project Memory overlay and off-thread environment refresh.
//!
//! All SQLite reads/writes, SHA-256 rules-file inspection, and filesystem environment scans run
//! off the UI thread; snapshots arrive back via `AppEvent::MemorySnapshot`.

use std::path::{Path, PathBuf};

use twitty_store::{MemoryUpdate, NewMemory, ProjectMemorySnapshot};
use winit::event::{KeyEvent, MouseButton, MouseScrollDelta};
use winit::window::CursorIcon;

use super::{App, AppEvent};
use crate::input::mouse::wheel_lines;
use crate::input::{key_shortcut_with, InputAction};
use crate::render::scene::memory::MemoryView;
use crate::ui::memory_panel::{MemoryAction, MemoryLayout, MemoryOutcome, MemoryPanelState};

impl App {
    pub(super) fn memory_open(&self) -> bool {
        self.memory.is_some()
    }

    /// Resolve the focused local pane's project root (git root if present, else cwd).
    fn focused_project_root(&self) -> Option<PathBuf> {
        let id = self.focused_id()?;
        let pane = self.ws.pane(id)?;
        if pane.remote.is_some() {
            return None;
        }
        crate::memory::resolve_active_project_root(pane.meta.cwd.as_deref())
    }

    /// Open the Project Memory overlay for the focused pane's project (idempotent when open).
    pub(super) fn open_memory(&mut self) {
        if self.memory.is_some() {
            return;
        }
        self.close_palette();
        self.close_settings();
        self.close_welcome();
        self.reset_pointer_gesture();

        let language = self.window.ui_language();
        let project_root = self.focused_project_root();
        let root_str = project_root
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());
        self.memory = Some(MemoryPanelState::new(
            root_str,
            self.config.memory_exclude.clone(),
            language,
        ));
        self.sync_memory();
        if let Some(root) = project_root {
            self.spawn_memory_snapshot(root);
        } else if let Some(state) = self.memory.as_mut() {
            state.loading = false;
            self.sync_memory();
        }
    }

    /// Close the Project Memory overlay cleanly.
    pub(super) fn close_memory(&mut self) {
        if self.memory.take().is_none() {
            return;
        }
        self.window.set_memory_panel(None);
        self.refresh_pointer_hover();
        self.window.request_redraw();
    }

    /// Hand the current Project Memory view snapshot to the renderer.
    pub(super) fn sync_memory(&mut self) {
        let Some(state) = &self.memory else {
            return;
        };
        self.window.set_memory_panel(Some(MemoryView {
            state: state.clone(),
        }));
        self.window.request_redraw();
    }

    /// Receive an off-thread memory snapshot; drop stale generations.
    pub(super) fn on_memory_snapshot(&mut self, generation: u64, snapshot: ProjectMemorySnapshot) {
        if generation != self.memory_generation {
            return;
        }
        let Some(state) = self.memory.as_mut() else {
            return;
        };
        state.apply_snapshot(snapshot);
        self.sync_memory();
    }

    /// Refresh environment notes once per working directory per session off the UI thread.
    pub(super) fn maybe_refresh_project_env(&mut self, cwd: &Path) {
        if cwd.as_os_str().is_empty() || !self.refreshed_env_projects.insert(cwd.to_path_buf()) {
            return;
        }
        self.store
            .refresh_env_notes_async(cwd.to_path_buf(), self.config.memory_exclude.clone());
    }

    /// Store a user-approved fact from the agent panel (`AgentEvent::RememberFact`), honoring
    /// `config.memory_exclude`.
    pub(crate) fn remember_fact(&self, project_root: Option<&Path>, text: &str) -> bool {
        crate::memory::remember_fact_with_excludes(
            &self.store,
            project_root,
            text,
            &self.config.memory_exclude,
        )
    }

    fn spawn_memory_snapshot(&mut self, project_root: PathBuf) {
        self.memory_generation = self.memory_generation.wrapping_add(1);
        let generation = self.memory_generation;
        let store = self.store.clone();
        let excludes = self.config.memory_exclude.clone();
        let proxy = self.proxy.clone();
        if let Err(err) = std::thread::Builder::new()
            .name("twitty-memory-load".into())
            .spawn(move || {
                let snapshot = match store.load_project_snapshot(project_root, excludes) {
                    Ok(snapshot) => snapshot,
                    Err(err) => {
                        tracing::warn!("twitty: failed to load project memory snapshot: {err}");
                        twitty_store::ProjectMemorySnapshot::default()
                    }
                };
                let _ = proxy.send_event(AppEvent::MemorySnapshot(generation, snapshot));
            })
        {
            tracing::warn!("twitty: memory snapshot thread failed to start: {err}");
            if let Some(state) = self.memory.as_mut() {
                state.apply_snapshot(twitty_store::ProjectMemorySnapshot::default());
                self.sync_memory();
            }
        }
    }

    fn memory_geometry(&self) -> ((f32, f32), f32) {
        let surface = self.window.surface_size().unwrap_or((960.0, 580.0));
        let scale = self.window.metrics().map_or(1.0, |m| m.scale);
        (surface, scale)
    }

    pub(super) fn handle_memory_key(&mut self, event: &KeyEvent) -> bool {
        if !event.state.is_pressed() {
            return false;
        }
        if self
            .memory
            .as_ref()
            .is_some_and(MemoryPanelState::accepts_text_input)
            && key_shortcut_with(event, self.modifiers, &self.keybindings)
                == Some(InputAction::Paste)
        {
            if let Some(text) = self.clipboard.text() {
                self.memory_text(&text);
            }
            return false;
        }
        let (surface, scale) = self.memory_geometry();
        let modifiers = self.modifiers;
        let outcome = {
            let Some(state) = self.memory.as_mut() else {
                return false;
            };
            state.on_key(
                &event.logical_key,
                event.text.as_deref(),
                modifiers,
                surface,
                scale,
            )
        };
        self.apply_memory_outcome(outcome);
        false
    }

    pub(super) fn memory_text(&mut self, text: &str) {
        let outcome = {
            let Some(state) = self.memory.as_mut() else {
                return;
            };
            state.on_text(text)
        };
        self.apply_memory_outcome(outcome);
    }

    pub(super) fn on_memory_cursor_moved(&mut self, x: f64, y: f64) {
        let (surface, scale) = self.memory_geometry();
        let (outcome, cursor) = {
            let Some(state) = self.memory.as_mut() else {
                return;
            };
            let outcome = state.on_mouse_move(x, y, surface, scale);
            let hit = MemoryLayout::build(surface, scale, state).hit(x, y);
            let cursor = if hit.is_text_input() {
                CursorIcon::Text
            } else if hit.is_interactive() {
                CursorIcon::Pointer
            } else {
                CursorIcon::Default
            };
            (outcome, cursor)
        };
        self.window.set_cursor_icon(cursor);
        self.apply_memory_outcome(outcome);
    }

    pub(super) fn on_memory_cursor_left(&mut self) {
        let outcome = {
            let Some(state) = self.memory.as_mut() else {
                return;
            };
            state.on_mouse_leave()
        };
        self.apply_memory_outcome(outcome);
    }

    pub(super) fn on_memory_mouse(&mut self, button: MouseButton, pressed: bool) {
        if !pressed {
            return;
        }
        let (x, y) = self.pointer.pos;
        let (surface, scale) = self.memory_geometry();
        if button != MouseButton::Left {
            let outside = self.memory.as_ref().is_some_and(|state| {
                !MemoryLayout::build(surface, scale, state)
                    .panel
                    .contains(x, y)
            });
            if outside {
                self.close_memory();
            }
            return;
        }
        let outcome = {
            let Some(state) = self.memory.as_mut() else {
                return;
            };
            state.on_mouse_down(x, y, surface, scale)
        };
        self.apply_memory_outcome(outcome);
    }

    pub(super) fn on_memory_wheel(&mut self, delta: MouseScrollDelta) {
        let line_height = self.window.line_height().unwrap_or(20.0);
        let lines = wheel_lines(delta, line_height);
        if !lines.is_finite() || lines == 0.0 {
            return;
        }
        let (x, y) = self.pointer.pos;
        let (surface, scale) = self.memory_geometry();
        let outcome = {
            let Some(state) = self.memory.as_mut() else {
                return;
            };
            state.on_wheel(x, y, lines, surface, scale)
        };
        self.apply_memory_outcome(outcome);
    }

    fn apply_memory_outcome(&mut self, outcome: MemoryOutcome) {
        if outcome.close {
            self.close_memory();
            return;
        }
        let project_root = self
            .memory
            .as_ref()
            .and_then(|s| s.project_root.clone())
            .map(PathBuf::from);
        let mut config_changed = false;
        let mut store_actions = Vec::new();

        for action in outcome.actions {
            match action {
                MemoryAction::AddExclusion { pattern } => {
                    if !self.config.memory_exclude.iter().any(|p| p == &pattern) {
                        self.config.memory_exclude.push(pattern);
                        config_changed = true;
                    }
                }
                MemoryAction::RemoveExclusion { index } => {
                    if index < self.config.memory_exclude.len() {
                        self.config.memory_exclude.remove(index);
                        config_changed = true;
                    }
                }
                other => store_actions.push(other),
            }
        }

        if config_changed {
            self.config.save();
            self.config_dirty = false;
        }

        if let Some(root) = project_root {
            if !store_actions.is_empty() || config_changed {
                self.spawn_memory_mutations_and_reload(root, store_actions);
            }
        }

        if outcome.redraw {
            self.sync_memory();
        }
    }

    fn spawn_memory_mutations_and_reload(
        &mut self,
        project_root: PathBuf,
        actions: Vec<MemoryAction>,
    ) {
        self.memory_generation = self.memory_generation.wrapping_add(1);
        let generation = self.memory_generation;
        let store = self.store.clone();
        let excludes = self.config.memory_exclude.clone();
        let proxy = self.proxy.clone();
        let root_str = project_root.to_string_lossy().into_owned();

        let _ = std::thread::Builder::new()
            .name("twitty-memory-op".into())
            .spawn(move || {
                for action in actions {
                    let res: Result<(), twitty_store::StoreError> = match action {
                        MemoryAction::AddMemory { kind, text } => store
                            .create_memory(NewMemory {
                                project_root: root_str.clone(),
                                kind,
                                text,
                                source: "user".to_owned(),
                                approved: true,
                            })
                            .map(|_| ()),
                        MemoryAction::UpdateMemory { id, text } => store
                            .update_memory(
                                id,
                                MemoryUpdate {
                                    text: Some(text),
                                    source: Some("user".to_owned()),
                                    ..Default::default()
                                },
                            )
                            .map(|_| ()),
                        MemoryAction::ApproveMemory { id } => {
                            store.set_memory_approved(id, true).map(|_| ())
                        }
                        MemoryAction::DeleteMemory { id } => store.delete_memory(id).map(|_| ()),
                        MemoryAction::ApproveRulesFile { rel_path, sha256 } => store
                            .approve_rules_file(root_str.clone(), rel_path, sha256)
                            .map(|_| ()),
                        MemoryAction::RevokeRulesFile { rel_path } => store
                            .revoke_rules_file(root_str.clone(), rel_path)
                            .map(|_| ()),
                        MemoryAction::RefreshEnv => {
                            store.refresh_env_notes(project_root.clone()).map(|_| ())
                        }
                        MemoryAction::ForgetProject => {
                            store.forget_project(root_str.clone()).map(|_| ())
                        }
                        MemoryAction::AddExclusion { .. }
                        | MemoryAction::RemoveExclusion { .. } => Ok(()),
                    };
                    if let Err(err) = res {
                        tracing::warn!("twitty: memory mutation failed: {err}");
                    }
                }
                match store.load_project_snapshot(project_root, excludes) {
                    Ok(snapshot) => {
                        let _ = proxy.send_event(AppEvent::MemorySnapshot(generation, snapshot));
                    }
                    Err(err) => {
                        tracing::warn!("twitty: memory reload after mutation failed: {err}");
                    }
                }
            });
    }
}
