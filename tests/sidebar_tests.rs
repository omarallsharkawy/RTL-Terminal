//! Sidebar behaviour is exercised without a window, GPU, shell, or git process.
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use alacritty_terminal::index::Side;
use cosmic_text::FontSystem;
use twitty::color::Palette;
use twitty::config::TwittyConfig;
use twitty::layout::{
    dividers, layout, ratio_at, set_ratio, sidebar_layout, Node, PaneId, Rect, SidebarPosition,
    SplitDir,
};
use twitty::notify::RateLimiter;
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::frame::{build_frame, FrameOptions};
use twitty::render::scene::preedit::PreeditState;
use twitty::render::scene::sidebar::{attention_quads, build, LabelCache};
use twitty::render::{CellMetrics, PaneFrame, RowCache, TabLabel};
use twitty::sidebar::git::{branch, BranchCache};
use twitty::sidebar::{display_path, entry_title, Entry, Notice, Placement, ScrollState};
use twitty::terminal::Terminal;
use twitty::ui::hit::{hit_test, Hit, HitGeometry};
use unicode_segmentation::UnicodeSegmentation;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "twitty-sidebar-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn write(&self, name: &str, text: &str) {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn entry() -> Entry {
    Entry::new("shell", None, false, None, [])
}
fn metrics() -> CellMetrics {
    CellMetrics {
        font_size: 14.0,
        scale: 1.0,
        char_width: 9.0,
        line_height: 22.0,
        padding_left: 8.0,
        padding_top: 6.0,
    }
}

#[test]
fn entry_keeps_focused_state_and_counts_other_panes_notifications() {
    let e = Entry::new(
        "  محرر\u{202e}\n ",
        Some(Path::new("/home/omar/مشاريع/طرفية")),
        true,
        Some(17),
        [
            Notice::new(Some("جاهز\u{202e}"), "تم العمل\n"),
            Notice::new(None, "waiting"),
        ],
    );
    assert_eq!(e.title, "محرر");
    assert!(e.running);
    assert_eq!(e.last_exit, Some(17));
    assert_eq!(e.attention_count, 2);
    assert_eq!(e.notification.as_ref().unwrap().label(), "جاهز: تم العمل");
    assert_eq!(e.height(20.0), 90.0);
    assert_eq!(entry().height(20.0), 70.0);
    assert_eq!(Notice::new(Some("\u{202e}"), "body").label(), "body");
    assert_eq!(Notice::new(Some("title"), "").label(), "title");
    let bounded = Entry::new(&"x".repeat(1000), None, false, None, []);
    assert_eq!(bounded.title.len(), 256);
    assert_eq!(
        Notice::new(None, &"س".repeat(900)).body.chars().count(),
        512
    );
}

#[test]
fn sidebar_title_prefers_the_program_title_and_falls_back_to_the_shell_name() {
    assert_eq!(entry_title(Some("محرر"), Some("bash"), 0), "محرر");
    assert_eq!(
        entry_title(Some(" \u{202e}\u{200f}"), Some("bash"), 0),
        "bash"
    );
    assert_eq!(entry_title(None, None, 2), "Tab 3");
}

#[test]
fn paths_abbreviate_home_by_component_and_clean_untrusted_text() {
    let home = Some(Path::new("/home/omar"));
    assert_eq!(display_path(Path::new("/home/omar"), home), "~");
    assert_eq!(
        display_path(Path::new("/home/omar/مشاريع"), home),
        "~/مشاريع"
    );
    assert_eq!(
        display_path(Path::new("/home/omar-other"), home),
        "/home/omar-other"
    );
    assert_eq!(
        display_path(Path::new("/home/omar/long/path/طرفية"), home),
        "~/long/path/طرفية"
    );
    assert_eq!(
        display_path(Path::new("/tmp/\u{202e}safe\n"), None),
        "/tmp/safe"
    );
}

#[test]
fn visible_sidebar_replaces_bar_and_scales_on_either_side() {
    let surface = Rect::new(0.0, 0.0, 1600.0, 1000.0);
    let old = Rect::new(0.0, 44.0, 1600.0, 956.0);
    let (right, side) = sidebar_layout(surface, old, SidebarPosition::Right, 220.0, 2.0);
    assert_eq!(right, Rect::new(0.0, 0.0, 1160.0, 1000.0));
    assert_eq!(side, Some(Rect::new(1160.0, 0.0, 440.0, 1000.0)));
    let (left, side) = sidebar_layout(surface, old, SidebarPosition::Left, 220.0, 2.0);
    assert_eq!(left, Rect::new(440.0, 0.0, 1160.0, 1000.0));
    assert_eq!(side, Some(Rect::new(0.0, 0.0, 440.0, 1000.0)));
    assert_eq!(
        sidebar_layout(surface, old, SidebarPosition::Hidden, 220.0, 2.0),
        (old, None)
    );
    let tiny = Rect::new(0.0, 0.0, 100.0, 50.0);
    let (pane, side) = sidebar_layout(tiny, tiny, SidebarPosition::Right, 480.0, 1.0);
    assert_eq!(pane.w, 80.0);
    assert_eq!(side.unwrap().w, 20.0);
    let narrower = Rect::new(0.0, 0.0, 79.0, 50.0);
    let narrower_legacy = Rect::new(0.0, 22.0, 79.0, 28.0);
    assert_eq!(
        sidebar_layout(
            narrower,
            narrower_legacy,
            SidebarPosition::Right,
            220.0,
            1.0
        ),
        (narrower_legacy, None)
    );
}

#[test]
fn split_dividers_cells_and_hits_share_the_reduced_area() {
    let m = metrics();
    let surface = Rect::new(0.0, 0.0, 1000.0, 600.0);
    for position in [SidebarPosition::Left, SidebarPosition::Right] {
        let (content, sidebar) = sidebar_layout(surface, surface, position, 220.0, 1.0);
        let mut root = Node::Leaf(PaneId(1));
        root.split(PaneId(1), PaneId(2), SplitDir::Horizontal);
        let panes = layout(&root, content, 2.0);
        let divider = dividers(&root, content, 2.0).remove(0);
        let g = HitGeometry {
            tab_bar_height: 0.0,
            tabs: vec![],
            content,
            gap: 2.0,
            slop: 3.0,
        };
        assert_eq!(
            hit_test(&g, Some(&root), (panes[0].1.x + 20.0) as f64, 20.0),
            Hit::Pane(PaneId(1))
        );
        assert!(matches!(
            hit_test(&g, Some(&root), divider.rect.x as f64, 20.0),
            Hit::Divider(_)
        ));
        let ratio = ratio_at(
            &divider,
            (content.x + (content.w - 2.0) * 0.25 + 1.0) as f64,
            2.0,
        );
        assert_eq!(ratio, 0.25);
        assert!(set_ratio(&mut root, &divider.path, ratio));
        assert!(dividers(&root, content, 2.0)[0].rect.x < divider.rect.x);
        let sidebar = sidebar.unwrap();
        assert_eq!(
            hit_test(&g, Some(&root), (sidebar.x + sidebar.w / 2.0) as f64, 20.0),
            Hit::Nothing
        );
        for (_, rect) in panes {
            assert!(rect.x >= content.x && rect.right() <= content.right());
            let x = rect.x + m.padding_left + 2.5 * m.char_width;
            let y = rect.y + m.padding_top + 1.5 * m.line_height;
            assert_eq!(m.pixel_to_cell(rect, x as f64, y as f64), (2, 1));
        }
    }
}

#[test]
fn a_stationary_pointer_uses_the_current_pane_origin_for_links_and_mouse_reports() {
    let m = metrics();
    let surface = Rect::new(0.0, 0.0, 600.0, 200.0);
    let id = PaneId(1);
    let cache = RowCache::default();
    for side in [SidebarPosition::Left, SidebarPosition::Right] {
        let (pane, sidebar) = sidebar_layout(surface, surface, side, 220.0, 1.0);
        let sidebar = sidebar.unwrap();
        let (cols, rows) = m.grid_size(pane);
        let mut terminal = Terminal::new(cols, rows, |_| {});
        terminal.process_bytes(
            b"\x1b[?1000h\x1b[?1006h\x1b[2;4H\x1b]8;;https://example.com\x07link\x1b]8;;\x07",
        );
        let x = f64::from(pane.x + m.padding_left + 3.5 * m.char_width);
        let y = f64::from(pane.y + m.padding_top + 1.5 * m.line_height);
        let (col, row, _) = cache.cell_at(id, m, pane, x, y);
        assert_eq!((col, row), (3, 1));
        assert_eq!(
            terminal.link_at(col, row).unwrap().uri,
            "https://example.com"
        );
        assert_eq!(
            twitty::input::encode_mouse_for_mode(
                64,
                col,
                row,
                true,
                winit::keyboard::ModifiersState::empty(),
                terminal.mode()
            ),
            b"\x1b[<64;4;2M"
        );
        let x = f64::from(sidebar.x + sidebar.w / 2.0);
        let edge = if side == SidebarPosition::Left {
            (0, 1, Side::Left)
        } else {
            (cols - 1, 1, Side::Right)
        };
        assert_eq!(cache.cell_at(id, m, pane, x, y), edge);
    }
}

#[test]
fn scrolling_clamps_hits_and_reveals_keyboard_selection() {
    let entries = vec![entry(); 8];
    let area = Rect::new(600.0, 0.0, 200.0, 100.0);
    let mut scroll = ScrollState::default();
    scroll.update(&entries, 0, area, 20.0);
    let v = scroll.view(&entries, 0, area, 20.0, SidebarPosition::Right);
    assert!(v.right_aligned);
    assert!(v.items[0].active);
    assert_eq!(v.hit_test(610.0, 69.0), Some(0));
    assert_eq!(v.pointer_hit(610.0, 69.0), Some(Hit::Tab(0)));
    assert_eq!(v.hit_test(610.0, 70.0), Some(1));
    assert_eq!(v.hit_test(599.0, 20.0), None);
    assert_eq!(v.hit_test(800.0, 20.0), None);
    scroll.scroll(85.0, &entries, area, 20.0);
    let v = scroll.view(&entries, 0, area, 20.0, SidebarPosition::Right);
    assert_eq!(v.hit_test(610.0, 0.0), Some(1));
    assert_eq!(v.hit_test(610.0, 55.0), Some(2));
    assert_eq!(v.hit_test(610.0, 100.0), None);
    scroll.scroll(10000.0, &entries, area, 20.0);
    let v = scroll.view(&entries, 0, area, 20.0, SidebarPosition::Right);
    assert_eq!(v.hit_test(610.0, 99.0), Some(7));
    scroll.update(&entries, 2, area, 20.0);
    assert_eq!(
        scroll
            .view(&entries, 2, area, 20.0, SidebarPosition::Left)
            .hit_test(610.0, 0.0),
        Some(2)
    );
    // Closing tabs and shrinking the list cannot strand the scroll offset.
    scroll.update(&entries[..1], 0, area, 20.0);
    let v = scroll.view(&entries[..1], 0, area, 20.0, SidebarPosition::Left);
    assert_eq!(v.hit_test(610.0, 0.0), Some(0));
    assert_eq!(v.hit_test(610.0, 90.0), None);
    assert_eq!(v.pointer_hit(610.0, 90.0), Some(Hit::TabBar));
    assert_eq!(v.pointer_hit(800.0, 90.0), None);
    assert!(!v.right_aligned);
}

#[test]
fn notification_rows_participate_in_scrolling_and_hit_testing() {
    let e = Entry::new("agent", None, false, Some(0), [Notice::new(None, "done")]);
    let entries = [e, entry()];
    let area = Rect::new(0.0, 0.0, 200.0, 200.0);
    let v = ScrollState::default().view(&entries, 0, area, 20.0, SidebarPosition::Left);
    assert_eq!(v.hit_test(10.0, 89.0), Some(0));
    assert_eq!(v.hit_test(10.0, 90.0), Some(1));
    assert_eq!(v.hit_test(10.0, 170.0), None);
}

#[test]
fn git_finds_parent_repo_named_branches_and_detached_head() {
    let tmp = Scratch::new();
    tmp.write("repo/.git/HEAD", "ref: refs/heads/feature/طرفية\n");
    let cwd = tmp.0.join("repo/a/b");
    std::fs::create_dir_all(&cwd).unwrap();
    assert_eq!(branch(&cwd).as_deref(), Some("feature/طرفية"));
    tmp.write(
        "repo/.git/HEAD",
        "0123456789abcdef0123456789abcdef01234567\n",
    );
    assert_eq!(branch(&cwd).as_deref(), Some("0123456"));
    tmp.write("repo/.git/HEAD", "ref: refs/tags/nope\n");
    assert_eq!(branch(&cwd), None);
    assert_eq!(branch(Path::new("relative")), None);
}

#[test]
fn gitdir_files_follow_relative_and_absolute_worktree_or_submodule_paths() {
    let tmp = Scratch::new();
    tmp.write("repo/.git/HEAD", "ref: refs/heads/main\n");
    tmp.write("repo/.git/modules/sub/HEAD", "ref: refs/heads/submodule\n");
    tmp.write("repo/sub/.git", "gitdir: ../.git/modules/sub\n");
    assert_eq!(
        branch(&tmp.0.join("repo/sub")).as_deref(),
        Some("submodule")
    );
    tmp.write("repo/.git/worktrees/agent/HEAD", "ref: refs/heads/agent\n");
    tmp.write(
        "worktree/.git",
        &format!(
            "gitdir: {}\n",
            tmp.0.join("repo/.git/worktrees/agent").display()
        ),
    );
    assert_eq!(branch(&tmp.0.join("worktree")).as_deref(), Some("agent"));
    tmp.write("worktree/.git", "gitdir:\n");
    assert_eq!(branch(&tmp.0.join("worktree")), None);
}

#[test]
fn git_cache_refreshes_at_two_seconds_and_when_directory_changes() {
    let tmp = Scratch::new();
    let now = Instant::now();
    tmp.write("one/.git/HEAD", "ref: refs/heads/first\n");
    tmp.write("two/.git/HEAD", "ref: refs/heads/second\n");
    let one = tmp.0.join("one");
    let two = tmp.0.join("two");
    let mut cache = BranchCache::default();
    assert_eq!(cache.get(&one, now).as_deref(), Some("first"));
    tmp.write("one/.git/HEAD", "ref: refs/heads/changed\n");
    assert_eq!(
        cache
            .get(&one, now + Duration::from_millis(1999))
            .as_deref(),
        Some("first")
    );
    assert_eq!(cache.get(&two, now).as_deref(), Some("second"));
    assert_eq!(
        cache.get(&one, now + Duration::from_secs(2)).as_deref(),
        Some("changed")
    );
    cache.retain(&[two].into_iter().collect());
    tmp.write("one/.git/HEAD", "ref: refs/heads/uncached\n");
    assert_eq!(
        cache.get(&one, now + Duration::from_secs(2)).as_deref(),
        Some("uncached")
    );
    let none = tmp.0.join("new");
    assert_eq!(cache.get(&none, now), None);
    tmp.write("new/.git/HEAD", "ref: refs/heads/new\n");
    assert_eq!(cache.get(&none, now + Duration::from_secs(1)), None);
    assert_eq!(
        cache.get(&none, now + Duration::from_secs(2)).as_deref(),
        Some("new")
    );
}

#[test]
fn git_rejects_large_malformed_files_and_caps_parent_walk() {
    let tmp = Scratch::new();
    tmp.write("repo/.git/HEAD", "ref: refs/heads/root\n");
    let mut deep = tmp.0.join("repo");
    for _ in 0..63 {
        deep.push("d");
    }
    std::fs::create_dir_all(&deep).unwrap();
    assert_eq!(branch(&deep).as_deref(), Some("root"));
    deep.push("d");
    std::fs::create_dir(&deep).unwrap();
    assert_eq!(branch(&deep), None);
    tmp.write("repo/.git/HEAD", &"a".repeat(4097));
    assert_eq!(branch(&tmp.0.join("repo")), None);
    tmp.write("repo/.git/HEAD", "not a hash");
    assert_eq!(branch(&tmp.0.join("repo")), None);
    tmp.write("repo/.git/HEAD", "deadbee");
    assert_eq!(branch(&tmp.0.join("repo")), None);
    tmp.write("repo/.git/HEAD", &"a".repeat(64));
    assert_eq!(branch(&tmp.0.join("repo")).as_deref(), Some("aaaaaaa"));
    tmp.write("other/.git", &format!("gitdir: {}", "x".repeat(4097)));
    assert_eq!(branch(&tmp.0.join("other")), None);
}

#[cfg(unix)]
#[test]
fn git_walk_handles_symlink_loops_and_rejects_nonregular_head() {
    use std::os::unix::fs::symlink;

    let tmp = Scratch::new();
    tmp.write("repo/.git/HEAD", "ref: refs/heads/main\n");
    std::fs::create_dir(tmp.0.join("repo/sub")).unwrap();
    symlink(".git", tmp.0.join("repo/sub/.git")).unwrap();
    assert_eq!(branch(&tmp.0.join("repo/sub")), None);
    symlink("loop", tmp.0.join("loop")).unwrap();
    assert_eq!(branch(&tmp.0.join("loop/nested")), None);
    symlink("repo/sub", tmp.0.join("shortcut")).unwrap();
    std::fs::remove_file(tmp.0.join("repo/sub/.git")).unwrap();
    assert_eq!(branch(&tmp.0.join("shortcut")).as_deref(), Some("main"));
    assert_eq!(branch(&tmp.0.join("repo/sub/../..")), branch(&tmp.0));
    std::fs::remove_file(tmp.0.join("repo/.git/HEAD")).unwrap();
    symlink("HEAD", tmp.0.join("repo/.git/HEAD")).unwrap();
    assert_eq!(branch(&tmp.0.join("repo")), None);
    std::fs::remove_file(tmp.0.join("repo/.git/HEAD")).unwrap();
    std::fs::create_dir(tmp.0.join("repo/.git/HEAD")).unwrap();
    assert_eq!(branch(&tmp.0.join("repo")), None);
}

#[test]
fn desktop_limit_is_per_pane_and_resets_at_three_seconds() {
    let mut limit = RateLimiter::default();
    let now = Instant::now();
    assert!(limit.allow(PaneId(1), now));
    assert!(!limit.allow(PaneId(1), now + Duration::from_millis(2999)));
    assert!(limit.allow(PaneId(2), now));
    assert!(limit.allow(PaneId(1), now + Duration::from_secs(3)));
    assert!(!limit.allow(PaneId(1), now + Duration::from_secs(4)));
    limit.forget(PaneId(1));
    assert!(limit.allow(PaneId(1), now + Duration::from_secs(4)));
}

#[test]
fn config_defaults_and_loading_clamp_sidebar_values() {
    let tmp = Scratch::new();
    assert_eq!(TwittyConfig::default().sidebar, "right");
    for (json, side, width) in [
        (r#"{"sidebar":"left","sidebar_width":20}"#, "left", 140.0),
        (
            r#"{"sidebar":"hidden","sidebar_width":999}"#,
            "hidden",
            480.0,
        ),
        (r#"{"sidebar":"invalid"}"#, "right", 220.0),
    ] {
        tmp.write("config.json", json);
        let c = TwittyConfig::load_from(&tmp.0.join("config.json"));
        assert_eq!(c.sidebar, side);
        assert_eq!(c.sidebar_width, width);
    }
    tmp.write("config.json", r#"{"sidebar":false,"sidebar_width":240}"#);
    let c = TwittyConfig::load_from(&tmp.0.join("config.json"));
    assert_eq!(c.sidebar, "right");
    assert_eq!(c.sidebar_width, 240.0);
    let warnings = twitty::config::take_warnings();
    assert!(warnings
        .iter()
        .any(|w| w.contains("invalid sidebar \"invalid\", using right")));
    assert!(warnings
        .iter()
        .any(|w| w.contains("invalid sidebar value, using right")));
    assert_eq!(twitty::layout::clamp_sidebar_width(f32::NAN), 220.0);
}

#[test]
fn toggling_restores_the_session_side_and_round_trips_through_config_save() {
    let tmp = Scratch::new();
    let path = tmp.0.join("config.json");
    tmp.write("config.json", r#"{"sidebar":"left","other_setting":42}"#);
    let mut config = TwittyConfig::load_from(&path);
    let mut placement = Placement::new(config.sidebar.parse().unwrap());
    config.sidebar = placement.toggle().as_str().into();
    config.save_to(&path).unwrap();
    let saved = TwittyConfig::load_from(&path);
    assert_eq!(saved.sidebar, "hidden");
    assert_eq!(saved.extra["other_setting"], 42);
    assert_eq!(
        Placement::new(saved.sidebar.parse().unwrap()).toggle(),
        SidebarPosition::Right
    );
    assert_eq!(placement.toggle(), SidebarPosition::Left);
    config.sidebar = placement.position.as_str().into();
    config.save_to(&path).unwrap();
    assert_eq!(TwittyConfig::load_from(&path).sidebar, "left");
}

#[test]
fn title_and_notice_input_caps_keep_whole_graphemes() {
    let title = format!("{}بِ", "x".repeat(255));
    assert_eq!(twitty::app::clean_title(&title), "x".repeat(255));
    assert_eq!(
        Entry::new(&title, None, false, None, []).title,
        "x".repeat(255)
    );
    let body = format!("{}\u{200e}بِ\u{2028}", "x".repeat(511));
    assert_eq!(Notice::new(None, &body).body, "x".repeat(511));
    assert_eq!(Notice::new(None, " ِ أ ").body, " ِ أ");
}

#[test]
fn arabic_labels_are_shaped_right_aligned_and_clipped_with_colored_states() {
    let mut fs = FontSystem::new();
    let palette = Palette::default();
    let mut ctx = DrawContext {
        font_system: &mut fs,
        family: None,
        palette: &palette,
        metrics: metrics(),
        is_srgb: false,
    };
    let mut e = Entry::new(
        "محرر",
        Some(Path::new("/tmp/مشاريع")),
        true,
        Some(7),
        [Notice::new(Some("جاهز"), "تم")],
    );
    e.branch = Some("ميزة".into());
    let area = Rect::new(500.0, 0.0, 400.0, 200.0);
    let view = ScrollState::default().view(&[e], 0, area, 22.0, SidebarPosition::Right);
    let scene = build(&mut ctx, &view, &mut LabelCache::default());
    let texts: Vec<_> = scene
        .labels
        .iter()
        .map(|l| l.buffer.lines[0].text())
        .collect();
    assert_eq!(
        texts,
        ["● 1 محرر", "جاهز: تم", "/tmp/مشاريع", "⎇ ميزة", "●", "✗ 7"]
    );
    for l in scene.labels.iter().take(3) {
        let width = l.buffer.layout_runs().map(|r| r.line_w).fold(0.0, f32::max);
        assert!((l.left + width - (area.right() - 8.0)).abs() < 0.1);
        assert!(l.buffer.layout_runs().any(|r| !r.glyphs.is_empty()));
        assert!(l.bounds.left >= area.x as i32 && l.bounds.right <= area.right() as i32);
    }
    // Full shaping preserves RTL glyph order for the plain Arabic notification.
    let glyphs: Vec<_> = scene.labels[1]
        .buffer
        .layout_runs()
        .flat_map(|r| r.glyphs.iter())
        .collect();
    let leftmost = glyphs.iter().min_by(|a, b| a.x.total_cmp(&b.x)).unwrap();
    let rightmost = glyphs.iter().max_by(|a, b| a.x.total_cmp(&b.x)).unwrap();
    assert!(leftmost.start > rightmost.start);
    assert!(glyphs.iter().any(|g| g.level.is_rtl()));
    let red = palette.ansi[1];
    assert_eq!(
        scene.labels.last().unwrap().color,
        glyphon::Color::rgb(
            (red.r * 255.0) as u8,
            (red.g * 255.0) as u8,
            (red.b * 255.0) as u8
        )
    );
    assert_eq!(scene.labels[1].top - scene.labels[0].top, 22.0);
    assert!(scene.quads.iter().any(|q| q.0 == area.x && q.2 == 2.0));
}

#[test]
fn pixel_fitting_keeps_arabic_graphemes_and_the_original_bidi_direction() {
    let mut fs = FontSystem::new();
    let palette = Palette::default();
    let mut ctx = DrawContext {
        font_system: &mut fs,
        family: None,
        palette: &palette,
        metrics: metrics(),
        is_srgb: false,
    };
    let title = "بِسْمِ ".repeat(30);
    let body = "سَلَامٌ ".repeat(30);
    let path = format!("/{}proj-12", "مَسَارٌ/".repeat(30));
    let path = Path::new(&path);
    let entries = [Entry::new(
        &title,
        Some(path),
        false,
        None,
        [Notice::new(None, &body)],
    )];
    let mut cache = LabelCache::default();
    for width in [95.0, 115.0, 145.0] {
        let area = Rect::new(400.0, 0.0, width, 200.0);
        let view = ScrollState::default().view(&entries, 0, area, 22.0, SidebarPosition::Right);
        let scene = build(&mut ctx, &view, &mut cache);
        let title = format!("● 1 {}", entries[0].title);
        let body = &entries[0].notification.as_ref().unwrap().body;
        let original = [title.as_str(), body, path.to_str().unwrap()];
        for (i, (label, original)) in scene.labels.iter().zip(original).enumerate() {
            let text = label.buffer.lines[0].text();
            let text = text.trim_start_matches('\u{200f}');
            let kept = if i == 2 {
                text.strip_prefix('…')
            } else {
                text.strip_suffix('…')
            }
            .unwrap();
            let cut = if i == 2 {
                assert!(original.ends_with(kept));
                assert!(kept.ends_with("proj-12"));
                original.len() - kept.len()
            } else {
                assert!(original.starts_with(kept));
                kept.len()
            };
            assert!(original
                .grapheme_indices(true)
                .any(|(offset, _)| offset == cut));
            let run = label.buffer.layout_runs().next().unwrap();
            assert!(run.rtl);
            assert!(run.line_w <= width - 16.0);
            assert!((label.left + run.line_w - (area.right() - 8.0)).abs() < 0.1);
            assert!(label.buffer.layout_runs().nth(1).is_none());
        }
    }
}

#[test]
fn label_cache_reuses_shaping_until_text_width_or_font_changes() {
    let mut fs = FontSystem::new();
    let palette = Palette::default();
    let mut ctx = DrawContext {
        font_system: &mut fs,
        family: None,
        palette: &palette,
        metrics: metrics(),
        is_srgb: false,
    };
    let entries = [Entry::new("محرر", None, false, None, [])];
    let mut cache = LabelCache::default();
    let mut view = ScrollState::default().view(
        &entries,
        0,
        Rect::new(0.0, 0.0, 220.0, 120.0),
        22.0,
        SidebarPosition::Left,
    );
    let first = build(&mut ctx, &view, &mut cache);
    view.items[0].active = false;
    let second = build(&mut ctx, &view, &mut cache);
    assert!(Rc::ptr_eq(
        &first.labels[0].buffer,
        &second.labels[0].buffer
    ));
    view.items[0].entry.title = "طرفية".into();
    let third = build(&mut ctx, &view, &mut cache);
    assert!(!Rc::ptr_eq(
        &second.labels[0].buffer,
        &third.labels[0].buffer
    ));
    view.items[0].rect.w = 180.0;
    let fourth = build(&mut ctx, &view, &mut cache);
    assert!(!Rc::ptr_eq(
        &third.labels[0].buffer,
        &fourth.labels[0].buffer
    ));
    ctx.metrics.font_size += 1.0;
    let fifth = build(&mut ctx, &view, &mut cache);
    assert!(!Rc::ptr_eq(
        &fourth.labels[0].buffer,
        &fifth.labels[0].buffer
    ));
}

#[test]
fn attention_ring_stays_inside_each_pane_and_disappears_without_attention() {
    let mut fs = FontSystem::new();
    let palette = Palette::default();
    let ctx = DrawContext {
        font_system: &mut fs,
        family: None,
        palette: &palette,
        metrics: metrics(),
        is_srgb: false,
    };
    let rects = [
        Rect::new(220.0, 0.0, 300.0, 500.0),
        Rect::new(0.0, 0.0, 1.0, 1.0),
    ];
    let quads = attention_quads(&ctx, &rects);
    assert_eq!(quads.len(), 8);
    for (r, qs) in rects.iter().zip(quads.chunks(4)) {
        for &(x, y, w, h, color) in qs {
            assert!(x >= r.x && y >= r.y && x + w <= r.right() && y + h <= r.bottom());
            assert_eq!(color, palette.ansi[4].to_array());
        }
    }
    assert!(attention_quads(&ctx, &[]).is_empty());
}

#[test]
fn ime_candidate_and_attention_follow_panes_on_both_sides() {
    let mut fs = FontSystem::new();
    let palette = Palette::default();
    let mut ctx = DrawContext {
        font_system: &mut fs,
        family: None,
        palette: &palette,
        metrics: metrics(),
        is_srgb: false,
    };
    let surface = Rect::new(0.0, 0.0, 500.0, 200.0);
    let options = FrameOptions {
        window_focused: true,
        cursor_style: "block",
        cursor_blink_on: true,
        surface: (surface.w, surface.h),
    };
    let mut cache = RowCache::default();
    let mut ime = PreeditState::default();
    for side in [SidebarPosition::Left, SidebarPosition::Right] {
        let (content, sidebar) = sidebar_layout(surface, surface, side, 220.0, 1.0);
        let sidebar = sidebar.unwrap();
        let (cols, rows) = ctx.metrics.grid_size(content);
        let mut terminal = Terminal::new(cols, rows, |_| {});
        terminal.process_bytes(format!("\x1b[1;{cols}H").as_bytes());
        let (lines, cursor) = terminal.snapshot();
        let panes = [PaneFrame {
            id: PaneId(1),
            rect: content,
            lines,
            cursor,
            focused: true,
            status: None,
        }];
        ime.set(None);
        let plain = build_frame(&mut ctx, &options, &mut cache, &mut ime, &[], &panes, &[]);
        let (x, _, _, _) = plain.ime_area.unwrap();
        assert_eq!(
            x as f32,
            content.x + ctx.metrics.padding_left + (cols - 1) as f32 * ctx.metrics.char_width
        );
        ime.set(Some(("تَحرير ".repeat(40), None)));
        let mut frame = build_frame(&mut ctx, &options, &mut cache, &mut ime, &[], &panes, &[]);
        let (x, y, w, h) = frame.ime_area.unwrap();
        assert!(content.contains(x as f64, y as f64));
        assert!(!sidebar.contains(x as f64, y as f64));
        assert!((x as f32 + w as f32) <= content.right());
        assert!((y as f32 + h as f32) <= content.bottom());
        let count = frame.quads.len();
        frame.add_attention(&ctx, &[content]);
        let ring = &frame.quads[count..];
        assert_eq!(ring.len(), 4);
        assert_eq!(ring[0].0, content.x);
        assert_eq!(ring[0].0 + ring[0].2, content.right());
    }
}

#[test]
fn headless_frame_keeps_legacy_chrome_until_sidebar_is_explicit() {
    let mut fs = FontSystem::new();
    let palette = Palette::default();
    let mut ctx = DrawContext {
        font_system: &mut fs,
        family: None,
        palette: &palette,
        metrics: metrics(),
        is_srgb: false,
    };
    let opts = FrameOptions {
        window_focused: true,
        cursor_style: "block",
        cursor_blink_on: true,
        surface: (800.0, 600.0),
    };
    let mut cache = RowCache::default();
    let mut ime = PreeditState::default();
    let tabs = [
        TabLabel {
            title: "one".into(),
            active: true,
        },
        TabLabel {
            title: "two".into(),
            active: false,
        },
    ];
    let old = build_frame(&mut ctx, &opts, &mut cache, &mut ime, &tabs, &[], &[]);
    assert_eq!(old.quads[0].2, 800.0);
    assert_eq!(old.quads[0].3, 22.0);
    let mut frame = build_frame(&mut ctx, &opts, &mut cache, &mut ime, &[], &[], &[]);
    let view = ScrollState::default().view(
        &[entry()],
        0,
        Rect::new(580.0, 0.0, 220.0, 600.0),
        22.0,
        SidebarPosition::Right,
    );
    frame.add_sidebar(&mut ctx, &view, &mut LabelCache::default());
    assert_eq!(frame.quads[0].0, 580.0);
    assert_eq!(frame.quads[0].2, 220.0);
    assert_eq!(frame.text_runs(&[], &cache, &ime).len(), 2);
}

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
#[test]
fn desktop_command_uses_literal_arguments_and_ends_option_parsing() {
    let command = twitty::notify::command(
        "--icon=error $(echo unsafe)\u{202e}",
        "جاهز; `echo literal`\n",
    );
    assert_eq!(command.get_program(), "notify-send");
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        [
            "--app-name=Twitty",
            "--",
            "--icon=error $(echo unsafe)",
            "جاهز; `echo literal`"
        ]
    );
}
