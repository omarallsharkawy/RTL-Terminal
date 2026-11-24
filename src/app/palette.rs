//! The command palette: opening it, the keys, mouse clicks and text typed while it is open,
//! and running the chosen built-in or custom command (including trust confirmation for project
//! `.twitty/commands.json` files).
//!
//! While the palette is open it owns all keyboard, IME and pointer input so nothing can reach
//! panes or the PTY behind it. Clicking outside the palette closes it and consumes the click
//! (never starting a selection, pasting, or switching tabs).

use std::path::PathBuf;
use std::time::Instant;

use alacritty_terminal::term::TermMode;
use winit::event::{KeyEvent, MouseButton};

use super::App;
use crate::commands;
use crate::input::{key_shortcut, InputAction};
use crate::layout::{PaneId, SplitDir};
use crate::pty::{Pty, SpawnOptions};
use crate::render::scene::palette::{PaletteHit, PaletteItem, PaletteView};
use crate::ui::palette::{matches, PaletteCmd, PaletteEntry, PaletteId, PaletteState, MAX_RESULTS};
use crate::workflows::{
    encode_insert, resolve_run_cwd, run_shell_argv, trust_confirm_view, CommandMode, ConfirmCmd,
    TrustConfirmState, TrustStore, Workflow, WorkflowSet,
};

pub(super) struct OpenPalette {
    state: PaletteState,
    workflows: WorkflowSet,
    entries: Vec<PaletteEntry>,
    pending_trust: Option<PendingTrust>,
}

struct PendingTrust {
    workflow_index: usize,
    confirm: TrustConfirmState,
}

impl App {
    pub(super) fn palette_open(&self) -> bool {
        self.palette.is_some()
    }

    /// Working directory of pane `id`: OSC 7 first, else the PTY's cwd.
    pub(super) fn pane_cwd(&self, id: PaneId) -> Option<PathBuf> {
        let pane = self.ws.pane(id)?;
        pane.meta
            .cwd
            .clone()
            .or_else(|| pane.pty.as_ref().and_then(Pty::cwd))
    }

    pub(super) fn open_palette(&mut self) {
        self.reset_pointer_gesture();
        let cwd = self.focused_id().and_then(|id| self.pane_cwd(id));
        let workflows = WorkflowSet::load(&self.config.commands, cwd.as_deref());
        let entries = workflows.palette_entries();
        self.palette = Some(OpenPalette {
            state: PaletteState::default(),
            workflows,
            entries,
            pending_trust: None,
        });
        self.sync_palette();
    }

    pub(super) fn close_palette(&mut self) {
        if self.palette.take().is_some() {
            self.window.set_command_palette(None);
            self.window.request_redraw();
        }
    }

    /// Hand the current query and results (or trust confirmation) to the renderer.
    fn sync_palette(&mut self) {
        let Some(open) = &self.palette else {
            return;
        };
        let view = if let Some(pending) = &open.pending_trust {
            if let Some(project) = open.workflows.project() {
                trust_confirm_view(project, pending.confirm.scroll)
            } else {
                PaletteView::default()
            }
        } else {
            let items = matches(&open.entries, &open.state.query)
                .into_iter()
                .take(MAX_RESULTS)
                .map(|i| PaletteItem {
                    title_en: open.entries[i].title_en.clone(),
                    title_ar: open.entries[i].title_ar.clone(),
                    shortcut: open.entries[i].shortcut.clone(),
                })
                .collect::<Vec<_>>();
            let selected = open.state.selected.min(items.len().saturating_sub(1));
            PaletteView {
                query: open.state.query.clone(),
                items,
                selected,
            }
        };
        self.window.set_command_palette(Some(view));
        self.window.request_redraw();
    }

    /// A key while the palette is open; nothing reaches the program. Returns true when the
    /// window should close (the command run closed the last pane).
    pub(super) fn handle_palette_key(&mut self, event: &KeyEvent) -> bool {
        if !event.state.is_pressed() {
            return false;
        }
        // The palette shortcut closes it again.
        if key_shortcut(event, self.modifiers) == Some(InputAction::CommandPalette) {
            self.close_palette();
            return false;
        }
        let modifiers = self.modifiers;
        let Some(open) = self.palette.as_mut() else {
            return false;
        };

        if let Some(pending) = open.pending_trust.as_mut() {
            let cmd_count = open.workflows.project().map_or(0, |p| p.commands.len());
            match pending.confirm.key(&event.logical_key, cmd_count) {
                ConfirmCmd::None => self.sync_palette(),
                ConfirmCmd::Cancel => {
                    open.pending_trust = None;
                    self.sync_palette();
                }
                ConfirmCmd::Trust => self.confirm_pending_trust(),
            }
            return false;
        }

        match open.state.key(
            &open.entries,
            &event.logical_key,
            event.text.as_deref(),
            modifiers,
        ) {
            PaletteCmd::None => {
                self.sync_palette();
                false
            }
            PaletteCmd::Close => {
                self.close_palette();
                false
            }
            PaletteCmd::Run(id) => self.activate_palette_id(id),
        }
    }

    /// A mouse press while the palette is open. Clicking outside closes the palette and consumes
    /// the click so nothing reaches panes or tabs; left-clicking an item row runs it. Returns
    /// true when the window should close.
    pub(super) fn on_palette_mouse_press(&mut self, button: MouseButton) -> bool {
        let (x, y) = self.pointer.pos;
        match self.window.palette_hit(x, y) {
            PaletteHit::Outside => {
                self.close_palette();
                false
            }
            PaletteHit::Inside => false,
            PaletteHit::Item(index) if button == MouseButton::Left => {
                let confirming = self
                    .palette
                    .as_ref()
                    .is_some_and(|open| open.pending_trust.is_some());
                if confirming {
                    if index == 0 {
                        self.confirm_pending_trust();
                    }
                    return false;
                }
                let chosen = self.palette.as_ref().and_then(|open| {
                    matches(&open.entries, &open.state.query)
                        .into_iter()
                        .take(MAX_RESULTS)
                        .nth(index)
                        .map(|i| open.entries[i].id.clone())
                });
                match chosen {
                    Some(id) => self.activate_palette_id(id),
                    None => {
                        self.close_palette();
                        false
                    }
                }
            }
            PaletteHit::Item(_) => false,
        }
    }

    fn activate_palette_id(&mut self, id: PaletteId) -> bool {
        match id {
            PaletteId::Builtin(name) => {
                self.close_palette();
                if name == "palette.open" {
                    return false;
                }
                self.run_command(name)
            }
            PaletteId::Custom(index) => {
                let Some(open) = self.palette.as_mut() else {
                    return false;
                };
                let Some(workflow) = open.workflows.get(index).cloned() else {
                    self.close_palette();
                    return false;
                };
                if !workflow.trusted {
                    open.pending_trust = Some(PendingTrust {
                        workflow_index: index,
                        confirm: TrustConfirmState::default(),
                    });
                    self.sync_palette();
                    return false;
                }
                self.close_palette();
                if let Some(pane_id) = self.focused_id() {
                    if let Err(err) = self.run_workflow(pane_id, &workflow) {
                        self.show_pane_warning(pane_id, &err);
                    }
                }
                false
            }
        }
    }

    fn confirm_pending_trust(&mut self) {
        let Some((project, mut workflow)) = self.palette.as_ref().and_then(|open| {
            let pending = open.pending_trust.as_ref()?;
            let project = open.workflows.project()?.clone();
            let workflow = open.workflows.get(pending.workflow_index)?.clone();
            Some((project, workflow))
        }) else {
            self.close_palette();
            return;
        };

        let mut store = TrustStore::load_default();
        store.trust(&project.root, &project.sha256);
        if let Err(err) = store.save_default() {
            self.close_palette();
            if let Some(pane_id) = self.focused_id() {
                self.show_pane_warning(pane_id, &format!("could not save trust store: {err}"));
            }
            return;
        }
        workflow.trusted = true;
        self.close_palette();
        if let Some(pane_id) = self.focused_id() {
            if let Err(err) = self.run_workflow(pane_id, &workflow) {
                self.show_pane_warning(pane_id, &err);
            }
        }
    }

    /// Text committed by an input method while the palette is open (Arabic typing).
    pub(super) fn palette_text(&mut self, text: &str) {
        if let Some(open) = self.palette.as_mut() {
            if open.pending_trust.is_some() {
                return;
            }
            open.state.push_text(text);
            self.sync_palette();
        }
    }

    /// Run a built-in command by its stable name on the focused pane. Returns true when the
    /// window should close.
    pub(super) fn run_command(&mut self, name: &str) -> bool {
        match (commands::action(name), self.focused_id()) {
            (Some(action), Some(id)) => self.run_action(id, action),
            _ => false,
        }
    }

    /// Run a trusted custom workflow command on pane `id`.
    pub(super) fn run_workflow(&mut self, id: PaneId, workflow: &Workflow) -> Result<(), String> {
        if !workflow.trusted {
            return Err(format!(
                "refusing to run untrusted project command {:?}",
                workflow.spec.name
            ));
        }
        match workflow.spec.mode {
            CommandMode::Insert => {
                let bracketed = self.mode_of(id).contains(TermMode::BRACKETED_PASTE);
                let bytes = encode_insert(&workflow.spec.command, bracketed);
                self.send_input(id, &bytes);
                self.cursor_blink.reset(Instant::now());
                Ok(())
            }
            CommandMode::Run => {
                let pane_cwd = self.pane_cwd(id);
                let cwd = resolve_run_cwd(workflow, pane_cwd.as_deref())?;
                let options = SpawnOptions {
                    cwd,
                    command: run_shell_argv(&workflow.spec.command),
                    env: Vec::new(),
                };
                let spawned = if self.ws.can_split_focused(SplitDir::Horizontal) {
                    self.split_focused_with(SplitDir::Horizontal, options)
                } else if self.ws.can_split_focused(SplitDir::Vertical) {
                    self.split_focused_with(SplitDir::Vertical, options)
                } else {
                    self.new_tab_with(options)
                };
                if spawned.is_none() {
                    return Err("failed to start custom command".into());
                }
                Ok(())
            }
        }
    }

    fn show_pane_warning(&mut self, id: PaneId, message: &str) {
        let note = format!("\r\n\x1b[33mTwitty: {message}\x1b[0m\r\n");
        self.term(id, |t| t.process_bytes(note.as_bytes()));
        self.window.request_redraw();
    }
}
