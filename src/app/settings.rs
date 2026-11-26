//! App hooks for the Settings overlay: opening/closing it, routing keys, IME text, and pointer
//! events while it is open, applying live changes (including theme live preview + revert), and
//! persisting changes atomically via `TwittyConfig::save`.

use winit::event::{KeyEvent, MouseButton, MouseScrollDelta};
use winit::keyboard::PhysicalKey;
use winit::window::CursorIcon;

use super::App;
use crate::config::TwittyConfig;
use crate::input::keybindings::KeyBindings;
use crate::input::mouse::wheel_lines;
use crate::input::{key_shortcut_with, InputAction};
use crate::layout::PaneId;
use crate::render::scene::settings::SettingsView;
use crate::ui::settings_panel::{SettingId, SettingsLayout, SettingsOutcome, SettingsPanelState};

impl App {
    pub(super) fn settings_open(&self) -> bool {
        self.settings.is_some()
    }

    /// Open the settings panel (idempotent when already open).
    pub(super) fn open_settings(&mut self) {
        if self.settings.is_some() {
            return;
        }
        self.close_palette();
        self.close_welcome();
        self.reset_pointer_gesture();
        let (mono, all) = self.window.font_families();
        let config_path = TwittyConfig::config_path().map(|p| p.display().to_string());
        let env_language = crate::ui::chrome::UiLanguage::from_environment();
        let language = self.window.ui_language();
        self.settings = Some(
            SettingsPanelState::new(mono, all, config_path, language)
                .with_env_language(env_language),
        );
        self.sync_settings();
    }

    /// Close the settings panel and revert any uncommitted theme preview.
    pub(super) fn close_settings(&mut self) {
        let Some(mut state) = self.settings.take() else {
            return;
        };
        let revert = state.revert_preview(&self.config);
        if let Some(preview) = revert.preview_theme {
            let mut cfg = self.config.clone();
            cfg.theme = preview;
            self.window.set_palette(cfg.palette());
        }
        self.window.set_settings_panel(None);
        self.refresh_pointer_hover();
        self.window.request_redraw();
    }

    /// Hand the current settings snapshot to the renderer.
    pub(super) fn sync_settings(&mut self) {
        let Some(state) = &self.settings else {
            return;
        };
        self.window.set_settings_panel(Some(SettingsView {
            state: state.clone(),
            config: self.config.clone(),
        }));
        self.window.request_redraw();
    }

    fn settings_geometry(&self) -> ((f32, f32), f32) {
        let surface = self.window.surface_size().unwrap_or((960.0, 580.0));
        let scale = self.window.metrics().map_or(1.0, |m| m.scale);
        (surface, scale)
    }

    /// Handle a key event while Settings is open. Nothing reaches the PTY.
    pub(super) fn handle_settings_key(&mut self, event: &KeyEvent) -> bool {
        if !event.state.is_pressed() {
            return false;
        }
        let capturing = self
            .settings
            .as_ref()
            .is_some_and(SettingsPanelState::is_capturing_key);
        if !capturing {
            match key_shortcut_with(event, self.modifiers, &self.keybindings) {
                Some(InputAction::Settings) => {
                    self.close_settings();
                    return false;
                }
                Some(InputAction::Paste)
                    if self
                        .settings
                        .as_ref()
                        .is_some_and(SettingsPanelState::accepts_text_input) =>
                {
                    if let Some(text) = self.clipboard.text() {
                        self.settings_text(&text);
                    }
                    return false;
                }
                _ => {}
            }
        }
        let (surface, scale) = self.settings_geometry();
        let modifiers = self.modifiers;
        let physical = match event.physical_key {
            PhysicalKey::Code(c) => Some(c),
            _ => None,
        };
        let outcome = {
            let Some(state) = self.settings.as_mut() else {
                return false;
            };
            state.on_key_with_physical(
                physical,
                &event.logical_key,
                event.text.as_deref(),
                modifiers,
                &mut self.config,
                surface,
                scale,
            )
        };
        self.apply_settings_outcome(outcome);
        false
    }

    /// Route IME-committed text (including Arabic) into the active settings input/search field.
    pub(super) fn settings_text(&mut self, text: &str) {
        let outcome = {
            let Some(state) = self.settings.as_mut() else {
                return;
            };
            state.on_text(text, &mut self.config)
        };
        self.apply_settings_outcome(outcome);
    }

    pub(super) fn on_settings_cursor_moved(&mut self, x: f64, y: f64) {
        let (surface, scale) = self.settings_geometry();
        let (outcome, cursor) = {
            let Some(state) = self.settings.as_mut() else {
                return;
            };
            let outcome = state.on_mouse_move(x, y, &mut self.config, surface, scale);
            let hit = SettingsLayout::build(surface, scale, state, &self.config).hit(x, y);
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
        self.apply_settings_outcome(outcome);
    }

    pub(super) fn on_settings_cursor_left(&mut self) {
        let outcome = {
            let Some(state) = self.settings.as_mut() else {
                return;
            };
            state.on_mouse_leave(&self.config)
        };
        self.apply_settings_outcome(outcome);
    }

    pub(super) fn on_settings_mouse(&mut self, button: MouseButton, pressed: bool) {
        if !pressed {
            if button == MouseButton::Left {
                if let Some(state) = self.settings.as_mut() {
                    state.on_mouse_up();
                }
            }
            return;
        }
        let (x, y) = self.pointer.pos;
        let (surface, scale) = self.settings_geometry();
        if button != MouseButton::Left {
            // Clicking outside with any button closes settings cleanly.
            let outside = self.settings.as_ref().is_some_and(|state| {
                !SettingsLayout::build(surface, scale, state, &self.config)
                    .panel
                    .contains(x, y)
            });
            if outside {
                self.close_settings();
            }
            return;
        }
        let outcome = {
            let Some(state) = self.settings.as_mut() else {
                return;
            };
            state.on_mouse_down(x, y, &mut self.config, surface, scale)
        };
        self.apply_settings_outcome(outcome);
    }

    pub(super) fn on_settings_wheel(&mut self, delta: MouseScrollDelta) {
        let line_height = self.window.line_height().unwrap_or(20.0);
        let lines = wheel_lines(delta, line_height);
        if !lines.is_finite() || lines == 0.0 {
            return;
        }
        let (x, y) = self.pointer.pos;
        let (surface, scale) = self.settings_geometry();
        let outcome = {
            let Some(state) = self.settings.as_mut() else {
                return;
            };
            state.on_wheel(x, y, lines, &self.config, surface, scale)
        };
        self.apply_settings_outcome(outcome);
    }

    fn apply_settings_outcome(&mut self, outcome: SettingsOutcome) {
        if let Some(text) = outcome.copy_text {
            self.clipboard.set_text(text);
        }
        if let Some(ref preview) = outcome.preview_theme {
            let mut preview_cfg = self.config.clone();
            preview_cfg.theme = preview.clone();
            self.window.set_palette(preview_cfg.palette());
        }
        if outcome.keybindings_changed {
            self.keybindings = KeyBindings::from_config(&self.config.keybindings);
        }
        if !outcome.changed.is_empty() || outcome.keybindings_changed {
            let mut needs_layout = false;
            for setting in &outcome.changed {
                match setting {
                    SettingId::Theme => {
                        self.window.set_palette(self.config.palette());
                    }
                    SettingId::FontFamily => {
                        self.window
                            .set_font_family(self.config.font_family.as_deref());
                        needs_layout = true;
                    }
                    SettingId::FontSize => {
                        self.window.set_font_size(self.config.font_size);
                        needs_layout = true;
                    }
                    SettingId::UiFontFamily => {
                        self.window
                            .set_ui_font_family(self.config.ui_font_family.as_deref());
                    }
                    SettingId::UiLanguage => {
                        let lang = self
                            .settings
                            .as_ref()
                            .map(|s| s.language)
                            .unwrap_or_else(|| self.config.effective_ui_language());
                        self.window.set_ui_language(lang);
                        needs_layout = true;
                    }
                    SettingId::UiAccent => {
                        self.window.set_ui_accent(self.config.ui_accent_color());
                    }
                    SettingId::BackgroundOpacity => {
                        self.window.set_opacity(self.config.background_opacity);
                    }
                    SettingId::CursorStyle => {
                        self.window
                            .set_cursor_style(self.config.cursor_style.clone());
                    }
                    SettingId::CursorBlink => {
                        let blink = self.config.cursor_blink;
                        let ids: Vec<PaneId> = self.ws.panes().map(|p| p.id).collect();
                        for id in ids {
                            if let Some(pane) = self.ws.pane_mut(id) {
                                pane.terminal.set_default_cursor_blink(blink);
                            }
                        }
                    }
                    SettingId::SidebarPosition => {
                        self.window
                            .set_sidebar_position(self.config.sidebar.parse().unwrap_or_default());
                        needs_layout = true;
                    }
                    SettingId::SidebarWidth => {
                        self.window.set_sidebar_width(self.config.sidebar_width);
                        needs_layout = true;
                    }
                    SettingId::TopBar => {
                        self.window.set_top_bar(self.config.top_bar);
                        needs_layout = true;
                    }
                    SettingId::PaneHeaders => {
                        self.window.set_pane_headers(self.config.pane_headers);
                        needs_layout = true;
                    }
                    SettingId::BlockCards => {
                        self.window.set_block_cards(self.config.block_cards);
                    }
                    SettingId::DesktopNotifications
                    | SettingId::RestoreSession
                    | SettingId::ShellIntegration
                    | SettingId::ClaudeHooks
                    | SettingId::ControlSocket
                    | SettingId::CmuxCompat
                    | SettingId::AgentCommand
                    | SettingId::CopyConfigPath => {}
                }
            }
            self.config.save();
            self.config_dirty = false;
            if needs_layout {
                self.apply_layout();
            }
        }
        if outcome.close {
            self.settings = None;
            self.window.set_settings_panel(None);
            self.refresh_pointer_hover();
            self.window.request_redraw();
        } else if outcome.redraw
            || !outcome.changed.is_empty()
            || outcome.keybindings_changed
            || outcome.preview_theme.is_some()
        {
            self.sync_settings();
        }
    }
}
