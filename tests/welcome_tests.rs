use std::ffi::OsStr;
use std::fs;
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use winit::keyboard::{Key, ModifiersState, NamedKey};

use twitty::color::Palette;
use twitty::commands::{self, Command};
use twitty::config::TwittyConfig;
use twitty::input::InputAction;
use twitty::layout::{PaneId, Rect};
use twitty::render::metrics::CellMetrics;
use twitty::render::row::cache::RowCache;
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::frame::{build_frame, FrameOptions};
use twitty::render::scene::preedit::PreeditState;
use twitty::render::scene::welcome::WelcomeView;
use twitty::render::text::fonts::new_font_system;
use twitty::render::text::ui::UiFont;
use twitty::render::ui_style::UiStyle;
use twitty::render::PaneFrame;
use twitty::terminal::Terminal;
use twitty::ui::chrome::{UiLanguage, UiLanguagePref};
use twitty::ui::palette::{matches, PaletteEntry};
use twitty::ui::welcome::{
    build_shortcut_entries, flag_path_from, is_done_at, mark_done_at, should_show_first_run,
    AgentsInfo, ShellStatus, WelcomeHit, WelcomeLayout, WelcomeState, WelcomeStep,
    DEFAULT_TRANSPARENT_OPACITY, FIRST_RUN_FLAG_NAME, WELCOME_SHORTCUT_COMMANDS, WELCOME_THEMES,
};

static TEST_DIR_SEQ: AtomicU64 = AtomicU64::new(0);

fn fresh_temp_dir(prefix: &str) -> PathBuf {
    let seq = TEST_DIR_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "twitty-welcome-test-{prefix}-{}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn rect_center(r: Rect) -> (f64, f64) {
    (f64::from(r.x + r.w * 0.5), f64::from(r.y + r.h * 0.5))
}

fn sample_state(config: &TwittyConfig, env_lang: UiLanguage) -> WelcomeState {
    let shell = ShellStatus::detect(Some("/usr/bin/zsh"), true);
    let agents = AgentsInfo::detect(None, None, None);
    WelcomeState::with_default_commands(config, env_lang, shell, agents)
}

#[cfg(unix)]
fn write_executable_stub(dir: &Path, name: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

// ---- 1. First-run detection & done flag persistence ----

#[test]
fn flag_path_respects_xdg_state_home_and_ignores_relative_paths() {
    // Platform-absolute paths, so the test also holds on Windows.
    let xdg_root = std::env::temp_dir().join("state-root");
    let home_root = std::env::temp_dir().join("home-tester");
    let xdg = xdg_root.as_os_str();
    let home = home_root.as_os_str();
    let home_flag = home_root
        .join(".local")
        .join("state")
        .join("twitty")
        .join(FIRST_RUN_FLAG_NAME);
    assert_eq!(
        flag_path_from(Some(xdg), Some(home)),
        Some(xdg_root.join("twitty").join(FIRST_RUN_FLAG_NAME))
    );
    assert_eq!(flag_path_from(None, Some(home)), Some(home_flag.clone()));
    // Relative XDG_STATE_HOME falls back to HOME.
    assert_eq!(
        flag_path_from(Some(OsStr::new("relative/state")), Some(home)),
        Some(home_flag)
    );
    assert_eq!(flag_path_from(Some(OsStr::new("")), None), None);
}

#[test]
fn first_run_detection_matrix() {
    let root = fresh_temp_dir("detect");
    let config_path = root.join("config/twitty/config.json");
    let flag_path = root.join("state/twitty").join(FIRST_RUN_FLAG_NAME);

    // Neither config nor flag -> show first-run welcome.
    assert!(should_show_first_run(Some(&config_path), Some(&flag_path)));
    // Missing flag_path -> never auto-show (cannot persist dismissal).
    assert!(!should_show_first_run(Some(&config_path), None));

    // After marking done (e.g. user pressed Esc/Skip without saving config) -> never show again.
    mark_done_at(&flag_path).unwrap();
    assert!(is_done_at(&flag_path));
    assert!(!should_show_first_run(Some(&config_path), Some(&flag_path)));

    // Existing config file without flag -> existing user, do not auto-show.
    let flag_path_2 = root.join("state2/twitty").join(FIRST_RUN_FLAG_NAME);
    TwittyConfig::default().save_to(&config_path).unwrap();
    assert!(!should_show_first_run(
        Some(&config_path),
        Some(&flag_path_2)
    ));

    // Both exist -> do not show.
    mark_done_at(&flag_path_2).unwrap();
    assert!(!should_show_first_run(
        Some(&config_path),
        Some(&flag_path_2)
    ));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn done_flag_persistence_is_atomic_0600_and_refuses_symlinks() {
    let root = fresh_temp_dir("flag");
    let state_dir = root.join("state/twitty");
    let flag_path = state_dir.join(FIRST_RUN_FLAG_NAME);

    assert!(!is_done_at(&flag_path));
    mark_done_at(&flag_path).unwrap();
    assert!(is_done_at(&flag_path));

    // No stray temporary files left behind in state dir.
    let entries: Vec<_> = fs::read_dir(&state_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, vec![FIRST_RUN_FLAG_NAME]);

    #[cfg(unix)]
    {
        use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
        let dir_mode = fs::symlink_metadata(&state_dir).unwrap().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "state directory should be 0700");
        let file_mode = fs::symlink_metadata(&flag_path).unwrap().mode() & 0o777;
        assert_eq!(file_mode, 0o600, "first_run_done flag should be 0600");

        // Group/world-writable flag is rejected by is_done_at.
        fs::set_permissions(&flag_path, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(!is_done_at(&flag_path));

        // Symlink target is refused for both reading and writing.
        let decoy = root.join("decoy");
        fs::write(&decoy, "decoy").unwrap();
        let sym_flag = state_dir.join("sym_done");
        symlink(&decoy, &sym_flag).unwrap();
        assert!(!is_done_at(&sym_flag));
        assert!(mark_done_at(&sym_flag).is_err());

        // Symlinked state parent directory is refused.
        let real_dir = root.join("real_dir");
        fs::create_dir_all(&real_dir).unwrap();
        let sym_dir = root.join("sym_dir");
        symlink(&real_dir, &sym_dir).unwrap();
        let nested = sym_dir.join(FIRST_RUN_FLAG_NAME);
        assert!(mark_done_at(&nested).is_err());
        assert!(!is_done_at(&nested));
    }

    let _ = fs::remove_dir_all(&root);
}

// ---- 2. ui_language config parsing, sanitization & fallback ----

#[test]
fn ui_language_parsing_and_environment_fallback() {
    assert_eq!("auto".parse::<UiLanguagePref>(), Ok(UiLanguagePref::Auto));
    assert_eq!("en".parse::<UiLanguagePref>(), Ok(UiLanguagePref::English));
    assert_eq!("ar".parse::<UiLanguagePref>(), Ok(UiLanguagePref::Arabic));
    assert_eq!(" AR ".parse::<UiLanguagePref>(), Ok(UiLanguagePref::Arabic));
    assert_eq!("En".parse::<UiLanguagePref>(), Ok(UiLanguagePref::English));
    assert!("fr".parse::<UiLanguagePref>().is_err());

    let mut cfg = TwittyConfig::default();
    assert_eq!(cfg.ui_language, "auto");
    assert_eq!(
        cfg.resolve_ui_language(UiLanguage::English),
        UiLanguage::English
    );
    assert_eq!(
        cfg.resolve_ui_language(UiLanguage::Arabic),
        UiLanguage::Arabic
    );

    cfg.ui_language = "ar".into();
    assert_eq!(
        cfg.resolve_ui_language(UiLanguage::English),
        UiLanguage::Arabic
    );

    cfg.ui_language = "en".into();
    assert_eq!(
        cfg.resolve_ui_language(UiLanguage::Arabic),
        UiLanguage::English
    );

    // Loading invalid or non-string ui_language from JSON falls back safely to "auto".
    let root = fresh_temp_dir("lang-cfg");
    let invalid_str = root.join("invalid_str.json");
    fs::write(&invalid_str, r#"{"ui_language": "zz"}"#).unwrap();
    let loaded = TwittyConfig::load_from(&invalid_str);
    assert_eq!(loaded.ui_language, "auto");
    assert_eq!(loaded.ui_language_pref(), UiLanguagePref::Auto);

    let invalid_type = root.join("invalid_type.json");
    fs::write(&invalid_type, r#"{"ui_language": 42}"#).unwrap();
    let loaded_type = TwittyConfig::load_from(&invalid_type);
    assert_eq!(loaded_type.ui_language, "auto");

    let valid = root.join("valid.json");
    fs::write(&valid, r#"{"ui_language": "AR"}"#).unwrap();
    let loaded_valid = TwittyConfig::load_from(&valid);
    assert_eq!(loaded_valid.ui_language, "ar");
    assert_eq!(loaded_valid.ui_language_pref(), UiLanguagePref::Arabic);

    let _ = fs::remove_dir_all(&root);
}

// ---- 3. Command palette registration ----

#[test]
fn welcome_command_is_registered_and_searchable_in_english_and_arabic() {
    let all = commands::all();
    let cmd = all
        .iter()
        .find(|c| c.name == "welcome.open")
        .expect("welcome.open command missing");
    assert_eq!(cmd.title_en, "Welcome");
    assert_eq!(cmd.title_ar, "الترحيب");
    assert_eq!(commands::action("welcome.open"), Some(InputAction::Welcome));

    let entries = PaletteEntry::builtins();
    let en_hits = matches(&entries, "welcome");
    assert!(en_hits.iter().any(|&i| entries[i].name == "welcome.open"));
    let ar_hits = matches(&entries, "الترحيب");
    assert!(ar_hits.iter().any(|&i| entries[i].name == "welcome.open"));
}

// ---- 4. Each step's model ----

#[test]
fn step_1_welcome_language_switches_live_and_mirrors() {
    let mut cfg = TwittyConfig::default();
    let mut state = sample_state(&cfg, UiLanguage::English);
    assert_eq!(state.step, WelcomeStep::Welcome);
    assert_eq!(state.language, UiLanguage::English);
    assert!(!state.mirrored());

    // Switch to Arabic.
    let out = state.set_language_pref(UiLanguagePref::Arabic, &mut cfg);
    assert_eq!(cfg.ui_language, "ar");
    assert_eq!(state.language, UiLanguage::Arabic);
    assert!(state.mirrored());
    assert!(out.apply.language);
    assert!(out.redraw);

    // Switch to Auto -> falls back to env_language (English).
    let out = state.set_language_pref(UiLanguagePref::Auto, &mut cfg);
    assert_eq!(cfg.ui_language, "auto");
    assert_eq!(state.language, UiLanguage::English);
    assert!(out.apply.language);
}

#[test]
fn step_2_look_themes_font_size_and_transparency_apply_live() {
    // Every featured swatch resolves to a real built-in palette.
    assert!(WELCOME_THEMES.len() >= 4);
    assert_eq!(WELCOME_THEMES[0].id, "twitty-dark");
    assert_eq!(WELCOME_THEMES[1].id, "twitty-light");
    for spec in WELCOME_THEMES {
        assert!(
            Palette::named(spec.id).is_some(),
            "missing palette for {}",
            spec.id
        );
    }

    let mut cfg = TwittyConfig::default();
    cfg.background_opacity = 0.85;
    let mut state = sample_state(&cfg, UiLanguage::English);
    state.go_to(WelcomeStep::Look);

    // Pick Twitty Light.
    let out = state.select_theme(1, &mut cfg);
    assert_eq!(cfg.theme.as_deref(), Some("twitty-light"));
    assert!(out.apply.palette);
    assert_eq!(WelcomeState::active_theme_index(&cfg), Some(1));

    // Font size +/- with clamping.
    let orig = cfg.font_size;
    let out = state.step_font_size(2, &mut cfg);
    assert!((cfg.font_size - (orig + 1.0)).abs() < 1e-4);
    assert!(out.apply.font_size);

    let out = state.step_font_size(-100, &mut cfg);
    assert_eq!(cfg.font_size, 8.0);
    assert!(out.apply.font_size);

    // Transparency off -> 1.0, then back on -> restores remembered 0.85.
    assert!(WelcomeState::transparency_enabled(&cfg));
    let out = state.toggle_transparency(&mut cfg);
    assert!((cfg.background_opacity - 1.0).abs() < 1e-4);
    assert!(out.apply.opacity);
    assert!(!WelcomeState::transparency_enabled(&cfg));

    let out = state.toggle_transparency(&mut cfg);
    assert!((cfg.background_opacity - 0.85).abs() < 1e-4);
    assert!(out.apply.opacity);

    // Starting opaque (1.0) and toggling on uses DEFAULT_TRANSPARENT_OPACITY.
    let mut solid_cfg = TwittyConfig::default();
    solid_cfg.background_opacity = 1.0;
    let mut solid_state = sample_state(&solid_cfg, UiLanguage::English);
    let out = solid_state.toggle_transparency(&mut solid_cfg);
    assert!((solid_cfg.background_opacity - DEFAULT_TRANSPARENT_OPACITY).abs() < 1e-4);
    assert!(out.apply.opacity);
}

#[test]
fn step_3_shell_detection_marks_and_toggle() {
    let bash = ShellStatus::detect(Some("/bin/bash"), false);
    assert_eq!(bash.name, "bash");
    assert!(bash.supported);
    assert!(!bash.marks_seen);

    let fish = ShellStatus::detect(Some("/usr/bin/fish"), true);
    assert_eq!(fish.name, "fish");
    assert!(fish.supported);
    assert!(fish.marks_seen);

    let custom = ShellStatus::detect(Some("/usr/local/bin/nu"), false);
    assert_eq!(custom.name, "nu");
    assert!(!custom.supported);

    let mut cfg = TwittyConfig::default();
    let mut state =
        WelcomeState::with_default_commands(&cfg, UiLanguage::English, bash, AgentsInfo::default());
    assert!(state.set_shell_marks_seen(true));
    assert!(!state.set_shell_marks_seen(true));
    assert!(state.shell.marks_seen);

    assert!(cfg.shell_integration);
    let out = state.toggle_shell_integration(&mut cfg);
    assert!(!cfg.shell_integration);
    assert!(out.redraw);
}

#[test]
fn step_4_ai_agent_detection_priority_override_and_toggle() {
    let empty = AgentsInfo::detect(None, None, None);
    assert_eq!(empty.agents.len(), 4);
    assert_eq!(empty.installed_count(), 0);
    assert_eq!(empty.selected, None);

    #[cfg(unix)]
    {
        let bin = fresh_temp_dir("agents");
        write_executable_stub(&bin, "codex");
        write_executable_stub(&bin, "opencode");

        // Auto-detect selects codex (first installed in priority order).
        let detected = AgentsInfo::detect(None, None, Some(bin.as_os_str()));
        assert_eq!(detected.installed_count(), 2);
        assert_eq!(detected.selected.as_deref(), Some("codex"));
        assert!(!detected.from_config);
        assert!(detected
            .agents
            .iter()
            .any(|a| a.name == "codex" && a.installed && a.active));
        assert!(detected
            .agents
            .iter()
            .any(|a| a.name == "opencode" && a.installed && !a.active));
        assert!(detected
            .agents
            .iter()
            .any(|a| a.name == "claude" && !a.installed && !a.active));

        // Explicit config override takes precedence.
        let overridden = AgentsInfo::detect(Some("opencode"), None, Some(bin.as_os_str()));
        assert_eq!(overridden.selected.as_deref(), Some("opencode"));
        assert!(overridden.from_config);
        assert!(overridden
            .agents
            .iter()
            .any(|a| a.name == "opencode" && a.active));
        assert!(overridden
            .agents
            .iter()
            .any(|a| a.name == "codex" && !a.active));

        let _ = fs::remove_dir_all(&bin);
    }

    let mut cfg = TwittyConfig::default();
    let mut state = sample_state(&cfg, UiLanguage::English);
    assert!(cfg.ai);
    let out = state.toggle_ai(&mut cfg);
    assert!(!cfg.ai);
    assert!(out.redraw);
}

#[test]
fn step_5_shortcuts_reflect_effective_command_table_chords() {
    let entries = build_shortcut_entries(&commands::all());
    assert_eq!(entries.len(), WELCOME_SHORTCUT_COMMANDS.len());
    assert!((8..=10).contains(&entries.len()));

    for required in [
        "palette.open",
        "composer.open",
        "settings.open",
        "split.right",
        "search.all",
        "project.open",
        "review.toggle",
        "agent.ask",
    ] {
        let entry = entries
            .iter()
            .find(|e| e.name == required)
            .unwrap_or_else(|| panic!("missing shortcut {required}"));
        assert!(entry.chord.is_some(), "expected chord for {required}");
        assert!(!entry.title_en.is_empty());
        assert!(!entry.title_ar.is_empty());
    }

    // Custom chords in the command table flow straight through.
    let custom_table = vec![Command {
        name: "palette.open",
        title_en: "Command palette",
        title_ar: "لوحة الأوامر",
        shortcut: Some("Alt+P"),
    }];
    let custom_entries = build_shortcut_entries(&custom_table);
    assert_eq!(custom_entries.len(), 1);
    assert_eq!(custom_entries[0].chord.as_deref(), Some("Alt+P"));
}

// ---- 5. Live apply / revert & keyboard wizard navigation ----

#[test]
fn esc_reverts_all_live_changes_and_marks_done_without_saving_config() {
    let mut cfg = TwittyConfig::default();
    cfg.ui_language = "en".into();
    cfg.theme = Some("twitty-dark".into());
    cfg.font_size = 14.5;
    cfg.background_opacity = 0.78;
    cfg.shell_integration = true;
    cfg.ai = true;
    let original_lang = cfg.ui_language.clone();
    let original_theme = cfg.theme.clone();
    let original_font = cfg.font_size;
    let original_opacity = cfg.background_opacity;

    let mut state = sample_state(&cfg, UiLanguage::English);
    state.set_language_pref(UiLanguagePref::Arabic, &mut cfg);
    state.select_theme(1, &mut cfg);
    state.step_font_size(4, &mut cfg);
    state.toggle_transparency(&mut cfg);
    state.toggle_shell_integration(&mut cfg);
    state.toggle_ai(&mut cfg);

    assert_eq!(cfg.ui_language, "ar");
    assert_eq!(cfg.theme.as_deref(), Some("twitty-light"));
    assert!((cfg.font_size - 16.5).abs() < 1e-4);
    assert!((cfg.background_opacity - 1.0).abs() < 1e-4);
    assert!(!cfg.shell_integration);
    assert!(!cfg.ai);

    let out = state.on_key(
        &Key::Named(NamedKey::Escape),
        ModifiersState::empty(),
        &mut cfg,
    );
    assert!(out.close);
    assert!(out.mark_done);
    assert!(!out.save_config);
    assert!(out.apply.language);
    assert!(out.apply.palette);
    assert!(out.apply.font_size);
    assert!(out.apply.opacity);

    assert_eq!(cfg.ui_language, original_lang);
    assert_eq!(cfg.theme, original_theme);
    assert!((cfg.font_size - original_font).abs() < 1e-4);
    assert!((cfg.background_opacity - original_opacity).abs() < 1e-4);
    assert!(cfg.shell_integration);
    assert!(cfg.ai);
    assert_eq!(state.language, UiLanguage::English);
}

#[test]
fn keyboard_wizard_advances_through_steps_and_finishes_with_save() {
    let mut cfg = TwittyConfig::default();
    let mut state = sample_state(&cfg, UiLanguage::English);

    // Step 1 -> Step 2 via Enter.
    let out = state.on_key(
        &Key::Named(NamedKey::Enter),
        ModifiersState::empty(),
        &mut cfg,
    );
    assert!(!out.close);
    assert_eq!(state.step, WelcomeStep::Look);

    // Select a theme via ArrowRight, tab to font size, increase, tab to transparency, toggle.
    let out = state.on_key(
        &Key::Named(NamedKey::ArrowRight),
        ModifiersState::empty(),
        &mut cfg,
    );
    assert!(out.apply.palette);
    assert_eq!(cfg.theme.as_deref(), Some("twitty-light"));

    state.on_key(
        &Key::Named(NamedKey::Tab),
        ModifiersState::empty(),
        &mut cfg,
    );
    assert_eq!(state.focused_control, 1);
    let out = state.on_key(
        &Key::Named(NamedKey::ArrowRight),
        ModifiersState::empty(),
        &mut cfg,
    );
    assert!(out.apply.font_size);

    state.on_key(
        &Key::Named(NamedKey::Tab),
        ModifiersState::empty(),
        &mut cfg,
    );
    assert_eq!(state.focused_control, 2);
    let out = state.on_key(
        &Key::Named(NamedKey::Space),
        ModifiersState::empty(),
        &mut cfg,
    );
    assert!(out.apply.opacity);

    // Jump straight to step 5 via number key '5', then Enter finishes.
    state.on_key(
        &Key::Character("5".into()),
        ModifiersState::empty(),
        &mut cfg,
    );
    assert_eq!(state.step, WelcomeStep::Shortcuts);

    let out = state.on_key(
        &Key::Named(NamedKey::Enter),
        ModifiersState::empty(),
        &mut cfg,
    );
    assert!(out.close);
    assert!(out.save_config);
    assert!(out.mark_done);
    assert_eq!(cfg.theme.as_deref(), Some("twitty-light"));

    // Atomic config round-trip preserves saved changes.
    let root = fresh_temp_dir("save-roundtrip");
    let cfg_path = root.join("config.json");
    cfg.save_to(&cfg_path).unwrap();
    let reloaded = TwittyConfig::load_from(&cfg_path);
    assert_eq!(reloaded.theme.as_deref(), Some("twitty-light"));
    assert!((reloaded.font_size - cfg.font_size).abs() < 1e-4);
    assert!((reloaded.background_opacity - cfg.background_opacity).abs() < 1e-4);
    let _ = fs::remove_dir_all(&root);
}

// ---- 6. Arabic mirroring & pointer hit testing ----

#[test]
fn arabic_layout_mirrors_chrome_and_step_controls() {
    let surface = (1280.0, 800.0);
    let scale = 1.0;

    let mut en_cfg = TwittyConfig::default();
    en_cfg.ui_language = "en".into();
    let mut ar_cfg = TwittyConfig::default();
    ar_cfg.ui_language = "ar".into();
    let mut en_state = sample_state(&en_cfg, UiLanguage::English);
    let mut ar_state = sample_state(&ar_cfg, UiLanguage::Arabic);

    // Step 1: header, footer, dots, language options mirrored.
    let en_l = WelcomeLayout::build(surface, scale, &en_state);
    let ar_l = WelcomeLayout::build(surface, scale, &ar_state);

    assert_eq!(en_l.card, ar_l.card);
    assert!(en_l.header_title.x < en_l.close_button.x);
    assert!(ar_l.header_title.x > ar_l.close_button.x);

    assert!(en_l.skip_button.x < en_l.next_button.x);
    assert!(ar_l.skip_button.x > ar_l.next_button.x);

    assert!(en_l.dots[0].1.x < en_l.dots[4].1.x);
    assert!(ar_l.dots[0].1.x > ar_l.dots[4].1.x);

    assert!(en_l.language_options[0].1.x < en_l.language_options[2].1.x);
    assert!(ar_l.language_options[0].1.x > ar_l.language_options[2].1.x);

    // Step 2: theme swatches, font stepper, transparency toggle mirrored.
    en_state.go_to(WelcomeStep::Look);
    ar_state.go_to(WelcomeStep::Look);
    let en_l2 = WelcomeLayout::build(surface, scale, &en_state);
    let ar_l2 = WelcomeLayout::build(surface, scale, &ar_state);

    assert!(en_l2.theme_swatches[0].x < en_l2.theme_swatches[4].x);
    assert!(ar_l2.theme_swatches[0].x > ar_l2.theme_swatches[4].x);
    assert!(en_l2.font_label.x < en_l2.font_minus.x);
    assert!(ar_l2.font_label.x > ar_l2.font_plus.x);
    assert!(en_l2.transparency_label.x < en_l2.transparency_track.x);
    assert!(ar_l2.transparency_label.x > ar_l2.transparency_track.x);

    // Step 5: shortcut title and chord mirrored.
    en_state.go_to(WelcomeStep::Shortcuts);
    ar_state.go_to(WelcomeStep::Shortcuts);
    let en_l5 = WelcomeLayout::build(surface, scale, &en_state);
    let ar_l5 = WelcomeLayout::build(surface, scale, &ar_state);
    assert!(en_l5.shortcut_rows[0].1.x < en_l5.shortcut_rows[0].2.x);
    assert!(ar_l5.shortcut_rows[0].1.x > ar_l5.shortcut_rows[0].2.x);
}

#[test]
fn hit_testing_and_mouse_clicks_work_in_english_and_arabic_at_multiple_scales() {
    for (lang_pref, env_lang, scale) in [
        ("en", UiLanguage::English, 1.0_f32),
        ("ar", UiLanguage::Arabic, 1.5_f32),
    ] {
        let surface = (1280.0 * scale, 800.0 * scale);
        let mut cfg = TwittyConfig::default();
        cfg.ui_language = lang_pref.into();
        let mut state = sample_state(&cfg, env_lang);

        // Step 1: outside, close, skip, dots, language options, next.
        let l1 = WelcomeLayout::build(surface, scale, &state);
        assert_eq!(l1.hit(2.0, 2.0), WelcomeHit::Outside);
        let (cx, cy) = rect_center(l1.close_button);
        assert_eq!(l1.hit(cx, cy), WelcomeHit::Close);
        let (sx, sy) = rect_center(l1.skip_button);
        assert_eq!(l1.hit(sx, sy), WelcomeHit::Skip);
        assert!(l1.back_button.is_none());

        for &(step, r) in &l1.dots {
            let (dx, dy) = rect_center(r);
            assert_eq!(l1.hit(dx, dy), WelcomeHit::StepDot(step));
        }
        for &(pref, r) in &l1.language_options {
            let (lx, ly) = rect_center(r);
            assert_eq!(l1.hit(lx, ly), WelcomeHit::LanguageOption(pref));
        }

        // Click Next -> Step 2.
        let (nx, ny) = rect_center(l1.next_button);
        let out = state.on_mouse_down(nx, ny, &mut cfg, surface, scale);
        assert!(out.redraw);
        assert_eq!(state.step, WelcomeStep::Look);

        // Step 2: Back button now present, theme swatch + font +/- + transparency toggle.
        let l2 = WelcomeLayout::build(surface, scale, &state);
        assert!(l2.back_button.is_some());
        let (bx, by) = rect_center(l2.back_button.unwrap());
        assert_eq!(l2.hit(bx, by), WelcomeHit::Back);

        let (swx, swy) = rect_center(l2.theme_swatches[2]);
        assert_eq!(l2.hit(swx, swy), WelcomeHit::ThemeSwatch(2));
        let out = state.on_mouse_down(swx, swy, &mut cfg, surface, scale);
        assert!(out.apply.palette);
        assert_eq!(cfg.theme.as_deref(), Some(WELCOME_THEMES[2].id));

        let (px, py) = rect_center(l2.font_plus);
        let before = cfg.font_size;
        state.on_mouse_down(px, py, &mut cfg, surface, scale);
        assert!(cfg.font_size > before);

        let (mx, my) = rect_center(l2.font_minus);
        state.on_mouse_down(mx, my, &mut cfg, surface, scale);
        assert!((cfg.font_size - before).abs() < 1e-4);

        let (tx, ty) = rect_center(l2.transparency_track);
        assert_eq!(l2.hit(tx, ty), WelcomeHit::TransparencyToggle);
        state.on_mouse_down(tx, ty, &mut cfg, surface, scale);
        assert!(!WelcomeState::transparency_enabled(&cfg));

        // Jump to Step 3 via dot click & toggle shell integration.
        let (d3x, d3y) = rect_center(l2.dots[2].1);
        state.on_mouse_down(d3x, d3y, &mut cfg, surface, scale);
        assert_eq!(state.step, WelcomeStep::Shell);
        let l3 = WelcomeLayout::build(surface, scale, &state);
        let (stx, sty) = rect_center(l3.shell_toggle_track);
        assert_eq!(l3.hit(stx, sty), WelcomeHit::ShellIntegrationToggle);
        state.on_mouse_down(stx, sty, &mut cfg, surface, scale);
        assert!(!cfg.shell_integration);

        // Jump to Step 4 via Next & toggle AI.
        let (nx, ny) = rect_center(l3.next_button);
        state.on_mouse_down(nx, ny, &mut cfg, surface, scale);
        assert_eq!(state.step, WelcomeStep::Ai);
        let l4 = WelcomeLayout::build(surface, scale, &state);
        let (atx, aty) = rect_center(l4.ai_toggle_track);
        assert_eq!(l4.hit(atx, aty), WelcomeHit::AiToggle);
        state.on_mouse_down(atx, aty, &mut cfg, surface, scale);
        assert!(!cfg.ai);

        // Clicking Skip reverts everything and marks done.
        let (skx, sky) = rect_center(l4.skip_button);
        let out = state.on_mouse_down(skx, sky, &mut cfg, surface, scale);
        assert!(out.close);
        assert!(out.mark_done);
        assert!(!out.save_config);
        assert!(cfg.shell_integration);
        assert!(cfg.ai);
    }
}

// ---- 7. Headless rendering & Glyphon text clipping ----

#[test]
fn welcome_overlay_renders_all_steps_in_both_themes_and_clips_underlying_terminal_text() {
    let mut font_system = new_font_system();
    let ui_font = UiFont::new(&mut font_system, None);
    let surface = (1000.0_f32, 680.0_f32);

    let mut term = Terminal::new(80, 24, |_| {});
    term.process_bytes("Hello terminal text underneath مرحباً\r\n".as_bytes());
    let (lines, cursor) = term.snapshot();

    for (theme_name, lang, scale) in [
        ("twitty-dark", UiLanguage::English, 1.0_f32),
        ("twitty-light", UiLanguage::Arabic, 1.5_f32),
    ] {
        let palette = Palette::named(theme_name).unwrap();
        let metrics = CellMetrics::measure(&mut font_system, None, 14.5, scale);
        let style = UiStyle::new(&palette, None, scale);
        let mut cfg = TwittyConfig::default();
        cfg.theme = Some(theme_name.into());
        cfg.ui_language = if lang == UiLanguage::Arabic {
            "ar".into()
        } else {
            "en".into()
        };
        let mut state = sample_state(&cfg, lang);

        for step in WelcomeStep::ALL {
            state.go_to(step);
            let layout = WelcomeLayout::build(surface, scale, &state);
            let view = WelcomeView {
                state: state.clone(),
                config: cfg.clone(),
            };
            let panes = [PaneFrame {
                id: PaneId(1),
                rect: Rect::new(0.0, 0.0, surface.0, surface.1),
                lines: lines.clone(),
                cursor: cursor.clone(),
                focused: true,
                status: None,
                blocks: Vec::new(),
            }];
            let mut cache = RowCache::default();
            let mut ime = PreeditState::default();
            let mut ctx = DrawContext {
                metrics,
                font_system: &mut font_system,
                family: None,
                palette: &palette,
                is_srgb: false,
            };
            let opts = FrameOptions {
                window_focused: true,
                cursor_style: "block",
                cursor_blink_on: true,
                surface,
            };
            let mut frame = build_frame(&mut ctx, &opts, &mut cache, &mut ime, &[], &panes, &[]);
            frame.add_welcome(&mut ctx, style, &ui_font, &view);

            assert!(
                !frame.shapes.overlay.is_empty(),
                "expected overlay shapes for {theme_name} {step:?}"
            );

            let runs = frame.text_runs(&panes, &cache, &ime);
            // Verify overlay labels exist inside the card, AND any terminal text run whose bounds
            // were clipped does not overlap the card interior.
            let card_l = layout.card.x.floor() as i32;
            let card_t = layout.card.y.floor() as i32;
            let card_r = layout.card.right().ceil() as i32;
            let card_b = layout.card.bottom().ceil() as i32;

            let mut inside_card_runs = 0usize;
            for run in &runs {
                let strictly_inside = run.bounds.left >= card_l
                    && run.bounds.top >= card_t
                    && run.bounds.right <= card_r
                    && run.bounds.bottom <= card_b;
                if strictly_inside {
                    inside_card_runs += 1;
                } else {
                    // Any non-overlay run must be clipped outside the card rect.
                    let overlaps_card = run.bounds.right > card_l
                        && run.bounds.left < card_r
                        && run.bounds.bottom > card_t
                        && run.bounds.top < card_b;
                    assert!(
                        !overlaps_card,
                        "underlying text run {:?} overlapped welcome card {:?}",
                        (
                            run.bounds.left,
                            run.bounds.top,
                            run.bounds.right,
                            run.bounds.bottom
                        ),
                        (card_l, card_t, card_r, card_b),
                    );
                }
            }
            assert!(
                inside_card_runs > 0,
                "expected overlay text labels inside card for {theme_name} {step:?}"
            );
        }
    }
}
