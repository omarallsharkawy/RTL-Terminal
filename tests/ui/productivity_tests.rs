//! Window-free tests for lane N12 productivity features:
//! 1. Project switcher (MRU persistence & bounds, git repo scan limits & symlink loops, matching).
//! 2. Named layouts (validation, save/load round trip, bounds, delete, Arabic names).
//! 3. Tab reorder & reopen closed tab (shortcuts, bounded LIFO stack, cwd/title restore).
//! 4. Broadcast input (off by default, per-tab routing, toggle & Esc-Esc turn-off, badge).

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use alacritty_terminal::term::TermMode;
use twitty::commands;
use twitty::config::TwittyConfig;
use twitty::input::broadcast::{format_broadcast_header_title, BroadcastState, DOUBLE_ESC_WINDOW};
use twitty::input::{handle_key_mode_raw, handle_key_raw, InputAction};
use twitty::layout::{Node, PaneId, SplitDir};
use twitty::layouts::{
    capture_tree, clean_layout_name, LayoutNode, LayoutSplitDir, LayoutStore, NamedLayout,
    MAX_LAYOUTS,
};
use twitty::projects::mru::{ProjectMru, MAX_MRU_ENTRIES};
use twitty::projects::scan::{
    default_project_roots, expand_root, scan_roots_with_limits, ScanLimits,
};
use twitty::projects::{
    abbreviate_home, find_existing_tab_for_project, merge_projects, ProjectItem, TabProjectCwds,
};
use twitty::session::closed::{ClosedTab, ClosedTabs, MAX_CLOSED_TABS};
use twitty::ui::chrome::UiLanguage;
use twitty::ui::palette::{matches, PaletteEntry};
use winit::event::ElementState;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().canonicalize().unwrap();
        loop {
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = base.join(format!("twitty-n12-{tag}-{}-{n}", std::process::id()));
            match fs::create_dir(&dir) {
                Ok(()) => return Self(dir),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => panic!("create temp dir: {err}"),
            }
        }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn make_git_repo(dir: &Path, branch: &str) {
    fs::create_dir_all(dir.join(".git")).unwrap();
    fs::write(
        dir.join(".git").join("HEAD"),
        format!("ref: refs/heads/{branch}\n"),
    )
    .unwrap();
}

// ---- 1. Project switcher: MRU persistence & bounds ----

#[test]
fn mru_persists_atomically_with_0600_and_enforces_bounds() {
    let tmp = TempDir::new("mru");
    let state_file = tmp.0.join("state").join("recent_projects.json");

    let mut mru = ProjectMru::default();
    assert!(mru.is_empty());

    // Relative paths are ignored.
    assert!(!mru.record(Path::new("relative/path")));
    assert!(mru.is_empty());

    // Record more than MAX_MRU_ENTRIES distinct absolute directories.
    for i in 0..(MAX_MRU_ENTRIES + 15) {
        let p = tmp.0.join(format!("proj-{i:03}"));
        assert!(mru.record(&p));
    }
    assert_eq!(mru.len(), MAX_MRU_ENTRIES);
    // Most recent is at index 0; oldest 15 were dropped.
    let newest = tmp.0.join(format!("proj-{:03}", MAX_MRU_ENTRIES + 14));
    assert_eq!(mru.entries()[0], newest);
    assert!(!mru.entries().contains(&tmp.0.join("proj-000")));

    // Re-recording an existing entry moves it to front without changing length.
    let older = tmp.0.join(format!("proj-{:03}", MAX_MRU_ENTRIES));
    assert!(mru.record(&older));
    assert_eq!(mru.entries()[0], older);
    assert_eq!(mru.len(), MAX_MRU_ENTRIES);
    // Recording the current top again is a no-op.
    assert!(!mru.record(&older));

    // Persist and reload.
    mru.save_to(&state_file).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let file_mode = fs::symlink_metadata(&state_file)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let dir_mode = fs::symlink_metadata(state_file.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
        assert_eq!(dir_mode, 0o700);
    }

    let reloaded = ProjectMru::load_from(&state_file);
    assert_eq!(reloaded, mru);

    // Wrong version or corrupt JSON yields empty MRU cleanly.
    fs::write(&state_file, r#"{"version": 99, "entries": ["/tmp"]}"#).unwrap();
    assert!(ProjectMru::load_from(&state_file).is_empty());
    fs::write(&state_file, "{not json").unwrap();
    assert!(ProjectMru::load_from(&state_file).is_empty());

    // Symlink target is refused on both read and write.
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let real = tmp.0.join("real.json");
        fs::write(&real, r#"{"version": 1, "entries": ["/tmp"]}"#).unwrap();
        let link = tmp.0.join("state").join("link.json");
        symlink(&real, &link).unwrap();
        assert!(ProjectMru::load_from(&link).is_empty());
        assert!(mru.save_to(&link).is_err());
    }
}

#[test]
fn mru_path_lives_next_to_session_in_state_dir() {
    // Platform-absolute bases (a Unix-style "/var/state" is not absolute on Windows).
    let base = std::env::temp_dir();
    let state = base.join("var-state");
    let home = base.join("home-u");
    let in_home = home
        .join(".local")
        .join("state")
        .join("twitty")
        .join("recent_projects.json");
    assert_eq!(
        ProjectMru::path_from(Some(state.as_os_str()), Some(home.as_os_str())),
        Some(state.join("twitty").join("recent_projects.json"))
    );
    assert_eq!(
        ProjectMru::path_from(None, Some(home.as_os_str())),
        Some(in_home.clone())
    );
    // Relative XDG_STATE_HOME is ignored.
    assert_eq!(
        ProjectMru::path_from(Some(OsStr::new("relative")), Some(home.as_os_str())),
        Some(in_home)
    );
    assert_eq!(ProjectMru::path_from(None, None), None);
}

// ---- 1b. Project switcher: repo scan limits & symlink loops ----

#[test]
fn repo_scan_finds_one_and_two_levels_deep_reads_branch_and_respects_limits() {
    let tmp = TempDir::new("scan");
    let root = tmp.0.join("Projects");
    fs::create_dir(&root).unwrap();

    // Level 1 repo (with Arabic branch name).
    let repo1 = root.join("twitty");
    make_git_repo(&repo1, "ميزة-عربي");
    // Nested repo inside level 1 repo should NOT be scanned (stop at repo boundary).
    make_git_repo(&repo1.join("subrepo"), "nested");

    // Level 2 repo under a grouping folder.
    let repo2 = root.join("org").join("api");
    make_git_repo(&repo2, "develop");

    // Level 3 repo should NOT be discovered (max depth is 2).
    let repo3 = root.join("org").join("deep").join("too-deep");
    make_git_repo(&repo3, "main");

    // Hidden directory should be skipped.
    make_git_repo(&root.join(".hidden-repo"), "secret");

    // Worktree-style `.git` file at level 1.
    let wt_gitdir = tmp.0.join("bare-gitdir");
    fs::create_dir(&wt_gitdir).unwrap();
    fs::write(wt_gitdir.join("HEAD"), "ref: refs/heads/worktree-branch\n").unwrap();
    let wt_repo = root.join("wt");
    fs::create_dir(&wt_repo).unwrap();
    fs::write(
        wt_repo.join(".git"),
        format!("gitdir: {}\n", wt_gitdir.display()),
    )
    .unwrap();

    let found = scan_roots_with_limits(std::slice::from_ref(&root), ScanLimits::default());
    let paths: Vec<PathBuf> = found.iter().map(|r| r.path.clone()).collect();
    assert_eq!(paths, vec![repo2.clone(), repo1.clone(), wt_repo.clone()]);
    assert_eq!(
        found
            .iter()
            .find(|r| r.path == repo1)
            .unwrap()
            .branch
            .as_deref(),
        Some("ميزة-عربي")
    );
    assert_eq!(
        found
            .iter()
            .find(|r| r.path == repo2)
            .unwrap()
            .branch
            .as_deref(),
        Some("develop")
    );
    assert_eq!(
        found
            .iter()
            .find(|r| r.path == wt_repo)
            .unwrap()
            .branch
            .as_deref(),
        Some("worktree-branch")
    );

    // Enforce max_repos limit.
    let limited = scan_roots_with_limits(
        std::slice::from_ref(&root),
        ScanLimits {
            max_repos: 2,
            ..ScanLimits::default()
        },
    );
    assert_eq!(limited.len(), 2);

    // Enforce max_dirs_visited limit.
    let tight = scan_roots_with_limits(
        &[root],
        ScanLimits {
            max_dirs_visited: 1,
            ..ScanLimits::default()
        },
    );
    assert!(tight.len() <= 1);
}

#[cfg(unix)]
#[test]
fn repo_scan_handles_symlink_loops_and_broken_links_cleanly() {
    use std::os::unix::fs::symlink;

    let tmp = TempDir::new("symlinks");
    let root = tmp.0.join("Projects");
    fs::create_dir(&root).unwrap();

    let real_repo = root.join("real-repo");
    make_git_repo(&real_repo, "main");

    // Ancestor loop: Projects/loop_dir/back -> Projects
    let loop_dir = root.join("loop_dir");
    fs::create_dir(&loop_dir).unwrap();
    symlink(&root, loop_dir.join("back")).unwrap();
    // Self loop: Projects/loop_dir/self -> Projects/loop_dir
    symlink(&loop_dir, loop_dir.join("self")).unwrap();
    // Mutual cycle: a/to_b -> b, b/to_a -> a
    let a = root.join("cycle_a");
    let b = root.join("cycle_b");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    symlink(&b, a.join("to_b")).unwrap();
    symlink(&a, b.join("to_a")).unwrap();
    // Broken symlink
    symlink(tmp.0.join("does-not-exist"), root.join("broken")).unwrap();
    // Symlink alias to the same repo should not duplicate it.
    symlink(&real_repo, root.join("alias-repo")).unwrap();

    let found = scan_roots_with_limits(&[root], ScanLimits::default());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].branch.as_deref(), Some("main"));
}

#[test]
fn project_roots_config_and_switcher_merge_and_tab_match() {
    assert_eq!(
        default_project_roots(),
        vec!["~/Projects", "~/src", "~/code"]
    );
    let default_cfg: TwittyConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(default_cfg.project_roots, default_project_roots());
    let custom_cfg: TwittyConfig =
        serde_json::from_str(r#"{"project_roots": ["/work/repos", "~/oss"]}"#).unwrap();
    assert_eq!(custom_cfg.project_roots, vec!["/work/repos", "~/oss"]);

    let home_buf = std::env::temp_dir().join("home-tester");
    let home = home_buf.as_path();
    assert_eq!(
        expand_root("~/Projects", Some(home)),
        Some(home.join("Projects"))
    );
    assert_eq!(expand_root("relative/dir", Some(home)), None);
    assert_eq!(
        abbreviate_home(&home.join("Projects").join("twitty"), Some(home)),
        format!("~/{}", Path::new("Projects").join("twitty").display())
    );

    let tmp = TempDir::new("merge");
    let p_mru = tmp.0.join("مشروع-عربي");
    let p_scan = tmp.0.join("scanned-app");
    make_git_repo(&p_mru, "main");
    make_git_repo(&p_scan, "feat");

    let scanned = scan_roots_with_limits(std::slice::from_ref(&tmp.0), ScanLimits::default());
    let items = merge_projects(std::slice::from_ref(&p_mru), scanned, Some(&tmp.0), true);
    assert_eq!(items.len(), 2);
    // MRU entry comes first and keeps its branch without duplication.
    assert_eq!(items[0].path, p_mru);
    assert!(items[0].recent);
    assert_eq!(items[0].branch.as_deref(), Some("main"));
    assert_eq!(items[1].path, p_scan);
    assert!(!items[1].recent);

    // Arabic + branch palette matching works.
    assert_eq!(matches(&items, "مشروع"), vec![0]);
    assert_eq!(matches(&items, "feat"), vec![1]);
    let row = items[0].to_palette_item();
    assert!(row.title_ar.contains("main"));

    // Existing tab lookup prefers focused pane, then falls back to any pane.
    let tabs = vec![
        TabProjectCwds {
            focused: PaneId(1),
            panes: vec![
                (PaneId(1), Some(p_scan.clone())),
                (PaneId(2), Some(p_mru.clone())),
            ],
        },
        TabProjectCwds {
            focused: PaneId(3),
            panes: vec![(PaneId(3), Some(p_mru.clone()))],
        },
    ];
    assert_eq!(
        find_existing_tab_for_project(&tabs, &p_mru),
        Some((1, PaneId(3)))
    );
    assert_eq!(
        find_existing_tab_for_project(&tabs, &p_scan),
        Some((0, PaneId(1)))
    );
    assert_eq!(
        find_existing_tab_for_project(&tabs, &tmp.0.join("other")),
        None
    );
}

// ---- 2. Named layouts: validation, round trip, bounds, delete ----

#[test]
fn named_layouts_save_load_delete_validation_and_bounds() {
    let tmp = TempDir::new("layouts");
    let path = tmp.0.join("state").join("layouts.json");

    let tree = Node::Split {
        dir: SplitDir::Horizontal,
        ratio: 0.4,
        a: Box::new(Node::Leaf(PaneId(1))),
        b: Box::new(Node::Split {
            dir: SplitDir::Vertical,
            ratio: 0.6,
            a: Box::new(Node::Leaf(PaneId(2))),
            b: Box::new(Node::Leaf(PaneId(3))),
        }),
    };
    let dir1 = tmp.0.join("front");
    let dir2 = tmp.0.join("back");
    fs::create_dir(&dir1).unwrap();
    fs::create_dir(&dir2).unwrap();

    let layout = capture_tree(
        "تخطيط العمل",
        Some("العمل"),
        &tree,
        PaneId(2),
        |id| match id.0 {
            1 => (Some(dir1.clone()), Some("vim".into())),
            2 => (Some(dir2.clone()), Some("اختبار".into())),
            _ => (None, None),
        },
    )
    .unwrap();
    assert_eq!(layout.pane_count(), 3);

    let mut store = LayoutStore::default();
    store.upsert(layout.clone()).unwrap();
    store.save_to(&path).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::symlink_metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    let mut loaded = LayoutStore::load_from(&path);
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded.get("تخطيط العمل"), Some(&layout));
    // Arabic palette search matches layout names.
    assert_eq!(matches(loaded.layouts(), "تخطيط"), vec![0]);

    // Delete works and persists.
    assert!(loaded.delete("تخطيط العمل"));
    assert!(!loaded.delete("تخطيط العمل"));
    loaded.save_to(&path).unwrap();
    assert!(LayoutStore::load_from(&path).is_empty());

    // Bounded to MAX_LAYOUTS.
    let single = Node::Leaf(PaneId(1));
    let mut bounded = LayoutStore::default();
    for i in 0..(MAX_LAYOUTS + 10) {
        let l = capture_tree(&format!("lay-{i:03}"), None, &single, PaneId(1), |_| {
            (None, None)
        })
        .unwrap();
        bounded.upsert(l).unwrap();
    }
    assert_eq!(bounded.len(), MAX_LAYOUTS);
    assert!(bounded
        .get(&format!("lay-{:03}", MAX_LAYOUTS + 9))
        .is_some());
    assert!(bounded.get("lay-000").is_none());

    // Invalid names and malformed trees are rejected.
    assert!(clean_layout_name("   \x07\u{202E} ").is_none());
    let cycle = NamedLayout {
        name: "bad".into(),
        tab_title: None,
        nodes: vec![
            LayoutNode::Pane {
                cwd: None,
                title: None,
            },
            LayoutNode::Split {
                direction: LayoutSplitDir::Horizontal,
                ratio: 0.5,
                a: 0,
                b: 1,
            },
        ],
        focused: 0,
    };
    assert!(cycle.validate().is_err());

    let bad_focus = NamedLayout {
        name: "bad-focus".into(),
        tab_title: None,
        nodes: vec![
            LayoutNode::Pane {
                cwd: None,
                title: None,
            },
            LayoutNode::Pane {
                cwd: None,
                title: None,
            },
            LayoutNode::Split {
                direction: LayoutSplitDir::Horizontal,
                ratio: 0.5,
                a: 0,
                b: 1,
            },
        ],
        focused: 2,
    };
    assert!(bad_focus.validate().is_err());

    let nan_ratio = NamedLayout {
        name: "nan".into(),
        tab_title: None,
        nodes: vec![
            LayoutNode::Pane {
                cwd: None,
                title: None,
            },
            LayoutNode::Pane {
                cwd: None,
                title: None,
            },
            LayoutNode::Split {
                direction: LayoutSplitDir::Horizontal,
                ratio: f32::NAN,
                a: 0,
                b: 1,
            },
        ],
        focused: 0,
    };
    assert!(nan_ratio.validate().is_err());
}

// ---- 3. Tab move & reopen closed tab ----

#[test]
fn shortcuts_for_project_switcher_tab_move_and_reopen_work_in_all_modes() {
    let cs = ModifiersState::CONTROL | ModifiersState::SHIFT;
    let csa = cs | ModifiersState::ALT;
    let modes = [
        TermMode::empty(),
        TermMode::DISAMBIGUATE_ESC_CODES,
        TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES,
        TermMode::REPORT_ALL_KEYS_AS_ESC,
    ];
    for mode in modes {
        let press = |code, key: Key, mods| {
            handle_key_mode_raw(
                Some(code),
                &key,
                None,
                None,
                ElementState::Pressed,
                false,
                mods,
                mode,
            )
        };
        assert_eq!(
            press(KeyCode::KeyL, Key::Dead(None), cs),
            Some(InputAction::OpenProject)
        );
        assert_eq!(
            press(KeyCode::PageUp, Key::Named(NamedKey::PageUp), cs),
            Some(InputAction::MoveTabLeft)
        );
        assert_eq!(
            press(KeyCode::PageDown, Key::Named(NamedKey::PageDown), cs),
            Some(InputAction::MoveTabRight)
        );
        assert_eq!(
            press(KeyCode::KeyT, Key::Dead(None), csa),
            Some(InputAction::ReopenClosedTab)
        );
        // Arabic layout: physical key still triggers Ctrl+Shift+L and Ctrl+Shift+Alt+T.
        assert_eq!(
            press(KeyCode::KeyL, Key::Character("م".into()), cs),
            Some(InputAction::OpenProject)
        );
        assert_eq!(
            press(KeyCode::KeyT, Key::Character("ف".into()), csa),
            Some(InputAction::ReopenClosedTab)
        );
    }
    // Plain Ctrl+L still goes to the shell (clear screen!).
    assert_eq!(
        handle_key_raw(
            Some(KeyCode::KeyL),
            &Key::Character("l".into()),
            None,
            ElementState::Pressed,
            ModifiersState::CONTROL,
            false,
        ),
        Some(InputAction::Bytes(vec![0x0c]))
    );
}

#[test]
fn closed_tabs_stack_is_lifo_bounded_and_validates_cwd() {
    let tmp = TempDir::new("closed-tabs");
    let existing = tmp.0.join("alive");
    fs::create_dir(&existing).unwrap();
    let missing = tmp.0.join("gone");

    let mut stack = ClosedTabs::new();
    assert!(stack.is_empty());
    assert_eq!(stack.pop(), None);

    for i in 0..(MAX_CLOSED_TABS + 5) {
        stack.push(ClosedTab::new(
            Some(if i % 2 == 0 {
                existing.clone()
            } else {
                missing.clone()
            }),
            Some(format!("Tab {i} · تاب {i}")),
            Some(format!("shell-{i}")),
        ));
    }
    assert_eq!(stack.len(), MAX_CLOSED_TABS);

    // LIFO order: newest popped first.
    let top = stack.pop().unwrap();
    assert_eq!(
        top.tab_title.as_deref(),
        Some(format!("Tab {} · تاب {}", MAX_CLOSED_TABS + 4, MAX_CLOSED_TABS + 4).as_str())
    );
    // Relative paths and control chars are stripped.
    let sanitized = ClosedTab::new(
        Some(PathBuf::from("relative")),
        Some("عربي\x07\u{202E}".into()),
        Some("   ".into()),
    );
    assert_eq!(sanitized.cwd, None);
    assert_eq!(sanitized.tab_title.as_deref(), Some("عربي"));
    assert_eq!(sanitized.pane_title, None);

    let with_existing = ClosedTab::new(Some(existing.clone()), None, None);
    assert_eq!(with_existing.existing_cwd(), Some(existing));
    let with_missing = ClosedTab::new(Some(missing), None, None);
    assert_eq!(with_missing.existing_cwd(), None);
}

// ---- 4. Broadcast input ----

#[test]
fn broadcast_is_off_by_default_routes_to_all_panes_and_turns_off_cleanly() {
    let mut bc = BroadcastState::new();
    let tab1 = 1;
    let tab2 = 2;
    let panes1 = [PaneId(1), PaneId(2), PaneId(3)];
    let panes2 = [PaneId(4), PaneId(5)];

    // Off by default: only focused pane receives input.
    assert!(!bc.is_active(tab1));
    assert_eq!(bc.target_panes(tab1, PaneId(2), &panes1), vec![PaneId(2)]);

    // Toggle on for tab 1: every pane of tab 1 receives input; tab 2 stays single-pane.
    assert!(bc.toggle(tab1));
    assert!(bc.is_active(tab1));
    assert!(!bc.is_active(tab2));
    assert_eq!(
        bc.target_panes(tab1, PaneId(2), &panes1),
        vec![PaneId(1), PaneId(2), PaneId(3)]
    );
    assert_eq!(bc.target_panes(tab2, PaneId(4), &panes2), vec![PaneId(4)]);

    // Second toggle turns it off cleanly.
    assert!(!bc.toggle(tab1));
    assert!(!bc.is_active(tab1));
    assert_eq!(bc.target_panes(tab1, PaneId(2), &panes1), vec![PaneId(2)]);

    // Toggle back on, then turn off with Esc-Esc within DOUBLE_ESC_WINDOW.
    assert!(bc.toggle(tab1));
    let t0 = Instant::now();
    // First Esc arms the detector but does not disable yet.
    assert!(!bc.on_plain_escape(tab1, t0));
    assert!(bc.is_active(tab1));
    // Another key cancels the pending first Esc.
    bc.cancel_escape();
    assert!(!bc.on_plain_escape(tab1, t0 + Duration::from_millis(100)));
    assert!(bc.is_active(tab1));
    // Waiting longer than DOUBLE_ESC_WINDOW treats the next Esc as a fresh first Esc.
    let t_late = t0 + Duration::from_millis(100) + DOUBLE_ESC_WINDOW + Duration::from_millis(50);
    assert!(!bc.on_plain_escape(tab1, t_late));
    assert!(bc.is_active(tab1));
    // Second Esc within window disables broadcast cleanly!
    assert!(bc.on_plain_escape(tab1, t_late + Duration::from_millis(150)));
    assert!(!bc.is_active(tab1));
    assert_eq!(bc.target_panes(tab1, PaneId(1), &panes1), vec![PaneId(1)]);

    // Closing a tab forgets its broadcast state.
    assert!(bc.toggle(tab2));
    bc.forget_tab(tab2);
    assert!(!bc.is_active(tab2));

    // Bilingual pane header badge formatting.
    let en = format_broadcast_header_title("bash", UiLanguage::English);
    let ar = format_broadcast_header_title("bash", UiLanguage::Arabic);
    assert!(en.contains("ALL") && en.contains("بث") && en.contains("bash"));
    assert!(ar.contains("ALL") && ar.contains("بث") && ar.contains("bash"));
}

// ---- 5. Command palette bilingual entries ----

#[test]
fn all_n12_commands_have_english_and_arabic_names_and_match_in_palette() {
    let expected = [
        ("project.open", "Open project", "افتح مشروع"),
        ("layout.save", "Save layout…", "احفظ التخطيط…"),
        ("layout.open", "Open layout…", "افتح تخطيط…"),
        ("layout.delete", "Delete layout…", "احذف تخطيط…"),
        ("tab.move-left", "Move tab left", "حرّك التاب شمال"),
        ("tab.move-right", "Move tab right", "حرّك التاب يمين"),
        ("tab.reopen", "Reopen closed tab", "رجّع آخر تاب اتقفل"),
        (
            "input.broadcast",
            "Type in all panes of this tab",
            "اكتب في كل اللوحات",
        ),
    ];
    let all = commands::all();
    let entries = PaletteEntry::builtins();
    for (name, en, ar) in expected {
        let cmd = all
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("missing command {name}"));
        assert_eq!(cmd.title_en, en);
        assert_eq!(cmd.title_ar, ar);
        assert!(commands::action(name).is_some());
        assert!(
            !matches(&entries, en).is_empty(),
            "English match for {name}"
        );
        assert!(!matches(&entries, ar).is_empty(), "Arabic match for {name}");
    }
    // Verify Arabic query without tashkeel/hamza matches too.
    assert!(!matches(&entries, "افتح مشروع").is_empty());
    assert!(!matches(&entries, "رجع اخر تاب").is_empty());
    assert!(!matches(&entries, "اكتب في كل اللوحات").is_empty());
    assert!(!matches(&entries, "حرك التاب").is_empty());

    // Sanity check ProjectItem with Arabic directory name.
    let item = ProjectItem::new(
        PathBuf::from("/tmp/تطبيق"),
        None,
        Some("main".into()),
        false,
    );
    assert_eq!(item.name, "تطبيق");
    assert_eq!(item.to_palette_item().title_ar, "main");
}
