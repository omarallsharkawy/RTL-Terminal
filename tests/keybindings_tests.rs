//! Window-free tests for customizable keyboard shortcuts:
//! chord parsing (valid, invalid, aliases, Arabic/AZERTY/AltGr), default table parity,
//! overrides, multiple chords, unbinding, conflict resolution (including JSON map order),
//! plain-typing guard, Settings -> Keys interactive capture/reset/conflict UI, atomic config
//! round-trip, and effective chords in the Command Palette / control API / markdown table.

use alacritty_terminal::term::TermMode;
use cosmic_text::FontSystem;
use twitty::color::Palette;
use twitty::commands;
use twitty::config::TwittyConfig;
use twitty::input::keybindings::{
    Chord, ChordParseError, KeyBindings, KeybindingValue, KeybindingsConfig,
};
use twitty::input::{handle_key_mode_raw_with, handle_key_raw_with, InputAction};
use twitty::layout::{FocusDir, PaneId, Rect};
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::frame::{build_frame, FrameOptions};
use twitty::render::scene::preedit::PreeditState;
use twitty::render::scene::settings::SettingsView;
use twitty::render::text::ui::UiFont;
use twitty::render::ui_style::UiStyle;
use twitty::render::{CellMetrics, PaneFrame, RowCache};
use twitty::terminal::Terminal;
use twitty::ui::chrome::UiLanguage;
use twitty::ui::palette::{PaletteEntry, PaletteId};
use twitty::ui::settings_panel::{
    FocusZone, KeyCaptureNotice, SettingsHit, SettingsLayout, SettingsPanelState, SettingsSection,
};
use twitty::workflows::WorkflowSet;
use winit::event::ElementState::{Pressed, Released};
use winit::keyboard::{Key, KeyCode, ModifiersState as M, NamedKey, SmolStr};

const SURFACE: (f32, f32) = (1024.0, 720.0);

fn ch(s: &str) -> Key {
    Key::Character(SmolStr::new(s))
}

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
        "twitty-keybindings-test-{tag}-{}-{nanos}.json",
        std::process::id()
    ))
}

fn sample_keys_panel(language: UiLanguage, cfg: &TwittyConfig) -> SettingsPanelState {
    let mut state = SettingsPanelState::new(
        Vec::new(),
        Vec::new(),
        Some("/home/user/.config/twitty/config.json".into()),
        language,
    );
    state.select_section(SettingsSection::Keys, cfg);
    state.focus = FocusZone::Content;
    state
}

#[test]
fn chord_parsing_good_aliases_and_round_trip() {
    let cases = [
        (
            "Ctrl+Shift+T",
            Chord::new(true, true, false, false, KeyCode::KeyT),
            "Ctrl+Shift+T",
        ),
        (
            "shift+control+t",
            Chord::new(true, true, false, false, KeyCode::KeyT),
            "Ctrl+Shift+T",
        ),
        (
            "Alt+Enter",
            Chord::new(false, false, true, false, KeyCode::Enter),
            "Alt+Enter",
        ),
        (
            "opt+return",
            Chord::new(false, false, true, false, KeyCode::Enter),
            "Alt+Enter",
        ),
        (
            "Super+Space",
            Chord::new(false, false, false, true, KeyCode::Space),
            "Super+Space",
        ),
        (
            "Meta+Tab",
            Chord::new(false, false, false, true, KeyCode::Tab),
            "Super+Tab",
        ),
        (
            "Cmd+Escape",
            Chord::new(false, false, false, true, KeyCode::Escape),
            "Super+Escape",
        ),
        (
            "Ctrl+,",
            Chord::new(true, false, false, false, KeyCode::Comma),
            "Ctrl+,",
        ),
        (
            "Ctrl+comma",
            Chord::new(true, false, false, false, KeyCode::Comma),
            "Ctrl+,",
        ),
        (
            "Ctrl+=",
            Chord::new(true, false, false, false, KeyCode::Equal),
            "Ctrl+=",
        ),
        (
            "Ctrl++",
            Chord::new(true, false, false, false, KeyCode::Equal),
            "Ctrl+=",
        ),
        (
            "Ctrl+Plus",
            Chord::new(true, false, false, false, KeyCode::Equal),
            "Ctrl+=",
        ),
        (
            "Ctrl+-",
            Chord::new(true, false, false, false, KeyCode::Minus),
            "Ctrl+-",
        ),
        (
            "Ctrl+Shift+Alt+T",
            Chord::new(true, true, true, false, KeyCode::KeyT),
            "Ctrl+Shift+Alt+T",
        ),
        (
            "Alt+Up",
            Chord::new(false, false, true, false, KeyCode::ArrowUp),
            "Alt+Up",
        ),
        (
            "Ctrl+PageUp",
            Chord::new(true, false, false, false, KeyCode::PageUp),
            "Ctrl+PageUp",
        ),
        (
            "Ctrl+PgDn",
            Chord::new(true, false, false, false, KeyCode::PageDown),
            "Ctrl+PageDown",
        ),
        (
            "F1",
            Chord::new(false, false, false, false, KeyCode::F1),
            "F1",
        ),
        (
            "Shift+F12",
            Chord::new(false, true, false, false, KeyCode::F12),
            "Shift+F12",
        ),
        (
            "Ctrl+NumpadAdd",
            Chord::new(true, false, false, false, KeyCode::NumpadAdd),
            "Ctrl+NumpadAdd",
        ),
        (
            "Alt+;",
            Chord::new(false, false, true, false, KeyCode::Semicolon),
            "Alt+;",
        ),
        (
            "Ctrl+Shift+0",
            Chord::new(true, true, false, false, KeyCode::Digit0),
            "Ctrl+Shift+0",
        ),
    ];
    for (input, expected, canonical) in cases {
        let parsed =
            Chord::parse(input).unwrap_or_else(|e| panic!("failed to parse {input:?}: {e}"));
        assert_eq!(parsed, expected, "input {input:?}");
        assert_eq!(
            parsed.canonical_string(),
            canonical,
            "canonical for {input:?}"
        );
        assert_eq!(
            Chord::parse(&parsed.canonical_string()),
            Ok(parsed),
            "round-trip for {input:?}"
        );
    }
}

#[test]
fn chord_parsing_rejects_bad_and_multi_chords() {
    assert_eq!(Chord::parse(""), Err(ChordParseError::Empty));
    assert_eq!(Chord::parse("   "), Err(ChordParseError::Empty));
    assert_eq!(Chord::parse("Ctrl"), Err(ChordParseError::MissingKey));
    assert_eq!(Chord::parse("Ctrl+Shift"), Err(ChordParseError::MissingKey));
    assert_eq!(Chord::parse("Ctrl+"), Err(ChordParseError::MissingKey));
    assert_eq!(
        Chord::parse("Ctrl+Ctrl+T"),
        Err(ChordParseError::DuplicateModifier("Ctrl"))
    );
    assert_eq!(
        Chord::parse("Super+Meta+T"),
        Err(ChordParseError::DuplicateModifier("Super"))
    );
    assert_eq!(Chord::parse("Ctrl+A+B"), Err(ChordParseError::MultipleKeys));
    assert!(matches!(
        Chord::parse("Ctrl+K Ctrl+S"),
        Err(ChordParseError::UnknownToken(_))
    ));
    assert!(matches!(
        Chord::parse("Ctrl+UnknownKey"),
        Err(ChordParseError::UnknownToken(_))
    ));
    assert!(matches!(
        Chord::parse("+T"),
        Err(ChordParseError::UnknownToken(_))
    ));
}

#[test]
fn default_table_matches_every_builtin_command_and_legacy_binding() {
    let resolved = KeyBindings::resolve(&KeybindingsConfig::default());
    assert!(resolved.warnings.is_empty(), "{:?}", resolved.warnings);
    assert!(resolved.bindings.conflicts().is_empty());
    let defaults = &resolved.bindings;

    // Every command in commands::all() keeps its exact default display shortcut.
    for cmd in commands::all() {
        assert_eq!(
            defaults.display_shortcut(cmd.name).as_deref(),
            cmd.shortcut,
            "default shortcut mismatch for {}",
            cmd.name
        );
    }

    // Verify multi-chord defaults & SelectTab(0..8) are all intact.
    let cs = M::CONTROL | M::SHIFT;
    let csa = cs | M::ALT;
    let check = |code: KeyCode, mods: M, expected: InputAction| {
        assert_eq!(
            defaults.lookup(code, mods),
            Some(expected.clone()),
            "lookup({code:?}, {mods:?}) != {expected:?}"
        );
    };
    check(KeyCode::KeyT, cs, InputAction::NewTab);
    check(KeyCode::KeyT, csa, InputAction::ReopenClosedTab);
    check(KeyCode::Tab, M::CONTROL, InputAction::NextTab);
    check(KeyCode::PageDown, M::CONTROL, InputAction::NextTab);
    check(KeyCode::Tab, cs, InputAction::PrevTab);
    check(KeyCode::PageUp, M::CONTROL, InputAction::PrevTab);
    check(KeyCode::Equal, M::CONTROL, InputAction::ZoomIn);
    check(KeyCode::NumpadAdd, M::CONTROL, InputAction::ZoomIn);
    check(KeyCode::Minus, M::CONTROL, InputAction::ZoomOut);
    check(KeyCode::NumpadSubtract, M::CONTROL, InputAction::ZoomOut);
    check(KeyCode::Digit0, M::CONTROL, InputAction::ZoomReset);
    check(KeyCode::Numpad0, M::CONTROL, InputAction::ZoomReset);
    check(KeyCode::Comma, M::CONTROL, InputAction::Settings);
    check(KeyCode::Space, cs, InputAction::ToggleComposer);
    check(
        KeyCode::ArrowLeft,
        M::ALT,
        InputAction::Focus(FocusDir::Left),
    );
    check(
        KeyCode::ArrowRight,
        M::ALT,
        InputAction::Focus(FocusDir::Right),
    );
    check(KeyCode::ArrowUp, M::ALT, InputAction::Focus(FocusDir::Up));
    check(
        KeyCode::ArrowDown,
        M::ALT,
        InputAction::Focus(FocusDir::Down),
    );
    for (i, digit) in [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ]
    .into_iter()
    .enumerate()
    {
        check(digit, cs, InputAction::SelectTab(i));
    }
}

#[test]
fn arabic_layout_azerty_and_altgr_work_with_remapped_bindings() {
    let mut cfg = KeybindingsConfig::default();
    cfg.set("tab.new", KeybindingValue::Single("Alt+;".into()));
    cfg.set("split.right", KeybindingValue::Single("Ctrl+Alt+Q".into()));
    cfg.set("composer.open", KeybindingValue::Single("Alt+A".into()));
    let resolved = KeyBindings::resolve(&cfg);
    assert!(resolved.warnings.is_empty(), "{:?}", resolved.warnings);
    let bindings = &resolved.bindings;

    // 1. Arabic layout: physical Semicolon types "ك", Alt+ك triggers remapped Alt+; -> NewTab.
    assert_eq!(
        handle_key_raw_with(
            Some(KeyCode::Semicolon),
            &ch("ك"),
            Some("ك"),
            Pressed,
            M::ALT,
            false,
            bindings,
        ),
        Some(InputAction::NewTab)
    );

    // 2. AZERTY layout: physical KeyQ types "a", Alt+A follows logical Latin letter -> ToggleComposer.
    assert_eq!(
        handle_key_raw_with(
            Some(KeyCode::KeyQ),
            &ch("a"),
            Some("a"),
            Pressed,
            M::ALT,
            false,
            bindings,
        ),
        Some(InputAction::ToggleComposer)
    );

    // 3. AltGr (Ctrl+Alt on Windows) typing "@" on physical KeyQ MUST pass through as "@" even
    // though Ctrl+Alt+Q is bound to split.right! Both legacy and kitty modes preserve AltGr.
    let ca = M::CONTROL | M::ALT;
    assert_eq!(
        handle_key_raw_with(
            Some(KeyCode::KeyQ),
            &ch("@"),
            Some("@"),
            Pressed,
            ca,
            false,
            bindings,
        ),
        Some(InputAction::Bytes(b"@".to_vec()))
    );
    assert_eq!(
        handle_key_mode_raw_with(
            Some(KeyCode::KeyQ),
            &ch("@"),
            Some(&ch("@")),
            Some("@"),
            Pressed,
            false,
            ca,
            TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES,
            bindings,
        ),
        Some(InputAction::Bytes(b"@".to_vec()))
    );

    // 4. Actual Ctrl+Alt+Q (no AltGr symbol text, e.g. Arabic logical "ض" with text None)
    // still triggers split.right!
    assert_eq!(
        handle_key_raw_with(
            Some(KeyCode::KeyQ),
            &ch("ض"),
            None,
            Pressed,
            ca,
            false,
            bindings,
        ),
        Some(InputAction::SplitRight)
    );
}

#[test]
fn override_multiple_chords_and_unbind_in_legacy_and_kitty_modes() {
    let mut cfg = KeybindingsConfig::default();
    cfg.set(
        "tab.new",
        KeybindingValue::Multiple(vec!["Alt+Enter".into(), "F6".into()]),
    );
    cfg.set("pane.close", KeybindingValue::Unbind);
    let resolved = KeyBindings::resolve(&cfg);
    assert!(resolved.warnings.is_empty(), "{:?}", resolved.warnings);
    let bindings = &resolved.bindings;

    // Both remapped chords work for tab.new.
    assert_eq!(
        handle_key_raw_with(
            Some(KeyCode::Enter),
            &Key::Named(NamedKey::Enter),
            None,
            Pressed,
            M::ALT,
            false,
            bindings,
        ),
        Some(InputAction::NewTab)
    );
    assert_eq!(
        handle_key_raw_with(
            Some(KeyCode::F6),
            &Key::Named(NamedKey::F6),
            None,
            Pressed,
            M::empty(),
            false,
            bindings,
        ),
        Some(InputAction::NewTab)
    );

    // Old default Ctrl+Shift+T is no longer NewTab (passes through to terminal).
    let cs = M::CONTROL | M::SHIFT;
    assert_ne!(
        handle_key_raw_with(
            Some(KeyCode::KeyT),
            &ch("T"),
            None,
            Pressed,
            cs,
            false,
            bindings,
        ),
        Some(InputAction::NewTab)
    );

    // Unbound pane.close (Ctrl+Shift+W) reaches the terminal as bytes in both legacy and kitty.
    assert_eq!(
        handle_key_raw_with(
            Some(KeyCode::KeyW),
            &ch("W"),
            None,
            Pressed,
            cs,
            false,
            bindings,
        ),
        Some(InputAction::Bytes(vec![0x17]))
    );
    let kitty_mode = TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES;
    // Remapped F6 suppresses its release event in kitty mode; unbound Ctrl+Shift+W reports release.
    assert_eq!(
        handle_key_mode_raw_with(
            Some(KeyCode::F6),
            &Key::Named(NamedKey::F6),
            None,
            None,
            Released,
            false,
            M::empty(),
            kitty_mode,
            bindings,
        ),
        None
    );
    assert!(matches!(
        handle_key_mode_raw_with(
            Some(KeyCode::KeyW),
            &ch("W"),
            None,
            None,
            Released,
            false,
            cs,
            kitty_mode,
            bindings,
        ),
        Some(InputAction::Bytes(_))
    ));
}

#[test]
fn plain_typing_guard_blocks_printable_and_allows_function_and_nav_keys() {
    for forbidden in [
        "a",
        "Shift+A",
        "1",
        ",",
        "=",
        "Space",
        "Shift+Space",
        "Enter",
        "Tab",
        "Backspace",
        "Escape",
    ] {
        let chord = Chord::parse(forbidden).unwrap_or_else(|e| panic!("{forbidden}: {e}"));
        assert!(
            chord.swallows_plain_typing(),
            "{forbidden} must be rejected by plain-typing guard"
        );
    }

    for allowed in [
        "F1",
        "F12",
        "Shift+F5",
        "PageUp",
        "Shift+PageDown",
        "Home",
        "End",
        "Insert",
        "Delete",
        "Up",
        "Shift+Left",
        "Alt+Enter",
        "Ctrl+Space",
        "Super+A",
    ] {
        let chord = Chord::parse(allowed).unwrap_or_else(|e| panic!("{allowed}: {e}"));
        assert!(
            !chord.swallows_plain_typing(),
            "{allowed} should be allowed"
        );
    }

    // In config resolution, a plain-typing override emits a bilingual warning and keeps the default.
    let mut cfg = KeybindingsConfig::default();
    cfg.set("tab.new", KeybindingValue::Single("Shift+T".into()));
    let resolved = KeyBindings::resolve(&cfg);
    assert_eq!(resolved.warnings.len(), 1);
    assert!(resolved.warnings[0].contains("plain typing"));
    assert!(resolved.warnings[0].contains("الكتابة العادية"));
    assert_eq!(
        resolved.bindings.display_shortcut("tab.new").as_deref(),
        Some("Ctrl+Shift+T")
    );
}

#[test]
fn conflict_resolution_last_user_entry_wins_over_defaults_and_earlier_entries() {
    // 1. User binding overrides a default binding and emits a bilingual conflict warning.
    let mut cfg = KeybindingsConfig::default();
    cfg.set("tab.new", KeybindingValue::Single("Ctrl+Shift+W".into()));
    let resolved = KeyBindings::resolve(&cfg);
    assert_eq!(resolved.bindings.conflicts().len(), 1);
    let c = &resolved.bindings.conflicts()[0];
    assert_eq!(c.winner, "tab.new");
    assert_eq!(c.shadowed, vec!["pane.close"]);
    assert_eq!(
        resolved
            .bindings
            .lookup(KeyCode::KeyW, M::CONTROL | M::SHIFT),
        Some(InputAction::NewTab)
    );
    assert_eq!(
        resolved.bindings.display_shortcut("tab.new").as_deref(),
        Some("Ctrl+Shift+W")
    );
    assert_eq!(resolved.bindings.display_shortcut("pane.close"), None);
    assert!(resolved
        .warnings
        .iter()
        .any(|w| w.contains("conflict") && w.contains("تعارض")));

    // 2. Two user bindings on one chord: LAST one in the JSON map wins (even when alphabetically earlier!).
    let json = r#"{
        "keybindings": {
            "tab.new": "Alt+N",
            "split.right": "Alt+N"
        }
    }"#;
    let parsed: TwittyConfig = serde_json::from_str(json).expect("valid config json");
    let resolved = KeyBindings::resolve(&parsed.keybindings);
    assert_eq!(
        resolved.bindings.lookup(KeyCode::KeyN, M::ALT),
        Some(InputAction::SplitRight),
        "split.right appeared AFTER tab.new in JSON so split.right must win"
    );
    assert_eq!(resolved.bindings.conflicts().len(), 1);
    assert_eq!(resolved.bindings.conflicts()[0].winner, "split.right");
    assert_eq!(resolved.bindings.conflicts()[0].shadowed, vec!["tab.new"]);

    let info_winner = resolved.bindings.conflict_for("split.right").unwrap();
    assert!(info_winner.is_winner);
    assert_eq!(info_winner.other, "tab.new");
    let info_shadowed = resolved.bindings.conflict_for("tab.new").unwrap();
    assert!(!info_shadowed.is_winner);
    assert_eq!(info_shadowed.other, "split.right");

    // 3. Swapping two defaults produces zero conflicts!
    let mut swap = KeybindingsConfig::default();
    swap.set("tab.new", KeybindingValue::Single("Ctrl+Shift+W".into()));
    swap.set("pane.close", KeybindingValue::Single("Ctrl+Shift+T".into()));
    let resolved_swap = KeyBindings::resolve(&swap);
    assert!(
        resolved_swap.warnings.is_empty(),
        "{:?}",
        resolved_swap.warnings
    );
    assert!(resolved_swap.bindings.conflicts().is_empty());
    assert_eq!(
        resolved_swap
            .bindings
            .lookup(KeyCode::KeyW, M::CONTROL | M::SHIFT),
        Some(InputAction::NewTab)
    );
    assert_eq!(
        resolved_swap
            .bindings
            .lookup(KeyCode::KeyT, M::CONTROL | M::SHIFT),
        Some(InputAction::ClosePane)
    );
}

#[test]
fn config_loading_skips_unknown_ids_and_bad_chords_with_bilingual_warnings() {
    let path = temp_config_path("warnings");
    std::fs::write(
        &path,
        r#"{
            "keybindings": {
                "no.such.command": "Alt+X",
                "tab.new": "Ctrl+K Ctrl+S",
                "composer.open": "Alt+Enter",
                "pane.flash": "",
                "pane.zoom": null
            }
        }"#,
    )
    .unwrap();
    let _ = twitty::config::take_warnings();
    let cfg = TwittyConfig::load_from(&path);
    let _ = std::fs::remove_file(&path);
    let warnings = twitty::config::take_warnings();

    assert!(
        warnings
            .iter()
            .any(|w| w.contains("no.such.command") && w.contains("غير معروف")),
        "expected bilingual warning for unknown command id: {warnings:?}"
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("Ctrl+K Ctrl+S") && w.contains("اختصار غير صالح")),
        "expected bilingual warning for unparsable chord: {warnings:?}"
    );

    let bindings = KeyBindings::from_config(&cfg.keybindings);
    // Valid entries still applied!
    assert_eq!(
        bindings.display_shortcut("composer.open").as_deref(),
        Some("Alt+Enter")
    );
    assert_eq!(bindings.display_shortcut("pane.flash"), None);
    assert_eq!(bindings.display_shortcut("pane.zoom"), None);
    // Invalid entry kept its default!
    assert_eq!(
        bindings.display_shortcut("tab.new").as_deref(),
        Some("Ctrl+Shift+T")
    );
}

#[test]
fn settings_capture_flow_escape_backspace_reset_reset_all_and_atomic_save() {
    let mut cfg = TwittyConfig::default();
    let mut state = sample_keys_panel(UiLanguage::English, &cfg);

    // Filter down to "tab.new" so index 0 is unambiguously "tab.new".
    state.on_text("tab.new", &mut cfg);
    assert_eq!(state.filtered_commands().len(), 1);
    assert_eq!(state.filtered_commands()[0].name, "tab.new");

    // Click "Change" button on row 0 -> enters capture mode.
    let layout = SettingsLayout::build(SURFACE, 1.0, &state, &cfg);
    let row0 = layout.keys_row_layouts[0];
    let (cx, cy) = center(row0.change_button);
    assert_eq!(layout.hit(cx, cy), SettingsHit::KeyChange(0));
    let start = state.on_mouse_down(cx, cy, &mut cfg, SURFACE, 1.0);
    assert!(start.redraw);
    assert_eq!(state.capturing_key, Some("tab.new"));
    assert!(!state.accepts_text_input());

    // Lone modifier is ignored while capturing.
    let mod_only = state.on_key_with_physical(
        Some(KeyCode::ControlLeft),
        &Key::Named(NamedKey::Control),
        None,
        M::CONTROL,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert!(!mod_only.keybindings_changed);
    assert_eq!(state.capturing_key, Some("tab.new"));

    // Plain printable key is rejected with PlainTyping notice and keeps capture open.
    let plain = state.on_key_with_physical(
        Some(KeyCode::KeyA),
        &ch("a"),
        Some("a"),
        M::empty(),
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert!(!plain.keybindings_changed);
    assert_eq!(state.capturing_key, Some("tab.new"));
    assert_eq!(
        state.key_capture_notice,
        Some(KeyCaptureNotice::PlainTyping)
    );

    // Esc cancels capture WITHOUT closing settings or mutating config.
    let esc = state.on_key_with_physical(
        Some(KeyCode::Escape),
        &Key::Named(NamedKey::Escape),
        None,
        M::empty(),
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert!(!esc.close);
    assert!(!esc.keybindings_changed);
    assert_eq!(state.capturing_key, None);
    assert_eq!(state.key_capture_notice, None);

    // Enter starts capture on the selected row; Arabic layout Alt+ك (physical Semicolon) captures "Alt+;".
    state.on_key(
        &Key::Named(NamedKey::Enter),
        None,
        M::empty(),
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert_eq!(state.capturing_key, Some("tab.new"));
    let bound = state.on_key_with_physical(
        Some(KeyCode::Semicolon),
        &ch("ك"),
        Some("ك"),
        M::ALT,
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert!(bound.keybindings_changed);
    assert_eq!(state.capturing_key, None);
    assert_eq!(
        cfg.keybindings.get("tab.new"),
        Some(&KeybindingValue::Single("Alt+;".into()))
    );

    // Per-row Reset button now exists for the customized row; clicking it restores default!
    let layout_after = SettingsLayout::build(SURFACE, 1.0, &state, &cfg);
    let reset_rect = layout_after.keys_row_layouts[0]
        .reset_button
        .expect("customized row must show Reset button");
    let (rx, ry) = center(reset_rect);
    assert_eq!(layout_after.hit(rx, ry), SettingsHit::KeyReset(0));
    let reset = state.on_mouse_down(rx, ry, &mut cfg, SURFACE, 1.0);
    assert!(reset.keybindings_changed);
    assert!(cfg.keybindings.is_empty());

    // Capture again -> Backspace clears/unbinds the command!
    state.on_key(
        &Key::Named(NamedKey::Enter),
        None,
        M::empty(),
        &mut cfg,
        SURFACE,
        1.0,
    );
    let cleared = state.on_key_with_physical(
        Some(KeyCode::Backspace),
        &Key::Named(NamedKey::Backspace),
        None,
        M::empty(),
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert!(cleared.keybindings_changed);
    assert_eq!(
        cfg.keybindings.get("tab.new"),
        Some(&KeybindingValue::Unbind)
    );

    // Round-trip through atomic save_to and load_from!
    cfg.keybindings
        .set("split.right", KeybindingValue::Single("Alt+Enter".into()));
    let path = temp_config_path("roundtrip");
    cfg.save_to(&path).expect("atomic save succeeds");
    let loaded = TwittyConfig::load_from(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(loaded.keybindings, cfg.keybindings);

    // Click "Reset all" -> clears every override.
    let layout_reset_all = SettingsLayout::build(SURFACE, 1.0, &state, &cfg);
    let (ax, ay) = center(layout_reset_all.keys_reset_all.unwrap());
    assert_eq!(layout_reset_all.hit(ax, ay), SettingsHit::KeysResetAll);
    let reset_all = state.on_mouse_down(ax, ay, &mut cfg, SURFACE, 1.0);
    assert!(reset_all.keybindings_changed);
    assert!(cfg.keybindings.is_empty());
}

#[test]
fn palette_control_list_and_markdown_table_show_effective_remapped_chords() {
    let mut cfg = KeybindingsConfig::default();
    cfg.set("tab.new", KeybindingValue::Single("Alt+Enter".into()));
    cfg.set("pane.flash", KeybindingValue::Unbind);
    let bindings = KeyBindings::from_config(&cfg);

    // Command palette entries show remapped chord and None for unbound.
    let entries = PaletteEntry::builtins_with(&bindings);
    let tab_new = entries
        .iter()
        .find(|e| e.id == PaletteId::Builtin("tab.new"))
        .unwrap();
    assert_eq!(tab_new.shortcut.as_deref(), Some("Alt+Enter"));
    let flash = entries
        .iter()
        .find(|e| e.id == PaletteId::Builtin("pane.flash"))
        .unwrap();
    assert_eq!(flash.shortcut, None);

    // WorkflowSet palette_entries_with and control_list_json_with both reflect remapped chords.
    let wf = WorkflowSet::default();
    let wf_entries = wf.palette_entries_with(&bindings);
    assert_eq!(
        wf_entries
            .iter()
            .find(|e| e.id == PaletteId::Builtin("tab.new"))
            .unwrap()
            .shortcut
            .as_deref(),
        Some("Alt+Enter")
    );
    let ctl = wf.control_list_json_with(&bindings);
    let ctl_tab_new = ctl
        .iter()
        .find(|v| v.get("name").and_then(|n| n.as_str()) == Some("tab.new"))
        .unwrap();
    assert_eq!(
        ctl_tab_new.get("shortcut").and_then(|s| s.as_str()),
        Some("Alt+Enter")
    );

    // Markdown shortcut table shows remapped chord and em-dash for unbound.
    let table = bindings.markdown_shortcut_table();
    assert!(table.contains("| `tab.new` | New tab | Alt+Enter |"));
    assert!(table.contains("| `pane.flash` | Flash focused pane | — |"));
}

#[test]
fn settings_keys_renders_conflicts_capture_and_arabic_rtl_in_dark_and_light() {
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

    let mut cfg = TwittyConfig::default();
    // Create an explicit conflict (tab.new overrides pane.close's Ctrl+Shift+W) and an unbind.
    cfg.keybindings
        .set("tab.new", KeybindingValue::Single("Ctrl+Shift+W".into()));
    cfg.keybindings.set("pane.flash", KeybindingValue::Unbind);

    for (theme_name, language, scale) in [
        ("twitty-dark", UiLanguage::English, 1.0),
        ("twitty-light", UiLanguage::Arabic, 1.5),
    ] {
        let surface = (1024.0 * scale, 720.0 * scale);
        let palette = Palette::named(theme_name).unwrap();
        let mut ctx = DrawContext {
            font_system: &mut font_system,
            family: None,
            palette: &palette,
            metrics: CellMetrics { scale, ..metrics },
            is_srgb: false,
        };
        let opts = FrameOptions {
            window_focused: true,
            cursor_style: "block",
            cursor_blink_on: true,
            surface,
        };
        let mut term = Terminal::new(80, 24, |_| {});
        term.process_bytes("مرحبا بالعالم terminal text\r\n".as_bytes());
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
        let style = UiStyle::new(&palette, None, scale);

        let mut state = sample_keys_panel(language, &cfg);
        state.capturing_key = Some("composer.open");
        state.key_capture_notice = Some(KeyCaptureNotice::PlainTyping);
        let layout = SettingsLayout::build(surface, scale, &state, &cfg);
        assert!(layout.keys_banner.is_some());
        assert!(layout.keys_reset_all.is_some());
        if language == UiLanguage::Arabic {
            assert!(layout.keys_reset_all.unwrap().right() <= layout.keys_search.unwrap().x);
        } else {
            assert!(layout.keys_search.unwrap().right() <= layout.keys_reset_all.unwrap().x);
        }

        frame.add_settings(
            &mut ctx,
            style,
            &font,
            &SettingsView {
                state,
                config: cfg.clone(),
            },
        );
        let runs = frame.text_runs(&panes, &cache, &ime);
        assert!(!runs.is_empty());
    }
}
