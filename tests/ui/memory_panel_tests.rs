//! Window-free integration tests for the Project Memory screen, palette `memory.open`,
//! Settings -> AI -> "Project memory", `remember_fact`, config `memory_exclude`, Arabic RTL
//! mirroring, hit testing, and headless overlay rendering/clipping.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use cosmic_text::FontSystem;
use twitty::blocks::{Block, BlockList};
use twitty::color::Palette;
use twitty::commands;
use twitty::config::TwittyConfig;
use twitty::history::record_finished_block_with_excludes;
use twitty::input::InputAction;
use twitty::layout::{PaneId, Rect};
use twitty::memory::{remember_fact, remember_fact_with_excludes};
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::frame::{build_frame, FrameOptions};
use twitty::render::scene::memory::MemoryView;
use twitty::render::scene::preedit::PreeditState;
use twitty::render::shapes::Shape;
use twitty::render::text::ui::UiFont;
use twitty::render::ui_style::UiStyle;
use twitty::render::{CellMetrics, PaneFrame, RowCache};
use twitty::terminal::Terminal;
use twitty::ui::chrome::UiLanguage;
use twitty::ui::memory_panel::{
    MemoryAction, MemoryHit, MemoryInputTarget, MemoryLayout, MemoryPanelState, MemoryTab,
};
use twitty::ui::palette::{matches, PaletteEntry, PaletteId};
use twitty::ui::settings_panel::{
    ControlGeometry, SettingId, SettingsHit, SettingsLayout, SettingsPanelState, SettingsSection,
};
use twitty_store::{
    FrequentCommand, MemoryEntry, MemoryKind, ProjectMemorySnapshot, RulesFileInfo, RulesFileState,
    StoreHandle, AUTO_COMMAND_SOURCE, AUTO_ENV_SOURCE,
};
use winit::keyboard::{Key, ModifiersState, NamedKey};

const SURFACE: (f32, f32) = (1024.0, 720.0);

fn center(r: Rect) -> (f64, f64) {
    (f64::from(r.x + r.w / 2.0), f64::from(r.y + r.h / 2.0))
}

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "twitty-memory-panel-{tag}-{}-{id}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn sample_snapshot() -> ProjectMemorySnapshot {
    ProjectMemorySnapshot {
        project_root: "/home/user/projects/twitty".into(),
        excluded: false,
        rules_files: vec![
            RulesFileInfo {
                rel_path: "AGENTS.md".into(),
                current_sha256: "a1b2c3d4e5f600001111222233334444".into(),
                approved_sha256: Some("a1b2c3d4e5f600001111222233334444".into()),
                state: RulesFileState::Approved,
                approved_content: Some("Keep grid logical".into()),
            },
            RulesFileInfo {
                rel_path: "CLAUDE.md".into(),
                current_sha256: "ff00ee11dd22cc33bb44aa5599887766".into(),
                approved_sha256: Some("00000000000000000000000000000000".into()),
                state: RulesFileState::Changed,
                approved_content: None,
            },
            RulesFileInfo {
                rel_path: "TWITTY.md".into(),
                current_sha256: "1234567890abcdef1234567890abcdef".into(),
                approved_sha256: None,
                state: RulesFileState::Unapproved,
                approved_content: None,
            },
        ],
        entries: vec![
            MemoryEntry {
                id: 1,
                project_root: "/home/user/projects/twitty".into(),
                kind: MemoryKind::Rule,
                text: "Every UI string has English and Arabic".into(),
                source: "user".into(),
                created_at: 100,
                updated_at: 200,
                approved: true,
            },
            MemoryEntry {
                id: 2,
                project_root: "/home/user/projects/twitty".into(),
                kind: MemoryKind::Fact,
                text: "Run tests with cargo test --workspace".into(),
                source: "agent".into(),
                created_at: 110,
                updated_at: 210,
                approved: false,
            },
            MemoryEntry {
                id: 3,
                project_root: "/home/user/projects/twitty".into(),
                kind: MemoryKind::Fact,
                text: "الشبكة تبقى منطقية والرسم ثنائي الاتجاه".into(),
                source: "user".into(),
                created_at: 120,
                updated_at: 220,
                approved: true,
            },
            MemoryEntry {
                id: 4,
                project_root: "/home/user/projects/twitty".into(),
                kind: MemoryKind::Command,
                text: "`cargo test --workspace` (12 runs, 92% ok)".into(),
                source: AUTO_COMMAND_SOURCE.into(),
                created_at: 130,
                updated_at: 230,
                approved: true,
            },
            MemoryEntry {
                id: 5,
                project_root: "/home/user/projects/twitty".into(),
                kind: MemoryKind::Env,
                text: "Rust toolchain: 1.90.0 · Package managers: cargo".into(),
                source: AUTO_ENV_SOURCE.into(),
                created_at: 140,
                updated_at: 240,
                approved: true,
            },
        ],
        frequent_commands: vec![FrequentCommand {
            command: "cargo test --workspace".into(),
            runs: 12,
            successes: 11,
            known_exits: 12,
            success_rate_pct: Some(92),
            last_run_at: 230,
        }],
    }
}

fn sample_memory_panel(language: UiLanguage) -> MemoryPanelState {
    let mut state = MemoryPanelState::new(
        Some("/home/user/projects/twitty".into()),
        vec!["~/secrets".into(), "**/private/**".into()],
        language,
    );
    state.apply_snapshot(sample_snapshot());
    state
}

#[test]
fn palette_command_and_settings_ai_entry_point_open_memory() {
    let cmd = commands::all()
        .into_iter()
        .find(|c| c.name == "memory.open")
        .expect("memory.open command must exist");
    assert_eq!(cmd.title_en, "Project memory");
    assert_eq!(cmd.title_ar, "ذاكرة المشروع");
    assert_eq!(
        commands::action("memory.open"),
        Some(InputAction::ProjectMemory)
    );

    let entries = PaletteEntry::builtins();
    for query in ["memory", "project memory", "ذاكرة المشروع", "ذاكره"] {
        let hits = matches(&entries, query);
        assert!(
            hits.iter()
                .any(|&i| entries[i].id == PaletteId::Builtin("memory.open")),
            "palette query {query:?} must find memory.open"
        );
    }

    // Settings -> AI -> "Project memory" opens the memory screen via keyboard and mouse.
    let mut cfg = TwittyConfig::default();
    let mut settings = SettingsPanelState::new(
        vec!["Fira Code".into()],
        vec!["Inter".into()],
        None,
        UiLanguage::English,
    );
    settings.select_section(SettingsSection::Ai, &cfg);
    assert_eq!(settings.focused_setting(), Some(SettingId::ProjectMemory));

    let kb = settings.on_key(
        &Key::Named(NamedKey::Enter),
        None,
        ModifiersState::empty(),
        &mut cfg,
        SURFACE,
        1.0,
    );
    assert!(kb.open_memory);

    let layout = SettingsLayout::build(SURFACE, 1.0, &settings, &cfg);
    let btn = match &layout.rows[0].geometry {
        ControlGeometry::ActionButton { button } => *button,
        other => panic!("expected ActionButton, got {other:?}"),
    };
    let (bx, by) = center(btn);
    assert_eq!(layout.hit(bx, by), SettingsHit::OpenMemory);
    let clicked = settings.on_mouse_down(bx, by, &mut cfg, SURFACE, 1.0);
    assert!(clicked.open_memory);
}

#[test]
fn remember_fact_and_excluded_history_recording() {
    let dir = TempDir::new("facts");
    let repo = dir.path().join("app");
    let secret_repo = dir.path().join("secret-client");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(secret_repo.join(".git")).unwrap();

    let store = StoreHandle::open_in_memory();

    // Empty / missing root ignored.
    assert!(!remember_fact(&store, None, "anything"));
    assert!(!remember_fact(&store, Some(&repo), "   "));

    // Valid fact is stored already approved with redaction applied.
    let fact_text = format!(
        "Run integration tests with {}",
        concat!("sk-", "proj-abcdefghijklmnopqrstuvwxyz0123456789")
    );
    assert!(remember_fact(&store, Some(&repo), &fact_text));

    // Excluded folder is NOT recorded.
    let excludes = vec!["*secret-client*".to_owned()];
    assert!(!remember_fact_with_excludes(
        &store,
        Some(&secret_repo),
        "Should never be saved",
        &excludes,
    ));

    store.flush();
    let saved = store.list_memory(repo.to_string_lossy(), true, 10).unwrap();
    assert_eq!(saved.len(), 1);
    assert!(saved[0].approved);
    assert_eq!(saved[0].kind, MemoryKind::Fact);
    assert_eq!(saved[0].source, "agent");
    assert!(!saved[0].text.contains("abcdefghijklmnopqrstuvwxyz"));
    assert!(saved[0].text.contains("[REDACTED:"));

    assert!(store
        .list_memory(secret_repo.to_string_lossy(), false, 10)
        .unwrap()
        .is_empty());

    // Finished block inside excluded folder is not recorded in history.
    let mut term = Terminal::new(80, 24, |_| {});
    let mut blocks = BlockList::new();
    let t0 = Instant::now();
    term.process_bytes(b"\x1b]133;A\x07$ \x1b]133;B\x07cargo test\r\n\x1b]133;C\x07ok\r\n");
    for ev in term.take_shell_events() {
        blocks.on_event(&ev, &term, t0);
    }
    term.process_bytes(b"\x1b]133;D;0\x07");
    for ev in term.take_shell_events() {
        blocks.on_event(&ev, &term, t0 + Duration::from_millis(50));
    }
    let finished: &Block = blocks.blocks().next_back().unwrap();
    assert!(!record_finished_block_with_excludes(
        &store,
        true,
        &excludes,
        finished,
        Some(&secret_repo),
        true,
        Some("bash"),
    ));
    store.flush();
    assert!(store.recent_history(None, 10).unwrap().is_empty());
}

#[test]
fn memory_panel_interactions_rules_facts_edit_delete_exclusions_and_forget() {
    let mut state = sample_memory_panel(UiLanguage::English);
    let none = ModifiersState::empty();

    // 1. Rules tab shows changed attention dot and counts.
    assert_eq!(state.tab, MemoryTab::Rules);
    assert!(state.tab_has_attention(MemoryTab::Rules));
    assert!(state.tab_has_attention(MemoryTab::Facts));
    assert_eq!(state.tab_count(MemoryTab::Rules), 4);

    // Click Re-approve on CLAUDE.md (index 1, Changed) and Revoke on AGENTS.md (index 0, Approved).
    let layout = MemoryLayout::build(SURFACE, 1.0, &state);
    let (ax, ay) = center(layout.rules_rows[1].approve_btn.unwrap());
    assert_eq!(layout.hit(ax, ay), MemoryHit::RulesApprove(1));
    let out = state.on_mouse_down(ax, ay, SURFACE, 1.0);
    assert_eq!(
        out.actions,
        vec![MemoryAction::ApproveRulesFile {
            rel_path: "CLAUDE.md".into(),
            sha256: "ff00ee11dd22cc33bb44aa5599887766".into(),
        }]
    );
    assert_eq!(state.rules_files[1].state, RulesFileState::Approved);

    let layout = MemoryLayout::build(SURFACE, 1.0, &state);
    let (rx, ry) = center(layout.rules_rows[0].revoke_btn.unwrap());
    let out = state.on_mouse_down(rx, ry, SURFACE, 1.0);
    assert_eq!(
        out.actions,
        vec![MemoryAction::RevokeRulesFile {
            rel_path: "AGENTS.md".into(),
        }]
    );
    assert_eq!(state.rules_files[0].state, RulesFileState::Unapproved);

    // 2. Add a new rule with Arabic IME text.
    let layout = MemoryLayout::build(SURFACE, 1.0, &state);
    let (ix, iy) = center(layout.add_input.unwrap());
    state.on_mouse_down(ix, iy, SURFACE, 1.0);
    assert_eq!(state.input_target, Some(MemoryInputTarget::AddBar));
    state.on_text("احفظ كل النصوص بالعربية والإنجليزية");
    let out = state.on_key(&Key::Named(NamedKey::Enter), None, none, SURFACE, 1.0);
    assert_eq!(
        out.actions,
        vec![MemoryAction::AddMemory {
            kind: MemoryKind::Rule,
            text: "احفظ كل النصوص بالعربية والإنجليزية".into(),
        }]
    );

    // 3. Switch to Facts tab -> pending fact #2 appears first, click Approve, then Edit & Delete.
    state.select_tab(MemoryTab::Facts);
    let facts = state.entries_for_tab(MemoryTab::Facts);
    assert_eq!(facts[0].id, 2);
    assert!(!facts[0].approved);

    let layout = MemoryLayout::build(SURFACE, 1.0, &state);
    let (apx, apy) = center(layout.entry_rows[0].approve_btn.unwrap());
    let out = state.on_mouse_down(apx, apy, SURFACE, 1.0);
    assert_eq!(out.actions, vec![MemoryAction::ApproveMemory { id: 2 }]);
    assert!(!state.tab_has_attention(MemoryTab::Facts));

    // Inline Edit on fact #2.
    let layout = MemoryLayout::build(SURFACE, 1.0, &state);
    let row_2 = layout.entry_rows.iter().find(|r| r.id == 2).unwrap();
    let (ex, ey) = center(row_2.edit_btn.unwrap());
    state.on_mouse_down(ex, ey, SURFACE, 1.0);
    assert_eq!(state.input_target, Some(MemoryInputTarget::EditEntry(2)));
    state.on_text(" --all-targets");
    let out = state.on_key(&Key::Named(NamedKey::Enter), None, none, SURFACE, 1.0);
    assert_eq!(
        out.actions,
        vec![MemoryAction::UpdateMemory {
            id: 2,
            text: "Run tests with cargo test --workspace --all-targets".into(),
        }]
    );

    // Delete fact #2.
    let layout = MemoryLayout::build(SURFACE, 1.0, &state);
    let row_2 = layout.entry_rows.iter().find(|r| r.id == 2).unwrap();
    let (dx, dy) = center(row_2.delete_btn.unwrap());
    let out = state.on_mouse_down(dx, dy, SURFACE, 1.0);
    assert_eq!(out.actions, vec![MemoryAction::DeleteMemory { id: 2 }]);

    // 4. Exclusions tab: add pattern and remove existing pattern.
    state.select_tab(MemoryTab::Exclusions);
    let layout = MemoryLayout::build(SURFACE, 1.0, &state);
    let (ix, iy) = center(layout.add_input.unwrap());
    state.on_mouse_down(ix, iy, SURFACE, 1.0);
    state.on_text("/work/confidential/*");
    let out = state.on_key(&Key::Named(NamedKey::Enter), None, none, SURFACE, 1.0);
    assert_eq!(
        out.actions,
        vec![MemoryAction::AddExclusion {
            pattern: "/work/confidential/*".into(),
        }]
    );
    assert_eq!(state.exclusions.len(), 3);

    let layout = MemoryLayout::build(SURFACE, 1.0, &state);
    let (rmx, rmy) = center(layout.exclusion_rows[0].remove_btn);
    let out = state.on_mouse_down(rmx, rmy, SURFACE, 1.0);
    assert_eq!(
        out.actions,
        vec![MemoryAction::RemoveExclusion { index: 0 }]
    );
    assert_eq!(state.exclusions.len(), 2);

    // 5. Two-step "Forget this project" confirmation.
    let layout = MemoryLayout::build(SURFACE, 1.0, &state);
    let (fx, fy) = center(layout.forget_button.unwrap());
    let first = state.on_mouse_down(fx, fy, SURFACE, 1.0);
    assert!(first.actions.is_empty());
    assert!(state.confirm_forget);
    // Escape cancels confirmation without closing.
    let esc = state.on_key(&Key::Named(NamedKey::Escape), None, none, SURFACE, 1.0);
    assert!(!esc.close);
    assert!(!state.confirm_forget);
    // Double click confirms and emits ForgetProject.
    state.on_mouse_down(fx, fy, SURFACE, 1.0);
    let second = state.on_mouse_down(fx, fy, SURFACE, 1.0);
    assert_eq!(second.actions, vec![MemoryAction::ForgetProject]);
    assert!(state.entries.is_empty());
}

#[test]
fn arabic_rtl_mirroring_scaling_and_overlay_text_clipping() {
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

    for scale in [1.0_f32, 1.5_f32] {
        let surface = (SURFACE.0 * scale, SURFACE.1 * scale);
        let en = sample_memory_panel(UiLanguage::English);
        let ar = sample_memory_panel(UiLanguage::Arabic);
        let le = MemoryLayout::build(surface, scale, &en);
        let la = MemoryLayout::build(surface, scale, &ar);

        assert!(!le.mirrored && la.mirrored);
        // Sidebar on left in English, on right in Arabic.
        assert!(le.sidebar.x < le.viewport.x);
        assert!(la.sidebar.x > la.viewport.x);
        // Close button on right in English, on left in Arabic.
        assert!(le.close_button.x > le.header_title.x);
        assert!(la.close_button.x < la.header_title.x);

        // Hit-testing every sidebar tab matches in both languages and scales.
        for (tab, rect) in &la.tabs {
            let (tx, ty) = center(*rect);
            assert_eq!(la.hit(tx, ty), MemoryHit::Tab(*tab));
        }
    }

    // Headless overlay render clips underlying terminal text in Dark/Light × EN/AR.
    for (theme, lang) in [
        ("twitty-dark", UiLanguage::English),
        ("twitty-light", UiLanguage::Arabic),
    ] {
        let palette = Palette::named(theme).unwrap();
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
            surface: SURFACE,
        };
        let mut term = Terminal::new(90, 28, |_| {});
        for i in 0..28 {
            term.process_bytes(format!("سطر عربي {i} terminal output row {i}\r\n").as_bytes());
        }
        let (lines, cursor) = term.snapshot();
        let panes = vec![PaneFrame {
            id: PaneId(1),
            rect: Rect::new(0.0, 0.0, SURFACE.0, SURFACE.1),
            lines,
            cursor,
            focused: true,
            status: None,
            blocks: Vec::new(),
        }];
        let mut cache = RowCache::default();
        let mut ime = PreeditState::default();
        let mut frame = build_frame(&mut ctx, &opts, &mut cache, &mut ime, &[], &panes, &[]);
        let style = UiStyle::new(&palette, None, 1.0);

        let state = sample_memory_panel(lang);
        let panel = MemoryLayout::build(SURFACE, 1.0, &state).panel;
        frame.add_memory(&mut ctx, style, &font, &MemoryView { state });

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
        let mut inside_count = 0usize;
        for run in &runs {
            let b = run.bounds;
            let inside = (b.left as f32) >= panel.x - 1.0
                && (b.right as f32) <= panel.right() + 1.0
                && (b.top as f32) >= panel.y - 1.0
                && (b.bottom as f32) <= panel.bottom() + 1.0;
            if inside {
                inside_count += 1;
            } else {
                let overlaps = (b.left as f32) < panel.right()
                    && (b.right as f32) > panel.x
                    && (b.top as f32) < panel.bottom()
                    && (b.bottom as f32) > panel.y;
                assert!(
                    !overlaps,
                    "underlying pane text leaked into memory panel: {b:?} vs {panel:?}"
                );
            }
        }
        assert!(
            inside_count > 10,
            "expected memory screen labels inside panel"
        );
    }

    // Config `memory_exclude` round-trips cleanly through save/load.
    let dir = TempDir::new("cfg");
    let path = dir.path().join("config.json");
    let mut cfg = TwittyConfig::default();
    cfg.memory_exclude = vec!["~/secrets".into(), "**/private/**".into()];
    cfg.save_to(&path).unwrap();
    let loaded = TwittyConfig::load_from(&path);
    assert_eq!(loaded.memory_exclude, cfg.memory_exclude);
}
