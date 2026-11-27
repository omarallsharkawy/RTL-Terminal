//! Reading and writing each setting's value in the config, and text field editing.

use super::*;

pub fn read_bool(setting: SettingId, config: &TwittyConfig) -> bool {
    match setting.descriptor().kind {
        ControlKind::Toggle(flag) => (flag.get)(config),
        _ => false,
    }
}

pub fn write_bool(setting: SettingId, value: bool, config: &mut TwittyConfig) -> bool {
    let ControlKind::Toggle(flag) = setting.descriptor().kind else {
        return false;
    };
    let slot = (flag.slot)(config);
    let changed = *slot != value;
    *slot = value;
    changed
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
        SettingId::ComposerMode => match config.composer_mode() {
            crate::composer::ComposerMode::Off => 0,
            crate::composer::ComposerMode::OnDemand => 1,
            crate::composer::ComposerMode::Always => 2,
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
        SettingId::ComposerMode if config.composer != opt.value => {
            config.composer = opt.value.to_owned();
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
        SettingId::AiTimeout => config.ai_timeout_secs as f32,
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
        SettingId::AiTimeout => {
            let next = value.round().clamp(5.0, 300.0) as u64;
            if config.ai_timeout_secs != next {
                config.ai_timeout_secs = next;
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

pub fn is_text_setting(setting: SettingId) -> bool {
    matches!(setting.descriptor().kind, ControlKind::TextField { .. })
}

/// Saved value of a text-field setting, as shown in its field.
pub fn read_text(setting: SettingId, config: &TwittyConfig) -> String {
    match setting {
        SettingId::AgentCommand => config.agent_command.clone().unwrap_or_default(),
        SettingId::ProjectRoots => config.project_roots.join(", "),
        SettingId::ColorAnsi => config
            .colors
            .as_ref()
            .and_then(|c| c.ansi.as_ref())
            .map(|a| a.join(" "))
            .unwrap_or_default(),
        id if SettingId::COLORS.contains(&id) => config
            .colors
            .as_ref()
            .and_then(|c| color_slot_ref(id, c).clone())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn color_slot_ref(id: SettingId, colors: &crate::config::ThemeColors) -> &Option<String> {
    match id {
        SettingId::ColorBackground => &colors.background,
        SettingId::ColorForeground => &colors.foreground,
        SettingId::ColorCursor => &colors.cursor,
        _ => &colors.selection,
    }
}

fn color_slot_mut(id: SettingId, colors: &mut crate::config::ThemeColors) -> &mut Option<String> {
    match id {
        SettingId::ColorBackground => &mut colors.background,
        SettingId::ColorForeground => &mut colors.foreground,
        SettingId::ColorCursor => &mut colors.cursor,
        _ => &mut colors.selection,
    }
}

/// Parse a color field: `Some(None)` clears the override, `Some(Some(hex))` sets it, `None` is
/// not (yet) a valid color.
fn parse_color_field(value: &str) -> Option<Option<String>> {
    let value = value.trim();
    if value.is_empty() {
        return Some(None);
    }
    crate::color::parse_hex(value).map(|_| {
        let hex = value.trim_start_matches('#').to_ascii_lowercase();
        Some(format!("#{hex}"))
    })
}

/// Parse the 16-color field: up to 16 hex colors separated by spaces or commas.
fn parse_ansi_field(value: &str) -> Option<Option<Vec<String>>> {
    let mut out = Vec::new();
    for token in value
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
    {
        out.push(parse_color_field(token)??);
    }
    if out.len() > 16 {
        return None;
    }
    Some((!out.is_empty()).then_some(out))
}

/// Store a colors edit, dropping `colors` entirely when no override is left.
fn update_colors(
    config: &mut TwittyConfig,
    edit: impl FnOnce(&mut crate::config::ThemeColors),
) -> bool {
    let before = serde_json::to_value(&config.colors).ok();
    let mut colors = config.colors.take().unwrap_or_default();
    edit(&mut colors);
    let empty = colors.background.is_none()
        && colors.foreground.is_none()
        && colors.cursor.is_none()
        && colors.selection.is_none()
        && colors.ansi.is_none();
    config.colors = (!empty).then_some(colors);
    serde_json::to_value(&config.colors).ok() != before
}

fn parse_project_roots(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|p| crate::shell_integration::clean_text(p.trim(), MAX_INPUT_CHARS))
        .filter(|p| !p.is_empty())
        .collect()
}

/// A draft still describes the saved value (it was not changed elsewhere meanwhile).
pub(super) fn draft_matches(setting: SettingId, draft: &str, config: &TwittyConfig) -> bool {
    match setting {
        SettingId::ProjectRoots => parse_project_roots(draft) == config.project_roots,
        // An unfinished color (`#1e1`) is still being typed: keep showing it.
        SettingId::ColorAnsi => parse_ansi_field(draft).is_none_or(|next| {
            next.map(|a| a.join(" ")).unwrap_or_default() == read_text(setting, config)
        }),
        id if SettingId::COLORS.contains(&id) => parse_color_field(draft)
            .is_none_or(|next| next.unwrap_or_default() == read_text(setting, config)),
        _ => read_text(setting, config) == draft.trim_end() || read_text(setting, config) == draft,
    }
}

/// Save `value` into a text-field setting; true when the config changed.
pub fn write_text(setting: SettingId, value: &str, config: &mut TwittyConfig) -> bool {
    match setting {
        SettingId::AgentCommand => {
            let cleaned = crate::shell_integration::clean_text(value, MAX_INPUT_CHARS);
            let next = (!cleaned.trim().is_empty()).then_some(cleaned);
            if config.agent_command != next {
                config.agent_command = next;
                return true;
            }
            false
        }
        SettingId::ProjectRoots => {
            let next = parse_project_roots(value);
            if config.project_roots != next {
                config.project_roots = next;
                return true;
            }
            false
        }
        SettingId::ColorAnsi => match parse_ansi_field(value) {
            Some(next) => update_colors(config, |c| c.ansi = next),
            None => false,
        },
        id if SettingId::COLORS.contains(&id) => match parse_color_field(value) {
            Some(next) => update_colors(config, |c| *color_slot_mut(id, c) = next),
            None => false,
        },
        _ => false,
    }
}

pub(super) fn push_bounded(target: &mut String, text: &str) -> bool {
    let before = target.len();
    for c in text.chars().filter(|c| !c.is_control()) {
        if target.chars().count() >= MAX_INPUT_CHARS {
            break;
        }
        target.push(c);
    }
    target.len() != before
}

pub(super) fn backspace_text(target: &mut String, whole_word: bool) -> bool {
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
