//! App hooks for the first-run Welcome wizard overlay: first-run launch check, re-opening from
//! the palette, keyboard and pointer routing, live config apply/revert, atomic config save, and
//! `first_run_done` flag persistence.

use winit::event::{KeyEvent, MouseButton};
use winit::window::CursorIcon;

use super::App;
use crate::composer::PromptState;
use crate::render::scene::welcome::WelcomeView;
use crate::ui::chrome::UiLanguage;
use crate::ui::welcome::{
    mark_done_default, should_show_first_run_default, AgentsInfo, ShellStatus, WelcomeLayout,
    WelcomeOutcome, WelcomeState,
};

impl App {
    pub(super) fn welcome_open(&self) -> bool {
        self.welcome.is_some()
    }

    /// Show the Welcome wizard automatically on first start (skipping direct SSH sessions).
    pub(super) fn maybe_open_first_run_welcome(&mut self) {
        if self.ws.panes().any(|p| p.remote.is_some()) {
            return;
        }
        if should_show_first_run_default() {
            self.open_welcome();
        }
    }

    /// Open the Welcome wizard (idempotent when already open).
    pub(super) fn open_welcome(&mut self) {
        if self.welcome.is_some() {
            return;
        }
        self.close_palette();
        self.close_settings();
        self.close_memory();
        self.close_context_menu();
        self.reset_pointer_gesture();

        let pane_program = self
            .focused_id()
            .and_then(|id| self.ws.pane(id))
            .and_then(|pane| pane.pty.as_ref()?.program_name())
            .map(str::to_owned);
        let marks_seen = self
            .ws
            .panes()
            .any(|p| p.meta.prompt_state != PromptState::Unknown);
        let shell = ShellStatus::detect(pane_program.as_deref(), marks_seen);
        let agents = AgentsInfo::detect_env(&self.config);
        let env_language = UiLanguage::from_environment();

        self.welcome = Some(WelcomeState::with_default_commands(
            &self.config,
            env_language,
            shell,
            agents,
        ));
        self.sync_welcome();
    }

    /// Close the Welcome wizard programmatically (reverting any uncommitted live preview).
    pub(super) fn close_welcome(&mut self) {
        let Some(mut state) = self.welcome.take() else {
            return;
        };
        let mut outcome = state.cancel(&mut self.config);
        outcome.mark_done = false;
        self.apply_welcome_outcome(outcome);
    }

    /// Refresh Step 3's OSC 133 badge if a shell prompt mark arrives while Welcome is open.
    pub(super) fn on_welcome_shell_mark(&mut self) {
        if let Some(state) = self.welcome.as_mut() {
            if state.set_shell_marks_seen(true) {
                self.sync_welcome();
            }
        }
    }

    /// Hand the current welcome snapshot to the renderer.
    pub(super) fn sync_welcome(&mut self) {
        let Some(state) = &self.welcome else {
            return;
        };
        self.window.set_welcome(Some(WelcomeView {
            state: state.clone(),
            config: self.config.clone(),
        }));
        self.window.request_redraw();
    }

    fn welcome_geometry(&self) -> ((f32, f32), f32) {
        let surface = self.window.surface_size().unwrap_or((960.0, 580.0));
        let scale = self.window.metrics().map_or(1.0, |m| m.scale);
        (surface, scale)
    }

    /// Handle a key event while Welcome is open. Nothing reaches the PTY.
    pub(super) fn handle_welcome_key(&mut self, event: &KeyEvent) -> bool {
        if !event.state.is_pressed() {
            return false;
        }
        let modifiers = self.modifiers;
        let outcome = {
            let Some(state) = self.welcome.as_mut() else {
                return false;
            };
            state.on_key(&event.logical_key, modifiers, &mut self.config)
        };
        self.apply_welcome_outcome(outcome);
        false
    }

    pub(super) fn on_welcome_cursor_moved(&mut self, x: f64, y: f64) {
        let (surface, scale) = self.welcome_geometry();
        let (outcome, cursor) = {
            let Some(state) = self.welcome.as_mut() else {
                return;
            };
            let outcome = state.on_mouse_move(x, y, surface, scale);
            let hit = WelcomeLayout::build(surface, scale, state).hit(x, y);
            let cursor = if hit.is_interactive() {
                CursorIcon::Pointer
            } else {
                CursorIcon::Default
            };
            (outcome, cursor)
        };
        self.window.set_cursor_icon(cursor);
        self.apply_welcome_outcome(outcome);
    }

    pub(super) fn on_welcome_cursor_left(&mut self) {
        let outcome = {
            let Some(state) = self.welcome.as_mut() else {
                return;
            };
            state.on_mouse_leave()
        };
        self.apply_welcome_outcome(outcome);
    }

    pub(super) fn on_welcome_mouse(&mut self, button: MouseButton, pressed: bool) {
        if !pressed {
            return;
        }
        let (x, y) = self.pointer.pos;
        let (surface, scale) = self.welcome_geometry();
        if button != MouseButton::Left {
            let outside = self.welcome.as_ref().is_some_and(|state| {
                !WelcomeLayout::build(surface, scale, state)
                    .card
                    .contains(x, y)
            });
            if outside {
                let outcome = {
                    let Some(state) = self.welcome.as_mut() else {
                        return;
                    };
                    state.cancel(&mut self.config)
                };
                self.apply_welcome_outcome(outcome);
            }
            return;
        }
        let outcome = {
            let Some(state) = self.welcome.as_mut() else {
                return;
            };
            state.on_mouse_down(x, y, &mut self.config, surface, scale)
        };
        self.apply_welcome_outcome(outcome);
    }

    fn apply_welcome_outcome(&mut self, outcome: WelcomeOutcome) {
        let mut needs_layout = false;
        if outcome.apply.language {
            let lang = self
                .welcome
                .as_ref()
                .map(|s| s.language)
                .unwrap_or_else(|| self.config.effective_ui_language());
            self.window.set_ui_language(lang);
            needs_layout = true;
        }
        if outcome.apply.palette || outcome.apply.font_size || outcome.apply.opacity {
            needs_layout |= self
                .window
                .set_appearance(super::window::Appearance::from_config(&self.config));
        }
        if needs_layout {
            self.apply_layout();
        }
        if outcome.save_config {
            self.config.save();
            self.config_dirty = false;
        }
        if outcome.mark_done {
            if let Err(err) = mark_done_default() {
                crate::config::warn(format!("twitty: could not save first_run_done flag: {err}"));
            }
        }
        if outcome.close {
            self.welcome = None;
            self.window.set_welcome(None);
            self.refresh_pointer_hover();
            self.window.request_redraw();
        } else if outcome.redraw || outcome.apply.any() {
            self.sync_welcome();
        }
    }
}
