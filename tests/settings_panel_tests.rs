//! Window-free tests for the Settings screen: shortcut/palette wiring, config model & atomic
//! round-trip, keyboard navigation, live theme preview + Esc revert, Arabic IME input, Arabic
//! RTL layout mirroring, hit testing, and headless overlay rendering/clipping.

use cosmic_text::FontSystem;
use twitty::color::{theme_names, Palette};
use twitty::commands;
use twitty::config::TwittyConfig;
use twitty::input::{handle_key_raw, InputAction};
use twitty::layout::{PaneId, Rect};
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::frame::{build_frame, FrameOptions};
use twitty::render::scene::preedit::PreeditState;
use twitty::render::scene::settings::SettingsView;
use twitty::render::shapes::Shape;
use twitty::render::text::fonts::{all_families, monospace_families, new_font_system};
use twitty::render::text::ui::UiFont;
use twitty::render::ui_style::UiStyle;
use twitty::render::{CellMetrics, PaneFrame, RowCache};
use twitty::terminal::Terminal;
use twitty::ui::chrome::UiLanguage;
use twitty::ui::palette::{matches, PaletteEntry, PaletteId};
use twitty::ui::settings_panel::{
    current_picker_value, read_bool, read_segmented_index, read_slider_value, read_swatch_index,
    ControlGeometry, FocusZone, SettingId, SettingsHit, SettingsLayout, SettingsPanelState,
    SettingsSection, ACCENT_SWATCHES,
};
use winit::event::ElementState::Pressed;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, SmolStr};

const SURFACE: (f32, f32) = (1024.0, 720.0);

fn center(rect: Rect) -> (f64, f64) {
    (
        f64::from(rect.x + rect.w / 2.0),
        f64::from(rect.y + rect.h / 2.0),
    )
}

fn temp_config_path(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "twitty-settings-test-{tag}-{}-{nanos}.json",
        std::process::id()
    ))
}

fn sample_panel(language: UiLanguage) -> SettingsPanelState {
    SettingsPanelState::new(
        vec![
            "Fira Code".into(),
            "JetBrains Mono".into(),
            "Noto Sans Mono".into(),
        ],
        vec![
            "DejaVu Sans".into(),
            "Inter".into(),
            "Noto Naskh Arabic".into(),
        ],
        Some("/home/user/.config/twitty/config.json".into()),
        language,
    )
}

#[test]
fn ctrl_comma_and_command_palette_open_settings() {
    // Ctrl+, on US layout (logical comma).
    assert_eq!(
        handle_key_raw(
            Some(KeyCode::Comma),
            &Key::Character(SmolStr::new(",")),
            Some(","),
            Pressed,
            ModifiersState::CONTROL,
            false,
        ),
        Some(InputAction::Settings)
    );
    // Ctrl+, on Arabic layout (logical "و", physical KeyCode::Comma).
    assert_eq!(
        handle_key_raw(
            Some(KeyCode::Comma),
            &Key::Character(SmolStr::new("و")),
            Some("و"),
            Pressed,
            ModifiersState::CONTROL,
            false,
        ),
        Some(InputAction::Settings)
    );
    // Command table includes settings.open with English + Arabic titles and Ctrl+,.
    let cmd = commands::all()
        .into_iter()
        .find(|c| c.name == "settings.open")
        .expect("settings.open command must exist");
    assert_eq!(cmd.title_en, "Settings");
    assert_eq!(cmd.title_ar, "الإعدادات");
    assert_eq!(cmd.shortcut, Some("Ctrl+,"));
    assert_eq!(
        commands::action("settings.open"),
        Some(InputAction::Settings)
    );

    // Palette matching finds it in both English and Arabic (even without hamza).
    let entries = PaletteEntry::builtins();
    for query in ["settings", "الإعدادات", "اعدادات"] {
        let hits = matches(&entries, query);
        assert!(
            hits.iter()
                .any(|&i| entries[i].id == PaletteId::Builtin("settings.open")),
            "query {query:?} should match settings.open"
        );
    }
}

#[test]
fn model_sections_labels_and_every_control_round_trips_through_save_and_load() {
    // All 4 sections have English and Arabic labels.
    assert_eq!(SettingsSection::ALL.len(), 4);
    for section in SettingsSection::ALL {
        assert!(!section.label_en().is_empty());
        assert!(!section.label_ar().is_empty());
    }

    // Every config setting has bilingual labels and maps to a real JSON key in TwittyConfig.
    let mut cfg = TwittyConfig::default();
    let default_json = serde_json::to_value(&cfg).unwrap();
    for &id in &SettingId::CONFIG_SETTINGS {
        let desc = id.descriptor();
        assert!(!desc.label_en.is_empty(), "{id:?}");
        assert!(!desc.label_ar.is_empty(), "{id:?}");
        let key = id.config_key().expect("config setting must have key");
        // Optional fields with skip_serializing_if may be absent when None, so verify below after setting them.
        assert!(!key.is_empty());
        if desc.note_en.is_some() {
            assert!(desc.note_ar.is_some(), "{id:?} missing Arabic note");
        }
        let _ = &default_json;
    }

    // Restart-only / new-shell settings explicitly say so.
    for id in [
        SettingId::RestoreSession,
        SettingId::ShellIntegration,
        SettingId::ClaudeHooks,
        SettingId::ControlSocket,
        SettingId::CmuxCompat,
    ] {
        let desc = id.descriptor();
        assert!(desc.note_en.is_some() && desc.note_ar.is_some(), "{id:?}");
    }

    // Mutate EVERY SINGLE one of the 20 config settings via SettingsPanelState interactions.
    let mut state = sample_panel(UiLanguage::English);
    let none = ModifiersState::empty();

    // 1. Theme -> pick second theme ("catppuccin-mocha")
    state.on_key(
        &Key::Named(NamedKey::Enter),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    state.on_key(
        &Key::Named(NamedKey::ArrowDown),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    let res = state.on_key(
        &Key::Named(NamedKey::Enter),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(res.changed, vec![SettingId::Theme]);
    assert_eq!(cfg.theme.as_deref(), Some(theme_names()[1]));

    // 2. FontFamily -> pick "Fira Code"
    state.on_key(
        &Key::Named(NamedKey::ArrowDown),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(state.focused_setting(), Some(SettingId::FontFamily));
    state.on_key(
        &Key::Named(NamedKey::Enter),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    state.on_key(
        &Key::Named(NamedKey::ArrowDown),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    let res = state.on_key(
        &Key::Named(NamedKey::Enter),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(res.changed, vec![SettingId::FontFamily]);
    assert_eq!(cfg.font_family.as_deref(), Some("Fira Code"));

    // 3. FontSize -> ArrowRight steps +0.5
    state.on_key(
        &Key::Named(NamedKey::ArrowDown),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(state.focused_setting(), Some(SettingId::FontSize));
    let before_size = cfg.font_size;
    let res = state.on_key(
        &Key::Named(NamedKey::ArrowRight),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(res.changed, vec![SettingId::FontSize]);
    assert!((cfg.font_size - (before_size + 0.5)).abs() < 1e-4);

    // 4. UiFontFamily -> pick "Inter" via search
    state.on_key(
        &Key::Named(NamedKey::ArrowDown),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(state.focused_setting(), Some(SettingId::UiFontFamily));
    state.on_key(
        &Key::Named(NamedKey::Enter),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    state.on_text("inter", &mut cfg);
    let res = state.on_key(
        &Key::Named(NamedKey::Enter),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(res.changed, vec![SettingId::UiFontFamily]);
    assert_eq!(cfg.ui_font_family.as_deref(), Some("Inter"));

    // 5. UiAccent -> ArrowRight picks swatch 1 (#7aa2f7)
    state.on_key(
        &Key::Named(NamedKey::ArrowDown),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(state.focused_setting(), Some(SettingId::UiAccent));
    let res = state.on_key(
        &Key::Named(NamedKey::ArrowRight),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(res.changed, vec![SettingId::UiAccent]);
    assert_eq!(cfg.ui_accent.as_deref(), ACCENT_SWATCHES[1].hex);

    // 6. BackgroundOpacity -> ArrowRight increases opacity
    state.on_key(
        &Key::Named(NamedKey::ArrowDown),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(state.focused_setting(), Some(SettingId::BackgroundOpacity));
    let res = state.on_key(
        &Key::Named(NamedKey::ArrowRight),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(res.changed, vec![SettingId::BackgroundOpacity]);
    assert!((cfg.background_opacity - 0.85).abs() < 0.01);

    // 7. CursorStyle -> ArrowRight -> "beam"
    state.on_key(
        &Key::Named(NamedKey::ArrowDown),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(state.focused_setting(), Some(SettingId::CursorStyle));
    let res = state.on_key(
        &Key::Named(NamedKey::ArrowRight),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(res.changed, vec![SettingId::CursorStyle]);
    assert_eq!(cfg.cursor_style, "beam");

    // 8. CursorBlink -> Space toggles false
    state.on_key(
        &Key::Named(NamedKey::ArrowDown),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(state.focused_setting(), Some(SettingId::CursorBlink));
    let res = state.on_key(
        &Key::Named(NamedKey::Space),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(res.changed, vec![SettingId::CursorBlink]);
    assert!(!cfg.cursor_blink);

    // 9. SidebarPosition -> ArrowLeft -> "left"
    state.on_key(
        &Key::Named(NamedKey::ArrowDown),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(state.focused_setting(), Some(SettingId::SidebarPosition));
    let res = state.on_key(
        &Key::Named(NamedKey::ArrowLeft),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(res.changed, vec![SettingId::SidebarPosition]);
    assert_eq!(cfg.sidebar, "left");

    // 10. SidebarWidth -> ArrowRight -> +10
    state.on_key(
        &Key::Named(NamedKey::ArrowDown),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(state.focused_setting(), Some(SettingId::SidebarWidth));
    let before_w = cfg.sidebar_width;
    let res = state.on_key(
        &Key::Named(NamedKey::ArrowRight),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(res.changed, vec![SettingId::SidebarWidth]);
    assert!((cfg.sidebar_width - (before_w + 10.0)).abs() < 1e-4);

    // 11..13. TopBar, PaneHeaders, BlockCards -> Space toggles each
    for expected in [
        SettingId::TopBar,
        SettingId::PaneHeaders,
        SettingId::BlockCards,
    ] {
        state.on_key(
            &Key::Named(NamedKey::ArrowDown),
            None,
            none,
            &mut cfg,
            SURFACE,
            1.0,
        );
        assert_eq!(state.focused_setting(), Some(expected));
        let res = state.on_key(
            &Key::Named(NamedKey::Space),
            None,
            none,
            &mut cfg,
            SURFACE,
            1.0,
        );
        assert_eq!(res.changed, vec![expected]);
        assert!(!read_bool(expected, &cfg));
    }

    // Switch to Behaviour section and toggle all 6 booleans + edit AgentCommand.
    state.select_section(SettingsSection::Behaviour, &cfg);
    for (i, &expected) in SettingId::BEHAVIOUR[..6].iter().enumerate() {
        if i > 0 {
            state.on_key(
                &Key::Named(NamedKey::ArrowDown),
                None,
                none,
                &mut cfg,
                SURFACE,
                1.0,
            );
        }
        assert_eq!(state.focused_setting(), Some(expected));
        let res = state.on_key(
            &Key::Named(NamedKey::Space),
            None,
            none,
            &mut cfg,
            SURFACE,
            1.0,
        );
        assert_eq!(res.changed, vec![expected]);
        assert!(!read_bool(expected, &cfg));
    }
    state.on_key(
        &Key::Named(NamedKey::ArrowDown),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(state.focused_setting(), Some(SettingId::AgentCommand));
    let res = state.on_text("codex --model o3", &mut cfg);
    assert_eq!(res.changed, vec![SettingId::AgentCommand]);
    assert_eq!(cfg.agent_command.as_deref(), Some("codex --model o3"));

    // Verify every config_key appears in the serialized JSON!
    let json = serde_json::to_value(&cfg).unwrap();
    for &id in &SettingId::CONFIG_SETTINGS {
        let key = id.config_key().unwrap();
        assert!(
            json.get(key).is_some(),
            "serialized config missing key {key:?} for {id:?}"
        );
    }

    // Round-trip through atomic save_to + load_from.
    let path = temp_config_path("roundtrip");
    cfg.save_to(&path).expect("atomic save should succeed");
    let loaded = TwittyConfig::load_from(&path);
    let _ = std::fs::remove_file(&path);

    assert_eq!(loaded.theme, cfg.theme);
    assert_eq!(loaded.font_family, cfg.font_family);
    assert!((loaded.font_size - cfg.font_size).abs() < 1e-4);
    assert_eq!(loaded.ui_font_family, cfg.ui_font_family);
    assert_eq!(loaded.ui_accent, cfg.ui_accent);
    assert!((loaded.background_opacity - cfg.background_opacity).abs() < 1e-4);
    assert_eq!(loaded.cursor_style, cfg.cursor_style);
    assert_eq!(loaded.cursor_blink, cfg.cursor_blink);
    assert_eq!(loaded.sidebar, cfg.sidebar);
    assert!((loaded.sidebar_width - cfg.sidebar_width).abs() < 1e-4);
    assert_eq!(loaded.top_bar, cfg.top_bar);
    assert_eq!(loaded.pane_headers, cfg.pane_headers);
    assert_eq!(loaded.block_cards, cfg.block_cards);
    assert_eq!(loaded.desktop_notifications, cfg.desktop_notifications);
    assert_eq!(loaded.restore_session, cfg.restore_session);
    assert_eq!(loaded.shell_integration, cfg.shell_integration);
    assert_eq!(loaded.claude_hooks, cfg.claude_hooks);
    assert_eq!(loaded.control_socket, cfg.control_socket);
    assert_eq!(loaded.cmux_compat, cfg.cmux_compat);
    assert_eq!(loaded.agent_command, cfg.agent_command);

    // Accessor sanity checks on loaded config.
    assert_eq!(read_segmented_index(SettingId::CursorStyle, &loaded), 1);
    assert_eq!(read_segmented_index(SettingId::SidebarPosition, &loaded), 0);
    assert_eq!(read_swatch_index(&loaded), Some(1));
    assert!((read_slider_value(SettingId::FontSize, &loaded) - loaded.font_size).abs() < 1e-4);
    assert_eq!(
        current_picker_value(SettingId::Theme, &loaded),
        loaded.theme
    );
}

#[test]
fn theme_live_preview_reverts_on_escape_and_hover_leave() {
    let mut cfg = TwittyConfig::default();
    cfg.theme = Some("nord".into());
    let mut state = sample_panel(UiLanguage::English);
    let none = ModifiersState::empty();

    // Open Theme picker via Enter.
    state.on_key(
        &Key::Named(NamedKey::Enter),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert!(state.expanded.is_some());
    assert_eq!(state.preview_theme(), None);

    // Move down to preview the next theme without committing.
    let down = state.on_key(
        &Key::Named(NamedKey::ArrowDown),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert!(down.changed.is_empty(), "preview must not mutate config");
    assert_eq!(cfg.theme.as_deref(), Some("nord"));
    assert!(state.preview_theme().is_some());
    assert_ne!(state.preview_theme(), Some("nord"));
    assert_eq!(
        down.preview_theme,
        Some(state.preview_theme().map(str::to_owned))
    );

    // Esc cancels picker and reverts preview back to committed "nord".
    let esc = state.on_key(
        &Key::Named(NamedKey::Escape),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert!(!esc.close, "first Esc cancels open theme picker");
    assert!(state.expanded.is_none());
    assert_eq!(state.preview_theme(), None);
    assert_eq!(esc.preview_theme, Some(Some("nord".into())));
    assert_eq!(cfg.theme.as_deref(), Some("nord"));

    // Second Esc closes the settings panel.
    let esc2 = state.on_key(
        &Key::Named(NamedKey::Escape),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert!(esc2.close);

    // Hover live preview in expanded Theme picker + revert on pointer leaving picker.
    let mut state = sample_panel(UiLanguage::English);
    state.open_picker(SettingId::Theme, &cfg);
    let layout = SettingsLayout::build(SURFACE, 1.0, &state, &cfg);
    let picker = layout.rows[0].picker.as_ref().unwrap();
    // Hover the first visible theme item whose name is not "nord" (the list may be scrolled).
    let (opt_idx, item_rect) = *picker
        .items
        .iter()
        .find(|(i, _)| theme_names()[*i] != "nord")
        .unwrap();
    let hovered = theme_names()[opt_idx];
    let (hx, hy) = center(item_rect);
    let hover_res = state.on_mouse_move(hx, hy, &mut cfg, SURFACE, 1.0);
    assert_eq!(state.preview_theme(), Some(hovered));
    assert_eq!(hover_res.preview_theme, Some(Some(hovered.into())));
    assert_eq!(cfg.theme.as_deref(), Some("nord"));

    // Leaving the window reverts the hover preview.
    let leave_res = state.on_mouse_leave(&cfg);
    assert_eq!(state.preview_theme(), None);
    assert_eq!(leave_res.preview_theme, Some(Some("nord".into())));

    // Hovering again and clicking outside reverts the preview AND closes settings.
    state.on_mouse_move(hx, hy, &mut cfg, SURFACE, 1.0);
    assert_eq!(state.preview_theme(), Some(hovered));
    let outside = state.on_mouse_down(1.0, 1.0, &mut cfg, SURFACE, 1.0);
    assert!(outside.close);
    assert_eq!(outside.preview_theme, Some(Some("nord".into())));
    assert_eq!(cfg.theme.as_deref(), Some("nord"));
}

#[test]
fn keyboard_navigation_tab_shift_tab_sidebar_and_about_copy() {
    let mut cfg = TwittyConfig::default();
    let mut state = sample_panel(UiLanguage::English);
    let none = ModifiersState::empty();
    let shift = ModifiersState::SHIFT;

    // Starts in Content at index 0. Shift+Tab jumps to Sidebar.
    assert_eq!(state.focus, FocusZone::Content);
    assert_eq!(state.selected, 0);
    state.on_key(
        &Key::Named(NamedKey::Tab),
        None,
        shift,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(state.focus, FocusZone::Sidebar);

    // ArrowDown in Sidebar cycles sections: Appearance -> Behaviour -> Keys -> About.
    state.on_key(
        &Key::Named(NamedKey::ArrowDown),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(state.section, SettingsSection::Behaviour);
    state.on_key(
        &Key::Named(NamedKey::ArrowDown),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(state.section, SettingsSection::Keys);
    state.on_key(
        &Key::Named(NamedKey::ArrowDown),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(state.section, SettingsSection::About);

    // Tab enters About content (Copy config path button), Enter copies config path.
    state.on_key(
        &Key::Named(NamedKey::Tab),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(state.focus, FocusZone::Content);
    assert_eq!(state.focused_setting(), Some(SettingId::CopyConfigPath));
    let copied = state.on_key(
        &Key::Named(NamedKey::Enter),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(
        copied.copy_text.as_deref(),
        Some("/home/user/.config/twitty/config.json")
    );
    assert!(state.copied_config_path);

    // Tab from last control in About wraps back to Sidebar.
    state.on_key(
        &Key::Named(NamedKey::Tab),
        None,
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(state.focus, FocusZone::Sidebar);
}

#[test]
fn arabic_ime_input_works_in_agent_command_font_search_and_keys_search() {
    let mut cfg = TwittyConfig::default();
    let mut state = sample_panel(UiLanguage::Arabic);
    let none = ModifiersState::empty();

    // 1. Arabic IME in Keys search (matches Arabic command titles with hamza/tashkeel folding).
    state.select_section(SettingsSection::Keys, &cfg);
    state.focus = FocusZone::Content;
    state.on_text("قسم يمين", &mut cfg);
    assert_eq!(state.keys_query, "قسم يمين");
    let cmds = state.filtered_commands();
    assert!(cmds.iter().any(|c| c.name == "split.right"), "{cmds:?}");
    state.on_key(
        &Key::Named(NamedKey::Backspace),
        None,
        ModifiersState::CONTROL,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(state.keys_query, "قسم ");

    // 2. Arabic IME in Behaviour -> Agent command text field.
    state.select_section(SettingsSection::Behaviour, &cfg);
    state.focus = FocusZone::Content;
    state.selected = SettingId::BEHAVIOUR.len() - 1;
    assert_eq!(state.focused_setting(), Some(SettingId::AgentCommand));
    assert!(state.accepts_text_input());
    let res = state.on_text("وكيل", &mut cfg);
    assert_eq!(res.changed, vec![SettingId::AgentCommand]);
    assert_eq!(cfg.agent_command.as_deref(), Some("وكيل"));
    // Clearing with Ctrl+Backspace resets agent_command to None.
    let res = state.on_key(
        &Key::Named(NamedKey::Backspace),
        None,
        ModifiersState::CONTROL,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(res.changed, vec![SettingId::AgentCommand]);
    assert_eq!(cfg.agent_command, None);

    // 3. Space types a space inside searchable font picker instead of committing early.
    state.select_section(SettingsSection::Appearance, &cfg);
    state.focus = FocusZone::Content;
    state.selected = 1; // FontFamily
    state.open_picker(SettingId::FontFamily, &cfg);
    state.on_text("Noto", &mut cfg);
    state.on_key(
        &Key::Named(NamedKey::Space),
        Some(" "),
        none,
        &mut cfg,
        SURFACE,
        1.0,
    );
    state.on_text("Mono", &mut cfg);
    let exp = state.expanded.as_ref().unwrap();
    assert_eq!(exp.query, "Noto Mono");
    let filtered = state.filtered_picker_options(SettingId::FontFamily, &exp.query);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].value.as_deref(), Some("Noto Sans Mono"));
}

#[test]
fn arabic_layout_mirrors_sidebar_header_controls_and_segments() {
    let cfg = TwittyConfig::default();
    let en_state = sample_panel(UiLanguage::English);
    let ar_state = sample_panel(UiLanguage::Arabic);

    let en = SettingsLayout::build(SURFACE, 1.0, &en_state, &cfg);
    let ar = SettingsLayout::build(SURFACE, 1.0, &ar_state, &cfg);

    assert!(!en.mirrored);
    assert!(ar.mirrored);
    assert_eq!(en.panel, ar.panel, "centred outer panel keeps same bounds");

    // Sidebar on left in English, right in Arabic.
    assert!(en.sidebar.right() <= en.content.x);
    assert!(ar.content.right() <= ar.sidebar.x);

    // Header close button on right in English, left in Arabic.
    assert!(en.header_title.right() <= en.close_button.x);
    assert!(ar.close_button.right() <= ar.header_title.x);

    // Row label on left in English, right in Arabic.
    assert!(en.rows[0].label.right() <= en.rows[0].widget.x);
    assert!(ar.rows[0].widget.right() <= ar.rows[0].label.x);

    // Segmented options reverse horizontal order in Arabic (option 0 at reading start).
    let en_cursor = en
        .rows
        .iter()
        .find(|r| r.id == SettingId::CursorStyle)
        .unwrap();
    let ar_cursor = ar
        .rows
        .iter()
        .find(|r| r.id == SettingId::CursorStyle)
        .unwrap();
    if let (
        ControlGeometry::Segmented { segments: en_s, .. },
        ControlGeometry::Segmented { segments: ar_s, .. },
    ) = (&en_cursor.geometry, &ar_cursor.geometry)
    {
        assert!(en_s[0].x < en_s[1].x && en_s[1].x < en_s[2].x);
        assert!(ar_s[0].x > ar_s[1].x && ar_s[1].x > ar_s[2].x);
    } else {
        panic!("expected segmented geometry for CursorStyle");
    }

    // Sidebar Left/Right arrow enters content in reading direction (Right in EN, Left in AR).
    let mut ar_nav = sample_panel(UiLanguage::Arabic);
    ar_nav.focus = FocusZone::Sidebar;
    let mut cfg_mut = cfg.clone();
    ar_nav.on_key(
        &Key::Named(NamedKey::ArrowLeft),
        None,
        ModifiersState::empty(),
        &mut cfg_mut,
        SURFACE,
        1.0,
    );
    assert_eq!(ar_nav.focus, FocusZone::Content);
}

#[test]
fn hit_testing_covers_all_controls_in_both_languages_and_scales() {
    for language in [UiLanguage::English, UiLanguage::Arabic] {
        for scale in [1.0, 1.5, 2.0] {
            let surface = (1024.0 * scale, 760.0 * scale);
            let mut cfg = TwittyConfig::default();
            let mut state = sample_panel(language);

            // Appearance controls
            let layout = SettingsLayout::build(surface, scale, &state, &cfg);
            assert_eq!(layout.hit(0.0, 0.0), SettingsHit::Outside);
            assert_eq!(
                layout.hit(center(layout.close_button).0, center(layout.close_button).1),
                SettingsHit::CloseButton
            );
            for &(sec, rect) in &layout.sections {
                let (x, y) = center(rect);
                assert_eq!(layout.hit(x, y), SettingsHit::Section(sec));
            }
            for row in &layout.rows {
                if row.rect.y < layout.viewport.y || row.rect.bottom() > layout.viewport.bottom() {
                    continue;
                }
                match &row.geometry {
                    ControlGeometry::Toggle { track } => {
                        let (x, y) = center(*track);
                        assert_eq!(layout.hit(x, y), SettingsHit::Toggle(row.id));
                    }
                    ControlGeometry::Segmented { segments, .. } => {
                        for (i, &seg) in segments.iter().enumerate() {
                            let (x, y) = center(seg);
                            assert_eq!(layout.hit(x, y), SettingsHit::Segment(row.id, i));
                        }
                    }
                    ControlGeometry::Slider {
                        minus, track, plus, ..
                    } => {
                        let (mx, my) = center(*minus);
                        assert_eq!(layout.hit(mx, my), SettingsHit::SliderMinus(row.id));
                        let (px, py) = center(*plus);
                        assert_eq!(layout.hit(px, py), SettingsHit::SliderPlus(row.id));
                        let (tx, ty) = center(*track);
                        assert!(matches!(
                            layout.hit(tx, ty),
                            SettingsHit::SliderTrack(id, r) if id == row.id && (r - 0.5).abs() < 0.05
                        ));
                    }
                    ControlGeometry::Swatches { swatches } => {
                        for (i, &sw) in swatches.iter().enumerate() {
                            let (x, y) = center(sw);
                            assert_eq!(layout.hit(x, y), SettingsHit::Swatch(i));
                        }
                    }
                    ControlGeometry::ListPicker { button } => {
                        let (x, y) = center(*button);
                        assert_eq!(layout.hit(x, y), SettingsHit::PickerToggle(row.id));
                    }
                    _ => {}
                }
            }

            // Mouse wheel scrolls Appearance when content overflows on a shorter window.
            let short = (900.0 * scale, 420.0 * scale);
            let wheel_res = state.on_wheel(
                f64::from(short.0 / 2.0),
                f64::from(short.1 / 2.0),
                -3.0,
                &cfg,
                short,
                scale,
            );
            assert!(wheel_res.redraw && state.scroll_y > 0.0);

            // Keys section hit testing
            state.select_section(SettingsSection::Keys, &cfg);
            let klayout = SettingsLayout::build(surface, scale, &state, &cfg);
            let (sx, sy) = center(klayout.keys_search.unwrap());
            assert_eq!(klayout.hit(sx, sy), SettingsHit::KeysSearch);
            let (idx, row_rect) = klayout.keys_rows[0];
            let (rx, ry) = center(row_rect);
            assert_eq!(klayout.hit(rx, ry), SettingsHit::KeyRow(idx));

            // About section Copy button hit testing + click
            state.select_section(SettingsSection::About, &cfg);
            let alayout = SettingsLayout::build(surface, scale, &state, &cfg);
            let (bx, by) = center(alayout.copy_button.unwrap());
            assert_eq!(alayout.hit(bx, by), SettingsHit::CopyConfigPath);
            let clicked = state.on_mouse_down(bx, by, &mut cfg, surface, scale);
            assert_eq!(
                clicked.copy_text.as_deref(),
                Some("/home/user/.config/twitty/config.json")
            );
        }
    }
}

#[test]
fn font_database_helpers_return_sorted_deduplicated_families() {
    let fs = new_font_system();
    let mono = monospace_families(&fs);
    let all = all_families(&fs);
    assert!(!all.is_empty(), "system + embedded fonts must not be empty");
    assert!(
        all.iter().any(|f| f.contains("Noto Naskh Arabic")),
        "embedded Arabic font should be listed"
    );
    for list in [&mono, &all] {
        for pair in list.windows(2) {
            assert!(
                pair[0].to_ascii_lowercase() < pair[1].to_ascii_lowercase(),
                "list must be strictly sorted and deduplicated: {pair:?}"
            );
        }
    }
}

#[test]
fn overlay_renders_and_clips_underlying_terminal_text_in_dark_light_en_ar() {
    let mut font_system = FontSystem::new();
    let font = UiFont::default();
    let metrics = CellMetrics {
        font_size: 14.5,
        scale: 1.0,
        char_width: 9.0,
        line_height: 22.0,
        padding_left: 8.0,
        padding_top: 6.0,
    };
    let surface = (1024.0, 720.0);

    for (theme_name, language) in [
        ("tokyo-night", UiLanguage::English),
        ("github-light", UiLanguage::Arabic),
    ] {
        let palette = Palette::named(theme_name).unwrap();
        let mut ctx = DrawContext {
            font_system: &mut font_system,
            family: None,
            palette: &palette,
            metrics,
            is_srgb: false,
        };
        let opts = FrameOptions {
            window_focused: true,
            cursor_style: "block",
            cursor_blink_on: true,
            surface,
        };
        let mut term = Terminal::new(90, 28, |_| {});
        for i in 0..28 {
            term.process_bytes(format!("سطر عربي {i} terminal row {i}\r\n").as_bytes());
        }
        let (lines, cursor) = term.snapshot();
        let panes = vec![PaneFrame {
            id: PaneId(1),
            rect: Rect::new(0.0, 0.0, surface.0, surface.1),
            lines,
            cursor,
            focused: true,
            status: None,
            blocks: Vec::new(),
        }];
        let mut cache = RowCache::default();
        let mut ime = PreeditState::default();
        let mut frame = build_frame(&mut ctx, &opts, &mut cache, &mut ime, &[], &panes, &[]);
        let style = UiStyle::new(&palette, None, metrics.scale);

        let mut state = sample_panel(language);
        let mut cfg = TwittyConfig::default();
        cfg.theme = Some(theme_name.into());
        state.open_picker(SettingId::Theme, &cfg);
        let panel = SettingsLayout::build(surface, 1.0, &state, &cfg).panel;

        let view = SettingsView {
            state: state.clone(),
            config: cfg.clone(),
        };
        frame.add_settings(&mut ctx, style, &font, &view);

        // First overlay shape is the rounded panel shell with border and shadow.
        assert!(matches!(
            frame.shapes.overlay.first(),
            Some(Shape::RoundedRect {
                rect,
                border: Some(_),
                shadow: Some(_),
                ..
            }) if *rect == panel
        ));

        let runs = frame.text_runs(&panes, &cache, &ime);
        assert!(!runs.is_empty());

        // Partition runs into pane runs (before overlay) vs settings overlay runs.
        // Every run whose bounds lie outside the panel must NOT overlap the panel rect.
        let mut overlay_count = 0usize;
        for run in &runs {
            let b = run.bounds;
            let inside_panel = (b.left as f32) >= panel.x - 1.0
                && (b.right as f32) <= panel.right() + 1.0
                && (b.top as f32) >= panel.y - 1.0
                && (b.bottom as f32) <= panel.bottom() + 1.0;
            if inside_panel {
                overlay_count += 1;
            } else {
                let overlaps = (b.left as f32) < panel.right()
                    && (b.right as f32) > panel.x
                    && (b.top as f32) < panel.bottom()
                    && (b.bottom as f32) > panel.y;
                assert!(
                    !overlaps,
                    "underlying pane text leaked into settings panel: {b:?} vs {panel:?}"
                );
            }
        }
        assert!(overlay_count > 5, "expected settings labels inside panel");

        // Also smoke-render Behaviour, Keys, and About sections without panicking.
        for sec in [
            SettingsSection::Behaviour,
            SettingsSection::Keys,
            SettingsSection::About,
        ] {
            state.select_section(sec, &cfg);
            let mut f = build_frame(&mut ctx, &opts, &mut cache, &mut ime, &[], &panes, &[]);
            f.add_settings(
                &mut ctx,
                style,
                &font,
                &SettingsView {
                    state: state.clone(),
                    config: cfg.clone(),
                },
            );
            let sec_runs = f.text_runs(&panes, &cache, &ime);
            assert!(!sec_runs.is_empty());
        }
    }
}
