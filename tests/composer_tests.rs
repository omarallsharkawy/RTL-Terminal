//! Window-free tests for the command composer (lane N11):
//! editing on mixed Arabic/English text, visual caret mapping for RTL runs, shell history
//! parsing and dedup, completion ranking and Arabic-aware matching, bracketed send bytes,
//! prompt-state show/hide rules, config/shortcuts, and CPU frame clipping.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use cosmic_text::{fontdb, FontSystem};
use twitty::color::Palette;
use twitty::commands;
use twitty::composer::complete::{
    complete, extract_path_query, folded_prefix_suffix, ghost_suggestion, read_dir_entries,
    CompletionKind, DirEntryInfo,
};
use twitty::composer::editor::{caret_is_rtl, TextEditor};
use twitty::composer::history::{
    dedup_newest_first, load_shell_history_from, parse_auto_history, parse_bash_history,
    parse_fish_history, parse_zsh_history, unmetafy_zsh, HistoryStore, MAX_HISTORY,
};
use twitty::composer::{
    encode_submit, should_show_composer, ComposerAction, ComposerMode, ComposerState, PromptState,
};
use twitty::config::TwittyConfig;
use twitty::input::{handle_key_raw, InputAction};
use twitty::layout::{PaneId, Rect};
use twitty::render::scene::composer::{
    build as build_composer_scene, shape_composer_line, ComposerHit, ComposerLayout,
    ComposerPopupItem, ComposerPopupView, ComposerView,
};
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::frame::{build_frame, FrameOptions};
use twitty::render::scene::preedit::PreeditState;
use twitty::render::text::ui::UiFont;
use twitty::render::ui_style::UiStyle;
use twitty::render::{CellMetrics, PaneFrame, RowCache};
use twitty::shell_integration::MarkKind;
use twitty::terminal::Terminal;
use twitty::ui::chrome::UiLanguage;
use twitty::workflows::{CommandMode, Workflow, WorkflowSource, WorkflowSpec};
use winit::event::ElementState;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey};

const NASKH: &[u8] = include_bytes!("../assets/fonts/NotoNaskhArabic-Regular.ttf");
const MONO: &[u8] = include_bytes!("golden/fonts/NotoSansMono-Subset.ttf");

fn test_font_system() -> FontSystem {
    let mut db = fontdb::Database::new();
    db.load_font_data(MONO.to_vec());
    db.load_font_data(NASKH.to_vec());
    db.set_monospace_family("Noto Sans Mono");
    db.set_sans_serif_family("Noto Sans Mono");
    FontSystem::new_with_locale_and_db("en-US".into(), db)
}

fn user_workflow(name: &str, name_ar: &str, command: &str) -> Workflow {
    Workflow {
        spec: WorkflowSpec {
            name: name.into(),
            name_ar: name_ar.into(),
            command: command.into(),
            mode: CommandMode::Insert,
            cwd: None,
            description: None,
        },
        source: WorkflowSource::User,
        trusted: true,
    }
}

fn untrusted_workflow(name: &str, command: &str) -> Workflow {
    Workflow {
        spec: WorkflowSpec {
            name: name.into(),
            name_ar: String::new(),
            command: command.into(),
            mode: CommandMode::Insert,
            cwd: None,
            description: None,
        },
        source: WorkflowSource::Project {
            root: PathBuf::from("/tmp/untrusted"),
            tag: "untrusted".into(),
            sha256: "0".repeat(64),
        },
        trusted: false,
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "twitty-composer-{prefix}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---- 1. Config, shortcut, and palette command ----

#[test]
fn config_composer_defaults_parses_and_sanitizes() {
    let def = TwittyConfig::default();
    assert_eq!(def.composer, "on-demand");
    assert_eq!(def.composer_mode(), ComposerMode::OnDemand);

    let off: TwittyConfig = serde_json::from_str(r#"{"composer": "off"}"#).unwrap();
    assert_eq!(off.composer_mode(), ComposerMode::Off);

    let always: TwittyConfig = serde_json::from_str(r#"{"composer": "always"}"#).unwrap();
    assert_eq!(always.composer_mode(), ComposerMode::Always);

    let dir = TempDir::new("cfg");
    let path = dir.0.join("config.json");
    std::fs::write(&path, r#"{"composer": "bogus"}"#).unwrap();
    let loaded = TwittyConfig::load_from(&path);
    assert_eq!(loaded.composer, "on-demand");
    assert_eq!(loaded.composer_mode(), ComposerMode::OnDemand);

    std::fs::write(&path, r#"{"composer": 123}"#).unwrap();
    let loaded_non_str = TwittyConfig::load_from(&path);
    assert_eq!(loaded_non_str.composer_mode(), ComposerMode::OnDemand);
}

#[test]
fn shortcut_and_palette_command_registered() {
    let ctrl_shift = ModifiersState::CONTROL | ModifiersState::SHIFT;
    let action = handle_key_raw(
        Some(KeyCode::Space),
        &Key::Named(NamedKey::Space),
        Some(" "),
        ElementState::Pressed,
        ctrl_shift,
        false,
    );
    assert_eq!(action, Some(InputAction::ToggleComposer));

    // Also works when logical key is Space without physical code (virtual keyboard).
    let virtual_action = handle_key_raw(
        None,
        &Key::Named(NamedKey::Space),
        Some(" "),
        ElementState::Pressed,
        ctrl_shift,
        false,
    );
    assert_eq!(virtual_action, Some(InputAction::ToggleComposer));

    let cmd = commands::all()
        .into_iter()
        .find(|c| c.name == "composer.open")
        .expect("composer.open command must exist");
    assert_eq!(cmd.title_en, "Compose command");
    assert_eq!(cmd.title_ar, "اكتب أمر");
    assert_eq!(cmd.shortcut, Some("Ctrl+Shift+Space"));
    assert_eq!(
        commands::action("composer.open"),
        Some(InputAction::ToggleComposer)
    );
}

// ---- 2. Prompt-state show/hide rules ----

#[test]
fn prompt_state_show_hide_rules() {
    let mut prompt = PromptState::default();
    assert_eq!(prompt, PromptState::Unknown);

    // Off mode: never shown even if manually opened or at prompt.
    assert!(!should_show_composer(
        ComposerMode::Off,
        true,
        false,
        PromptState::AtPrompt,
        false,
        false
    ));

    // OnDemand: hidden by default, shown when manually opened, hidden on alt screen.
    assert!(!should_show_composer(
        ComposerMode::OnDemand,
        false,
        false,
        PromptState::AtPrompt,
        false,
        false
    ));
    assert!(should_show_composer(
        ComposerMode::OnDemand,
        true,
        false,
        PromptState::Unknown,
        false,
        false
    ));
    assert!(!should_show_composer(
        ComposerMode::OnDemand,
        true,
        false,
        PromptState::AtPrompt,
        false,
        true
    ));

    // Always mode follows OSC 133 lifecycle:
    // Unknown -> hidden
    assert!(!should_show_composer(
        ComposerMode::Always,
        false,
        false,
        prompt,
        false,
        false
    ));
    // OSC 133 A (PromptStart) -> AtPrompt -> shown
    prompt.on_mark(MarkKind::PromptStart);
    assert_eq!(prompt, PromptState::AtPrompt);
    assert!(should_show_composer(
        ComposerMode::Always,
        false,
        false,
        prompt,
        false,
        false
    ));
    // OSC 133 B (CommandStart) -> still AtPrompt -> shown
    prompt.on_mark(MarkKind::CommandStart);
    assert!(should_show_composer(
        ComposerMode::Always,
        false,
        false,
        prompt,
        false,
        false
    ));
    // Dismissed with Esc -> hidden until next prompt (unless manually reopened).
    assert!(!should_show_composer(
        ComposerMode::Always,
        false,
        true,
        prompt,
        false,
        false
    ));
    assert!(should_show_composer(
        ComposerMode::Always,
        true,
        true,
        prompt,
        false,
        false
    ));
    // Running program (e.g. vim without C, or foreground program) -> hidden!
    assert!(!should_show_composer(
        ComposerMode::Always,
        false,
        false,
        prompt,
        true,
        false
    ));
    // Alternate screen -> hidden even if manual_open!
    assert!(!should_show_composer(
        ComposerMode::Always,
        true,
        false,
        prompt,
        false,
        true
    ));
    // OSC 133 C (OutputStart) -> Running -> hidden
    prompt.on_mark(MarkKind::OutputStart);
    assert_eq!(prompt, PromptState::Running);
    assert!(!should_show_composer(
        ComposerMode::Always,
        false,
        false,
        prompt,
        true,
        false
    ));
    // OSC 133 D (CommandEnd) -> awaiting next A -> hidden
    prompt.on_mark(MarkKind::CommandEnd { exit: Some(0) });
    assert_eq!(prompt, PromptState::CommandEnded);
    assert!(!should_show_composer(
        ComposerMode::Always,
        false,
        false,
        prompt,
        false,
        false
    ));
    // Next OSC 133 A -> shown again
    prompt.on_mark(MarkKind::PromptStart);
    assert!(should_show_composer(
        ComposerMode::Always,
        false,
        false,
        prompt,
        false,
        false
    ));
}

// ---- 3. Editing operations on mixed Arabic/English text ----

#[test]
fn editing_mixed_arabic_english_caret_and_word_moves() {
    // Pure Arabic: Left moves logically forward, Right moves logically backward.
    let mut ed = TextEditor::with_text("مرحبا بالعالم");
    let end = ed.text().len();
    assert_eq!(ed.caret(), end);
    assert!(ed.caret_is_rtl());

    ed.move_home(false, false);
    assert_eq!(ed.caret(), 0);
    assert!(ed.caret_is_rtl());

    // Visual Left inside Arabic advances logically forward by 1 grapheme (`م`).
    ed.move_visual_left(false);
    assert_eq!(ed.caret(), "م".len());
    ed.move_visual_left(false);
    assert_eq!(ed.caret(), "مر".len());

    // Visual Right inside Arabic moves logically backward.
    ed.move_visual_right(false);
    assert_eq!(ed.caret(), "م".len());

    // Ctrl+Left in Arabic advances logically forward by one word.
    ed.move_home(false, false);
    ed.move_word_visual_left(false);
    assert_eq!(ed.caret(), "مرحبا".len());
    ed.move_word_visual_left(false);
    assert_eq!(ed.caret(), "مرحبا بالعالم".len());

    // Ctrl+Right in Arabic moves logically backward by one word.
    ed.move_word_visual_right(false);
    assert_eq!(ed.caret(), "مرحبا ".len());

    // Mixed English lead + Arabic argument: LTR in `echo `, RTL in `مرحبا`.
    let mixed = "echo مرحبا 123 عالم";
    assert!(!caret_is_rtl(mixed, 0)); // 'e' -> LTR
    assert!(!caret_is_rtl(mixed, 4)); // ' ' -> LTR
    assert!(caret_is_rtl(mixed, 5)); // 'م' -> RTL
    assert!(caret_is_rtl(mixed, "echo مرحبا ".len() - 1)); // space inside RTL phrase -> RTL
    let num_pos = mixed.find("123").unwrap();
    assert!(!caret_is_rtl(mixed, num_pos)); // LRI-isolated number -> LTR!
    let alam_pos = mixed.find("عالم").unwrap();
    assert!(caret_is_rtl(mixed, alam_pos)); // 'ع' -> RTL
    assert!(caret_is_rtl(mixed, mixed.len())); // end after Arabic -> RTL

    // Moving in English part uses LTR direction.
    let mut ed2 = TextEditor::with_text(mixed);
    ed2.set_caret(0, false);
    ed2.move_visual_right(false);
    assert_eq!(ed2.caret(), 1);
    ed2.move_word_visual_right(false);
    assert_eq!(ed2.caret(), 4); // end of "echo"
}

#[test]
fn selection_clipboard_multiline_undo_redo_and_preedit() {
    let mut ed = TextEditor::new();
    ed.insert_text("echo مرحبا");
    ed.insert_newline();
    ed.insert_text("ls -la");
    assert_eq!(ed.line_count(), 2);
    assert_eq!(ed.caret_line_col(), (1, 6));

    // Up moves from line 1 to line 0, preserving grapheme column.
    assert!(ed.move_up(false));
    assert_eq!(ed.caret_line_col().0, 0);
    // Up on first line returns false (triggers history browse).
    assert!(!ed.move_up(false));
    // Down returns to line 1; Down on last line returns false.
    assert!(ed.move_down(false));
    assert_eq!(ed.caret_line_col().0, 1);
    assert!(!ed.move_down(false));

    // Selection with Shift + Home and Cut/Copy/Paste.
    ed.move_end(false, false);
    ed.move_home(false, true);
    assert_eq!(ed.selected_text(), Some("ls -la"));
    assert_eq!(ed.copy().as_deref(), Some("ls -la"));
    assert_eq!(ed.cut().as_deref(), Some("ls -la"));
    assert_eq!(ed.text(), "echo مرحبا\n");

    // Undo restores cut; redo reapplies it.
    assert!(ed.undo());
    assert_eq!(ed.text(), "echo مرحبا\nls -la");
    assert!(ed.redo());
    assert_eq!(ed.text(), "echo مرحبا\n");

    // Select all + paste replaces entire buffer.
    ed.select_all();
    assert_eq!(ed.selected_text(), Some("echo مرحبا\n"));
    ed.paste("git status\r\n");
    assert_eq!(ed.text(), "git status\n");

    // Inline preedit stored and cleared cleanly.
    ed.set_preedit(Some(("مر".into(), Some((2, 2)))));
    assert_eq!(ed.preedit().unwrap().text, "مر");
    ed.set_preedit(None);
    assert!(ed.preedit().is_none());
}

#[test]
fn composer_state_keys_history_browsing_and_ctrl_shortcuts() {
    let history = vec!["echo قديم".to_string(), "git status".to_string()];
    let mut state = ComposerState::new();
    let none = ModifiersState::empty();
    let shift = ModifiersState::SHIFT;
    let ctrl = ModifiersState::CONTROL;
    let ctrl_shift = ModifiersState::CONTROL | ModifiersState::SHIFT;

    // Type draft.
    state.key(
        &Key::Character("ls".into()),
        Some(KeyCode::KeyL),
        Some("ls"),
        none,
        &history,
        &[],
        None,
        None,
    );
    assert_eq!(state.editor.text(), "ls");

    // Up on first line recalls newest history ("echo قديم"), then older ("git status").
    state.key(
        &Key::Named(NamedKey::ArrowUp),
        None,
        None,
        none,
        &history,
        &[],
        None,
        None,
    );
    assert_eq!(state.editor.text(), "echo قديم");
    state.key(
        &Key::Named(NamedKey::ArrowUp),
        None,
        None,
        none,
        &history,
        &[],
        None,
        None,
    );
    assert_eq!(state.editor.text(), "git status");

    // Down walks back toward newest, then restores original draft "ls".
    state.key(
        &Key::Named(NamedKey::ArrowDown),
        None,
        None,
        none,
        &history,
        &[],
        None,
        None,
    );
    assert_eq!(state.editor.text(), "echo قديم");
    state.key(
        &Key::Named(NamedKey::ArrowDown),
        None,
        None,
        none,
        &history,
        &[],
        None,
        None,
    );
    assert_eq!(state.editor.text(), "ls");

    // Shift+Enter inserts newline; Ctrl+A selects all; Ctrl+C copies; Ctrl+Z/Ctrl+Shift+Z undo/redo.
    state.key(
        &Key::Named(NamedKey::Enter),
        None,
        None,
        shift,
        &history,
        &[],
        None,
        None,
    );
    assert_eq!(state.editor.text(), "ls\n");
    state.key(
        &Key::Character("a".into()),
        Some(KeyCode::KeyA),
        None,
        ctrl,
        &history,
        &[],
        None,
        None,
    );
    let act = state.key(
        &Key::Character("c".into()),
        Some(KeyCode::KeyC),
        None,
        ctrl,
        &history,
        &[],
        None,
        None,
    );
    assert_eq!(act, ComposerAction::Copy("ls\n".into()));

    state.key(
        &Key::Character("z".into()),
        Some(KeyCode::KeyZ),
        None,
        ctrl,
        &history,
        &[],
        None,
        None,
    );
    assert_eq!(state.editor.text(), "ls");

    state.key(
        &Key::Character("Z".into()),
        Some(KeyCode::KeyZ),
        None,
        ctrl_shift,
        &history,
        &[],
        None,
        None,
    );
    assert_eq!(state.editor.text(), "ls\n");

    // Enter submits and clears buffer.
    let sub = state.key(
        &Key::Named(NamedKey::Enter),
        None,
        None,
        none,
        &history,
        &[],
        None,
        None,
    );
    assert_eq!(sub, ComposerAction::Submit("ls\n".into()));
    assert_eq!(state.editor.text(), "");

    // Esc closes.
    let esc = state.key(
        &Key::Named(NamedKey::Escape),
        None,
        None,
        none,
        &history,
        &[],
        None,
        None,
    );
    assert_eq!(esc, ComposerAction::Close);
}

// ---- 4. Visual caret mapping for RTL runs (real FontSystem + RowShaper) ----

#[test]
fn visual_caret_mapping_for_rtl_and_mixed_runs() {
    let mut font_system = test_font_system();
    let palette = Palette::default();
    let metrics = CellMetrics::measure(&mut font_system, None, 15.0, 1.0);
    let mut ctx = DrawContext {
        font_system: &mut font_system,
        family: None,
        palette: &palette,
        metrics,
        is_srgb: true,
    };

    // Pure Arabic word: caret X moves monotonically right-to-left as logical index grows.
    let ar = "مرحبا";
    let mut xs = Vec::new();
    for (byte, _) in ar.char_indices() {
        let shaped = shape_composer_line(&mut ctx, ar, Some(byte), palette.foreground, None, None);
        assert!(shaped.extends_left, "byte {byte} should be RTL");
        xs.push(shaped.caret_x);
    }
    let end_shaped =
        shape_composer_line(&mut ctx, ar, Some(ar.len()), palette.foreground, None, None);
    assert!(end_shaped.extends_left);
    xs.push(end_shaped.caret_x);
    for pair in xs.windows(2) {
        assert!(
            pair[0] > pair[1],
            "expected strictly decreasing visual X across Arabic glyphs, got {xs:?}"
        );
    }

    // Mixed English lead + Arabic word: LTR increases left-to-right, RTL decreases right-to-left.
    let mixed = "echo مرحبا";
    let x_e = shape_composer_line(&mut ctx, mixed, Some(0), palette.foreground, None, None);
    let x_o = shape_composer_line(&mut ctx, mixed, Some(3), palette.foreground, None, None);
    let x_m = shape_composer_line(&mut ctx, mixed, Some(5), palette.foreground, None, None);
    let x_end = shape_composer_line(
        &mut ctx,
        mixed,
        Some(mixed.len()),
        palette.foreground,
        None,
        None,
    );
    assert!(!x_e.extends_left);
    assert!(!x_o.extends_left);
    assert!(x_o.caret_x > x_e.caret_x, "LTR should move left-to-right");
    assert!(x_m.extends_left);
    assert!(x_end.extends_left);
    assert!(
        x_m.caret_x > x_end.caret_x,
        "RTL phrase after LTR lead should move right-to-left: start={} end={}",
        x_m.caret_x,
        x_end.caret_x
    );

    // Inline preedit splices cells and reports preedit range.
    let pre = shape_composer_line(
        &mut ctx,
        "echo ",
        Some(5),
        palette.foreground,
        Some(("مرح", Some((4, 4)))),
        None,
    );
    assert_eq!(pre.preedit_cells, Some((5, 8)));
}

// ---- 5. History parsing of each shell format and dedup ----

#[test]
fn history_parses_bash_zsh_metafied_fish_and_dedups() {
    // Bash with HISTTIMEFORMAT timestamps and duplicates.
    let bash = b"#1700000001\nls -la\n#1700000002\necho \xd9\x85\xd8\xb1\xd8\xad\xd8\xa8\xd8\xa7\n#1700000003\nls -la\n";
    let parsed_bash = parse_bash_history(bash);
    assert_eq!(parsed_bash, vec!["ls -la", "echo مرحبا"]);

    // Zsh extended format + multiline continuation + metafied Arabic UTF-8.
    // "echo مرحبا": 'م' = D9 85 -> metafied as D9 83 A5.
    let mut zsh = Vec::new();
    zsh.extend_from_slice(b": 1700000010:0;git status\n");
    zsh.extend_from_slice(b": 1700000020:2;echo line1\\\nline2\n");
    zsh.extend_from_slice(b": 1700000030:0;echo \xd9\x83\xa5\xd8\xb1\xd8\xad\xd8\xa8\xd8\xa7\n");
    zsh.extend_from_slice(b": 1700000040:0;git status\n");
    let parsed_zsh = parse_zsh_history(&zsh);
    assert_eq!(
        parsed_zsh,
        vec!["git status", "echo مرحبا", "echo line1\nline2"]
    );
    assert_eq!(
        String::from_utf8(unmetafy_zsh(b"\xd9\x83\xa5")).unwrap(),
        "م"
    );

    // Fish format with escaped newlines and backslashes.
    let fish = concat!(
        "- cmd: echo old\n",
        "  when: 1700000001\n",
        "- cmd: printf 'a\\\\b\\nc'\n",
        "  when: 1700000002\n",
        "  paths:\n",
        "    - /tmp\n",
        "- cmd: echo مرحبا\n",
        "  when: 1700000003\n",
    );
    let parsed_fish = parse_fish_history(fish.as_bytes());
    assert_eq!(
        parsed_fish,
        vec!["echo مرحبا", "printf 'a\\b\nc'", "echo old"]
    );

    // Auto-detection on arbitrary $HISTFILE filename.
    assert_eq!(
        parse_auto_history(fish.as_bytes(), Some(Path::new("/tmp/my_custom_hist"))),
        parsed_fish
    );
    assert_eq!(
        parse_auto_history(&zsh, Some(Path::new("/tmp/my_custom_hist"))),
        parsed_zsh
    );

    // Bounded dedup enforces MAX_HISTORY and newest-first order.
    let many = (0..(MAX_HISTORY + 250)).map(|i| format!("cmd-{i}"));
    let bounded = dedup_newest_first(many, MAX_HISTORY);
    assert_eq!(bounded.len(), MAX_HISTORY);
    assert_eq!(bounded[0], "cmd-0");

    // HistoryStore merges session commands ahead of shell history and deduplicates.
    let dir = TempDir::new("hist");
    let bash_path = dir.0.join(".bash_history");
    std::fs::write(&bash_path, "old-cmd\nshared-cmd\n").unwrap();
    let loaded = load_shell_history_from(&[bash_path]);
    assert_eq!(loaded, vec!["shared-cmd", "old-cmd"]);

    let mut store = HistoryStore::new();
    store.set_shell_history(loaded);
    store.merge_session_commands(["block-newest".into(), "shared-cmd".into()]);
    store.push_session("composer-latest");
    assert_eq!(
        store.entries(),
        &[
            "composer-latest".to_string(),
            "block-newest".to_string(),
            "shared-cmd".to_string(),
            "old-cmd".to_string(),
        ]
    );
}

// ---- 6. Completion ranking, Arabic-aware matching, ghost suggestion, paths ----

#[test]
fn completion_ranking_arabic_matching_ghost_and_paths() {
    let history = vec![
        "git status --short".to_string(),
        "git stash pop".to_string(),
        "echo أكتب رسالة".to_string(),
        "cargo test --lib".to_string(),
    ];
    let workflows = vec![
        user_workflow("Run tests", "شغّل الاختبارات", "cargo test"),
        user_workflow("Git status", "حالة جيت", "git status"),
        untrusted_workflow("Evil", "git status && rm -rf /"),
    ];

    // Inline ghost suggestion: exact prefix + Arabic-folded prefix.
    assert_eq!(
        ghost_suggestion("git sta", 7, &history, &workflows).as_deref(),
        Some("tus --short")
    );
    assert_eq!(
        ghost_suggestion("echo اكتب", "echo اكتب".len(), &history, &workflows).as_deref(),
        Some(" رسالة")
    );
    assert_eq!(folded_prefix_suffix("أَكْتُبُ ملف", "اكتب"), Some(" ملف"));

    // Accepting ghost with Right on LTR line.
    let mut state = ComposerState::new();
    state.editor.insert_text("git sta");
    state.refresh_ghost(&history, &workflows);
    assert_eq!(state.ghost.as_deref(), Some("tus --short"));
    state.key(
        &Key::Named(NamedKey::ArrowRight),
        None,
        None,
        ModifiersState::empty(),
        &history,
        &workflows,
        None,
        None,
    );
    assert_eq!(state.editor.text(), "git status --short");

    // Ranking: exact contextual prefix beats fuzzy and history; untrusted workflow is excluded.
    let items = complete("git sta", 7, &history, &workflows, None, &[]);
    assert!(items.len() >= 4);
    assert_eq!(items[0].kind, CompletionKind::Subcommand);
    assert_eq!(items[0].replacement, "status");
    assert_eq!(items[1].kind, CompletionKind::Subcommand);
    assert_eq!(items[1].replacement, "stash");
    assert!(items
        .iter()
        .any(|i| i.kind == CompletionKind::History && i.replacement == "git status --short"));
    assert!(items
        .iter()
        .any(|i| i.kind == CompletionKind::History && i.replacement == "git stash pop"));
    assert!(
        items.iter().all(|i| !i.replacement.contains("rm -rf")),
        "untrusted project workflow must not appear in completions"
    );

    // Arabic workflow title matching (folding alef/hamza/tashkeel).
    let ar_items = complete("شغل", "شغل".len(), &history, &workflows, None, &[]);
    assert_eq!(ar_items.len(), 1);
    assert_eq!(ar_items[0].kind, CompletionKind::Workflow);
    assert_eq!(ar_items[0].replacement, "cargo test");

    // Path completion relative to pane cwd (including Arabic filename and space escaping).
    let dir = TempDir::new("paths");
    std::fs::create_dir(dir.0.join("مجلد")).unwrap();
    std::fs::write(dir.0.join("أوامر مهمة.txt"), b"").unwrap();
    std::fs::write(dir.0.join("notes.md"), b"").unwrap();
    let entries = read_dir_entries(&dir.0, 64);
    assert!(entries.iter().any(|e| e
        == &DirEntryInfo {
            name: "مجلد".into(),
            is_dir: true
        }));

    let input = "cat اوامر";
    let pq = extract_path_query(input, input.len(), Some(&dir.0)).unwrap();
    assert!(pq.path_context);
    let path_items = complete(input, input.len(), &[], &[], Some(&pq), &entries);
    assert_eq!(path_items.len(), 1);
    assert_eq!(path_items[0].kind, CompletionKind::Path);
    assert_eq!(path_items[0].replacement, "أوامر\\ مهمة.txt");
    assert_eq!(path_items[0].replace_range, (4, input.len()));

    // Tab opens popup for multiple matches; Down + Enter accepts the second match; Esc closes popup.
    let mut tab_state = ComposerState::new();
    tab_state.editor.insert_text("git st");
    tab_state.key(
        &Key::Named(NamedKey::Tab),
        None,
        None,
        ModifiersState::empty(),
        &history,
        &workflows,
        None,
        None,
    );
    assert!(tab_state.popup.is_some());
    assert!(tab_state.popup.as_ref().unwrap().items.len() >= 2);
    tab_state.key(
        &Key::Named(NamedKey::ArrowDown),
        None,
        None,
        ModifiersState::empty(),
        &history,
        &workflows,
        None,
        None,
    );
    assert_eq!(tab_state.popup.as_ref().unwrap().selected, 1);
    tab_state.key(
        &Key::Named(NamedKey::Enter),
        None,
        None,
        ModifiersState::empty(),
        &history,
        &workflows,
        None,
        None,
    );
    assert!(tab_state.popup.is_none());
    assert_eq!(tab_state.editor.text(), "git stash");
}

// ---- 7. Bracketed send bytes ----

#[test]
fn bracketed_and_plain_submit_bytes() {
    // Bracketed paste wraps command and appends CR.
    assert_eq!(
        encode_submit("echo مرحبا", true),
        "\x1b[200~echo مرحبا\x1b[201~\r".as_bytes().to_vec()
    );
    // Multi-line command preserves LF inside bracketed paste and appends CR.
    assert_eq!(
        encode_submit("echo a\necho b", true),
        b"\x1b[200~echo a\necho b\x1b[201~\r".to_vec()
    );
    // Hostile ESC / C1 CSI / NUL stripped so bracketed paste cannot be broken out of.
    assert_eq!(
        encode_submit("safe\x1b[201~evil\u{009b}x\0", true),
        b"\x1b[200~safe[201~evilx\x1b[201~\r".to_vec()
    );
    // Unbracketed mode converts newlines to CR and appends CR.
    assert_eq!(encode_submit("echo a\necho b", false), b"echo a\recho b\r");
    // Empty submit sends just CR.
    assert_eq!(encode_submit("", true), b"\r");
    assert_eq!(encode_submit("", false), b"\r");
}

// ---- 8. Headless CPU scene & clipping over terminal rows ----

#[test]
fn composer_scene_clips_underlying_terminal_text_and_scales_light_dark() {
    let mut font_system = test_font_system();
    for theme_name in ["tokyo-night", "github-light"] {
        for scale in [1.0_f32, 2.0_f32] {
            let palette = Palette::named(theme_name).unwrap();
            let metrics = CellMetrics::measure(&mut font_system, None, 15.0, scale);
            let width = 800.0 * scale;
            let height = 500.0 * scale;
            let pane_rect = Rect::new(0.0, 0.0, width, height);

            let mut term = Terminal::new(60, 16, |_| {});
            for i in 0..16 {
                term.process_bytes(format!("line {i} مرحبا بالعالم\r\n").as_bytes());
            }
            let (lines, cursor) = term.snapshot();
            let panes = [PaneFrame {
                id: PaneId(1),
                rect: pane_rect,
                lines,
                cursor,
                focused: true,
                status: None,
                blocks: Vec::new(),
            }];

            let ui_font = UiFont::default();
            let style = UiStyle::new(&palette, None, scale);
            let mut ctx = DrawContext {
                font_system: &mut font_system,
                family: None,
                palette: &palette,
                metrics,
                is_srgb: true,
            };
            let opts = FrameOptions {
                window_focused: true,
                cursor_style: "block",
                cursor_blink_on: true,
                surface: (width, height),
            };
            let mut cache = RowCache::default();
            let mut ime = PreeditState::default();
            let mut frame = build_frame(&mut ctx, &opts, &mut cache, &mut ime, &[], &panes, &[]);

            let view = ComposerView {
                pane_rect,
                lines: vec!["echo مرحبا".into(), "ls -la".into()],
                caret: (0, "echo مرح".len()),
                selection: Some(((0, 0), (0, 4))),
                preedit: None,
                ghost: None,
                highlights: Vec::new(),
                lints: Vec::new(),
                focused: true,
                cursor_blink_on: true,
                popup: Some(ComposerPopupView {
                    items: vec![
                        ComposerPopupItem {
                            kind: CompletionKind::History,
                            label: "echo مرحبا بالعالم".into(),
                            detail: None,
                        },
                        ComposerPopupItem {
                            kind: CompletionKind::Workflow,
                            label: "cargo test".into(),
                            detail: Some("Run tests / شغّل الاختبارات".into()),
                        },
                    ],
                    selected: 0,
                }),
                language: UiLanguage::Arabic,
                ai: Default::default(),
            };

            let layout = ComposerLayout::new(pane_rect, metrics, scale, 2, 0, 2);
            assert!(layout.card.bottom() <= pane_rect.bottom());
            assert!(layout.popup.is_some());
            let popup = layout.popup.unwrap();
            assert!(popup.bottom() <= layout.card.y);
            assert_eq!(
                layout.hit(
                    f64::from(layout.input.x + 2.0),
                    f64::from(layout.input.y + 2.0)
                ),
                ComposerHit::Input
            );
            let row0 = layout.popup_row(0).unwrap();
            assert_eq!(
                layout.hit(f64::from(row0.x + 2.0), f64::from(row0.y + 2.0)),
                ComposerHit::PopupItem(0)
            );
            assert_eq!(layout.hit(1.0, 1.0), ComposerHit::Outside);

            let scene = build_composer_scene(&mut ctx, style, &ui_font, &view);
            assert_eq!(scene.clips.len(), 2); // card + popup
            assert!(scene.ime_area.is_some());
            assert!(!scene.shapes.is_empty());
            assert_eq!(scene.texts.len(), 2);

            frame.add_composer(&mut ctx, style, &ui_font, &view);
            let runs = frame.text_runs(&panes, &cache, &ime);
            // Every run that belongs to terminal rows (top above card and not inside card/popup)
            // must not have bounds overlapping the composer card interior.
            let card_top = layout.card.y.floor() as i32;
            let card_left = layout.card.x.floor() as i32;
            let card_right = layout.card.right().ceil() as i32;
            for run in &runs {
                if (run.top as i32) < card_top {
                    // Any terminal text run from above or clipped around the card cannot bleed
                    // inside the card rect.
                    let overlaps_card = run.bounds.left < card_right
                        && run.bounds.right > card_left
                        && run.bounds.bottom > card_top
                        && run.bounds.top < layout.card.bottom().ceil() as i32;
                    assert!(
                        !overlaps_card,
                        "non-composer text run leaked into composer card: bounds={:?} card={:?}",
                        run.bounds, layout.card
                    );
                }
            }
        }
    }
}
