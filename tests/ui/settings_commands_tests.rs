//! Settings → Commands: add, edit and delete saved commands from inside the app.

use twitty::config::TwittyConfig;
use twitty::layout::Rect;
use twitty::ui::chrome::UiLanguage;
use twitty::ui::settings_commands::FormField;
use twitty::ui::settings_panel::{
    FocusZone, SettingsHit, SettingsLayout, SettingsPanelState, SettingsSection,
};
use twitty::workflows::{parse_entry, CommandMode};
use winit::keyboard::{Key, ModifiersState, NamedKey, SmolStr};

const SURFACE: (f32, f32) = (1024.0, 720.0);

fn center(rect: Rect) -> (f64, f64) {
    (
        f64::from(rect.x + rect.w / 2.0),
        f64::from(rect.y + rect.h / 2.0),
    )
}

fn commands_panel(lang: UiLanguage, cfg: &TwittyConfig) -> SettingsPanelState {
    let mut state = SettingsPanelState::new(Vec::new(), Vec::new(), None, lang);
    state.select_section(SettingsSection::Commands, cfg);
    state.focus = FocusZone::Content;
    state
}

fn press(
    state: &mut SettingsPanelState,
    cfg: &mut TwittyConfig,
    key: NamedKey,
    mods: ModifiersState,
) -> twitty::ui::settings_panel::SettingsOutcome {
    state.on_key(&Key::Named(key), None, mods, cfg, SURFACE, 1.0)
}

fn type_text(state: &mut SettingsPanelState, cfg: &mut TwittyConfig, text: &str) {
    for c in text.chars() {
        let s = c.to_string();
        if c == ' ' {
            press(state, cfg, NamedKey::Space, ModifiersState::empty());
        } else {
            state.on_key(
                &Key::Character(SmolStr::new(&s)),
                Some(&s),
                ModifiersState::empty(),
                cfg,
                SURFACE,
                1.0,
            );
        }
    }
}

fn sample_config() -> TwittyConfig {
    let mut cfg = TwittyConfig::default();
    cfg.commands = vec![
        parse_entry(&serde_json::json!({
            "name": "Run tests", "command": "cargo test", "cwd": "~/src/app",
            "description": "whole workspace"
        }))
        .unwrap(),
        parse_entry(&serde_json::json!({
            "name": "Deploy", "name_ar": "انشر", "command": "./deploy.sh", "mode": "run"
        }))
        .unwrap(),
    ];
    cfg
}

#[test]
fn commands_section_sits_between_ai_and_keys_with_bilingual_labels() {
    assert_eq!(SettingsSection::ALL.len(), 6);
    assert_eq!(SettingsSection::ALL[3], SettingsSection::Commands);
    assert_eq!(SettingsSection::Commands.label_en(), "Commands");
    assert_eq!(
        SettingsSection::Commands.label(UiLanguage::Arabic),
        "الأوامر"
    );
    // Arrow navigation in the sidebar reaches it from AI.
    let mut cfg = TwittyConfig::default();
    let mut state = SettingsPanelState::default();
    state.select_section(SettingsSection::Ai, &cfg);
    state.focus = FocusZone::Sidebar;
    press(
        &mut state,
        &mut cfg,
        NamedKey::ArrowDown,
        ModifiersState::empty(),
    );
    assert_eq!(state.section, SettingsSection::Commands);
}

#[test]
fn add_a_command_with_the_keyboard_including_arabic_name_and_run_mode() {
    let mut cfg = TwittyConfig::default();
    let mut state = commands_panel(UiLanguage::English, &cfg);
    assert!(!state.accepts_text_input());

    // Enter on the empty list opens the form.
    let res = press(
        &mut state,
        &mut cfg,
        NamedKey::Enter,
        ModifiersState::empty(),
    );
    assert!(res.redraw && !res.commands_changed);
    assert!(state.accepts_text_input());
    type_text(&mut state, &mut cfg, "Run tests");
    press(&mut state, &mut cfg, NamedKey::Tab, ModifiersState::empty());
    // IME commit path (Arabic).
    state.on_text("شغّل الاختبارات", &mut cfg);
    press(&mut state, &mut cfg, NamedKey::Tab, ModifiersState::empty());
    type_text(&mut state, &mut cfg, "cargo test --all");
    press(
        &mut state,
        &mut cfg,
        NamedKey::Backspace,
        ModifiersState::empty(),
    );
    type_text(&mut state, &mut cfg, "l");
    press(&mut state, &mut cfg, NamedKey::Tab, ModifiersState::empty());
    assert_eq!(
        state.commands_editor.form.as_ref().unwrap().field,
        FormField::Mode
    );
    press(
        &mut state,
        &mut cfg,
        NamedKey::ArrowRight,
        ModifiersState::empty(),
    );

    let res = press(
        &mut state,
        &mut cfg,
        NamedKey::Enter,
        ModifiersState::empty(),
    );
    assert!(res.commands_changed, "saving writes config.commands");
    assert!(!res.close, "Enter in the form never closes Settings");
    assert!(state.commands_editor.form.is_none());
    assert_eq!(cfg.commands.len(), 1);
    let spec = &cfg.commands[0];
    assert_eq!(spec.name, "Run tests");
    assert_eq!(spec.name_ar, "شغّل الاختبارات");
    assert_eq!(spec.command, "cargo test --all");
    assert_eq!(spec.mode, CommandMode::Run);

    // The saved command survives a save/load round trip through config.json.
    let json = serde_json::to_value(&cfg).unwrap();
    let back: TwittyConfig = serde_json::from_value(json).unwrap();
    assert_eq!(back.commands, cfg.commands);
}

#[test]
fn form_refuses_missing_fields_and_duplicate_names_with_bilingual_errors() {
    let mut cfg = sample_config();
    let mut state = commands_panel(UiLanguage::Arabic, &cfg);
    let ctrl = ModifiersState::CONTROL;
    state.on_key(
        &Key::Character(SmolStr::new("n")),
        Some("n"),
        ctrl,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert!(state.commands_editor.form.is_some(), "Ctrl+N adds");

    let res = press(
        &mut state,
        &mut cfg,
        NamedKey::Enter,
        ModifiersState::empty(),
    );
    assert!(!res.commands_changed);
    let err = state
        .commands_editor
        .form
        .as_ref()
        .unwrap()
        .error
        .clone()
        .unwrap();
    assert_eq!(err.1, "أدخل اسماً للأمر");

    type_text(&mut state, &mut cfg, "x");
    assert!(
        state.commands_editor.form.as_ref().unwrap().error.is_none(),
        "typing clears the error"
    );
    press(
        &mut state,
        &mut cfg,
        NamedKey::Enter,
        ModifiersState::empty(),
    );
    let err = state
        .commands_editor
        .form
        .as_ref()
        .unwrap()
        .error
        .clone()
        .unwrap();
    assert_eq!(err.0, "Write the command to run");

    // Duplicate name (case-insensitive).
    press(
        &mut state,
        &mut cfg,
        NamedKey::Backspace,
        ModifiersState::empty(),
    );
    type_text(&mut state, &mut cfg, "run TESTS");
    press(&mut state, &mut cfg, NamedKey::Tab, ModifiersState::empty());
    press(&mut state, &mut cfg, NamedKey::Tab, ModifiersState::empty());
    type_text(&mut state, &mut cfg, "make");
    let res = press(
        &mut state,
        &mut cfg,
        NamedKey::Enter,
        ModifiersState::empty(),
    );
    assert!(!res.commands_changed);
    let err = state
        .commands_editor
        .form
        .as_ref()
        .unwrap()
        .error
        .clone()
        .unwrap();
    assert!(err.1.contains("بالفعل"), "{err:?}");
    assert_eq!(cfg.commands.len(), 2);

    // Esc cancels the form without closing Settings; a second Esc closes Settings.
    let res = press(
        &mut state,
        &mut cfg,
        NamedKey::Escape,
        ModifiersState::empty(),
    );
    assert!(!res.close);
    assert!(state.commands_editor.form.is_none());
    let res = press(
        &mut state,
        &mut cfg,
        NamedKey::Escape,
        ModifiersState::empty(),
    );
    assert!(res.close);
}

#[test]
fn edit_keeps_hidden_fields_and_delete_removes_from_the_list() {
    let mut cfg = sample_config();
    let mut state = commands_panel(UiLanguage::English, &cfg);
    let none = ModifiersState::empty();

    // Enter edits the selected row; the form is prefilled.
    press(&mut state, &mut cfg, NamedKey::Enter, none);
    let form = state.commands_editor.form.clone().unwrap();
    assert_eq!(form.editing, Some(0));
    assert_eq!(form.name, "Run tests");
    for _ in 0.."tests".len() {
        press(&mut state, &mut cfg, NamedKey::Backspace, none);
    }
    type_text(&mut state, &mut cfg, "checks");
    let res = press(&mut state, &mut cfg, NamedKey::Enter, none);
    assert!(res.commands_changed);
    assert_eq!(cfg.commands[0].name, "Run checks");
    assert_eq!(cfg.commands[0].cwd.as_deref(), Some("~/src/app"));
    assert_eq!(
        cfg.commands[0].description.as_deref(),
        Some("whole workspace")
    );

    // Renaming to its own name is not a duplicate.
    press(&mut state, &mut cfg, NamedKey::Enter, none);
    assert!(press(&mut state, &mut cfg, NamedKey::Enter, none).commands_changed);

    // Down then Delete removes "Deploy".
    press(&mut state, &mut cfg, NamedKey::ArrowDown, none);
    assert_eq!(state.commands_editor.selected, 1);
    let res = press(&mut state, &mut cfg, NamedKey::Delete, none);
    assert!(res.commands_changed);
    assert_eq!(cfg.commands.len(), 1);
    assert_eq!(state.commands_editor.selected, 0);
}

#[test]
fn mouse_add_edit_delete_and_form_buttons_mirror_in_arabic() {
    for lang in [UiLanguage::English, UiLanguage::Arabic] {
        let mut cfg = sample_config();
        let mut state = commands_panel(lang, &cfg);
        let layout = SettingsLayout::build(SURFACE, 1.0, &state, &cfg);
        let list = layout.commands.clone().unwrap();
        let add = list.add_button.unwrap();
        let row = list.rows[1];
        let vp = layout.viewport;
        if lang == UiLanguage::Arabic {
            assert!(
                add.x < vp.x + vp.w / 2.0,
                "Add sits at the reading end (left)"
            );
            assert!(row.delete.x < row.edit.x);
        } else {
            assert!(add.x > vp.x + vp.w / 2.0);
            assert!(row.delete.x > row.edit.x);
        }
        let (x, y) = center(add);
        assert_eq!(layout.hit(x, y), SettingsHit::CommandsAdd);
        let (x, y) = center(row.edit);
        assert_eq!(layout.hit(x, y), SettingsHit::CommandEdit(1));

        // Click Delete on row 1.
        let (x, y) = center(row.delete);
        let res = state.on_mouse_down(x, y, &mut cfg, SURFACE, 1.0);
        assert!(res.commands_changed);
        assert_eq!(cfg.commands.len(), 1);

        // Click Add, fill via clicks on fields and the Run button, then Save.
        let (x, y) = center(add);
        state.on_mouse_down(x, y, &mut cfg, SURFACE, 1.0);
        let layout = SettingsLayout::build(SURFACE, 1.0, &state, &cfg);
        let form = layout.commands.clone().unwrap();
        assert!(form.add_button.is_none() && form.rows.is_empty());
        let command_input = form
            .fields
            .iter()
            .find(|(f, _, _)| *f == FormField::Command)
            .unwrap()
            .2;
        let (x, y) = center(command_input);
        assert_eq!(
            layout.hit(x, y),
            SettingsHit::CommandField(FormField::Command)
        );
        state.on_mouse_down(x, y, &mut cfg, SURFACE, 1.0);
        state.on_text("htop", &mut cfg);
        let name_input = form.fields[0].2;
        let (x, y) = center(name_input);
        state.on_mouse_down(x, y, &mut cfg, SURFACE, 1.0);
        state.on_text("Monitor", &mut cfg);
        let run = form
            .mode_buttons
            .iter()
            .find(|(m, _)| *m == CommandMode::Run)
            .unwrap()
            .1;
        let (x, y) = center(run);
        state.on_mouse_down(x, y, &mut cfg, SURFACE, 1.0);
        let save = form.save.unwrap();
        let cancel = form.cancel.unwrap();
        if lang == UiLanguage::Arabic {
            assert!(save.x < cancel.x);
        } else {
            assert!(save.x > cancel.x);
        }
        let (x, y) = center(save);
        let res = state.on_mouse_down(x, y, &mut cfg, SURFACE, 1.0);
        assert!(res.commands_changed, "{lang:?}");
        let last = cfg.commands.last().unwrap();
        assert_eq!(
            (last.name.as_str(), last.command.as_str(), last.mode),
            ("Monitor", "htop", CommandMode::Run)
        );
    }
}

#[test]
fn switching_sections_drops_an_unsaved_form_and_wheel_scrolls_a_long_list() {
    let mut cfg = TwittyConfig::default();
    cfg.commands = (0..30)
        .map(|i| {
            parse_entry(&serde_json::json!({"name": format!("cmd {i}"), "command": "true"}))
                .unwrap()
        })
        .collect();
    let mut state = commands_panel(UiLanguage::English, &cfg);
    let layout = SettingsLayout::build(SURFACE, 1.0, &state, &cfg);
    let list = layout.commands.clone().unwrap();
    assert!(list.visible_rows > 0 && list.visible_rows < 30);
    assert_eq!(list.rows.len(), list.visible_rows);
    let (x, y) = center(list.rows[0].rect);
    state.on_wheel(x, y, -1.0, &cfg, SURFACE, 1.0);
    assert_eq!(state.commands_editor.scroll, 1);
    let layout = SettingsLayout::build(SURFACE, 1.0, &state, &cfg);
    assert_eq!(layout.commands.unwrap().rows[0].index, 1);

    // Arrow past the bottom keeps the selection visible.
    for _ in 0..29 {
        press(
            &mut state,
            &mut cfg,
            NamedKey::ArrowDown,
            ModifiersState::empty(),
        );
    }
    let layout = SettingsLayout::build(SURFACE, 1.0, &state, &cfg);
    let rows = layout.commands.unwrap().rows;
    assert_eq!(rows.last().unwrap().index, 29);

    state.commands_editor.start_add();
    state.select_section(SettingsSection::Keys, &cfg);
    assert!(state.commands_editor.form.is_none());
}
