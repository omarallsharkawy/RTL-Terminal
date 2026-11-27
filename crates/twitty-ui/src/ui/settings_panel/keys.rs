//! Keyboard input for the settings panel: navigation, editing and key capture.

use super::*;

impl SettingsPanelState {
    /// Handle one key press (without a physical key code). Returns actions for the app to apply.
    pub fn on_key(
        &mut self,
        key: &Key,
        text: Option<&str>,
        modifiers: ModifiersState,
        config: &mut TwittyConfig,
        surface: (f32, f32),
        scale: f32,
    ) -> SettingsOutcome {
        self.on_key_with_physical(None, key, text, modifiers, config, surface, scale)
    }

    /// Handle one key press with its optional physical `KeyCode` (needed for non-Latin layouts
    /// such as Arabic when capturing new keybinding chords).
    #[allow(clippy::too_many_arguments)]
    pub fn on_key_with_physical(
        &mut self,
        physical: Option<KeyCode>,
        key: &Key,
        text: Option<&str>,
        modifiers: ModifiersState,
        config: &mut TwittyConfig,
        surface: (f32, f32),
        scale: f32,
    ) -> SettingsOutcome {
        let mut outcome = SettingsOutcome::default();

        // 0. Active shortcut capture in the `Keys` section owns every key press.
        if let Some(cmd_name) = self.capturing_key {
            return self.handle_key_capture(cmd_name, physical, key, modifiers, config);
        }

        // 1. Expanded inline picker owns arrows, Enter, Space, Backspace, text, Tab, and Esc.
        if let Some(exp) = self.expanded.clone() {
            let searchable = matches!(
                exp.setting.descriptor().kind,
                ControlKind::ListPicker { searchable: true }
            );
            match key {
                Key::Named(NamedKey::Escape) => {
                    self.cancel_picker_into(config, &mut outcome);
                    return outcome;
                }
                Key::Named(NamedKey::Tab) => {
                    self.cancel_picker_into(config, &mut outcome);
                    // Fall through to normal Tab movement below.
                }
                Key::Named(NamedKey::ArrowDown) => {
                    let opts = self.filtered_picker_options(exp.setting, &exp.query);
                    if !opts.is_empty() {
                        let next = (exp.highlighted.min(opts.len() - 1) + 1) % opts.len();
                        self.move_picker_highlight(next, opts.len());
                        if exp.setting == SettingId::Theme {
                            self.set_preview_theme(opts[next].value.clone(), config, &mut outcome);
                        }
                        outcome.redraw = true;
                    }
                    return outcome;
                }
                Key::Named(NamedKey::ArrowUp) => {
                    let opts = self.filtered_picker_options(exp.setting, &exp.query);
                    if !opts.is_empty() {
                        let n = opts.len();
                        let next = (exp.highlighted.min(n - 1) + n - 1) % n;
                        self.move_picker_highlight(next, n);
                        if exp.setting == SettingId::Theme {
                            self.set_preview_theme(opts[next].value.clone(), config, &mut outcome);
                        }
                        outcome.redraw = true;
                    }
                    return outcome;
                }
                Key::Named(NamedKey::Enter) => {
                    let opts = self.filtered_picker_options(exp.setting, &exp.query);
                    if let Some(opt) = opts.get(exp.highlighted) {
                        self.commit_picker_option(exp.setting, opt, config, &mut outcome);
                    } else {
                        self.cancel_picker_into(config, &mut outcome);
                    }
                    return outcome;
                }
                Key::Named(NamedKey::Space) if !searchable => {
                    let opts = self.filtered_picker_options(exp.setting, &exp.query);
                    if let Some(opt) = opts.get(exp.highlighted) {
                        self.commit_picker_option(exp.setting, opt, config, &mut outcome);
                    }
                    return outcome;
                }
                Key::Named(NamedKey::Backspace) if searchable => {
                    if let Some(active) = &mut self.expanded {
                        if backspace_text(&mut active.query, modifiers.control_key()) {
                            active.highlighted = 0;
                            active.scroll = 0;
                            outcome.redraw = true;
                        }
                    }
                    return outcome;
                }
                _ if searchable => {
                    if let Some(t) = text.filter(|_| {
                        !modifiers.control_key() && !modifiers.alt_key() && !modifiers.super_key()
                    }) {
                        return self.on_text(t, config);
                    }
                    return outcome;
                }
                _ => return outcome,
            }
        }

        // 1b. The Commands add/edit form owns every key while open (Esc cancels it, Tab moves
        // between its fields).
        if self.section == SettingsSection::Commands
            && self.focus == FocusZone::Content
            && self.commands_editor.form.is_some()
        {
            let result = self
                .commands_editor
                .on_form_key(key, text, modifiers, config);
            return Self::commands_outcome(result);
        }

        // 2. Escape closes settings and reverts any uncommitted preview.
        if matches!(key, Key::Named(NamedKey::Escape)) {
            let mut revert = self.revert_preview(config);
            revert.close = true;
            return revert;
        }

        // 3. Tab / Shift+Tab moves across Sidebar and controls in Content.
        if matches!(key, Key::Named(NamedKey::Tab)) {
            if modifiers.shift_key() {
                self.focus_prev();
            } else {
                self.focus_next();
            }
            self.ensure_focused_visible(config, surface, scale);
            outcome.redraw = true;
            return outcome;
        }

        // 4. Sidebar navigation.
        if self.focus == FocusZone::Sidebar {
            let enter_content_dir = if self.mirrored() {
                NamedKey::ArrowLeft
            } else {
                NamedKey::ArrowRight
            };
            match key {
                Key::Named(NamedKey::ArrowDown) => {
                    let next = SettingsSection::from_index(self.section.index() + 1);
                    return self.select_section(next, config);
                }
                Key::Named(NamedKey::ArrowUp) => {
                    let n = SettingsSection::ALL.len();
                    let next = SettingsSection::from_index((self.section.index() + n - 1) % n);
                    return self.select_section(next, config);
                }
                Key::Named(NamedKey::Enter | NamedKey::Space) => {
                    self.focus = FocusZone::Content;
                    self.selected = 0;
                    outcome.redraw = true;
                    return outcome;
                }
                Key::Named(k) if *k == enter_content_dir => {
                    self.focus = FocusZone::Content;
                    self.selected = 0;
                    outcome.redraw = true;
                    return outcome;
                }
                _ => return outcome,
            }
        }

        // 5. AI section content: text fields for Ollama URL, cost cap, allow-list, and keyring key entry.
        if self.section == SettingsSection::Ai {
            use crate::ui::settings_ai::AiTextField;
            if let Some(field) = self.ai.active_field {
                match key {
                    Key::Named(NamedKey::Escape) => {
                        if matches!(field, AiTextField::ApiKey(_)) {
                            self.ai.clear_key_draft();
                        } else {
                            self.ai.active_field = None;
                        }
                        outcome.redraw = true;
                        return outcome;
                    }
                    Key::Named(NamedKey::Enter) => {
                        match field {
                            AiTextField::ApiKey(idx) => {
                                if let Some(pair) = self.ai.take_key_secret(idx) {
                                    outcome.save_secret = Some(pair);
                                }
                            }
                            AiTextField::CostCap => {
                                if self.ai.commit_cost_cap(config, self.language) {
                                    outcome.ai_config_changed = true;
                                }
                                self.ai.active_field = None;
                            }
                            AiTextField::AllowListAdd => {
                                if self.ai.add_allow_list_entry(config, self.language) {
                                    outcome.ai_config_changed = true;
                                }
                            }
                            AiTextField::OllamaUrl => {
                                if config.ai_ollama_url.trim().is_empty() {
                                    config.ai_ollama_url =
                                        crate::ui::settings_ai::DEFAULT_OLLAMA_URL.to_owned();
                                }
                                outcome.ai_config_changed = true;
                                self.ai.active_field = None;
                            }
                        }
                        outcome.redraw = true;
                        return outcome;
                    }
                    Key::Named(NamedKey::Backspace) => {
                        let word = modifiers.control_key() || modifiers.alt_key();
                        match field {
                            AiTextField::OllamaUrl => {
                                if backspace_text(&mut config.ai_ollama_url, word) {
                                    outcome.ai_config_changed = true;
                                    outcome.redraw = true;
                                }
                            }
                            AiTextField::CostCap => {
                                if backspace_text(&mut self.ai.cost_cap_draft, word) {
                                    outcome.redraw = true;
                                }
                            }
                            AiTextField::AllowListAdd => {
                                if backspace_text(&mut self.ai.allow_list_draft, word) {
                                    outcome.redraw = true;
                                }
                            }
                            AiTextField::ApiKey(_) => {
                                if backspace_text(&mut self.ai.key_draft, word) {
                                    outcome.redraw = true;
                                }
                            }
                        }
                        return outcome;
                    }
                    _ => {
                        if !modifiers.control_key() && !modifiers.alt_key() {
                            if let Some(t) = text.or(match key {
                                Key::Character(c) => Some(c.as_str()),
                                _ => None,
                            }) {
                                return self.on_text(t, config);
                            }
                        }
                        return outcome;
                    }
                }
            }
        }

        // 5a. Commands list: arrows, Enter edits, Delete removes, Ctrl+N / Insert adds.
        if self.section == SettingsSection::Commands {
            let visible = SettingsLayout::build(surface, scale, self, config)
                .commands
                .as_ref()
                .map_or(1, |c| c.visible_rows);
            let result = self
                .commands_editor
                .on_list_key(key, modifiers, config, visible);
            return Self::commands_outcome(result);
        }

        // 5b. Keys section content: searchable, editable shortcut list.
        if self.section == SettingsSection::Keys {
            let count = self.filtered_commands().len();
            let visible = SettingsLayout::build(surface, scale, self, config)
                .keys_visible_rows
                .max(1);
            match key {
                Key::Named(NamedKey::Enter) => {
                    if let Some(cmd_name) =
                        self.filtered_commands().get(self.selected).map(|c| c.name)
                    {
                        self.capturing_key = Some(cmd_name);
                        self.key_capture_notice = None;
                        outcome.redraw = true;
                    }
                    return outcome;
                }
                Key::Named(NamedKey::ArrowDown) => {
                    if count > 0 {
                        self.selected = (self.selected.min(count - 1) + 1) % count;
                        self.keep_keys_row_visible(visible);
                        outcome.redraw = true;
                    }
                    return outcome;
                }
                Key::Named(NamedKey::ArrowUp) => {
                    if count > 0 {
                        self.selected = (self.selected.min(count - 1) + count - 1) % count;
                        self.keep_keys_row_visible(visible);
                        outcome.redraw = true;
                    }
                    return outcome;
                }
                Key::Named(NamedKey::PageDown) => {
                    if count > 0 {
                        self.selected = (self.selected + visible).min(count - 1);
                        self.keep_keys_row_visible(visible);
                        outcome.redraw = true;
                    }
                    return outcome;
                }
                Key::Named(NamedKey::PageUp) => {
                    if count > 0 {
                        self.selected = self.selected.saturating_sub(visible);
                        self.keep_keys_row_visible(visible);
                        outcome.redraw = true;
                    }
                    return outcome;
                }
                Key::Named(NamedKey::Backspace) => {
                    if backspace_text(&mut self.keys_query, modifiers.control_key()) {
                        self.selected = 0;
                        self.keys_scroll = 0;
                        outcome.redraw = true;
                    }
                    return outcome;
                }
                _ => {
                    if let Some(t) = text.filter(|_| {
                        !modifiers.control_key() && !modifiers.alt_key() && !modifiers.super_key()
                    }) {
                        return self.on_text(t, config);
                    }
                    return outcome;
                }
            }
        }

        // 6. Controls in Appearance, Behaviour, and About.
        let controls = Self::controls_in(self.section);
        let Some(&setting) = controls.get(self.selected) else {
            return outcome;
        };
        match key {
            Key::Named(NamedKey::ArrowDown) => {
                if !controls.is_empty() {
                    self.selected = (self.selected + 1) % controls.len();
                    self.ensure_focused_visible(config, surface, scale);
                    outcome.redraw = true;
                }
                outcome
            }
            Key::Named(NamedKey::ArrowUp) => {
                if !controls.is_empty() {
                    self.selected = (self.selected + controls.len() - 1) % controls.len();
                    self.ensure_focused_visible(config, surface, scale);
                    outcome.redraw = true;
                }
                outcome
            }
            Key::Named(NamedKey::ArrowLeft) => {
                self.step_control_horizontal(setting, -1, config, &mut outcome);
                outcome
            }
            Key::Named(NamedKey::ArrowRight) => {
                self.step_control_horizontal(setting, 1, config, &mut outcome);
                outcome
            }
            Key::Named(NamedKey::Enter) => {
                self.activate_control(setting, config, &mut outcome, true);
                self.ensure_focused_visible(config, surface, scale);
                outcome
            }
            Key::Named(NamedKey::Space) => {
                if is_text_setting(setting) {
                    return self.on_text(" ", config);
                }
                self.activate_control(setting, config, &mut outcome, false);
                self.ensure_focused_visible(config, surface, scale);
                outcome
            }
            Key::Named(NamedKey::Backspace) if is_text_setting(setting) => {
                let word = modifiers.control_key();
                self.edit_text(setting, config, |draft| backspace_text(draft, word))
            }
            _ if is_text_setting(setting) => {
                if let Some(t) = text.filter(|_| {
                    !modifiers.control_key() && !modifiers.alt_key() && !modifiers.super_key()
                }) {
                    return self.on_text(t, config);
                }
                outcome
            }
            _ => outcome,
        }
    }

    fn handle_key_capture(
        &mut self,
        cmd_name: &'static str,
        physical: Option<KeyCode>,
        key: &Key,
        modifiers: ModifiersState,
        config: &mut TwittyConfig,
    ) -> SettingsOutcome {
        let mut outcome = SettingsOutcome::default();
        // Plain Esc cancels capture without closing settings.
        if matches!(key, Key::Named(NamedKey::Escape)) && modifiers.is_empty() {
            self.capturing_key = None;
            self.key_capture_notice = None;
            outcome.redraw = true;
            return outcome;
        }
        // Plain Backspace clears / unbinds the command.
        if matches!(key, Key::Named(NamedKey::Backspace)) && modifiers.is_empty() {
            self.capturing_key = None;
            self.key_capture_notice = None;
            let changed = if default_chords_for(cmd_name).is_empty() {
                config.keybindings.remove(cmd_name)
            } else {
                let already_unbound =
                    config.keybindings.get(cmd_name) == Some(&KeybindingValue::Unbind);
                if already_unbound {
                    false
                } else {
                    config.keybindings.set(cmd_name, KeybindingValue::Unbind);
                    true
                }
            };
            outcome.keybindings_changed = changed;
            outcome.redraw = true;
            return outcome;
        }
        // Lone modifier key presses keep waiting for the actual key.
        if matches!(
            key,
            Key::Named(
                NamedKey::Shift
                    | NamedKey::Control
                    | NamedKey::Alt
                    | NamedKey::AltGraph
                    | NamedKey::Super
                    | NamedKey::Meta
                    | NamedKey::CapsLock
                    | NamedKey::NumLock
                    | NamedKey::ScrollLock
            )
        ) || physical.is_some_and(is_modifier_key_code)
        {
            return outcome;
        }
        let Some(code) = shortcut_key_code(physical, key).filter(|&c| is_bindable_key_code(c))
        else {
            self.key_capture_notice = Some(KeyCaptureNotice::UnsupportedKey);
            outcome.redraw = true;
            return outcome;
        };
        let chord = Chord::from_modifiers(modifiers, code);
        if chord.swallows_plain_typing() {
            self.key_capture_notice = Some(KeyCaptureNotice::PlainTyping);
            outcome.redraw = true;
            return outcome;
        }
        set_captured_chord(&mut config.keybindings, cmd_name, chord);
        self.capturing_key = None;
        self.key_capture_notice = None;
        outcome.keybindings_changed = true;
        outcome.redraw = true;
        outcome
    }

    fn focus_next(&mut self) {
        let count = Self::controls_in(self.section).len();
        match self.focus {
            FocusZone::Sidebar => {
                self.focus = FocusZone::Content;
                self.selected = 0;
            }
            FocusZone::Content => {
                if count == 0 || self.selected + 1 >= count {
                    self.focus = FocusZone::Sidebar;
                } else {
                    self.selected += 1;
                }
            }
        }
    }

    fn focus_prev(&mut self) {
        let count = Self::controls_in(self.section).len();
        match self.focus {
            FocusZone::Sidebar => {
                self.focus = FocusZone::Content;
                self.selected = count.saturating_sub(1);
            }
            FocusZone::Content => {
                if count == 0 || self.selected == 0 {
                    self.focus = FocusZone::Sidebar;
                } else {
                    self.selected -= 1;
                }
            }
        }
    }

    fn move_picker_highlight(&mut self, next: usize, len: usize) {
        if let Some(exp) = &mut self.expanded {
            let next = next.min(len.saturating_sub(1));
            exp.highlighted = next;
            if next < exp.scroll {
                exp.scroll = next;
            } else if next >= exp.scroll + MAX_PICKER_ROWS {
                exp.scroll = next + 1 - MAX_PICKER_ROWS;
            }
        }
    }

    pub(super) fn commands_outcome(
        result: crate::ui::settings_commands::CommandsOutcome,
    ) -> SettingsOutcome {
        SettingsOutcome {
            redraw: result.redraw,
            commands_changed: result.changed,
            ..Default::default()
        }
    }

    fn keep_keys_row_visible(&mut self, visible: usize) {
        if self.selected < self.keys_scroll {
            self.keys_scroll = self.selected;
        } else if self.selected >= self.keys_scroll + visible {
            self.keys_scroll = self.selected + 1 - visible;
        }
    }

    fn ensure_focused_visible(&mut self, config: &TwittyConfig, surface: (f32, f32), scale: f32) {
        if self.focus != FocusZone::Content
            || matches!(
                self.section,
                SettingsSection::Ai
                    | SettingsSection::Commands
                    | SettingsSection::Keys
                    | SettingsSection::About
            )
        {
            return;
        }
        let layout = SettingsLayout::build(surface, scale, self, config);
        let Some(row) = layout.rows.get(self.selected) else {
            return;
        };
        let top = row.rect.y;
        let bottom = row
            .picker
            .as_ref()
            .map_or(row.rect.bottom(), |p| p.rect.bottom());
        if top < layout.viewport.y {
            self.scroll_y = (self.scroll_y - (layout.viewport.y - top) / scale).max(0.0);
        } else if bottom > layout.viewport.bottom() {
            self.scroll_y = (self.scroll_y + (bottom - layout.viewport.bottom()) / scale)
                .min(layout.max_scroll);
        }
    }

    /// Step a segmented control, slider, or swatch picker left (`-1`) or right (`+1`).
    fn step_control_horizontal(
        &mut self,
        setting: SettingId,
        delta: i32,
        config: &mut TwittyConfig,
        outcome: &mut SettingsOutcome,
    ) {
        let logical_delta = if self.mirrored()
            && matches!(
                setting.descriptor().kind,
                ControlKind::Segmented(_) | ControlKind::Swatches
            ) {
            -delta
        } else {
            delta
        };
        match setting.descriptor().kind {
            ControlKind::Segmented(options) => {
                let current = read_segmented_index(setting, config);
                let next =
                    (current as i32 + logical_delta).rem_euclid(options.len() as i32) as usize;
                if write_segmented_index(setting, next, config) {
                    if setting == SettingId::UiLanguage {
                        self.sync_language(config);
                    }
                    outcome.changed.push(setting);
                    outcome.redraw = true;
                }
            }
            ControlKind::Slider(spec) => {
                let current = read_slider_value(setting, config);
                let next = spec.normalize(current + spec.step * delta as f32);
                if write_slider_value(setting, next, config) {
                    outcome.changed.push(setting);
                    outcome.redraw = true;
                }
            }
            ControlKind::Swatches => {
                let current = read_swatch_index(config).unwrap_or(0);
                let next = (current as i32 + logical_delta).rem_euclid(ACCENT_SWATCHES.len() as i32)
                    as usize;
                if write_swatch_index(next, config) {
                    outcome.changed.push(setting);
                    outcome.redraw = true;
                }
            }
            ControlKind::Toggle(_) => {
                let current = read_bool(setting, config);
                let desired = delta > 0;
                if current != desired && write_bool(setting, desired, config) {
                    outcome.changed.push(setting);
                    outcome.redraw = true;
                }
            }
            _ => {}
        }
    }

    fn activate_control(
        &mut self,
        setting: SettingId,
        config: &mut TwittyConfig,
        outcome: &mut SettingsOutcome,
        _is_enter: bool,
    ) {
        match setting.descriptor().kind {
            ControlKind::Toggle(_) => {
                let next = !read_bool(setting, config);
                if write_bool(setting, next, config) {
                    outcome.changed.push(setting);
                    outcome.redraw = true;
                }
            }
            ControlKind::Segmented(options) => {
                let next = (read_segmented_index(setting, config) + 1) % options.len();
                if write_segmented_index(setting, next, config) {
                    if setting == SettingId::UiLanguage {
                        self.sync_language(config);
                    }
                    outcome.changed.push(setting);
                    outcome.redraw = true;
                }
            }
            ControlKind::Swatches => {
                let next = (read_swatch_index(config).unwrap_or(0) + 1) % ACCENT_SWATCHES.len();
                if write_swatch_index(next, config) {
                    outcome.changed.push(setting);
                    outcome.redraw = true;
                }
            }
            ControlKind::ListPicker { .. } => {
                let opened = self.open_picker(setting, config);
                outcome.redraw |= opened.redraw;
                if opened.preview_theme.is_some() {
                    outcome.preview_theme = opened.preview_theme;
                }
            }
            ControlKind::ActionButton if setting == SettingId::CopyConfigPath => {
                if let Some(path) = self.config_path.clone() {
                    self.copied_config_path = true;
                    outcome.copy_text = Some(path);
                    outcome.redraw = true;
                }
            }
            ControlKind::ActionButton if setting == SettingId::ProjectMemory => {
                outcome.open_memory = true;
            }
            _ => {}
        }
    }
}
