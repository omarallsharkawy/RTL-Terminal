//! Window-free state, model, layout, keyboard navigation, and pointer hit testing for the
//! Settings overlay (`Ctrl+,` or "Settings / الإعدادات" in the command palette).
//!
//! Drawing lives in `render::scene::settings`; live application and persistence hooks live in
//! `app::settings`.

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

/// Sections shown in the settings navigation sidebar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SettingsSection {
    #[default]
    Appearance,
    Behaviour,
    Keys,
    About,
}

impl SettingsSection {
    pub const ALL: [Self; 4] = [Self::Appearance, Self::Behaviour, Self::Keys, Self::About];

    pub fn index(self) -> usize {
        match self {
            Self::Appearance => 0,
            Self::Behaviour => 1,
            Self::Keys => 2,
            Self::About => 3,
        }
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL[index % Self::ALL.len()]
    }

    pub fn label_en(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Behaviour => "Behaviour",
            Self::Keys => "Keys",
            Self::About => "About",
        }
    }

    pub fn label_ar(self) -> &'static str {
        match self {
            Self::Appearance => "المظهر",
            Self::Behaviour => "السلوك",
            Self::Keys => "الاختصارات",
            Self::About => "حول",
        }
    }

    pub fn label(self, language: UiLanguage) -> &'static str {
        language.text(self.label_en(), self.label_ar())
    }
}

/// Every configurable setting exposed in Appearance and Behaviour, plus About's copy action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingId {
    // Appearance
    Theme,
    FontFamily,
    FontSize,
    UiFontFamily,
    UiLanguage,
    UiAccent,
    BackgroundOpacity,
    CursorStyle,
    CursorBlink,
    SidebarPosition,
    SidebarWidth,
    TopBar,
    PaneHeaders,
    BlockCards,
    // Behaviour
    DesktopNotifications,
    RestoreSession,
    ShellIntegration,
    RecordHistory,
    ClaudeHooks,
    ControlSocket,
    CmuxCompat,
    AgentCommand,
    // About
    CopyConfigPath,
}

impl SettingId {
    pub const APPEARANCE: [Self; 14] = [
        Self::Theme,
        Self::FontFamily,
        Self::FontSize,
        Self::UiFontFamily,
        Self::UiLanguage,
        Self::UiAccent,
        Self::BackgroundOpacity,
        Self::CursorStyle,
        Self::CursorBlink,
        Self::SidebarPosition,
        Self::SidebarWidth,
        Self::TopBar,
        Self::PaneHeaders,
        Self::BlockCards,
    ];

    pub const BEHAVIOUR: [Self; 8] = [
        Self::DesktopNotifications,
        Self::RestoreSession,
        Self::ShellIntegration,
        Self::RecordHistory,
        Self::ClaudeHooks,
        Self::ControlSocket,
        Self::CmuxCompat,
        Self::AgentCommand,
    ];

    pub const ABOUT: [Self; 1] = [Self::CopyConfigPath];

    /// All config-backed settings (Appearance + Behaviour).
    pub const CONFIG_SETTINGS: [Self; 22] = [
        Self::Theme,
        Self::FontFamily,
        Self::FontSize,
        Self::UiFontFamily,
        Self::UiLanguage,
        Self::UiAccent,
        Self::BackgroundOpacity,
        Self::CursorStyle,
        Self::CursorBlink,
        Self::SidebarPosition,
        Self::SidebarWidth,
        Self::TopBar,
        Self::PaneHeaders,
        Self::BlockCards,
        Self::DesktopNotifications,
        Self::RestoreSession,
        Self::ShellIntegration,
        Self::RecordHistory,
        Self::ClaudeHooks,
        Self::ControlSocket,
        Self::CmuxCompat,
        Self::AgentCommand,
    ];

    pub fn section(self) -> SettingsSection {
        match self {
            Self::Theme
            | Self::FontFamily
            | Self::FontSize
            | Self::UiFontFamily
            | Self::UiLanguage
            | Self::UiAccent
            | Self::BackgroundOpacity
            | Self::CursorStyle
            | Self::CursorBlink
            | Self::SidebarPosition
            | Self::SidebarWidth
            | Self::TopBar
            | Self::PaneHeaders
            | Self::BlockCards => SettingsSection::Appearance,
            Self::DesktopNotifications
            | Self::RestoreSession
            | Self::ShellIntegration
            | Self::RecordHistory
            | Self::ClaudeHooks
            | Self::ControlSocket
            | Self::CmuxCompat
            | Self::AgentCommand => SettingsSection::Behaviour,
            Self::CopyConfigPath => SettingsSection::About,
        }
    }

    /// JSON key in `config.json` (`None` for non-config actions like CopyConfigPath).
    pub fn config_key(self) -> Option<&'static str> {
        Some(match self {
            Self::Theme => "theme",
            Self::FontFamily => "font_family",
            Self::FontSize => "font_size",
            Self::UiFontFamily => "ui_font_family",
            Self::UiLanguage => "ui_language",
            Self::UiAccent => "ui_accent",
            Self::BackgroundOpacity => "background_opacity",
            Self::CursorStyle => "cursor_style",
            Self::CursorBlink => "cursor_blink",
            Self::SidebarPosition => "sidebar",
            Self::SidebarWidth => "sidebar_width",
            Self::TopBar => "top_bar",
            Self::PaneHeaders => "pane_headers",
            Self::BlockCards => "block_cards",
            Self::DesktopNotifications => "desktop_notifications",
            Self::RestoreSession => "restore_session",
            Self::ShellIntegration => "shell_integration",
            Self::RecordHistory => "record_history",
            Self::ClaudeHooks => "claude_hooks",
            Self::ControlSocket => "control_socket",
            Self::CmuxCompat => "cmux_compat",
            Self::AgentCommand => "agent_command",
            Self::CopyConfigPath => return None,
        })
    }

    pub fn descriptor(self) -> SettingDescriptor {
        match self {
            Self::Theme => SettingDescriptor {
                id: self,
                label_en: "Theme",
                label_ar: "السمة",
                note_en: None,
                note_ar: None,
                kind: ControlKind::ListPicker { searchable: false },
            },
            Self::FontFamily => SettingDescriptor {
                id: self,
                label_en: "Terminal font",
                label_ar: "خط الطرفية",
                note_en: None,
                note_ar: None,
                kind: ControlKind::ListPicker { searchable: true },
            },
            Self::FontSize => SettingDescriptor {
                id: self,
                label_en: "Font size",
                label_ar: "حجم الخط",
                note_en: None,
                note_ar: None,
                kind: ControlKind::Slider(SliderSpec {
                    min: MIN_FONT_SIZE,
                    max: MAX_FONT_SIZE,
                    step: 0.5,
                    unit: SliderUnit::Points,
                }),
            },
            Self::UiFontFamily => SettingDescriptor {
                id: self,
                label_en: "UI font",
                label_ar: "خط الواجهة",
                note_en: None,
                note_ar: None,
                kind: ControlKind::ListPicker { searchable: true },
            },
            Self::UiLanguage => SettingDescriptor {
                id: self,
                label_en: "Interface language",
                label_ar: "لغة الواجهة",
                note_en: None,
                note_ar: None,
                kind: ControlKind::Segmented(UI_LANGUAGE_OPTIONS),
            },
            Self::UiAccent => SettingDescriptor {
                id: self,
                label_en: "Accent colour",
                label_ar: "لون التمييز",
                note_en: None,
                note_ar: None,
                kind: ControlKind::Swatches,
            },
            Self::BackgroundOpacity => SettingDescriptor {
                id: self,
                label_en: "Background opacity",
                label_ar: "شفافية الخلفية",
                note_en: None,
                note_ar: None,
                kind: ControlKind::Slider(SliderSpec {
                    min: MIN_OPACITY,
                    max: 1.0,
                    step: 0.05,
                    unit: SliderUnit::Percent,
                }),
            },
            Self::CursorStyle => SettingDescriptor {
                id: self,
                label_en: "Cursor style",
                label_ar: "شكل المؤشر",
                note_en: None,
                note_ar: None,
                kind: ControlKind::Segmented(CURSOR_STYLE_OPTIONS),
            },
            Self::CursorBlink => SettingDescriptor {
                id: self,
                label_en: "Cursor blink",
                label_ar: "وميض المؤشر",
                note_en: None,
                note_ar: None,
                kind: ControlKind::Toggle,
            },
            Self::SidebarPosition => SettingDescriptor {
                id: self,
                label_en: "Sidebar position",
                label_ar: "موضع الشريط الجانبي",
                note_en: None,
                note_ar: None,
                kind: ControlKind::Segmented(SIDEBAR_POSITION_OPTIONS),
            },
            Self::SidebarWidth => SettingDescriptor {
                id: self,
                label_en: "Sidebar width",
                label_ar: "عرض الشريط الجانبي",
                note_en: None,
                note_ar: None,
                kind: ControlKind::Slider(SliderSpec {
                    min: 140.0,
                    max: 480.0,
                    step: 10.0,
                    unit: SliderUnit::Pixels,
                }),
            },
            Self::TopBar => SettingDescriptor {
                id: self,
                label_en: "Top bar",
                label_ar: "الشريط العلوي",
                note_en: None,
                note_ar: None,
                kind: ControlKind::Toggle,
            },
            Self::PaneHeaders => SettingDescriptor {
                id: self,
                label_en: "Pane headers",
                label_ar: "عناوين اللوحات",
                note_en: None,
                note_ar: None,
                kind: ControlKind::Toggle,
            },
            Self::BlockCards => SettingDescriptor {
                id: self,
                label_en: "Block cards",
                label_ar: "بطاقات الأوامر",
                note_en: None,
                note_ar: None,
                kind: ControlKind::Toggle,
            },
            Self::DesktopNotifications => SettingDescriptor {
                id: self,
                label_en: "Desktop notifications",
                label_ar: "إشعارات سطح المكتب",
                note_en: None,
                note_ar: None,
                kind: ControlKind::Toggle,
            },
            Self::RestoreSession => SettingDescriptor {
                id: self,
                label_en: "Restore session",
                label_ar: "استعادة الجلسة",
                note_en: Some("Applies after restart"),
                note_ar: Some("يطبّق بعد إعادة التشغيل"),
                kind: ControlKind::Toggle,
            },
            Self::ShellIntegration => SettingDescriptor {
                id: self,
                label_en: "Shell integration",
                label_ar: "تكامل الشيل",
                note_en: Some("Applies to new shells"),
                note_ar: Some("يطبّق على الشيل الجديد"),
                kind: ControlKind::Toggle,
            },
            Self::RecordHistory => SettingDescriptor {
                id: self,
                label_en: "Record command history",
                label_ar: "حفظ سجل الأوامر",
                note_en: None,
                note_ar: None,
                kind: ControlKind::Toggle,
            },
            Self::ClaudeHooks => SettingDescriptor {
                id: self,
                label_en: "Claude hooks",
                label_ar: "خطافات كلود",
                note_en: Some("Applies to new shells"),
                note_ar: Some("يطبّق على الشيل الجديد"),
                kind: ControlKind::Toggle,
            },
            Self::ControlSocket => SettingDescriptor {
                id: self,
                label_en: "Control socket",
                label_ar: "مقبس التحكم",
                note_en: Some("Applies after restart"),
                note_ar: Some("يطبّق بعد إعادة التشغيل"),
                kind: ControlKind::Toggle,
            },
            Self::CmuxCompat => SettingDescriptor {
                id: self,
                label_en: "External socket compatibility",
                label_ar: "توافق مقبس التحكم الخارجي",
                note_en: Some("Applies to new shells"),
                note_ar: Some("يطبّق على الشيل الجديد"),
                kind: ControlKind::Toggle,
            },
            Self::AgentCommand => SettingDescriptor {
                id: self,
                label_en: "Agent command",
                label_ar: "أمر الوكيل",
                note_en: Some("Leave blank to auto-detect"),
                note_ar: Some("اتركه فارغاً للاكتشاف التلقائي"),
                kind: ControlKind::TextField {
                    placeholder_en: "Auto (claude, codex…)",
                    placeholder_ar: "تلقائي (claude, codex…)",
                },
            },
            Self::CopyConfigPath => SettingDescriptor {
                id: self,
                label_en: "Config file",
                label_ar: "ملف الإعدادات",
                note_en: None,
                note_ar: None,
                kind: ControlKind::ActionButton,
            },
        }
    }
}

/// Static metadata for one setting control.
#[derive(Clone, Copy, Debug)]
pub struct SettingDescriptor {
    pub id: SettingId,
    pub label_en: &'static str,
    pub label_ar: &'static str,
    pub note_en: Option<&'static str>,
    pub note_ar: Option<&'static str>,
    pub kind: ControlKind,
}

impl SettingDescriptor {
    pub fn label(&self, language: UiLanguage) -> &'static str {
        language.text(self.label_en, self.label_ar)
    }

    pub fn note(&self, language: UiLanguage) -> Option<&'static str> {
        match language {
            UiLanguage::English => self.note_en,
            UiLanguage::Arabic => self.note_ar,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegmentedOption {
    pub value: &'static str,
    pub label_en: &'static str,
    pub label_ar: &'static str,
}

impl SegmentedOption {
    pub fn label(self, language: UiLanguage) -> &'static str {
        language.text(self.label_en, self.label_ar)
    }
}

pub const UI_LANGUAGE_OPTIONS: &[SegmentedOption] = &[
    SegmentedOption {
        value: "auto",
        label_en: "Auto",
        label_ar: "تلقائي",
    },
    SegmentedOption {
        value: "en",
        label_en: "English",
        label_ar: "English",
    },
    SegmentedOption {
        value: "ar",
        label_en: "العربية",
        label_ar: "العربية",
    },
];

pub const CURSOR_STYLE_OPTIONS: &[SegmentedOption] = &[
    SegmentedOption {
        value: "block",
        label_en: "Block",
        label_ar: "مربع",
    },
    SegmentedOption {
        value: "beam",
        label_en: "Beam",
        label_ar: "شريط",
    },
    SegmentedOption {
        value: "underline",
        label_en: "Underline",
        label_ar: "خط سفلي",
    },
];

pub const SIDEBAR_POSITION_OPTIONS: &[SegmentedOption] = &[
    SegmentedOption {
        value: "left",
        label_en: "Left",
        label_ar: "يسار",
    },
    SegmentedOption {
        value: "right",
        label_en: "Right",
        label_ar: "يمين",
    },
    SegmentedOption {
        value: "hidden",
        label_en: "Hidden",
        label_ar: "مخفي",
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccentSwatch {
    pub hex: Option<&'static str>,
    pub label_en: &'static str,
    pub label_ar: &'static str,
}

pub const ACCENT_SWATCHES: &[AccentSwatch] = &[
    AccentSwatch {
        hex: None,
        label_en: "Default",
        label_ar: "افتراضي",
    },
    AccentSwatch {
        hex: Some("#7aa2f7"),
        label_en: "Blue",
        label_ar: "أزرق",
    },
    AccentSwatch {
        hex: Some("#bb9af7"),
        label_en: "Purple",
        label_ar: "بنفسجي",
    },
    AccentSwatch {
        hex: Some("#9ece6a"),
        label_en: "Green",
        label_ar: "أخضر",
    },
    AccentSwatch {
        hex: Some("#e0af68"),
        label_en: "Amber",
        label_ar: "كهرماني",
    },
    AccentSwatch {
        hex: Some("#f7768e"),
        label_en: "Rose",
        label_ar: "وردي",
    },
    AccentSwatch {
        hex: Some("#2ac3de"),
        label_en: "Cyan",
        label_ar: "سماوي",
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliderUnit {
    Points,
    Percent,
    Pixels,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderSpec {
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub unit: SliderUnit,
}

impl SliderSpec {
    pub fn format_value(self, value: f32) -> String {
        match self.unit {
            SliderUnit::Points => {
                if (value - value.round()).abs() < 0.05 {
                    format!("{:.0} pt", value.round())
                } else {
                    format!("{value:.1} pt")
                }
            }
            SliderUnit::Percent => format!("{}%", (value * 100.0).round() as i32),
            SliderUnit::Pixels => format!("{} px", value.round() as i32),
        }
    }

    pub fn normalize(self, value: f32) -> f32 {
        let clamped = value.clamp(self.min, self.max);
        let steps = ((clamped - self.min) / self.step).round();
        let snapped = (self.min + steps * self.step).clamp(self.min, self.max);
        // Avoid floating point noise like 0.85000002.
        (snapped * 100.0).round() / 100.0
    }

    pub fn ratio(self, value: f32) -> f32 {
        let span = (self.max - self.min).max(f32::EPSILON);
        ((value - self.min) / span).clamp(0.0, 1.0)
    }

    pub fn value_from_ratio(self, ratio: f32) -> f32 {
        self.normalize(self.min + ratio.clamp(0.0, 1.0) * (self.max - self.min))
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ControlKind {
    Toggle,
    Segmented(&'static [SegmentedOption]),
    Slider(SliderSpec),
    Swatches,
    TextField {
        placeholder_en: &'static str,
        placeholder_ar: &'static str,
    },
    ListPicker {
        searchable: bool,
    },
    ActionButton,
}

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
            monospace_fonts,
            ui_fonts,
            config_path,
            language,
            env_language: language,
            hover: None,
            commands: commands::all(),
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
        match section {
            SettingsSection::Appearance => &SettingId::APPEARANCE,
            SettingsSection::Behaviour => &SettingId::BEHAVIOUR,
            SettingsSection::Keys => &[],
            SettingsSection::About => &SettingId::ABOUT,
        }
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
        matches!(self.focused_setting(), Some(SettingId::AgentCommand))
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
            self.section = section;
            self.selected = 0;
            self.scroll_y = 0.0;
            self.copied_config_path = false;
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
        if self.focused_setting() == Some(SettingId::AgentCommand) {
            let mut val = config.agent_command.clone().unwrap_or_default();
            if push_bounded(&mut val, text) {
                set_agent_command(config, val);
                outcome.changed.push(SettingId::AgentCommand);
                outcome.redraw = true;
            }
        }
        outcome
    }

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

        // 5. Keys section content: searchable, editable shortcut list.
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
                if setting == SettingId::AgentCommand {
                    return self.on_text(" ", config);
                }
                self.activate_control(setting, config, &mut outcome, false);
                self.ensure_focused_visible(config, surface, scale);
                outcome
            }
            Key::Named(NamedKey::Backspace) if setting == SettingId::AgentCommand => {
                let mut val = config.agent_command.clone().unwrap_or_default();
                if backspace_text(&mut val, modifiers.control_key()) {
                    set_agent_command(config, val);
                    outcome.changed.push(SettingId::AgentCommand);
                    outcome.redraw = true;
                }
                outcome
            }
            _ if setting == SettingId::AgentCommand => {
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

    fn keep_keys_row_visible(&mut self, visible: usize) {
        if self.selected < self.keys_scroll {
            self.keys_scroll = self.selected;
        } else if self.selected >= self.keys_scroll + visible {
            self.keys_scroll = self.selected + 1 - visible;
        }
    }

    fn ensure_focused_visible(&mut self, config: &TwittyConfig, surface: (f32, f32), scale: f32) {
        if self.focus != FocusZone::Content
            || matches!(self.section, SettingsSection::Keys | SettingsSection::About)
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
            ControlKind::Toggle => {
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
            ControlKind::Toggle => {
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
            _ => {}
        }
    }

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

// ---- config accessors (every control maps to a TwittyConfig field) ----

pub fn read_bool(setting: SettingId, config: &TwittyConfig) -> bool {
    match setting {
        SettingId::CursorBlink => config.cursor_blink,
        SettingId::TopBar => config.top_bar,
        SettingId::PaneHeaders => config.pane_headers,
        SettingId::BlockCards => config.block_cards,
        SettingId::DesktopNotifications => config.desktop_notifications,
        SettingId::RestoreSession => config.restore_session,
        SettingId::ShellIntegration => config.shell_integration,
        SettingId::RecordHistory => config.record_history,
        SettingId::ClaudeHooks => config.claude_hooks,
        SettingId::ControlSocket => config.control_socket,
        SettingId::CmuxCompat => config.cmux_compat,
        _ => false,
    }
}

pub fn write_bool(setting: SettingId, value: bool, config: &mut TwittyConfig) -> bool {
    let slot = match setting {
        SettingId::CursorBlink => &mut config.cursor_blink,
        SettingId::TopBar => &mut config.top_bar,
        SettingId::PaneHeaders => &mut config.pane_headers,
        SettingId::BlockCards => &mut config.block_cards,
        SettingId::DesktopNotifications => &mut config.desktop_notifications,
        SettingId::RestoreSession => &mut config.restore_session,
        SettingId::ShellIntegration => &mut config.shell_integration,
        SettingId::RecordHistory => &mut config.record_history,
        SettingId::ClaudeHooks => &mut config.claude_hooks,
        SettingId::ControlSocket => &mut config.control_socket,
        SettingId::CmuxCompat => &mut config.cmux_compat,
        _ => return false,
    };
    if *slot != value {
        *slot = value;
        true
    } else {
        false
    }
}

pub fn read_segmented_index(setting: SettingId, config: &TwittyConfig) -> usize {
    match setting {
        SettingId::UiLanguage => match config.ui_language_pref() {
            UiLanguagePref::Auto => 0,
            UiLanguagePref::English => 1,
            UiLanguagePref::Arabic => 2,
        },
        SettingId::CursorStyle => match config.cursor_style.as_str() {
            "beam" | "bar" => 1,
            "underline" => 2,
            _ => 0,
        },
        SettingId::SidebarPosition => match config
            .sidebar
            .parse::<SidebarPosition>()
            .unwrap_or_default()
        {
            SidebarPosition::Left => 0,
            SidebarPosition::Right => 1,
            SidebarPosition::Hidden => 2,
        },
        _ => 0,
    }
}

pub fn write_segmented_index(setting: SettingId, index: usize, config: &mut TwittyConfig) -> bool {
    let ControlKind::Segmented(options) = setting.descriptor().kind else {
        return false;
    };
    let Some(opt) = options.get(index) else {
        return false;
    };
    match setting {
        SettingId::UiLanguage if config.ui_language != opt.value => {
            config.ui_language = opt.value.to_owned();
            true
        }
        SettingId::CursorStyle if config.cursor_style != opt.value => {
            config.cursor_style = opt.value.to_owned();
            true
        }
        SettingId::SidebarPosition if config.sidebar != opt.value => {
            config.sidebar = opt.value.to_owned();
            true
        }
        _ => false,
    }
}

pub fn read_slider_value(setting: SettingId, config: &TwittyConfig) -> f32 {
    match setting {
        SettingId::FontSize => config.font_size,
        SettingId::BackgroundOpacity => config.background_opacity,
        SettingId::SidebarWidth => config.sidebar_width,
        _ => 0.0,
    }
}

pub fn write_slider_value(setting: SettingId, value: f32, config: &mut TwittyConfig) -> bool {
    match setting {
        SettingId::FontSize => {
            let next = clamp_font_size(value);
            if (config.font_size - next).abs() > 1e-4 {
                config.font_size = next;
                true
            } else {
                false
            }
        }
        SettingId::BackgroundOpacity => {
            let next = value.clamp(MIN_OPACITY, 1.0);
            if (config.background_opacity - next).abs() > 1e-4 {
                config.background_opacity = next;
                true
            } else {
                false
            }
        }
        SettingId::SidebarWidth => {
            let next = clamp_sidebar_width(value);
            if (config.sidebar_width - next).abs() > 1e-4 {
                config.sidebar_width = next;
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

pub fn read_swatch_index(config: &TwittyConfig) -> Option<usize> {
    match config.ui_accent.as_deref() {
        None => Some(0),
        Some(hex) => ACCENT_SWATCHES
            .iter()
            .position(|s| s.hex.is_some_and(|h| h.eq_ignore_ascii_case(hex.trim()))),
    }
}

pub fn write_swatch_index(index: usize, config: &mut TwittyConfig) -> bool {
    let Some(swatch) = ACCENT_SWATCHES.get(index) else {
        return false;
    };
    let next = swatch.hex.map(str::to_owned);
    if config.ui_accent != next {
        config.ui_accent = next;
        true
    } else {
        false
    }
}

pub fn current_picker_value(setting: SettingId, config: &TwittyConfig) -> Option<String> {
    match setting {
        SettingId::Theme => Some(
            config
                .theme
                .clone()
                .unwrap_or_else(|| theme_names()[0].to_owned()),
        ),
        SettingId::FontFamily => config.font_family.clone(),
        SettingId::UiFontFamily => config.ui_font_family.clone(),
        _ => None,
    }
}

fn set_agent_command(config: &mut TwittyConfig, value: String) {
    let cleaned = crate::shell_integration::clean_text(&value, MAX_INPUT_CHARS);
    config.agent_command = if cleaned.trim().is_empty() {
        None
    } else {
        Some(cleaned)
    };
}

fn push_bounded(target: &mut String, text: &str) -> bool {
    let before = target.len();
    for c in text.chars().filter(|c| !c.is_control()) {
        if target.chars().count() >= MAX_INPUT_CHARS {
            break;
        }
        target.push(c);
    }
    target.len() != before
}

fn backspace_text(target: &mut String, whole_word: bool) -> bool {
    if target.is_empty() {
        return false;
    }
    if whole_word {
        let trimmed = target.trim_end();
        let cut = trimmed
            .char_indices()
            .rev()
            .find(|&(_, c)| c.is_whitespace())
            .map_or(0, |(i, c)| i + c.len_utf8());
        target.truncate(cut);
    } else {
        target.pop();
    }
    true
}

// ---- shared geometry & hit testing ----

/// Pointer hit target inside (or outside) the settings overlay.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SettingsHit {
    Outside,
    Inside,
    CloseButton,
    Section(SettingsSection),
    Toggle(SettingId),
    Segment(SettingId, usize),
    SliderMinus(SettingId),
    SliderPlus(SettingId),
    SliderTrack(SettingId, f32),
    Swatch(usize),
    TextField(SettingId),
    PickerToggle(SettingId),
    PickerSearch(SettingId),
    PickerItem(SettingId, usize),
    KeysSearch,
    KeysResetAll,
    KeyRow(usize),
    KeyChange(usize),
    KeyReset(usize),
    CopyConfigPath,
}

impl SettingsHit {
    /// True when the pointer should show a clickable hand cursor over this target.
    pub fn is_interactive(self) -> bool {
        !matches!(self, Self::Outside | Self::Inside)
    }

    /// True when the pointer should show a text I-beam cursor over this target.
    pub fn is_text_input(self) -> bool {
        matches!(
            self,
            Self::TextField(_) | Self::PickerSearch(_) | Self::KeysSearch
        )
    }
}

#[derive(Clone, Debug)]
pub enum ControlGeometry {
    Toggle {
        track: Rect,
    },
    Segmented {
        container: Rect,
        segments: Vec<Rect>,
    },
    Slider {
        minus: Rect,
        track: Rect,
        plus: Rect,
        value: Rect,
    },
    Swatches {
        swatches: Vec<Rect>,
    },
    TextField {
        input: Rect,
    },
    ListPicker {
        button: Rect,
    },
    ActionButton {
        button: Rect,
    },
}

#[derive(Clone, Debug)]
pub struct ExpandedPickerLayout {
    pub rect: Rect,
    pub search: Option<Rect>,
    /// Visible rows: `(filtered_option_index, row_rect)`.
    pub items: Vec<(usize, Rect)>,
    pub total_items: usize,
    pub scroll: usize,
}

#[derive(Clone, Debug)]
pub struct ControlRowLayout {
    pub id: SettingId,
    pub rect: Rect,
    pub label: Rect,
    pub widget: Rect,
    pub geometry: ControlGeometry,
    pub picker: Option<ExpandedPickerLayout>,
}

/// Physical-pixel geometry for one visible row in the `Keys` section.
#[derive(Clone, Copy, Debug)]
pub struct KeyRowLayout {
    pub cmd_idx: usize,
    pub rect: Rect,
    pub change_button: Rect,
    pub reset_button: Option<Rect>,
}

/// Physical-pixel geometry shared by `render::scene::settings` and pointer hit testing.
#[derive(Clone, Debug)]
pub struct SettingsLayout {
    pub panel: Rect,
    pub header: Rect,
    pub header_title: Rect,
    pub close_button: Rect,
    pub sidebar: Rect,
    pub divider: Rect,
    pub content: Rect,
    pub viewport: Rect,
    pub sections: [(SettingsSection, Rect); 4],
    pub rows: Vec<ControlRowLayout>,
    pub keys_search: Option<Rect>,
    pub keys_reset_all: Option<Rect>,
    pub keys_banner: Option<Rect>,
    /// Visible shortcut rows: `(filtered_command_index, row_rect)`.
    pub keys_rows: Vec<(usize, Rect)>,
    /// Detailed per-row geometry for the `Keys` section (including Change and Reset buttons).
    pub keys_row_layouts: Vec<KeyRowLayout>,
    pub keys_visible_rows: usize,
    pub about_header: Option<Rect>,
    pub about_info_rows: Vec<Rect>,
    pub copy_button: Option<Rect>,
    pub scrollbar: Option<Rect>,
    /// Maximum vertical scroll in logical pixels for Appearance/Behaviour.
    pub max_scroll: f32,
    pub mirrored: bool,
}

impl SettingsLayout {
    pub fn build(
        surface: (f32, f32),
        scale: f32,
        state: &SettingsPanelState,
        config: &TwittyConfig,
    ) -> Self {
        let px = |logical: f32| logical * scale;
        let mirrored = state.mirrored();
        let (sw, sh) = surface;
        let margin = px(16.0).min(sw / 8.0).min(sh / 8.0);
        let panel_w = px(760.0).min((sw - 2.0 * margin).max(0.0)).round();
        let panel_h = px(560.0).min((sh - 2.0 * margin).max(0.0)).round();
        let panel_x = ((sw - panel_w) / 2.0).round();
        let panel_y = ((sh - panel_h) / 2.0).round();
        let panel = Rect::new(panel_x, panel_y, panel_w, panel_h);

        let header_h = px(44.0).round().min(panel_h);
        let header = Rect::new(panel_x, panel_y, panel_w, header_h);
        let hpad = px(14.0).min(panel_w / 6.0);
        let close_side = px(26.0).min(header_h).min(panel_w / 4.0);
        let close_y = header.y + (header.h - close_side) / 2.0;
        let (close_button, header_title) = if mirrored {
            let close = Rect::new(header.x + hpad, close_y, close_side, close_side);
            let title_x = close.right() + px(8.0);
            let title = Rect::new(
                title_x,
                header.y,
                (header.right() - hpad - title_x).max(0.0),
                header.h,
            );
            (close, title)
        } else {
            let close = Rect::new(
                header.right() - hpad - close_side,
                close_y,
                close_side,
                close_side,
            );
            let title = Rect::new(
                header.x + hpad,
                header.y,
                (close.x - px(8.0) - (header.x + hpad)).max(0.0),
                header.h,
            );
            (close, title)
        };

        let body_y = header.bottom();
        let body_h = (panel.bottom() - body_y).max(0.0);
        let sidebar_w = px(164.0).min(panel_w * 0.32).round();
        let div_w = px(1.0).max(1.0);
        let (sidebar, divider, content) = if mirrored {
            let sb = Rect::new(panel.right() - sidebar_w, body_y, sidebar_w, body_h);
            let div = Rect::new(sb.x - div_w, body_y, div_w, body_h);
            let ct = Rect::new(panel.x, body_y, (div.x - panel.x).max(0.0), body_h);
            (sb, div, ct)
        } else {
            let sb = Rect::new(panel.x, body_y, sidebar_w, body_h);
            let div = Rect::new(sb.right(), body_y, div_w, body_h);
            let ct = Rect::new(
                div.right(),
                body_y,
                (panel.right() - div.right()).max(0.0),
                body_h,
            );
            (sb, div, ct)
        };

        let sb_pad = px(10.0).min(sidebar.w / 6.0);
        let sb_item_h = px(34.0).round();
        let sb_gap = px(4.0).round();
        let sections = SettingsSection::ALL.map(|sec| {
            let i = sec.index() as f32;
            let rect = Rect::new(
                sidebar.x + sb_pad,
                sidebar.y + sb_pad + i * (sb_item_h + sb_gap),
                (sidebar.w - 2.0 * sb_pad).max(0.0),
                sb_item_h,
            );
            (sec, rect)
        });

        let cpad = px(16.0).min(content.w / 8.0);
        let sb_track_w = px(6.0);
        let (vp_x, vp_w) = if mirrored {
            (
                content.x + cpad + sb_track_w,
                (content.w - 2.0 * cpad - sb_track_w).max(0.0),
            )
        } else {
            (
                content.x + cpad,
                (content.w - 2.0 * cpad - sb_track_w).max(0.0),
            )
        };
        let viewport = Rect::new(
            vp_x,
            content.y + cpad,
            vp_w,
            (content.h - 2.0 * cpad).max(0.0),
        );

        let mut layout = Self {
            panel,
            header,
            header_title,
            close_button,
            sidebar,
            divider,
            content,
            viewport,
            sections,
            rows: Vec::new(),
            keys_search: None,
            keys_reset_all: None,
            keys_banner: None,
            keys_rows: Vec::new(),
            keys_row_layouts: Vec::new(),
            keys_visible_rows: 0,
            about_header: None,
            about_info_rows: Vec::new(),
            copy_button: None,
            scrollbar: None,
            max_scroll: 0.0,
            mirrored,
        };

        match state.section {
            SettingsSection::Appearance | SettingsSection::Behaviour => {
                layout.build_control_rows(scale, state, config);
            }
            SettingsSection::Keys => {
                layout.build_keys_layout(scale, state, config);
            }
            SettingsSection::About => {
                layout.build_about_layout(scale);
            }
        }

        layout
    }

    fn build_control_rows(
        &mut self,
        scale: f32,
        state: &SettingsPanelState,
        _config: &TwittyConfig,
    ) {
        let px = |logical: f32| logical * scale;
        let vp = self.viewport;
        let controls = SettingsPanelState::controls_in(state.section);
        let row_gap = px(6.0).round();

        // First pass: measure logical height for scroll clamping.
        let mut total_h = 0.0_f32;
        for (i, &id) in controls.iter().enumerate() {
            let desc = id.descriptor();
            let row_h = if desc.note_en.is_some() {
                px(48.0).round()
            } else {
                px(40.0).round()
            };
            total_h += row_h;
            if let Some(exp) = state.expanded.as_ref().filter(|e| e.setting == id) {
                let count = state.filtered_picker_options(id, &exp.query).len();
                let visible = count.clamp(1, MAX_PICKER_ROWS);
                let searchable = matches!(desc.kind, ControlKind::ListPicker { searchable: true });
                let search_h = if searchable { px(34.0) + px(6.0) } else { 0.0 };
                let picker_h = px(12.0) + search_h + visible as f32 * px(28.0).round();
                total_h += px(4.0) + picker_h;
            }
            if i + 1 < controls.len() {
                total_h += row_gap;
            }
        }

        self.max_scroll = ((total_h - vp.h).max(0.0)) / scale;
        let scroll_px = (state.scroll_y.clamp(0.0, self.max_scroll) * scale).round();
        let mut y = vp.y - scroll_px;

        for &id in controls {
            let desc = id.descriptor();
            let row_h = if desc.note_en.is_some() {
                px(48.0).round()
            } else {
                px(40.0).round()
            };
            let rect = Rect::new(vp.x, y, vp.w, row_h);
            let inner_pad = px(10.0);
            let inner = Rect::new(
                rect.x + inner_pad,
                rect.y,
                (rect.w - 2.0 * inner_pad).max(0.0),
                rect.h,
            );
            let widget_w = match desc.kind {
                ControlKind::Toggle => px(44.0),
                ControlKind::Segmented(_) => px(220.0).min(inner.w * 0.56),
                ControlKind::Slider(_) => px(244.0).min(inner.w * 0.58),
                ControlKind::Swatches => px(228.0).min(inner.w * 0.58),
                ControlKind::TextField { .. } => px(220.0).min(inner.w * 0.56),
                ControlKind::ListPicker { .. } => px(220.0).min(inner.w * 0.56),
                ControlKind::ActionButton => px(130.0).min(inner.w * 0.45),
            };
            let gap = px(12.0);
            let (label, widget) = if self.mirrored {
                let w_rect = Rect::new(inner.x, inner.y, widget_w, inner.h);
                let l_x = w_rect.right() + gap;
                let l_rect = Rect::new(l_x, inner.y, (inner.right() - l_x).max(0.0), inner.h);
                (l_rect, w_rect)
            } else {
                let w_rect = Rect::new(inner.right() - widget_w, inner.y, widget_w, inner.h);
                let l_rect = Rect::new(
                    inner.x,
                    inner.y,
                    (w_rect.x - gap - inner.x).max(0.0),
                    inner.h,
                );
                (l_rect, w_rect)
            };

            let geometry = build_control_geometry(desc.kind, widget, scale, self.mirrored);
            y += row_h;

            let picker = if let Some(exp) = state.expanded.as_ref().filter(|e| e.setting == id) {
                let opts = state.filtered_picker_options(id, &exp.query);
                let visible = opts.len().clamp(1, MAX_PICKER_ROWS);
                let searchable = matches!(desc.kind, ControlKind::ListPicker { searchable: true });
                let pad = px(6.0);
                let search_h = px(30.0);
                let item_h = px(28.0).round();
                let picker_h = pad * 2.0
                    + if searchable { search_h + pad } else { 0.0 }
                    + visible as f32 * item_h;
                let picker_rect = Rect::new(vp.x, y + px(4.0), vp.w, picker_h);
                let mut py = picker_rect.y + pad;
                let search = if searchable {
                    let sr = Rect::new(
                        picker_rect.x + pad,
                        py,
                        (picker_rect.w - 2.0 * pad).max(0.0),
                        search_h,
                    );
                    py += search_h + pad;
                    Some(sr)
                } else {
                    None
                };
                let max_s = opts.len().saturating_sub(MAX_PICKER_ROWS);
                let scroll = exp.scroll.min(max_s);
                let mut items = Vec::with_capacity(visible);
                for (slot, opt_idx) in (scroll..opts.len()).take(MAX_PICKER_ROWS).enumerate() {
                    let ir = Rect::new(
                        picker_rect.x + pad,
                        py + slot as f32 * item_h,
                        (picker_rect.w - 2.0 * pad).max(0.0),
                        item_h,
                    );
                    items.push((opt_idx, ir));
                }
                y = picker_rect.bottom();
                Some(ExpandedPickerLayout {
                    rect: picker_rect,
                    search,
                    items,
                    total_items: opts.len(),
                    scroll,
                })
            } else {
                None
            };

            self.rows.push(ControlRowLayout {
                id,
                rect,
                label,
                widget,
                geometry,
                picker,
            });
            y += row_gap;
        }

        if self.max_scroll > 0.0 && vp.h > 0.0 {
            let total_px = total_h.max(vp.h);
            let thumb_h = (vp.h * (vp.h / total_px)).max(px(24.0)).min(vp.h);
            let frac = (state.scroll_y / self.max_scroll).clamp(0.0, 1.0);
            let thumb_y = vp.y + frac * (vp.h - thumb_h);
            let sb_w = px(4.0).max(2.0);
            let sb_x = if self.mirrored {
                self.content.x + px(4.0)
            } else {
                self.content.right() - px(4.0) - sb_w
            };
            self.scrollbar = Some(Rect::new(sb_x, thumb_y, sb_w, thumb_h));
        }
    }

    fn build_keys_layout(&mut self, scale: f32, state: &SettingsPanelState, config: &TwittyConfig) {
        let px = |logical: f32| logical * scale;
        let vp = self.viewport;
        let search_h = px(34.0).min(vp.h);
        let gap = px(8.0);
        let reset_all_w = px(106.0).min(vp.w * 0.30);
        let search_w = (vp.w - reset_all_w - gap).max(0.0);
        let (search, reset_all) = if self.mirrored {
            let ra = Rect::new(vp.x, vp.y, reset_all_w, search_h);
            let sr = Rect::new(ra.right() + gap, vp.y, search_w, search_h);
            (sr, ra)
        } else {
            let sr = Rect::new(vp.x, vp.y, search_w, search_h);
            let ra = Rect::new(vp.right() - reset_all_w, vp.y, reset_all_w, search_h);
            (sr, ra)
        };
        self.keys_search = Some(search);
        self.keys_reset_all = Some(reset_all);

        let list_y = if state.capturing_key.is_some() || state.key_capture_notice.is_some() {
            let banner_h = px(28.0).min((vp.bottom() - search.bottom()).max(0.0));
            let banner = Rect::new(vp.x, search.bottom() + px(6.0), vp.w, banner_h);
            self.keys_banner = Some(banner);
            banner.bottom() + px(6.0)
        } else {
            search.bottom() + px(10.0)
        };
        let list_h = (vp.bottom() - list_y).max(0.0);
        let row_h = px(40.0).round().max(1.0);
        let visible = (list_h / row_h).floor() as usize;
        self.keys_visible_rows = visible;

        let filtered = state.filtered_commands();
        let max_scroll = filtered.len().saturating_sub(visible);
        let scroll = state.keys_scroll.min(max_scroll);
        let pad = px(8.0);
        let btn_h = px(24.0).min((row_h - px(8.0)).max(1.0));
        let btn_gap = px(6.0);
        for (slot, cmd_idx) in (scroll..filtered.len()).take(visible).enumerate() {
            let rect = Rect::new(vp.x, list_y + slot as f32 * row_h, vp.w, row_h);
            let cmd = filtered[cmd_idx];
            let is_custom = config.keybindings.contains(cmd.name);
            let btn_y = rect.y + (rect.h - btn_h) / 2.0;
            let change_w = px(64.0).min(rect.w * 0.16);
            let reset_w = px(102.0).min(rect.w * 0.22);
            let (change_button, reset_button) = if self.mirrored {
                if is_custom {
                    let reset = Rect::new(rect.x + pad, btn_y, reset_w, btn_h);
                    let change = Rect::new(reset.right() + btn_gap, btn_y, change_w, btn_h);
                    (change, Some(reset))
                } else {
                    let change = Rect::new(rect.x + pad, btn_y, change_w, btn_h);
                    (change, None)
                }
            } else if is_custom {
                let reset = Rect::new(rect.right() - pad - reset_w, btn_y, reset_w, btn_h);
                let change = Rect::new(reset.x - btn_gap - change_w, btn_y, change_w, btn_h);
                (change, Some(reset))
            } else {
                let change = Rect::new(rect.right() - pad - change_w, btn_y, change_w, btn_h);
                (change, None)
            };
            self.keys_rows.push((cmd_idx, rect));
            self.keys_row_layouts.push(KeyRowLayout {
                cmd_idx,
                rect,
                change_button,
                reset_button,
            });
        }
        if max_scroll > 0 && list_h > 0.0 {
            let ratio = visible as f32 / filtered.len() as f32;
            let thumb_h = (list_h * ratio).max(px(24.0)).min(list_h);
            let frac = scroll as f32 / max_scroll as f32;
            let sb_w = px(4.0).max(2.0);
            let sb_x = if self.mirrored {
                self.content.x + px(4.0)
            } else {
                self.content.right() - px(4.0) - sb_w
            };
            self.scrollbar = Some(Rect::new(
                sb_x,
                list_y + frac * (list_h - thumb_h),
                sb_w,
                thumb_h,
            ));
        }
    }

    fn build_about_layout(&mut self, scale: f32) {
        let px = |logical: f32| logical * scale;
        let vp = self.viewport;
        let header_h = px(68.0);
        let about_header = Rect::new(vp.x, vp.y, vp.w, header_h);
        self.about_header = Some(about_header);

        let mut y = about_header.bottom() + px(14.0);
        let row_h = px(42.0);
        let gap = px(8.0);
        for _ in 0..3 {
            self.about_info_rows.push(Rect::new(vp.x, y, vp.w, row_h));
            y += row_h + gap;
        }
        if let Some(&config_row) = self.about_info_rows.get(2) {
            let btn_w = px(118.0).min(config_row.w * 0.4);
            let btn_h = px(28.0).min(config_row.h - px(6.0));
            let btn_y = config_row.y + (config_row.h - btn_h) / 2.0;
            let pad = px(10.0);
            let btn_x = if self.mirrored {
                config_row.x + pad
            } else {
                config_row.right() - pad - btn_w
            };
            self.copy_button = Some(Rect::new(btn_x, btn_y, btn_w, btn_h));
        }
    }

    /// Hit-test physical pixel `(x, y)` against the settings overlay.
    pub fn hit(&self, x: f64, y: f64) -> SettingsHit {
        if !self.panel.contains(x, y) {
            return SettingsHit::Outside;
        }
        if self.close_button.contains(x, y) {
            return SettingsHit::CloseButton;
        }
        for &(section, rect) in &self.sections {
            if rect.contains(x, y) {
                return SettingsHit::Section(section);
            }
        }
        if let Some(reset_all) = self.keys_reset_all {
            if reset_all.contains(x, y) {
                return SettingsHit::KeysResetAll;
            }
        }
        if let Some(search) = self.keys_search {
            if search.contains(x, y) {
                return SettingsHit::KeysSearch;
            }
        }
        for row in &self.keys_row_layouts {
            if row.change_button.contains(x, y) {
                return SettingsHit::KeyChange(row.cmd_idx);
            }
            if row.reset_button.is_some_and(|r| r.contains(x, y)) {
                return SettingsHit::KeyReset(row.cmd_idx);
            }
            if row.rect.contains(x, y) {
                return SettingsHit::KeyRow(row.cmd_idx);
            }
        }
        if let Some(copy_btn) = self.copy_button {
            if copy_btn.contains(x, y) {
                return SettingsHit::CopyConfigPath;
            }
        }
        if self.viewport.contains(x, y) {
            for row in &self.rows {
                if let Some(picker) = &row.picker {
                    if picker.rect.contains(x, y) {
                        if let Some(search) = picker.search {
                            if search.contains(x, y) {
                                return SettingsHit::PickerSearch(row.id);
                            }
                        }
                        for &(idx, rect) in &picker.items {
                            if rect.contains(x, y) {
                                return SettingsHit::PickerItem(row.id, idx);
                            }
                        }
                        return SettingsHit::Inside;
                    }
                }
                if row.rect.contains(x, y) {
                    match &row.geometry {
                        ControlGeometry::Toggle { .. } => {
                            return SettingsHit::Toggle(row.id);
                        }
                        ControlGeometry::Segmented { segments, .. } => {
                            for (i, seg) in segments.iter().enumerate() {
                                if seg.contains(x, y) {
                                    return SettingsHit::Segment(row.id, i);
                                }
                            }
                        }
                        ControlGeometry::Slider {
                            minus, track, plus, ..
                        } => {
                            if minus.contains(x, y) {
                                return SettingsHit::SliderMinus(row.id);
                            }
                            if plus.contains(x, y) {
                                return SettingsHit::SliderPlus(row.id);
                            }
                            if track.contains(x, y) {
                                let ratio = if track.w > 0.0 {
                                    ((x as f32 - track.x) / track.w).clamp(0.0, 1.0)
                                } else {
                                    0.0
                                };
                                return SettingsHit::SliderTrack(row.id, ratio);
                            }
                        }
                        ControlGeometry::Swatches { swatches } => {
                            for (i, sw) in swatches.iter().enumerate() {
                                if sw.contains(x, y) {
                                    return SettingsHit::Swatch(i);
                                }
                            }
                        }
                        ControlGeometry::TextField { input } => {
                            if input.contains(x, y) || row.rect.contains(x, y) {
                                return SettingsHit::TextField(row.id);
                            }
                        }
                        ControlGeometry::ListPicker { .. } => {
                            return SettingsHit::PickerToggle(row.id);
                        }
                        ControlGeometry::ActionButton { button } => {
                            if button.contains(x, y) {
                                return SettingsHit::CopyConfigPath;
                            }
                        }
                    }
                }
            }
        }
        SettingsHit::Inside
    }
}

fn build_control_geometry(
    kind: ControlKind,
    widget: Rect,
    scale: f32,
    mirrored: bool,
) -> ControlGeometry {
    let px = |logical: f32| logical * scale;
    match kind {
        ControlKind::Toggle => {
            let w = px(38.0).min(widget.w);
            let h = px(20.0).min(widget.h);
            let x = if mirrored {
                widget.x
            } else {
                widget.right() - w
            };
            let y = widget.y + (widget.h - h) / 2.0;
            ControlGeometry::Toggle {
                track: Rect::new(x, y, w, h),
            }
        }
        ControlKind::Segmented(options) => {
            let h = px(26.0).min(widget.h - px(6.0)).max(1.0);
            let y = widget.y + (widget.h - h) / 2.0;
            let container = Rect::new(widget.x, y, widget.w, h);
            let pad = px(2.0);
            let inner_w = (container.w - 2.0 * pad).max(0.0);
            let count = options.len().max(1);
            let seg_w = inner_w / count as f32;
            let segments = (0..count)
                .map(|i| {
                    // Option 0 starts at the reading start (left in LTR, right in RTL).
                    let visual_idx = if mirrored { count - 1 - i } else { i };
                    Rect::new(
                        container.x + pad + visual_idx as f32 * seg_w,
                        container.y + pad,
                        seg_w,
                        (container.h - 2.0 * pad).max(0.0),
                    )
                })
                .collect();
            ControlGeometry::Segmented {
                container,
                segments,
            }
        }
        ControlKind::Slider(_) => {
            let btn = px(24.0).min(widget.h - px(6.0));
            let val_w = px(54.0).min(widget.w * 0.3);
            let gap = px(6.0);
            let btn_y = widget.y + (widget.h - btn) / 2.0;
            let track_h = px(20.0).min(widget.h);
            let track_y = widget.y + (widget.h - track_h) / 2.0;
            if mirrored {
                let value = Rect::new(widget.right() - val_w, widget.y, val_w, widget.h);
                let minus = Rect::new(widget.x, btn_y, btn, btn);
                let plus_x = (value.x - gap - btn).max(minus.right() + gap);
                let plus = Rect::new(plus_x, btn_y, btn, btn);
                let track_x = minus.right() + gap;
                let track = Rect::new(track_x, track_y, (plus.x - gap - track_x).max(0.0), track_h);
                ControlGeometry::Slider {
                    minus,
                    track,
                    plus,
                    value,
                }
            } else {
                let value = Rect::new(widget.x, widget.y, val_w, widget.h);
                let minus = Rect::new(value.right() + gap, btn_y, btn, btn);
                let plus = Rect::new(widget.right() - btn, btn_y, btn, btn);
                let track_x = minus.right() + gap;
                let track = Rect::new(track_x, track_y, (plus.x - gap - track_x).max(0.0), track_h);
                ControlGeometry::Slider {
                    minus,
                    track,
                    plus,
                    value,
                }
            }
        }
        ControlKind::Swatches => {
            let count = ACCENT_SWATCHES.len();
            let gap = px(6.0);
            let side = px(22.0)
                .min((widget.w - gap * (count.saturating_sub(1)) as f32) / count as f32)
                .max(8.0);
            let total_w = count as f32 * side + (count.saturating_sub(1)) as f32 * gap;
            let y = widget.y + (widget.h - side) / 2.0;
            let start_x = if mirrored {
                widget.x
            } else {
                widget.right() - total_w
            };
            let swatches = (0..count)
                .map(|i| {
                    let visual_idx = if mirrored { count - 1 - i } else { i };
                    Rect::new(start_x + visual_idx as f32 * (side + gap), y, side, side)
                })
                .collect();
            ControlGeometry::Swatches { swatches }
        }
        ControlKind::TextField { .. } => {
            let h = px(28.0).min(widget.h - px(6.0)).max(1.0);
            let y = widget.y + (widget.h - h) / 2.0;
            ControlGeometry::TextField {
                input: Rect::new(widget.x, y, widget.w, h),
            }
        }
        ControlKind::ListPicker { .. } => {
            let h = px(28.0).min(widget.h - px(6.0)).max(1.0);
            let y = widget.y + (widget.h - h) / 2.0;
            ControlGeometry::ListPicker {
                button: Rect::new(widget.x, y, widget.w, h),
            }
        }
        ControlKind::ActionButton => {
            let h = px(28.0).min(widget.h - px(6.0)).max(1.0);
            let y = widget.y + (widget.h - h) / 2.0;
            ControlGeometry::ActionButton {
                button: Rect::new(widget.x, y, widget.w, h),
            }
        }
    }
}
