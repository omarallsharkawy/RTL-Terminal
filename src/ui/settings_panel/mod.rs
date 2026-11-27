//! Window-free state, model, layout, keyboard navigation, and pointer hit testing for the
//! Settings overlay (`Ctrl+,` or "Settings / الإعدادات" in the command palette).
//!
//! Drawing lives in `render::scene::settings`; live application and persistence hooks live in
//! `app::settings`.
//!
//! Parts: `table` (every setting and its control), `values` (reading and writing the config),
//! the state and its editing here, `keys` and `pointer` (input), `layout` (geometry and hit
//! testing).

use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey};

use crate::color::theme_names;
use crate::commands::{self, Command};
use crate::config::TwittyConfig;
use crate::input::keybindings::{
    default_chords_for, is_bindable_key_code, is_modifier_key_code, set_captured_chord, Chord,
    KeybindingValue,
};
use crate::input::shortcut_key_code;
use crate::layout::{clamp_sidebar_width, Rect, SidebarPosition};
use crate::ui::chrome::{UiLanguage, UiLanguagePref};
use crate::ui::palette::{fold, matches};
use crate::ui::settings::{clamp_font_size, MAX_FONT_SIZE, MIN_FONT_SIZE, MIN_OPACITY};

/// Maximum characters accepted in a settings text or search field.
const MAX_INPUT_CHARS: usize = 200;

/// Maximum visible rows inside an expanded inline picker (Theme / Font list).
pub const MAX_PICKER_ROWS: usize = 6;

mod keys;
mod layout;
mod pointer;
mod table;
mod values;

pub use layout::*;
pub use table::*;
pub use values::*;

/// Which zone of the settings panel currently owns keyboard navigation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FocusZone {
    Sidebar,
    #[default]
    Content,
}

/// An inline expanded picker list (Theme, Terminal font, or UI font).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpandedPicker {
    pub setting: SettingId,
    pub query: String,
    pub highlighted: usize,
    pub scroll: usize,
}

/// Inline feedback shown during shortcut capture when a pressed key cannot be bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCaptureNotice {
    /// Unmodified or Shift-only printable/control key that would swallow ordinary typing.
    PlainTyping,
    /// Key code is not supported as a shortcut.
    UnsupportedKey,
}

impl KeyCaptureNotice {
    pub fn message(self, language: UiLanguage) -> &'static str {
        match self {
            Self::PlainTyping => language.text(
                "Cannot bind plain typing key — add Ctrl, Alt, or Super",
                "لا يمكن ربط مفتاح كتابة عادي — أضف Ctrl أو Alt أو Super",
            ),
            Self::UnsupportedKey => {
                language.text("Unsupported shortcut key", "مفتاح اختصار غير مدعوم")
            }
        }
    }
}

/// Result of handling one input event on the settings panel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingsOutcome {
    pub close: bool,
    pub redraw: bool,
    /// Config settings that were modified and should be applied live + saved.
    pub changed: Vec<SettingId>,
    /// Keybindings in `config.keybindings` were modified and should be applied live + saved.
    pub keybindings_changed: bool,
    /// Live theme preview update: `Some(theme_name_opt)` tells the app to preview that theme
    /// (or revert to the committed theme) without modifying `config.theme`.
    pub preview_theme: Option<Option<String>>,
    /// Text to copy to the clipboard (About -> Copy config path).
    pub copy_text: Option<String>,
    /// AI section config keys changed and should be saved (`config.save()`).
    pub ai_config_changed: bool,
    /// Save an API key `(account, secret)` into the system keyring (never `config.json`).
    pub save_secret: Option<(String, twitty_store::SecretValue)>,
    /// Delete an API key `account` from the system keyring.
    pub delete_secret: Option<String>,
    /// Clear in-memory agent panel session history.
    pub clear_agent_history: bool,
    /// Open the Project Memory overlay (Settings -> AI -> Project memory).
    pub open_memory: bool,
    /// `config.commands` changed (Settings -> Commands): save and reload commands.
    pub commands_changed: bool,
}

/// One option inside a picker list (Theme / Font family).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerOption {
    /// Value stored in config (`None` = system default font; themes always carry a name).
    pub value: Option<String>,
    pub label_en: String,
    pub label_ar: String,
}

impl PickerOption {
    pub fn label(&self, language: UiLanguage) -> &str {
        match language {
            UiLanguage::English => &self.label_en,
            UiLanguage::Arabic => &self.label_ar,
        }
    }
}

/// Window-free state of the Settings overlay.
#[derive(Clone, Debug)]
pub struct SettingsPanelState {
    pub section: SettingsSection,
    pub focus: FocusZone,
    /// Focused control index within the active section (or highlighted row in `Keys`).
    pub selected: usize,
    /// Content scroll offset in logical pixels (for Appearance / Behaviour).
    pub scroll_y: f32,
    /// Currently expanded inline picker, if any.
    pub expanded: Option<ExpandedPicker>,
    /// Uncommitted theme preview currently shown on the window (`None` = showing committed theme).
    preview_theme: Option<String>,
    /// Theme currently hovered by the pointer inside the expanded Theme picker.
    hovered_theme: Option<String>,
    /// Slider currently being dragged by the left mouse button.
    dragging_slider: Option<SettingId>,
    /// Search query in the `Keys` section.
    pub keys_query: String,
    /// Scroll offset (in rows) in the `Keys` section.
    pub keys_scroll: usize,
    /// Stable command id currently capturing a new key chord in `Keys`, if any.
    pub capturing_key: Option<&'static str>,
    /// Inline feedback shown if the last captured key was rejected (e.g. plain typing).
    pub key_capture_notice: Option<KeyCaptureNotice>,
    /// Whether the user just copied the config path in `About`.
    pub copied_config_path: bool,
    /// Installed monospace font families from the system font database.
    pub monospace_fonts: Vec<String>,
    /// All installed font families from the system font database.
    pub ui_fonts: Vec<String>,
    /// Display string of the config file path.
    pub config_path: Option<String>,
    /// UI language (English = LTR, Arabic = RTL mirrored).
    pub language: UiLanguage,
    /// System locale fallback used when `config.ui_language == "auto"`.
    pub env_language: UiLanguage,
    /// Currently hovered hit target (for hover fills and cursor icon).
    pub hover: Option<SettingsHit>,
    /// Cached built-in commands for the `Keys` section.
    commands: Vec<Command>,
    /// State for the Settings → AI section (AGENT.md §1.6).
    pub ai: crate::ui::settings_ai::AiSettingsState,
    /// Raw text of the text field being edited, e.g. `~/a, ` before the next folder is typed.
    text_draft: Option<(SettingId, String)>,
    /// State for the Settings → Commands section.
    pub commands_editor: crate::ui::settings_commands::CommandsEditorState,
}

impl Default for SettingsPanelState {
    fn default() -> Self {
        Self::new(Vec::new(), Vec::new(), None, UiLanguage::English)
    }
}

impl SettingsPanelState {
    pub fn new(
        monospace_fonts: Vec<String>,
        ui_fonts: Vec<String>,
        config_path: Option<String>,
        language: UiLanguage,
    ) -> Self {
        Self {
            section: SettingsSection::Appearance,
            focus: FocusZone::Content,
            selected: 0,
            scroll_y: 0.0,
            expanded: None,
            preview_theme: None,
            hovered_theme: None,
            dragging_slider: None,
            keys_query: String::new(),
            keys_scroll: 0,
            capturing_key: None,
            key_capture_notice: None,
            copied_config_path: false,
            text_draft: None,
            monospace_fonts,
            ui_fonts,
            config_path,
            language,
            env_language: language,
            hover: None,
            commands: commands::all(),
            ai: crate::ui::settings_ai::AiSettingsState::default(),
            commands_editor: Default::default(),
        }
    }

    /// Override the system locale fallback used when `config.ui_language == "auto"`.
    pub fn with_env_language(mut self, env_language: UiLanguage) -> Self {
        self.env_language = env_language;
        self
    }

    /// Update the panel's effective language and RTL direction from `config`.
    /// Returns `true` when the effective language changed.
    pub fn sync_language(&mut self, config: &TwittyConfig) -> bool {
        let next = config.resolve_ui_language(self.env_language);
        if self.language != next {
            self.language = next;
            self.hover = None;
            true
        } else {
            false
        }
    }

    /// True when the layout is mirrored right-to-left (Arabic UI language).
    pub fn mirrored(&self) -> bool {
        self.language == UiLanguage::Arabic
    }

    /// The uncommitted theme currently being previewed, if any.
    pub fn preview_theme(&self) -> Option<&str> {
        self.preview_theme.as_deref()
    }

    /// The theme name effective on screen right now (preview takes precedence over committed).
    pub fn effective_theme<'a>(&'a self, config: &'a TwittyConfig) -> &'a str {
        self.preview_theme
            .as_deref()
            .or(config.theme.as_deref())
            .unwrap_or(theme_names()[0])
    }

    /// Controls listed in `section`.
    pub fn controls_in(section: SettingsSection) -> &'static [SettingId] {
        SettingId::in_section(section)
    }

    /// The currently focused setting when focus is in the content pane.
    pub fn focused_setting(&self) -> Option<SettingId> {
        if self.focus != FocusZone::Content {
            return None;
        }
        Self::controls_in(self.section).get(self.selected).copied()
    }

    /// Commands in `Keys` matching `self.keys_query` (English, Arabic, or stable name).
    pub fn filtered_commands(&self) -> Vec<&Command> {
        matches(&self.commands, &self.keys_query)
            .into_iter()
            .map(|i| &self.commands[i])
            .collect()
    }

    /// Full unfiltered list of options for a `ListPicker` setting.
    pub fn picker_options(&self, setting: SettingId) -> Vec<PickerOption> {
        match setting {
            SettingId::Theme => theme_names()
                .iter()
                .map(|&name| PickerOption {
                    value: Some(name.to_owned()),
                    label_en: name.to_owned(),
                    label_ar: name.to_owned(),
                })
                .collect(),
            SettingId::FontFamily => {
                let mut opts = Vec::with_capacity(self.monospace_fonts.len() + 1);
                opts.push(PickerOption {
                    value: None,
                    label_en: "System monospace".to_owned(),
                    label_ar: "خط النظام أحادي العرض".to_owned(),
                });
                for fam in &self.monospace_fonts {
                    opts.push(PickerOption {
                        value: Some(fam.clone()),
                        label_en: fam.clone(),
                        label_ar: fam.clone(),
                    });
                }
                opts
            }
            SettingId::UiFontFamily => {
                let mut opts = Vec::with_capacity(self.ui_fonts.len() + 1);
                opts.push(PickerOption {
                    value: None,
                    label_en: "System sans-serif".to_owned(),
                    label_ar: "خط النظام الافتراضي".to_owned(),
                });
                for fam in &self.ui_fonts {
                    opts.push(PickerOption {
                        value: Some(fam.clone()),
                        label_en: fam.clone(),
                        label_ar: fam.clone(),
                    });
                }
                opts
            }
            _ => Vec::new(),
        }
    }

    /// Filtered options for a `ListPicker` given `query`.
    pub fn filtered_picker_options(&self, setting: SettingId, query: &str) -> Vec<PickerOption> {
        let all = self.picker_options(setting);
        let words: Vec<String> = query.split_whitespace().map(fold).collect();
        if words.is_empty() {
            return all;
        }
        all.into_iter()
            .filter(|opt| {
                let en = fold(&opt.label_en);
                let ar = fold(&opt.label_ar);
                words
                    .iter()
                    .all(|w| en.contains(w.as_str()) || ar.contains(w.as_str()))
            })
            .collect()
    }

    /// True when the `Keys` section is waiting for the user to press a new shortcut chord.
    pub fn is_capturing_key(&self) -> bool {
        self.capturing_key.is_some()
    }

    /// True when keyboard text / IME commits should be routed into an active text or search box.
    pub fn accepts_text_input(&self) -> bool {
        if self.capturing_key.is_some() {
            return false;
        }
        if let Some(exp) = &self.expanded {
            return matches!(
                exp.setting.descriptor().kind,
                ControlKind::ListPicker { searchable: true }
            );
        }
        if self.focus != FocusZone::Content {
            return false;
        }
        if self.section == SettingsSection::Keys {
            return true;
        }
        if self.section == SettingsSection::Commands {
            return self.commands_editor.form.is_some();
        }
        if self.section == SettingsSection::Ai {
            return self.ai.active_field.is_some();
        }
        self.focused_setting().is_some_and(is_text_setting)
    }

    /// Text shown in a text-field setting: the in-progress draft while it is being edited
    /// (so a trailing `, ` survives), otherwise the saved value.
    pub fn text_value(&self, setting: SettingId, config: &TwittyConfig) -> String {
        match &self.text_draft {
            Some((id, draft)) if *id == setting && draft_matches(setting, draft, config) => {
                draft.clone()
            }
            _ => read_text(setting, config),
        }
    }

    /// Apply an edit to the focused text setting's draft and save the parsed value.
    fn edit_text(
        &mut self,
        setting: SettingId,
        config: &mut TwittyConfig,
        edit: impl FnOnce(&mut String) -> bool,
    ) -> SettingsOutcome {
        let mut outcome = SettingsOutcome::default();
        let mut draft = self.text_value(setting, config);
        if edit(&mut draft) {
            if write_text(setting, &draft, config) {
                outcome.changed.push(setting);
            }
            self.text_draft = Some((setting, draft));
            outcome.redraw = true;
        }
        outcome
    }

    /// Switch to `section`, closing any expanded picker and reverting uncommitted previews.
    pub fn select_section(
        &mut self,
        section: SettingsSection,
        config: &TwittyConfig,
    ) -> SettingsOutcome {
        let mut outcome = SettingsOutcome::default();
        self.cancel_picker_into(config, &mut outcome);
        if self.capturing_key.take().is_some() || self.key_capture_notice.take().is_some() {
            outcome.redraw = true;
        }
        if self.section != section {
            self.ai.clear_key_draft();
            self.ai.active_field = None;
            self.commands_editor.form = None;
            self.section = section;
            self.selected = 0;
            self.scroll_y = 0.0;
            self.copied_config_path = false;
            if section == SettingsSection::Ai {
                self.ai.sync_from_config(config);
            }
            outcome.redraw = true;
        }
        outcome
    }

    /// Revert any uncommitted theme preview (called when closing or cancelling).
    pub fn revert_preview(&mut self, config: &TwittyConfig) -> SettingsOutcome {
        let mut outcome = SettingsOutcome::default();
        self.hovered_theme = None;
        if self.preview_theme.take().is_some() {
            outcome.preview_theme = Some(config.theme.clone());
            outcome.redraw = true;
        }
        outcome
    }

    fn cancel_picker_into(&mut self, config: &TwittyConfig, outcome: &mut SettingsOutcome) -> bool {
        let was_open = self.expanded.take().is_some();
        self.hovered_theme = None;
        if self.preview_theme.take().is_some() {
            outcome.preview_theme = Some(config.theme.clone());
            outcome.redraw = true;
        }
        if was_open {
            outcome.redraw = true;
        }
        was_open
    }

    /// Set a preview theme if it differs from the current preview.
    fn set_preview_theme(
        &mut self,
        theme: Option<String>,
        config: &TwittyConfig,
        outcome: &mut SettingsOutcome,
    ) {
        let committed = config.theme.as_deref().unwrap_or(theme_names()[0]);
        let next = theme.filter(|t| !t.eq_ignore_ascii_case(committed));
        if self.preview_theme != next {
            self.preview_theme = next.clone();
            let target = next.or_else(|| config.theme.clone());
            outcome.preview_theme = Some(target);
            outcome.redraw = true;
        }
    }

    /// Expand the inline picker for `setting` and highlight its current value.
    pub fn open_picker(&mut self, setting: SettingId, config: &TwittyConfig) -> SettingsOutcome {
        let mut outcome = SettingsOutcome::default();
        if self.expanded.as_ref().is_some_and(|e| e.setting == setting) {
            self.cancel_picker_into(config, &mut outcome);
            return outcome;
        }
        self.cancel_picker_into(config, &mut outcome);
        let opts = self.picker_options(setting);
        let current = current_picker_value(setting, config);
        let highlighted = opts
            .iter()
            .position(|o| match (&o.value, &current) {
                (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
                (None, None) => true,
                _ => false,
            })
            .unwrap_or(0);
        let scroll = highlighted.saturating_sub(MAX_PICKER_ROWS.saturating_sub(1));
        self.expanded = Some(ExpandedPicker {
            setting,
            query: String::new(),
            highlighted,
            scroll,
        });
        outcome.redraw = true;
        outcome
    }

    /// Commit `option` for `setting`, updating `config` and clearing any preview.
    fn commit_picker_option(
        &mut self,
        setting: SettingId,
        option: &PickerOption,
        config: &mut TwittyConfig,
        outcome: &mut SettingsOutcome,
    ) {
        self.expanded = None;
        self.hovered_theme = None;
        self.preview_theme = None;
        let changed = match setting {
            SettingId::Theme => {
                let next = option.value.clone();
                if config.theme != next {
                    config.theme = next;
                    true
                } else {
                    // Ensure committed palette is restored if a preview was active.
                    outcome.preview_theme = Some(config.theme.clone());
                    false
                }
            }
            SettingId::FontFamily if config.font_family != option.value => {
                config.font_family = option.value.clone();
                true
            }
            SettingId::UiFontFamily if config.ui_font_family != option.value => {
                config.ui_font_family = option.value.clone();
                true
            }
            _ => false,
        };
        if changed {
            outcome.changed.push(setting);
        }
        outcome.redraw = true;
    }

    /// Handle text committed by an IME (Arabic or Latin) while settings is open.
    pub fn on_text(&mut self, text: &str, config: &mut TwittyConfig) -> SettingsOutcome {
        let mut outcome = SettingsOutcome::default();
        if self.capturing_key.is_some() {
            return outcome;
        }
        if let Some(exp) = &mut self.expanded {
            if matches!(
                exp.setting.descriptor().kind,
                ControlKind::ListPicker { searchable: true }
            ) && push_bounded(&mut exp.query, text)
            {
                exp.highlighted = 0;
                exp.scroll = 0;
                outcome.redraw = true;
            }
            return outcome;
        }
        if self.focus != FocusZone::Content {
            return outcome;
        }
        if self.section == SettingsSection::Keys {
            if push_bounded(&mut self.keys_query, text) {
                self.selected = 0;
                self.keys_scroll = 0;
                outcome.redraw = true;
            }
            return outcome;
        }
        if self.section == SettingsSection::Commands {
            return Self::commands_outcome(self.commands_editor.on_text(text));
        }
        if self.section == SettingsSection::Ai {
            use crate::ui::settings_ai::AiTextField;
            match self.ai.active_field {
                Some(AiTextField::OllamaUrl) => {
                    if push_bounded(&mut config.ai_ollama_url, text) {
                        outcome.ai_config_changed = true;
                        outcome.redraw = true;
                    }
                }
                Some(AiTextField::CostCap) => {
                    if push_bounded(&mut self.ai.cost_cap_draft, text) {
                        outcome.redraw = true;
                    }
                }
                Some(AiTextField::AllowListAdd) => {
                    if push_bounded(&mut self.ai.allow_list_draft, text) {
                        outcome.redraw = true;
                    }
                }
                Some(AiTextField::ApiKey(_)) if push_bounded(&mut self.ai.key_draft, text) => {
                    outcome.redraw = true;
                }
                Some(AiTextField::ApiKey(_)) | None => {}
            }
            return outcome;
        }
        if let Some(setting) = self.focused_setting().filter(|&s| is_text_setting(s)) {
            return self.edit_text(setting, config, |draft| push_bounded(draft, text));
        }
        outcome
    }
}
