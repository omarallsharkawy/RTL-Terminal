//! App hooks for the Settings overlay: opening/closing it, routing keys, IME text, and pointer
//! events while it is open, applying live changes (including theme live preview + revert), and
//! persisting changes atomically via `TwittyConfig::save`.

use winit::event::{KeyEvent, MouseButton, MouseScrollDelta};
use winit::keyboard::PhysicalKey;
use winit::window::CursorIcon;

use super::event::AppEvent;
use super::window::Appearance;
use super::App;
use crate::config::TwittyConfig;
use crate::input::keybindings::KeyBindings;
use crate::input::mouse::wheel_lines;
use crate::input::{key_shortcut_with, InputAction};
use crate::layout::PaneId;
use crate::render::scene::settings::SettingsView;
use crate::ui::chrome::UiLanguage;
use crate::ui::settings_ai::{detect_cli_agents_on_path, API_PROVIDERS};
use crate::ui::settings_panel::{SettingId, SettingsLayout, SettingsOutcome, SettingsPanelState};
use twitty_store::SecretStore;

fn probe_keyring_flags(store: &dyn SecretStore) -> [bool; 3] {
    let mut flags = [false; 3];
    for (i, spec) in API_PROVIDERS.iter().enumerate() {
        flags[i] = matches!(store.get(spec.keyring_account), Ok(Some(v)) if !v.is_empty());
    }
    flags
}

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
        self.close_memory();
        self.close_welcome();
        self.reset_pointer_gesture();
        let (mono, all) = self.window.font_families();
        let config_path = TwittyConfig::config_path().map(|p| p.display().to_string());
        let env_language = crate::ui::chrome::UiLanguage::from_environment();
        let language = self.window.ui_language();
        let mut panel = SettingsPanelState::new(mono, all, config_path, language)
            .with_env_language(env_language);
        panel.ai.active_project_root = self
            .focused_id()
            .and_then(|id| self.pane_cwd(id))
            .map(|p| p.display().to_string());
        panel.ai.sync_from_config(&self.config);
        self.settings = Some(panel);
        self.spawn_settings_ai_probe();
        self.sync_settings();
    }

    /// Open Settings directly to the Commands tab with the Add Command form pre-filled from
    /// a command block ("Save as workflow…").
    pub(super) fn open_settings_save_workflow(&mut self, command: String) {
        if self.settings.is_none() {
            self.open_settings();
        }
        let cleaned_cmd = crate::blocks::rerun_text(&command);
        let suggested_name: String = cleaned_cmd.chars().take(28).collect();
        if let Some(panel) = self.settings.as_mut() {
            panel.select_section(
                crate::ui::settings_panel::SettingsSection::Commands,
                &self.config,
            );
            panel
                .commands_editor
                .start_add_prefilled(suggested_name, cleaned_cmd);
        }
        self.sync_settings();
    }

    fn spawn_settings_ai_probe(&mut self) {
        self.settings_probe_gen = self.settings_probe_gen.wrapping_add(1);
        let generation = self.settings_probe_gen;
        let proxy = self.proxy.clone();
        if let Err(err) = std::thread::Builder::new()
            .name("twitty-settings-probe".into())
            .spawn(move || {
                let detected_cli_agents = detect_cli_agents_on_path();
                let keyring = twitty_store::SystemSecretStore::new();
                let saved_keys = probe_keyring_flags(&keyring);
                let external_themes = Some(crate::config::available_external_theme_names());
                let _ = proxy.send_event(AppEvent::SettingsAiProbed {
                    generation,
                    detected_cli_agents,
                    saved_keys,
                    external_themes,
                    notice: None,
                    restart_engine: false,
                });
            })
        {
            tracing::warn!("twitty settings probe thread failed to start: {err}");
        }
    }

    pub(super) fn on_settings_ai_probed(
        &mut self,
        generation: u64,
        detected_cli_agents: Vec<String>,
        saved_keys: [bool; 3],
        external_themes: Option<Vec<String>>,
        notice: Option<(bool, String)>,
        restart_engine: bool,
    ) {
        if restart_engine {
            self.restart_agent_engine_when_idle();
        }
        if generation != self.settings_probe_gen {
            return;
        }
        self.agent_panel.state.available_clis = detected_cli_agents.clone();
        self.sync_agent_panel_active_cli();
        if let Some(state) = self.settings.as_mut() {
            state.ai.detected_cli_agents = detected_cli_agents;
            state.ai.saved_keys = saved_keys;
            if let Some(themes) = external_themes {
                state.external_themes = themes;
            }
            if let Some(msg) = notice {
                state.ai.notice = Some(msg);
            }
            self.sync_settings();
        }
    }

    /// Close the settings panel and revert any uncommitted theme preview.
    pub(super) fn close_settings(&mut self) {
        self.flush_config_save();
        let Some(mut state) = self.settings.take() else {
            return;
        };
        let revert = state.revert_preview(&self.config);
        if let Some(preview) = revert.preview_theme {
            let mut cfg = self.config.clone();
            cfg.theme = preview;
            self.window.preview_palette(cfg.palette());
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
                self.flush_config_save();
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
            self.window.preview_palette(preview_cfg.palette());
        }
        if let Some((account, secret)) = outcome.save_secret {
            self.settings_probe_gen = self.settings_probe_gen.wrapping_add(1);
            let generation = self.settings_probe_gen;
            let lang: UiLanguage = self.window.ui_language();
            let proxy = self.proxy.clone();
            let detected = self
                .settings
                .as_ref()
                .map(|s| s.ai.detected_cli_agents.clone())
                .unwrap_or_default();
            if let Err(err) = std::thread::Builder::new()
                .name("twitty-settings-secret".into())
                .spawn(move || {
                    let keyring = twitty_store::SystemSecretStore::new();
                    let notice = match keyring.set(&account, secret.expose()) {
                        Ok(()) => {
                            Some((false, lang.t("API key saved to system keyring.").to_owned()))
                        }
                        Err(err) => Some((true, format!("{err}"))),
                    };
                    let saved_keys = probe_keyring_flags(&keyring);
                    let _ = proxy.send_event(AppEvent::SettingsAiProbed {
                        generation,
                        detected_cli_agents: detected,
                        saved_keys,
                        external_themes: None,
                        notice,
                        restart_engine: true,
                    });
                })
            {
                tracing::warn!("twitty settings secret thread failed to start: {err}");
            }
        }
        if let Some(account) = outcome.delete_secret {
            self.settings_probe_gen = self.settings_probe_gen.wrapping_add(1);
            let generation = self.settings_probe_gen;
            let lang: UiLanguage = self.window.ui_language();
            let proxy = self.proxy.clone();
            let detected = self
                .settings
                .as_ref()
                .map(|s| s.ai.detected_cli_agents.clone())
                .unwrap_or_default();
            if let Err(err) = std::thread::Builder::new()
                .name("twitty-settings-secret".into())
                .spawn(move || {
                    let keyring = twitty_store::SystemSecretStore::new();
                    let notice = match keyring.delete(&account) {
                        Ok(_) => Some((false, lang.t("API key removed from keyring.").to_owned())),
                        Err(err) => Some((true, format!("{err}"))),
                    };
                    let saved_keys = probe_keyring_flags(&keyring);
                    let _ = proxy.send_event(AppEvent::SettingsAiProbed {
                        generation,
                        detected_cli_agents: detected,
                        saved_keys,
                        external_themes: None,
                        notice,
                        restart_engine: true,
                    });
                })
            {
                tracing::warn!("twitty settings secret thread failed to start: {err}");
            }
        }
        if outcome.ai_config_changed {
            self.sync_agent_panel_active_cli();
            self.restart_agent_engine_when_idle();
        }
        if outcome.clear_agent_history {
            self.agent_panel.state.clear_history();
            self.sync_agent_needs_input();
        }
        if outcome.keybindings_changed {
            self.keybindings =
                std::sync::Arc::new(KeyBindings::from_config(&self.config.keybindings));
            self.composer.state.keybindings = Some(std::sync::Arc::clone(&self.keybindings));
        }
        if outcome.commands_changed {
            self.invalidate_composer_workflows();
        }
        if !outcome.changed.is_empty()
            || outcome.keybindings_changed
            || outcome.ai_config_changed
            || outcome.commands_changed
        {
            let mut needs_layout = self
                .window
                .set_appearance(Appearance::from_config(&self.config));
            for setting in &outcome.changed {
                match setting {
                    // Applied above, all at once.
                    SettingId::Theme
                    | SettingId::ColorBackground
                    | SettingId::ColorForeground
                    | SettingId::ColorCursor
                    | SettingId::ColorSelection
                    | SettingId::ColorAnsi
                    | SettingId::FontFamily
                    | SettingId::FontSize
                    | SettingId::UiFontFamily
                    | SettingId::UiAccent
                    | SettingId::BackgroundOpacity
                    | SettingId::CursorStyle
                    | SettingId::SidebarPosition
                    | SettingId::SidebarWidth
                    | SettingId::TopBar
                    | SettingId::PaneHeaders
                    | SettingId::BlockCards
                    | SettingId::ReduceMotion => {}
                    SettingId::UiLanguage => {
                        let lang = self
                            .settings
                            .as_ref()
                            .map(|s| s.language)
                            .unwrap_or_else(|| self.config.effective_ui_language());
                        self.window.set_ui_language(lang);
                        needs_layout = true;
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
                    SettingId::ComposerMode => {
                        if self.config.composer_mode() == crate::composer::ComposerMode::Off {
                            self.close_composer();
                        }
                    }
                    SettingId::ProjectRoots => self.start_project_scan(),
                    SettingId::BuiltInAi => {
                        self.composer.state.ai_enabled = self.config.ai;
                    }
                    SettingId::ControlSocket => {
                        if self.config.control_socket {
                            self.start_control_server();
                        } else {
                            self.stop_control_server();
                        }
                    }
                    SettingId::DesktopNotifications
                    | SettingId::AiTimeout
                    | SettingId::RestoreSession
                    | SettingId::ShellIntegration
                    | SettingId::RecordHistory
                    | SettingId::ClaudeHooks
                    | SettingId::CmuxCompat
                    | SettingId::AgentCommand
                    | SettingId::ProjectMemory
                    | SettingId::CopyConfigPath => {}
                }
            }
            self.schedule_config_save();
            if needs_layout {
                self.apply_layout();
            }
        }
        if outcome.open_memory {
            self.close_settings();
            self.open_memory();
            return;
        }
        if outcome.close {
            self.flush_config_save();
            self.settings = None;
            self.window.set_settings_panel(None);
            self.refresh_pointer_hover();
            self.window.request_redraw();
        } else if outcome.redraw
            || !outcome.changed.is_empty()
            || outcome.keybindings_changed
            || outcome.ai_config_changed
            || outcome.commands_changed
            || outcome.clear_agent_history
            || outcome.preview_theme.is_some()
        {
            self.sync_settings();
        }
    }
}
