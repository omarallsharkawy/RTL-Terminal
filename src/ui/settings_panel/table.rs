//! Every setting the panel shows: sections, ids, the settings table and control kinds.

use super::*;

/// Sections shown in the settings navigation sidebar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SettingsSection {
    #[default]
    Appearance,
    Behaviour,
    Ai,
    Commands,
    Keys,
    About,
}

impl SettingsSection {
    pub const ALL: [Self; 6] = [
        Self::Appearance,
        Self::Behaviour,
        Self::Ai,
        Self::Commands,
        Self::Keys,
        Self::About,
    ];

    pub fn index(self) -> usize {
        match self {
            Self::Appearance => 0,
            Self::Behaviour => 1,
            Self::Ai => 2,
            Self::Commands => 3,
            Self::Keys => 4,
            Self::About => 5,
        }
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL[index % Self::ALL.len()]
    }

    pub fn label_en(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Behaviour => "Behaviour",
            Self::Ai => "AI",
            Self::Commands => "Commands",
            Self::Keys => "Keys",
            Self::About => "About",
        }
    }

    pub fn label_ar(self) -> &'static str {
        match self {
            Self::Appearance => "المظهر",
            Self::Behaviour => "السلوك",
            Self::Ai => "الذكاء الاصطناعي",
            Self::Commands => "الأوامر",
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
    ReduceMotion,
    ColorBackground,
    ColorForeground,
    ColorCursor,
    ColorSelection,
    ColorAnsi,
    // Behaviour
    DesktopNotifications,
    RestoreSession,
    ShellIntegration,
    RecordHistory,
    ClaudeHooks,
    ControlSocket,
    CmuxCompat,
    AgentCommand,
    ComposerMode,
    ProjectRoots,
    // AI
    ProjectMemory,
    BuiltInAi,
    AiTimeout,
    // About
    CopyConfigPath,
}

impl SettingId {
    /// Theme color overrides (`config.colors`).
    pub const COLORS: [Self; 5] = [
        Self::ColorBackground,
        Self::ColorForeground,
        Self::ColorCursor,
        Self::ColorSelection,
        Self::ColorAnsi,
    ];

    /// Every setting, in the order the panel lists them.
    pub fn all() -> &'static [Self] {
        &IDS
    }

    /// The settings `section` lists, in order (none for Commands and Keys).
    pub fn in_section(section: SettingsSection) -> &'static [Self] {
        let start = SETTINGS.iter().position(|d| d.section == section);
        let Some(start) = start else {
            return &[];
        };
        let len = SETTINGS[start..]
            .iter()
            .take_while(|d| d.section == section)
            .count();
        &IDS[start..start + len]
    }

    /// Settings stored in `config.json`.
    pub fn config_settings() -> impl Iterator<Item = Self> {
        SETTINGS
            .iter()
            .filter(|d| d.config_key.is_some())
            .map(|d| d.id)
    }

    pub fn section(self) -> SettingsSection {
        self.descriptor().section
    }

    /// JSON key in `config.json` (`None` for actions like CopyConfigPath).
    pub fn config_key(self) -> Option<&'static str> {
        self.descriptor().config_key
    }

    pub fn descriptor(self) -> &'static SettingDescriptor {
        &SETTINGS[self as usize]
    }
}

/// Every setting, in the order the panel lists them (also the `SettingId` order, checked at
/// compile time). A new setting is a `SettingId` variant plus one entry here; a toggle also
/// names its config field here, so nothing else needs to list it.
pub static SETTINGS: [SettingDescriptor; SETTING_COUNT] = TABLE;

const SETTING_COUNT: usize = 34;

const TABLE: [SettingDescriptor; SETTING_COUNT] = [
    SettingDescriptor {
        id: SettingId::Theme,
        section: SettingsSection::Appearance,
        config_key: Some("theme"),
        label_en: "Theme",
        label_ar: "السمة",
        note_en: None,
        note_ar: None,
        kind: ControlKind::ListPicker { searchable: false },
    },
    SettingDescriptor {
        id: SettingId::FontFamily,
        section: SettingsSection::Appearance,
        config_key: Some("font_family"),
        label_en: "Terminal font",
        label_ar: "خط الطرفية",
        note_en: None,
        note_ar: None,
        kind: ControlKind::ListPicker { searchable: true },
    },
    SettingDescriptor {
        id: SettingId::FontSize,
        section: SettingsSection::Appearance,
        config_key: Some("font_size"),
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
    SettingDescriptor {
        id: SettingId::UiFontFamily,
        section: SettingsSection::Appearance,
        config_key: Some("ui_font_family"),
        label_en: "UI font",
        label_ar: "خط الواجهة",
        note_en: None,
        note_ar: None,
        kind: ControlKind::ListPicker { searchable: true },
    },
    SettingDescriptor {
        id: SettingId::UiLanguage,
        section: SettingsSection::Appearance,
        config_key: Some("ui_language"),
        label_en: "Interface language",
        label_ar: "لغة الواجهة",
        note_en: None,
        note_ar: None,
        kind: ControlKind::Segmented(UI_LANGUAGE_OPTIONS),
    },
    SettingDescriptor {
        id: SettingId::UiAccent,
        section: SettingsSection::Appearance,
        config_key: Some("ui_accent"),
        label_en: "Accent colour",
        label_ar: "لون التمييز",
        note_en: None,
        note_ar: None,
        kind: ControlKind::Swatches,
    },
    SettingDescriptor {
        id: SettingId::BackgroundOpacity,
        section: SettingsSection::Appearance,
        config_key: Some("background_opacity"),
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
    SettingDescriptor {
        id: SettingId::CursorStyle,
        section: SettingsSection::Appearance,
        config_key: Some("cursor_style"),
        label_en: "Cursor style",
        label_ar: "شكل المؤشر",
        note_en: None,
        note_ar: None,
        kind: ControlKind::Segmented(CURSOR_STYLE_OPTIONS),
    },
    SettingDescriptor {
        id: SettingId::CursorBlink,
        section: SettingsSection::Appearance,
        config_key: Some("cursor_blink"),
        label_en: "Cursor blink",
        label_ar: "وميض المؤشر",
        note_en: None,
        note_ar: None,
        kind: ControlKind::Toggle(Flag {
            get: |c| c.cursor_blink,
            slot: |c| &mut c.cursor_blink,
        }),
    },
    SettingDescriptor {
        id: SettingId::SidebarPosition,
        section: SettingsSection::Appearance,
        config_key: Some("sidebar"),
        label_en: "Sidebar position",
        label_ar: "موضع الشريط الجانبي",
        note_en: None,
        note_ar: None,
        kind: ControlKind::Segmented(SIDEBAR_POSITION_OPTIONS),
    },
    SettingDescriptor {
        id: SettingId::SidebarWidth,
        section: SettingsSection::Appearance,
        config_key: Some("sidebar_width"),
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
    SettingDescriptor {
        id: SettingId::TopBar,
        section: SettingsSection::Appearance,
        config_key: Some("top_bar"),
        label_en: "Top bar",
        label_ar: "الشريط العلوي",
        note_en: None,
        note_ar: None,
        kind: ControlKind::Toggle(Flag {
            get: |c| c.top_bar,
            slot: |c| &mut c.top_bar,
        }),
    },
    SettingDescriptor {
        id: SettingId::PaneHeaders,
        section: SettingsSection::Appearance,
        config_key: Some("pane_headers"),
        label_en: "Pane headers",
        label_ar: "عناوين اللوحات",
        note_en: None,
        note_ar: None,
        kind: ControlKind::Toggle(Flag {
            get: |c| c.pane_headers,
            slot: |c| &mut c.pane_headers,
        }),
    },
    SettingDescriptor {
        id: SettingId::BlockCards,
        section: SettingsSection::Appearance,
        config_key: Some("block_cards"),
        label_en: "Block cards",
        label_ar: "بطاقات الأوامر",
        note_en: None,
        note_ar: None,
        kind: ControlKind::Toggle(Flag {
            get: |c| c.block_cards,
            slot: |c| &mut c.block_cards,
        }),
    },
    SettingDescriptor {
        id: SettingId::ReduceMotion,
        section: SettingsSection::Appearance,
        config_key: Some("reduce_motion"),
        label_en: "Reduce motion",
        label_ar: "تقليل الحركة",
        note_en: Some("No animations; the cursor does not blink"),
        note_ar: Some("بدون حركات، والمؤشر ما بيومضش"),
        kind: ControlKind::Toggle(Flag {
            get: |c| c.reduce_motion,
            slot: |c| &mut c.reduce_motion,
        }),
    },
    SettingDescriptor {
        id: SettingId::ColorBackground,
        section: SettingsSection::Appearance,
        config_key: Some("colors"),
        label_en: "Background color",
        label_ar: "لون الخلفية",
        note_en: None,
        note_ar: None,
        kind: ControlKind::TextField {
            placeholder_en: "Theme color, e.g. #1e1e2e",
            placeholder_ar: "لون السمة، مثلاً #1e1e2e",
        },
    },
    SettingDescriptor {
        id: SettingId::ColorForeground,
        section: SettingsSection::Appearance,
        config_key: Some("colors"),
        label_en: "Text color",
        label_ar: "لون النص",
        note_en: None,
        note_ar: None,
        kind: ControlKind::TextField {
            placeholder_en: "Theme color, e.g. #1e1e2e",
            placeholder_ar: "لون السمة، مثلاً #1e1e2e",
        },
    },
    SettingDescriptor {
        id: SettingId::ColorCursor,
        section: SettingsSection::Appearance,
        config_key: Some("colors"),
        label_en: "Cursor color",
        label_ar: "لون المؤشر",
        note_en: None,
        note_ar: None,
        kind: ControlKind::TextField {
            placeholder_en: "Theme color, e.g. #1e1e2e",
            placeholder_ar: "لون السمة، مثلاً #1e1e2e",
        },
    },
    SettingDescriptor {
        id: SettingId::ColorSelection,
        section: SettingsSection::Appearance,
        config_key: Some("colors"),
        label_en: "Selection color",
        label_ar: "لون التحديد",
        note_en: None,
        note_ar: None,
        kind: ControlKind::TextField {
            placeholder_en: "Theme color, e.g. #1e1e2e",
            placeholder_ar: "لون السمة، مثلاً #1e1e2e",
        },
    },
    SettingDescriptor {
        id: SettingId::ColorAnsi,
        section: SettingsSection::Appearance,
        config_key: Some("colors"),
        label_en: "Terminal colors",
        label_ar: "ألوان الطرفية",
        note_en: Some("Up to 16 hex colors (black, red, … bright white), separated by spaces"),
        note_ar: Some("لحد 16 لون hex (أسود، أحمر، … أبيض فاتح) بينهم مسافات"),
        kind: ControlKind::TextField {
            placeholder_en: "Theme colors",
            placeholder_ar: "ألوان السمة",
        },
    },
    SettingDescriptor {
        id: SettingId::DesktopNotifications,
        section: SettingsSection::Behaviour,
        config_key: Some("desktop_notifications"),
        label_en: "Desktop notifications",
        label_ar: "إشعارات سطح المكتب",
        note_en: None,
        note_ar: None,
        kind: ControlKind::Toggle(Flag {
            get: |c| c.desktop_notifications,
            slot: |c| &mut c.desktop_notifications,
        }),
    },
    SettingDescriptor {
        id: SettingId::RestoreSession,
        section: SettingsSection::Behaviour,
        config_key: Some("restore_session"),
        label_en: "Restore session",
        label_ar: "استعادة الجلسة",
        note_en: Some("Applies after restart"),
        note_ar: Some("يطبّق بعد إعادة التشغيل"),
        kind: ControlKind::Toggle(Flag {
            get: |c| c.restore_session,
            slot: |c| &mut c.restore_session,
        }),
    },
    SettingDescriptor {
        id: SettingId::ShellIntegration,
        section: SettingsSection::Behaviour,
        config_key: Some("shell_integration"),
        label_en: "Shell integration",
        label_ar: "تكامل الشيل",
        note_en: Some("Applies to new shells"),
        note_ar: Some("يطبّق على الشيل الجديد"),
        kind: ControlKind::Toggle(Flag {
            get: |c| c.shell_integration,
            slot: |c| &mut c.shell_integration,
        }),
    },
    SettingDescriptor {
        id: SettingId::RecordHistory,
        section: SettingsSection::Behaviour,
        config_key: Some("record_history"),
        label_en: "Record command history",
        label_ar: "حفظ سجل الأوامر",
        note_en: None,
        note_ar: None,
        kind: ControlKind::Toggle(Flag {
            get: |c| c.record_history,
            slot: |c| &mut c.record_history,
        }),
    },
    SettingDescriptor {
        id: SettingId::ClaudeHooks,
        section: SettingsSection::Behaviour,
        config_key: Some("claude_hooks"),
        label_en: "Claude hooks",
        label_ar: "خطافات كلود",
        note_en: Some("Applies to new shells"),
        note_ar: Some("يطبّق على الشيل الجديد"),
        kind: ControlKind::Toggle(Flag {
            get: |c| c.claude_hooks,
            slot: |c| &mut c.claude_hooks,
        }),
    },
    SettingDescriptor {
        id: SettingId::ControlSocket,
        section: SettingsSection::Behaviour,
        config_key: Some("control_socket"),
        label_en: "Control socket",
        label_ar: "مقبس التحكم",
        note_en: Some("Applies after restart"),
        note_ar: Some("يطبّق بعد إعادة التشغيل"),
        kind: ControlKind::Toggle(Flag {
            get: |c| c.control_socket,
            slot: |c| &mut c.control_socket,
        }),
    },
    SettingDescriptor {
        id: SettingId::CmuxCompat,
        section: SettingsSection::Behaviour,
        config_key: Some("cmux_compat"),
        label_en: "External socket compatibility",
        label_ar: "توافق مقبس التحكم الخارجي",
        note_en: Some("Applies to new shells"),
        note_ar: Some("يطبّق على الشيل الجديد"),
        kind: ControlKind::Toggle(Flag {
            get: |c| c.cmux_compat,
            slot: |c| &mut c.cmux_compat,
        }),
    },
    SettingDescriptor {
        id: SettingId::AgentCommand,
        section: SettingsSection::Behaviour,
        config_key: Some("agent_command"),
        label_en: "Agent command",
        label_ar: "أمر الوكيل",
        note_en: Some("Leave blank to auto-detect"),
        note_ar: Some("اتركه فارغاً للاكتشاف التلقائي"),
        kind: ControlKind::TextField {
            placeholder_en: "Auto (claude, codex…)",
            placeholder_ar: "تلقائي: claude أو codex",
        },
    },
    SettingDescriptor {
        id: SettingId::ComposerMode,
        section: SettingsSection::Behaviour,
        config_key: Some("composer"),
        label_en: "Command composer",
        label_ar: "محرر الأوامر",
        note_en: Some("The editor box under the terminal (Ctrl+Shift+Space)"),
        note_ar: Some("خانة كتابة الأوامر تحت الطرفية (Ctrl+Shift+Space)"),
        kind: ControlKind::Segmented(COMPOSER_MODE_OPTIONS),
    },
    SettingDescriptor {
        id: SettingId::ProjectRoots,
        section: SettingsSection::Behaviour,
        config_key: Some("project_roots"),
        label_en: "Project folders",
        label_ar: "مجلدات المشاريع",
        note_en: Some("Scanned for git projects; separate with commas"),
        note_ar: Some("بيتدوّر فيها على مشاريع git؛ افصل بينها بفاصلة"),
        kind: ControlKind::TextField {
            placeholder_en: "~/Projects, ~/src",
            placeholder_ar: "~/Projects, ~/src",
        },
    },
    SettingDescriptor {
        id: SettingId::ProjectMemory,
        section: SettingsSection::Ai,
        config_key: None,
        label_en: "Project memory",
        label_ar: "ذاكرة المشروع",
        note_en: Some("Rules files, facts, frequent commands, environment notes, and exclusions"),
        note_ar: Some("ملفات القواعد والحقائق والأوامر المتكررة وملاحظات البيئة والاستثناءات"),
        kind: ControlKind::ActionButton,
    },
    SettingDescriptor {
        id: SettingId::BuiltInAi,
        section: SettingsSection::Ai,
        config_key: Some("ai"),
        label_en: "Built-in AI",
        label_ar: "الذكاء الاصطناعي المدمج",
        note_en: Some("Generate commands (#) and explain failed blocks"),
        note_ar: Some("توليد الأوامر (#) وشرح الأوامر اللي فشلت"),
        kind: ControlKind::Toggle(Flag {
            get: |c| c.ai,
            slot: |c| &mut c.ai,
        }),
    },
    SettingDescriptor {
        id: SettingId::AiTimeout,
        section: SettingsSection::Ai,
        config_key: Some("ai_timeout_secs"),
        label_en: "AI request timeout",
        label_ar: "مهلة طلب الذكاء الاصطناعي",
        note_en: None,
        note_ar: None,
        kind: ControlKind::Slider(SliderSpec {
            min: 5.0,
            max: 300.0,
            step: 5.0,
            unit: SliderUnit::Seconds,
        }),
    },
    SettingDescriptor {
        id: SettingId::CopyConfigPath,
        section: SettingsSection::About,
        config_key: None,
        label_en: "Config file",
        label_ar: "ملف الإعدادات",
        note_en: None,
        note_ar: None,
        kind: ControlKind::ActionButton,
    },
];

// The table is in `SettingId` order (so `descriptor` can index it) and each section's
// settings sit together (so a section is one slice).
const _: () = {
    let mut i = 0;
    while i < SETTING_COUNT {
        assert!(
            TABLE[i].id as usize == i,
            "SETTINGS must follow the SettingId order"
        );
        if i > 0 && TABLE[i].section as u8 != TABLE[i - 1].section as u8 {
            let mut j = 0;
            while j < i {
                assert!(
                    TABLE[j].section as u8 != TABLE[i].section as u8,
                    "a section's settings must sit together"
                );
                j += 1;
            }
        }
        i += 1;
    }
};

/// Setting ids in table order.
static IDS: [SettingId; SETTING_COUNT] = {
    let mut ids = [SettingId::Theme; SETTING_COUNT];
    let mut i = 0;
    while i < SETTING_COUNT {
        ids[i] = TABLE[i].id;
        i += 1;
    }
    ids
};

/// A boolean config field, for toggles.
#[derive(Clone, Copy)]
pub struct Flag {
    pub get: fn(&TwittyConfig) -> bool,
    pub slot: fn(&mut TwittyConfig) -> &mut bool,
}

impl std::fmt::Debug for Flag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Flag")
    }
}

/// Static metadata for one setting control.
#[derive(Clone, Copy, Debug)]
pub struct SettingDescriptor {
    pub id: SettingId,
    pub section: SettingsSection,
    /// JSON key in `config.json`; `None` for actions.
    pub config_key: Option<&'static str>,
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

pub const COMPOSER_MODE_OPTIONS: &[SegmentedOption] = &[
    SegmentedOption {
        value: "off",
        label_en: "Off",
        label_ar: "مقفول",
    },
    SegmentedOption {
        value: "on-demand",
        label_en: "On demand",
        label_ar: "عند الطلب",
    },
    SegmentedOption {
        value: "always",
        label_en: "Always",
        label_ar: "دايماً",
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
    Seconds,
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
            SliderUnit::Seconds => format!("{} s", value.round() as i32),
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
    Toggle(Flag),
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
