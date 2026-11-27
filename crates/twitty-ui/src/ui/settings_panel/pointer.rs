//! Pointer input for the settings panel: hover, presses, drags and the wheel.

use super::*;

impl SettingsPanelState {
    /// Handle pointer movement (hover styling, slider drag, and live Theme hover preview).
    pub fn on_mouse_move(
        &mut self,
        x: f64,
        y: f64,
        config: &mut TwittyConfig,
        surface: (f32, f32),
        scale: f32,
    ) -> SettingsOutcome {
        let mut outcome = SettingsOutcome::default();
        let layout = SettingsLayout::build(surface, scale, self, config);

        if let Some(setting) = self.dragging_slider {
            if let Some(row) = layout.rows.iter().find(|r| r.id == setting) {
                if let ControlGeometry::Slider { track, .. } = &row.geometry {
                    if let ControlKind::Slider(spec) = setting.descriptor().kind {
                        let ratio = if track.w > 0.0 {
                            ((x as f32 - track.x) / track.w).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        let value = spec.value_from_ratio(ratio);
                        if write_slider_value(setting, value, config) {
                            outcome.changed.push(setting);
                            outcome.redraw = true;
                        }
                    }
                }
            }
            return outcome;
        }

        let hit = layout.hit(x, y);
        let hover = (hit != SettingsHit::Outside && hit != SettingsHit::Inside).then_some(hit);
        if self.hover != hover {
            self.hover = hover;
            outcome.redraw = true;
        }

        // Theme live preview on hover inside an expanded Theme picker.
        if let Some(exp) = &self.expanded {
            if exp.setting == SettingId::Theme {
                let hovered_theme = match hit {
                    SettingsHit::PickerItem(SettingId::Theme, idx) => self
                        .filtered_picker_options(SettingId::Theme, &exp.query)
                        .get(idx)
                        .and_then(|o| o.value.clone()),
                    _ => None,
                };
                if self.hovered_theme != hovered_theme {
                    self.hovered_theme = hovered_theme.clone();
                    self.set_preview_theme(hovered_theme, config, &mut outcome);
                }
            }
        }

        outcome
    }

    /// Pointer left the window: clear hover and revert any hover-only theme preview.
    pub fn on_mouse_leave(&mut self, config: &TwittyConfig) -> SettingsOutcome {
        let mut outcome = SettingsOutcome::default();
        if self.hover.take().is_some() {
            outcome.redraw = true;
        }
        if self.hovered_theme.take().is_some() {
            self.set_preview_theme(None, config, &mut outcome);
        }
        outcome
    }

    /// Handle left mouse button press.
    pub fn on_mouse_down(
        &mut self,
        x: f64,
        y: f64,
        config: &mut TwittyConfig,
        surface: (f32, f32),
        scale: f32,
    ) -> SettingsOutcome {
        let mut outcome = SettingsOutcome::default();
        let layout = SettingsLayout::build(surface, scale, self, config);
        let hit = layout.hit(x, y);
        match hit {
            SettingsHit::Outside | SettingsHit::CloseButton => {
                let mut revert = self.revert_preview(config);
                revert.close = true;
                return revert;
            }
            SettingsHit::Section(section) => {
                self.focus = FocusZone::Sidebar;
                let mut res = self.select_section(section, config);
                res.redraw = true;
                return res;
            }
            SettingsHit::Toggle(setting) => {
                self.focus_setting(setting);
                self.cancel_picker_into(config, &mut outcome);
                let next = !read_bool(setting, config);
                if write_bool(setting, next, config) {
                    outcome.changed.push(setting);
                }
                outcome.redraw = true;
            }
            SettingsHit::Segment(setting, index) => {
                self.focus_setting(setting);
                self.cancel_picker_into(config, &mut outcome);
                if write_segmented_index(setting, index, config) {
                    if setting == SettingId::UiLanguage {
                        self.sync_language(config);
                    }
                    outcome.changed.push(setting);
                }
                outcome.redraw = true;
            }
            SettingsHit::SliderMinus(setting) => {
                self.focus_setting(setting);
                self.cancel_picker_into(config, &mut outcome);
                if let ControlKind::Slider(spec) = setting.descriptor().kind {
                    let next = spec.normalize(read_slider_value(setting, config) - spec.step);
                    if write_slider_value(setting, next, config) {
                        outcome.changed.push(setting);
                    }
                }
                outcome.redraw = true;
            }
            SettingsHit::SliderPlus(setting) => {
                self.focus_setting(setting);
                self.cancel_picker_into(config, &mut outcome);
                if let ControlKind::Slider(spec) = setting.descriptor().kind {
                    let next = spec.normalize(read_slider_value(setting, config) + spec.step);
                    if write_slider_value(setting, next, config) {
                        outcome.changed.push(setting);
                    }
                }
                outcome.redraw = true;
            }
            SettingsHit::SliderTrack(setting, ratio) => {
                self.focus_setting(setting);
                self.cancel_picker_into(config, &mut outcome);
                self.dragging_slider = Some(setting);
                if let ControlKind::Slider(spec) = setting.descriptor().kind {
                    let next = spec.value_from_ratio(ratio);
                    if write_slider_value(setting, next, config) {
                        outcome.changed.push(setting);
                    }
                }
                outcome.redraw = true;
            }
            SettingsHit::Swatch(index) => {
                self.focus_setting(SettingId::UiAccent);
                self.cancel_picker_into(config, &mut outcome);
                if write_swatch_index(index, config) {
                    outcome.changed.push(SettingId::UiAccent);
                }
                outcome.redraw = true;
            }
            SettingsHit::TextField(setting) => {
                self.focus_setting(setting);
                self.cancel_picker_into(config, &mut outcome);
                outcome.redraw = true;
            }
            SettingsHit::PickerToggle(setting) => {
                self.focus_setting(setting);
                return self.open_picker(setting, config);
            }
            SettingsHit::PickerSearch(_) => {
                outcome.redraw = true;
            }
            SettingsHit::PickerItem(setting, idx) => {
                let query = self
                    .expanded
                    .as_ref()
                    .map(|e| e.query.clone())
                    .unwrap_or_default();
                let opts = self.filtered_picker_options(setting, &query);
                if let Some(opt) = opts.get(idx) {
                    self.commit_picker_option(setting, opt, config, &mut outcome);
                }
            }
            SettingsHit::KeysSearch => {
                self.focus = FocusZone::Content;
                self.capturing_key = None;
                self.key_capture_notice = None;
                outcome.redraw = true;
            }
            SettingsHit::KeysResetAll => {
                self.focus = FocusZone::Content;
                self.capturing_key = None;
                self.key_capture_notice = None;
                if config.keybindings.clear() {
                    outcome.keybindings_changed = true;
                }
                outcome.redraw = true;
            }
            SettingsHit::KeyRow(index) => {
                self.focus = FocusZone::Content;
                self.selected = index;
                self.capturing_key = None;
                self.key_capture_notice = None;
                outcome.redraw = true;
            }
            SettingsHit::KeyChange(index) => {
                self.focus = FocusZone::Content;
                self.selected = index;
                if let Some(cmd_name) = self.filtered_commands().get(index).map(|c| c.name) {
                    if self.capturing_key == Some(cmd_name) {
                        self.capturing_key = None;
                        self.key_capture_notice = None;
                    } else {
                        self.capturing_key = Some(cmd_name);
                        self.key_capture_notice = None;
                    }
                }
                outcome.redraw = true;
            }
            SettingsHit::KeyReset(index) => {
                self.focus = FocusZone::Content;
                self.selected = index;
                self.capturing_key = None;
                self.key_capture_notice = None;
                if let Some(cmd_name) = self.filtered_commands().get(index).map(|c| c.name) {
                    if config.keybindings.remove(cmd_name) {
                        outcome.keybindings_changed = true;
                    }
                }
                outcome.redraw = true;
            }
            SettingsHit::CopyConfigPath => {
                self.focus_setting(SettingId::CopyConfigPath);
                if let Some(path) = self.config_path.clone() {
                    self.copied_config_path = true;
                    outcome.copy_text = Some(path);
                    outcome.redraw = true;
                }
            }
            SettingsHit::AiDefaultProvider(idx) => {
                self.focus = FocusZone::Content;
                self.ai.active_field = None;
                if let Some(&(id, _)) = crate::ui::settings_ai::DEFAULT_PROVIDER_CHOICES.get(idx) {
                    let next = (!id.is_empty()).then(|| id.to_owned());
                    if config.ai_default_provider != next {
                        config.ai_default_provider = next;
                        outcome.ai_config_changed = true;
                    }
                    outcome.redraw = true;
                }
            }
            SettingsHit::AiSetKey(idx) => {
                use crate::ui::settings_ai::AiTextField;
                self.focus = FocusZone::Content;
                if self.ai.active_field == Some(AiTextField::ApiKey(idx)) {
                    self.ai.clear_key_draft();
                } else {
                    self.ai.clear_key_draft();
                    self.ai.active_field = Some(AiTextField::ApiKey(idx));
                }
                outcome.redraw = true;
            }
            SettingsHit::AiSaveKey(idx) => {
                self.focus = FocusZone::Content;
                if let Some(pair) = self.ai.take_key_secret(idx) {
                    outcome.save_secret = Some(pair);
                }
                outcome.redraw = true;
            }
            SettingsHit::AiClearKey(idx) => {
                self.focus = FocusZone::Content;
                self.ai.clear_key_draft();
                if let Some(spec) = crate::ui::settings_ai::API_PROVIDERS.get(idx) {
                    outcome.delete_secret = Some(spec.keyring_account.to_owned());
                }
                outcome.redraw = true;
            }
            SettingsHit::AiKeyInput(idx) => {
                self.focus = FocusZone::Content;
                self.ai.active_field = Some(crate::ui::settings_ai::AiTextField::ApiKey(idx));
                outcome.redraw = true;
            }
            SettingsHit::AiOllamaInput => {
                self.focus = FocusZone::Content;
                self.ai.clear_key_draft();
                self.ai.active_field = Some(crate::ui::settings_ai::AiTextField::OllamaUrl);
                outcome.redraw = true;
            }
            SettingsHit::AiCostCapInput => {
                self.focus = FocusZone::Content;
                self.ai.clear_key_draft();
                self.ai.active_field = Some(crate::ui::settings_ai::AiTextField::CostCap);
                outcome.redraw = true;
            }
            SettingsHit::AiAllowInput => {
                self.focus = FocusZone::Content;
                self.ai.clear_key_draft();
                self.ai.active_field = Some(crate::ui::settings_ai::AiTextField::AllowListAdd);
                outcome.redraw = true;
            }
            SettingsHit::AiAllowAdd => {
                self.focus = FocusZone::Content;
                if self.ai.add_allow_list_entry(config, self.language) {
                    outcome.ai_config_changed = true;
                }
                outcome.redraw = true;
            }
            SettingsHit::AiAllowRemove(idx) => {
                self.focus = FocusZone::Content;
                if self.ai.remove_allow_list_entry(idx, config) {
                    outcome.ai_config_changed = true;
                }
                outcome.redraw = true;
            }
            SettingsHit::AiClearHistory => {
                self.focus = FocusZone::Content;
                self.ai.history_cleared = true;
                outcome.clear_agent_history = true;
                outcome.redraw = true;
            }
            SettingsHit::OpenMemory => {
                self.focus_setting(SettingId::ProjectMemory);
                outcome.open_memory = true;
            }
            SettingsHit::CommandsAdd => {
                self.focus = FocusZone::Content;
                self.commands_editor.start_add();
                outcome.redraw = true;
            }
            SettingsHit::CommandRow(index) => {
                self.focus = FocusZone::Content;
                self.commands_editor.selected = index;
                outcome.redraw = true;
            }
            SettingsHit::CommandEdit(index) => {
                self.focus = FocusZone::Content;
                self.commands_editor.start_edit(index, config);
                outcome.redraw = true;
            }
            SettingsHit::CommandDelete(index) => {
                self.focus = FocusZone::Content;
                outcome.commands_changed = self.commands_editor.delete(index, config);
                outcome.redraw = true;
            }
            SettingsHit::CommandField(field) => {
                self.focus = FocusZone::Content;
                if let Some(form) = &mut self.commands_editor.form {
                    form.field = field;
                }
                outcome.redraw = true;
            }
            SettingsHit::CommandModeButton(mode) => {
                self.focus = FocusZone::Content;
                if let Some(form) = &mut self.commands_editor.form {
                    form.mode = mode;
                    form.field = crate::ui::settings_commands::FormField::Mode;
                }
                outcome.redraw = true;
            }
            SettingsHit::CommandSave => {
                self.focus = FocusZone::Content;
                outcome.commands_changed = self.commands_editor.save(config);
                outcome.redraw = true;
            }
            SettingsHit::CommandCancel => {
                self.commands_editor.form = None;
                outcome.redraw = true;
            }
            SettingsHit::Inside => {
                // Clicking empty space inside the panel collapses an open picker (reverting preview)
                // or cancels an active key capture.
                self.cancel_picker_into(config, &mut outcome);
                if self.capturing_key.take().is_some() || self.key_capture_notice.take().is_some() {
                    outcome.redraw = true;
                }
            }
        }
        outcome
    }

    pub fn on_mouse_up(&mut self) {
        self.dragging_slider = None;
    }

    /// Handle mouse wheel scrolling (`lines > 0` scrolls up, `lines < 0` scrolls down — matching
    /// Twitty's `wheel_lines` convention).
    pub fn on_wheel(
        &mut self,
        x: f64,
        y: f64,
        lines: f64,
        config: &TwittyConfig,
        surface: (f32, f32),
        scale: f32,
    ) -> SettingsOutcome {
        let mut outcome = SettingsOutcome::default();
        let layout = SettingsLayout::build(surface, scale, self, config);
        if !layout.panel.contains(x, y) {
            return outcome;
        }
        // If pointer is inside an expanded picker list with overflow, scroll the picker first.
        if let Some((setting, query, scroll)) = self
            .expanded
            .as_ref()
            .map(|e| (e.setting, e.query.clone(), e.scroll))
        {
            if let Some(row) = layout.rows.iter().find(|r| r.id == setting) {
                if let Some(picker) = &row.picker {
                    if picker.rect.contains(x, y) {
                        let total = self.filtered_picker_options(setting, &query).len();
                        let max_scroll = total.saturating_sub(MAX_PICKER_ROWS);
                        if max_scroll > 0 {
                            let step = if lines < 0.0 { 1isize } else { -1isize };
                            let next =
                                (scroll as isize + step).clamp(0, max_scroll as isize) as usize;
                            if next != scroll {
                                if let Some(exp) = &mut self.expanded {
                                    exp.scroll = next;
                                }
                                outcome.redraw = true;
                            }
                            return outcome;
                        }
                    }
                }
            }
        }
        if self.section == SettingsSection::Commands {
            if let Some(list) = &layout.commands {
                let max = config
                    .commands
                    .len()
                    .saturating_sub(list.visible_rows.max(1));
                let scroll = self.commands_editor.scroll.min(max);
                let next = if lines < 0.0 {
                    (scroll + 1).min(max)
                } else {
                    scroll.saturating_sub(1)
                };
                if next != self.commands_editor.scroll {
                    self.commands_editor.scroll = next;
                    outcome.redraw = true;
                }
            }
            return outcome;
        }
        if self.section == SettingsSection::Keys {
            let total = self.filtered_commands().len();
            let visible = layout.keys_visible_rows.max(1);
            let max_scroll = total.saturating_sub(visible);
            if max_scroll > 0 {
                let delta = (-lines).round() as isize;
                let step = if delta == 0 {
                    if lines < 0.0 {
                        1
                    } else {
                        -1
                    }
                } else {
                    delta
                };
                let next =
                    (self.keys_scroll as isize + step).clamp(0, max_scroll as isize) as usize;
                if next != self.keys_scroll {
                    self.keys_scroll = next;
                    outcome.redraw = true;
                }
            }
            return outcome;
        }
        if layout.max_scroll > 0.0 {
            let next = (self.scroll_y - lines as f32 * 28.0).clamp(0.0, layout.max_scroll);
            if (next - self.scroll_y).abs() > 0.5 {
                self.scroll_y = next;
                outcome.redraw = true;
            }
        }
        outcome
    }

    fn focus_setting(&mut self, setting: SettingId) {
        self.focus = FocusZone::Content;
        if let Some(idx) = Self::controls_in(self.section)
            .iter()
            .position(|&s| s == setting)
        {
            self.selected = idx;
        }
    }
}
