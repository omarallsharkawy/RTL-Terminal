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
use twitty::render::scene::window_chrome::ChromeView;
use twitty::render::text::ui::{label_width, UiFont, UiSize, UiWeight};
use twitty::render::ui_style::UiStyle;
use twitty::render::{CellMetrics, PaneFrame, RowCache, TabLabel};
use twitty::sidebar::cards::{fit_title, Card, CardLayout, Drag, ScrollState};
use twitty::sidebar::git::{branch, BranchCache};
use twitty::sidebar::{
    display_path, entry_title, AgentState, AgentStatus, Entry, Notice, Placement,
};
use twitty::terminal::Terminal;
use twitty::ui::chrome::{
    ChromeAction, PaneHeaderLayout, TopBarLayout, UiLanguage, WindowLayout, SEARCH_AR, SEARCH_EN,
};
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
    let (right, side) = sidebar_layout(surface, old, SidebarPosition::Right, 180.0, 2.0);
    assert_eq!(right, Rect::new(0.0, 0.0, 1240.0, 1000.0));
    assert_eq!(side, Some(Rect::new(1240.0, 0.0, 360.0, 1000.0)));
    let (left, side) = sidebar_layout(surface, old, SidebarPosition::Left, 180.0, 2.0);
    assert_eq!(left, Rect::new(360.0, 0.0, 1240.0, 1000.0));
    assert_eq!(side, Some(Rect::new(0.0, 0.0, 360.0, 1000.0)));
    assert_eq!(
        sidebar_layout(surface, old, SidebarPosition::Hidden, 180.0, 2.0),
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
            180.0,
            1.0
        ),
        (narrower_legacy, None)
    );
}

#[test]
fn sidebar_caps_at_quarter_width_preserves_minimums_and_leaves_room_to_split() {
    let cfg = TwittyConfig::default();
    assert_eq!(cfg.sidebar_width, 180.0);

    // 951 px wide at scale 1.5: sidebar is capped at <= 25% and leaves >= 48 terminal columns.
    let mut fonts = twitty::render::text::fonts::new_font_system();
    let scale = 1.5;
    let default_metrics = CellMetrics::measure(&mut fonts, None, cfg.font_size, scale);
    let narrow_window = WindowLayout::new(
        default_metrics,
        (951.0, 1002.0),
        2,
        (SidebarPosition::Right, cfg.sidebar_width),
        true,
    );
    let sidebar = narrow_window.sidebar.unwrap();
    assert!(
        sidebar.w <= 951.0 * 0.25,
        "sidebar width {} exceeds 25% of 951",
        sidebar.w
    );
    assert!(sidebar.w >= 140.0 * scale);
    let pane = PaneHeaderLayout::new(narrow_window.content, scale, true);
    let (cols, _) = default_metrics.grid_size(pane.grid);
    assert!(
        cols >= 48,
        "expected at least 48 columns for a horizontal split, got {cols}"
    );

    // Configured width acts as a maximum: a larger configured width still respects the 25% cap.
    let narrow = Rect::new(0.0, 0.0, 951.0, 1002.0);
    let (_, capped) = sidebar_layout(narrow, narrow, SidebarPosition::Right, 300.0, scale);
    assert_eq!(capped.unwrap().w, sidebar.w);

    // Wide window (2560 px at scale 1.0) uses the full 180 logical px default.
    let wide = Rect::new(0.0, 0.0, 2560.0, 1440.0);
    let (wide_pane, wide_side) =
        sidebar_layout(wide, wide, SidebarPosition::Right, cfg.sidebar_width, 1.0);
    assert_eq!(wide_side.unwrap().w, 180.0);
    assert_eq!(wide_pane.w, 2560.0 - 180.0);

    // When 25% of the surface is below 140 logical px, the 140 logical px minimum holds.
    let mid = Rect::new(0.0, 0.0, 500.0, 400.0);
    let (mid_pane, mid_side) =
        sidebar_layout(mid, mid, SidebarPosition::Left, cfg.sidebar_width, 1.0);
    assert_eq!(mid_side.unwrap().w, 140.0);
    assert_eq!(mid_pane.w, 360.0);

    let mid_hidpi = Rect::new(0.0, 0.0, 600.0, 400.0);
    let (mid_hidpi_pane, mid_hidpi_side) = sidebar_layout(
        mid_hidpi,
        mid_hidpi,
        SidebarPosition::Right,
        cfg.sidebar_width,
        scale,
    );
    assert_eq!(mid_hidpi_side.unwrap().w, 140.0 * scale);
    assert_eq!(mid_hidpi_pane.w, 600.0 - 140.0 * scale);

    // When even 140 logical px would squeeze the terminal below 80 logical px, the 80 px rule wins.
    let cramped = Rect::new(0.0, 0.0, 200.0, 400.0);
    let (cramped_pane, cramped_side) = sidebar_layout(
        cramped,
        cramped,
        SidebarPosition::Right,
        cfg.sidebar_width,
        1.0,
    );
    assert_eq!(cramped_pane.w, 80.0);
    assert_eq!(cramped_side.unwrap().w, 120.0);

    let cramped_hidpi = Rect::new(0.0, 0.0, 280.0, 400.0);
    let (cramped_hidpi_pane, cramped_hidpi_side) = sidebar_layout(
        cramped_hidpi,
        cramped_hidpi,
        SidebarPosition::Right,
        cfg.sidebar_width,
        scale,
    );
    assert_eq!(cramped_hidpi_pane.w, 80.0 * scale);
    assert_eq!(cramped_hidpi_side.unwrap().w, 280.0 - 80.0 * scale);
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

fn card(entry: Entry) -> Card {
    Card::new(entry)
}

fn center(r: Rect) -> (f64, f64) {
    (f64::from(r.x + r.w / 2.0), f64::from(r.y + r.h / 2.0))
}

#[test]
fn scrolling_clamps_hits_and_reveals_keyboard_selection() {
    let entries = vec![card(entry()); 8];
    let area = Rect::new(600.0, 0.0, 200.0, 100.0);
    let mut scroll = ScrollState::default();
    let view = |s: &mut ScrollState, active| {
        s.view(
            &entries,
            active,
            area,
            1.0,
            SidebarPosition::Right,
            UiLanguage::English,
        )
    };
    let first = view(&mut scroll, 0);
    assert!(first.mirrored && first.items[0].active);
    assert_eq!(first.hit_test(610.0, 43.0), Some(0));
    assert_eq!(first.pointer_hit(610.0, 43.0), Some(Hit::Tab(0)));
    assert_eq!(first.hit_test(610.0, 44.0), None);
    assert_eq!(first.hit_test(610.0, 48.0), Some(1));
    assert_eq!(first.hit_test(599.0, 20.0), None);
    assert_eq!(first.hit_test(800.0, 20.0), None);
    scroll.scroll(55.0, &first);
    let middle = view(&mut scroll, 0);
    assert_eq!(middle.hit_test(610.0, 0.0), Some(1));
    assert_eq!(middle.hit_test(610.0, 55.0), Some(2));
    assert_eq!(middle.hit_test(610.0, 100.0), None);
    scroll.scroll(10000.0, &middle);
    let last = view(&mut scroll, 0);
    assert_eq!(last.hit_test(610.0, 55.0), Some(7));
    let (x, y) = center(last.new_tab);
    assert_eq!(
        last.pointer_hit(x, y),
        Some(Hit::Chrome(ChromeAction::NewTab))
    );
    let selected = view(&mut scroll, 2);
    assert!(selected.items.iter().find(|i| i.index == 2).unwrap().active);

    // Closing tabs and shrinking the list cannot strand the scroll offset.
    let short = scroll.view(
        &entries[..1],
        0,
        area,
        1.0,
        SidebarPosition::Left,
        UiLanguage::English,
    );
    assert_eq!(short.hit_test(610.0, 8.0), Some(0));
    assert_eq!(short.pointer_hit(610.0, 90.0), Some(Hit::TabBar));
    assert_eq!(short.pointer_hit(800.0, 90.0), None);
    assert!(!short.mirrored);
}

#[test]
fn messages_change_card_heights_and_every_part_has_a_hit() {
    let card_with_notice = card(
        Entry::new(
            "محرر",
            Some(Path::new("/tmp")),
            false,
            None,
            [Notice::new(None, "تم العمل")],
        )
        .with_agent_status(Some(AgentStatus::new(AgentState::NeedsInput, None))),
    );
    let single_height = CardLayout::height(&card_with_notice, 1.0);
    assert_eq!(
        CardLayout::height(&card_with_notice, 2.0),
        2.0 * single_height
    );
    let mut no_notice = card_with_notice.clone();
    no_notice.message = None;
    assert_eq!(single_height - CardLayout::height(&no_notice, 1.0), 20.0);

    let area = Rect::new(0.0, 0.0, 220.0, 280.0);
    let cards = [card_with_notice, no_notice];
    for side in [SidebarPosition::Left, SidebarPosition::Right] {
        let view = ScrollState::default().view(&cards, 0, area, 1.0, side, UiLanguage::Arabic);
        let item = &view.items[0];
        let layout = item.layout(&view);
        assert_eq!(item.rect.h, single_height);
        assert!(layout.message.y >= layout.footer.bottom());
        assert!(view.items[1].rect.y >= item.rect.bottom());
        for rect in [
            layout.title,
            layout.unread,
            layout.state,
            layout.message,
            layout.footer,
        ] {
            assert!(rect.w > 0.0 && rect.h > 0.0);
            let (x, y) = center(rect);
            assert_eq!(view.pointer_hit(x, y), Some(Hit::Tab(0)));
        }
        let (x, y) = center(layout.close);
        assert_eq!(
            view.pointer_hit(x, y),
            Some(Hit::Chrome(ChromeAction::CloseTab(0)))
        );
        let (x, y) = center(view.new_tab);
        assert_eq!(
            view.pointer_hit(x, y),
            Some(Hit::Chrome(ChromeAction::NewTab))
        );
        assert_eq!(layout.unread.x > layout.close.x, view.mirrored);
        let start = center(layout.title);
        let drag = Drag::new(12, start.1);
        assert_eq!(drag.target(&view, 0, (start.0, start.1 + 2.0)), None);
        assert_eq!(drag.target(&view, 0, center(view.items[1].rect)), Some(1));
        assert_eq!(drag.target(&view, 0, center(view.new_tab)), None);
    }
}

#[test]
fn titles_prefer_whole_words() {
    let fit = |s: &str| s.chars().count() <= 13;
    assert_eq!(fit_title("Review and merge the change", fit), "Review and…");
    assert_eq!(
        fit_title("مراجعة ودمج التغييرات دلوقتي", fit),
        "مراجعة ودمج…"
    );
    assert_eq!(
        fit_title("uninterruptedverylongtitle", fit),
        "uninterrupte…"
    );
}

#[test]
fn narrow_titles_keep_whole_graphemes_and_avoid_a_bare_ellipsis() {
    let fits = |s: &str| s.graphemes(true).count() <= 4;
    assert_eq!(fit_title("سَلامٌعَلَيكُم", fits), "سَلا…");
    assert_eq!(fit_title("iwide", |s| s == "i"), "i");
    assert_eq!(fit_title("WW", |s| s == "…" || s == "W"), "W");
    assert_eq!(fit_title("WW", |s| s == "…"), "");
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
    assert_eq!(TwittyConfig::default().sidebar_width, 180.0);
    for (json, side, width) in [
        (r#"{"sidebar":"left","sidebar_width":20}"#, "left", 140.0),
        (
            r#"{"sidebar":"hidden","sidebar_width":999}"#,
            "hidden",
            480.0,
        ),
        (r#"{"sidebar":"invalid"}"#, "right", 180.0),
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
    assert_eq!(twitty::layout::clamp_sidebar_width(f32::NAN), 180.0);
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
    let mut fs = twitty::render::text::fonts::new_font_system();
    let palette = Palette::default();
    let style = UiStyle::new(&palette, None, 1.0);
    let font = UiFont::default();
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
        false,
        Some(7),
        [Notice::new(Some("جاهز"), "تم")],
    );
    e.branch = Some("ميزة".into());
    let area = Rect::new(500.0, 0.0, 400.0, 200.0);
    let mut cache = LabelCache::default();
    let cards = cache.cards(&font, ctx.font_system, vec![e], area, 1.0);
    let view = ScrollState::default().view(
        &cards,
        0,
        area,
        1.0,
        SidebarPosition::Right,
        UiLanguage::English,
    );
    let scene = build(&mut ctx, &view, &mut cache, style, &font);
    let label = |text| {
        scene
            .labels
            .iter()
            .find(|l| l.buffer.lines[0].text() == text)
            .unwrap()
    };
    for text in ["محرر", "جاهز: تم", "ميزة"] {
        let label = label(text);
        assert!((label.left + label_width(&label.buffer) - label.bounds.right as f32).abs() <= 1.0);
        assert!(label
            .buffer
            .layout_runs()
            .any(|r| r.rtl && !r.glyphs.is_empty()));
        assert!(label.bounds.left >= area.x as i32 && label.bounds.right <= area.right() as i32);
    }
    let glyphs: Vec<_> = label("جاهز: تم")
        .buffer
        .layout_runs()
        .flat_map(|r| r.glyphs.iter())
        .collect();
    let leftmost = glyphs.iter().min_by(|a, b| a.x.total_cmp(&b.x)).unwrap();
    let rightmost = glyphs.iter().max_by(|a, b| a.x.total_cmp(&b.x)).unwrap();
    assert!(leftmost.start > rightmost.start);
    let red = label("Error").color;
    let [r, g, b, _] = red.as_rgba();
    assert!(r > g && r > b);
    assert!(scene.labels.iter().any(|l| l.buffer.lines[0].text() == "1"));
}

#[test]
fn pixel_fitting_preserves_arabic_direction_on_both_sides() {
    let mut fs = twitty::render::text::fonts::new_font_system();
    let palette = Palette::default();
    let style = UiStyle::new(&palette, None, 1.0);
    let font = UiFont::default();
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
    let e = Entry::new(
        &title,
        Some(Path::new(&path)),
        false,
        None,
        [Notice::new(None, &body)],
    );
    let mut cache = LabelCache::default();
    for side in [SidebarPosition::Left, SidebarPosition::Right] {
        for width in [115.0, 145.0, 220.0] {
            let area = Rect::new(400.0, 0.0, width, 200.0);
            let cards = cache.cards(&font, ctx.font_system, vec![e.clone()], area, 1.0);
            assert!(cards[0].message.is_some());
            let view = ScrollState::default().view(&cards, 0, area, 1.0, side, UiLanguage::Arabic);
            let scene = build(&mut ctx, &view, &mut cache, style, &font);
            let title_label = &scene.labels[0];
            let text = title_label.buffer.lines[0].text();
            let kept = text.strip_suffix('…').unwrap();
            assert!(!kept.is_empty());
            assert!(e.title[kept.len()..].starts_with(' '));
            let run = title_label.buffer.layout_runs().next().unwrap();
            assert!(run.rtl);
            let ellipsis = run
                .glyphs
                .iter()
                .find(|glyph| glyph.start == kept.len())
                .unwrap();
            assert!(
                run.glyphs.iter().all(|g| g.x >= ellipsis.x),
                "{text:?}: {:?}",
                run.glyphs
                    .iter()
                    .map(|g| (g.start, g.x, g.w, g.level))
                    .collect::<Vec<_>>()
            );
            assert!((title_label.left + run.line_w - title_label.bounds.right as f32).abs() <= 1.0);

            let path = scene
                .labels
                .iter()
                .find(|l| l.buffer.lines[0].text().ends_with("proj-12"))
                .unwrap();
            assert_eq!(path.buffer.metrics().font_size, 11.0);
            assert!(path.buffer.lines[0].text().starts_with('…'));
            let notice = scene
                .labels
                .iter()
                .filter(|label| label.buffer.lines[0].text().starts_with("سَلَامٌ"))
                .collect::<Vec<_>>();
            assert_eq!(notice.len(), 1);
            assert!(notice[0].buffer.lines[0].text().ends_with('…'));
        }
    }
}

#[test]
fn footer_fits_whole_words_then_graphemes_and_keeps_the_path_tail() {
    let mut fonts = twitty::render::text::fonts::new_font_system();
    let font = UiFont::default();
    let palette = Palette::default();
    let mut ctx = DrawContext {
        font_system: &mut fonts,
        family: None,
        palette: &palette,
        metrics: metrics(),
        is_srgb: false,
    };
    let area = Rect::new(0.0, 0.0, 150.0, 300.0);
    let mut cache = LabelCache::default();
    let path = Path::new("/tmp/a-very-long-directory-name/project");
    let mut entry = Entry::new("Shell", Some(path), false, None, []);
    for branch in [
        "main extremely-long-next-word-in-the-branch-label",
        "مَسَارٌمَسَارٌمَسَارٌمَسَارٌ",
    ] {
        entry.branch = Some(branch.to_owned());
        let cards = cache.cards(&font, ctx.font_system, vec![entry.clone()], area, 1.0);
        let view = ScrollState::default().view(
            &cards,
            0,
            area,
            1.0,
            SidebarPosition::Right,
            UiLanguage::English,
        );
        let scene = build(
            &mut ctx,
            &view,
            &mut cache,
            UiStyle::new(&palette, None, 1.0),
            &font,
        );
        let branch_label = scene
            .labels
            .iter()
            .map(|label| label.buffer.lines[0].text())
            .find(|label| branch.starts_with(label.trim_end_matches('…')) && *label != "…")
            .unwrap();
        let prefix = branch_label.strip_suffix('…').unwrap();
        assert!(
            !prefix.is_empty() && branch.starts_with(prefix),
            "{branch_label}"
        );
        assert!(branch
            .grapheme_indices(true)
            .any(|(offset, _)| offset == prefix.len()));
        if branch.is_ascii() {
            assert_eq!(branch_label, "main…");
        }
    }
    entry.branch = None;
    let cards = cache.cards(&font, ctx.font_system, vec![entry], area, 1.0);
    let view = ScrollState::default().view(
        &cards,
        0,
        area,
        1.0,
        SidebarPosition::Right,
        UiLanguage::English,
    );
    let scene = build(
        &mut ctx,
        &view,
        &mut cache,
        UiStyle::new(&palette, None, 1.0),
        &font,
    );
    assert_eq!(scene.labels[1].buffer.lines[0].text(), "…/project");
}

const LIVE_TAB_TITLE: &str = "omar@omarchy:~/Projects/twitty-bundle";

#[test]
fn active_and_inactive_workspace_titles_show_a_prefix_at_fractional_scale() {
    let mut fonts = twitty::render::text::fonts::new_font_system();
    let font = UiFont::default();
    let scale = 1.5;
    let metrics = CellMetrics { scale, ..metrics() };
    let palette = Palette::default();
    let style = UiStyle::new(&palette, None, scale);
    let mut ctx = DrawContext {
        font_system: &mut fonts,
        family: None,
        palette: &palette,
        metrics,
        is_srgb: false,
    };
    let working = Entry::new(LIVE_TAB_TITLE, None, true, None, []);
    let idle = Entry::new(LIVE_TAB_TITLE, None, false, None, []);
    let area = WindowLayout::new(
        metrics,
        (951.0, 1002.0),
        2,
        (SidebarPosition::Right, 180.0),
        true,
    )
    .sidebar
    .unwrap();
    let mut cache = LabelCache::default();
    let cards = cache.cards(&font, ctx.font_system, vec![working, idle], area, scale);
    let view = ScrollState::default().view(
        &cards,
        0,
        area,
        scale,
        SidebarPosition::Right,
        UiLanguage::English,
    );
    let scene = build(&mut ctx, &view, &mut cache, style, &font);
    for item in &view.items {
        let bounds = item.layout(&view).title;
        let label = scene
            .labels
            .iter()
            .find(|label| {
                label.bounds.left == bounds.x.ceil() as i32
                    && label.bounds.right == bounds.right().floor() as i32
                    && label.bounds.top == bounds.y.ceil() as i32
            })
            .unwrap();
        let text = label.buffer.lines[0].text();
        let prefix = text.strip_suffix('…').unwrap();
        assert!(
            !prefix.is_empty() && LIVE_TAB_TITLE.starts_with(prefix),
            "{text}"
        );
        assert!(label_width(&label.buffer).ceil() + 1.0 <= bounds.w);
        let next = LIVE_TAB_TITLE[prefix.len()..]
            .graphemes(true)
            .next()
            .unwrap();
        let longer = font.shape(
            ctx.font_system,
            UiSize::Body,
            UiWeight::Semibold,
            scale,
            &format!("{prefix}{next}…"),
        );
        assert!(label_width(&longer).ceil() + 1.0 > bounds.w);
    }
}

#[test]
fn status_line_measures_the_whole_label_then_falls_back_to_the_icon() {
    let mut fonts = twitty::render::text::fonts::new_font_system();
    let font = UiFont::default();
    let scale = 1.5;
    let palette = Palette::default();
    let style = UiStyle::new(&palette, None, scale);
    let mut ctx = DrawContext {
        font_system: &mut fonts,
        family: None,
        palette: &palette,
        metrics: CellMetrics { scale, ..metrics() },
        is_srgb: false,
    };
    let mut cache = LabelCache::default();
    for (language, name) in [
        (UiLanguage::English, "Working"),
        (UiLanguage::Arabic, "شغّال"),
    ] {
        let label = font.shape(
            ctx.font_system,
            UiSize::Small,
            UiWeight::Regular,
            scale,
            name,
        );
        let full_status = label_width(&label).ceil() + 1.0 + (12.0 + 4.0) * scale;
        let threshold = full_status + (2.0 * 12.0 + 2.0 * 8.0) * scale;
        for (area_w, expected_label) in [
            (330.0, true),
            (threshold + 1.0, true),
            (threshold - 1.0, false),
        ] {
            let area = Rect::new(951.0 - area_w, 54.0, area_w, 400.0);
            let entry = Entry::new(LIVE_TAB_TITLE, None, true, None, []);
            let cards = cache.cards(&font, ctx.font_system, vec![entry], area, scale);
            let view = ScrollState::default().view(
                &cards,
                0,
                area,
                scale,
                SidebarPosition::Right,
                language,
            );
            let scene = build(&mut ctx, &view, &mut cache, style, &font);
            let parts = view.items[0].layout(&view);
            assert_eq!(
                parts.state_has_label, expected_label,
                "{language:?}, width {area_w}"
            );
            assert!(view.animates(), "the working dots remain visible");
            let captions: Vec<_> = scene
                .labels
                .iter()
                .filter(|label| label.buffer.metrics().font_size == UiSize::Small.px(scale))
                .collect();
            if expected_label {
                let [caption] = captions.as_slice() else {
                    panic!("expected exactly one state caption")
                };
                assert_eq!(caption.buffer.lines[0].text(), name);
                assert!(
                    label_width(&caption.buffer).ceil()
                        <= (caption.bounds.right - caption.bounds.left) as f32
                );
                assert!(parts.state.y >= parts.title.bottom());
            } else {
                assert!(captions.is_empty());
            }
        }
    }
}

#[test]
fn narrow_sidebar_cards_layout_without_overflow_or_overlap() {
    let mut fonts = twitty::render::text::fonts::new_font_system();
    let font = UiFont::default();
    let palette = Palette::default();
    let mut cache = LabelCache::default();

    let mut working = Entry::new(
        LIVE_TAB_TITLE,
        Some(Path::new("/home/omar/Projects/twitty-lanes/n7")),
        true,
        None,
        [Notice::new(
            Some("Agent"),
            "Running tests on narrow sidebar",
        )],
    )
    .with_ports(vec![3000, 8080]);
    working.branch = Some("feature/narrow-sidebar".into());

    let mut needs_input = Entry::new(
        "Build and deploy",
        Some(Path::new("/tmp/workspace")),
        false,
        None,
        [Notice::new(None, "Approve migration")],
    )
    .with_agent_status(Some(AgentStatus::new(AgentState::NeedsInput, None)))
    .with_ports(vec![5173]);
    needs_input.branch = Some("main".into());

    let mut done = Entry::new(
        "محرر عربي طويل جدا",
        Some(Path::new("/home/omar/مشاريع/طرفية")),
        false,
        Some(0),
        [],
    );
    done.branch = Some("ميزة".into());

    let entries = vec![working, needs_input, done];

    let overlaps = |a: Rect, b: Rect| {
        a.w > 0.0
            && a.h > 0.0
            && b.w > 0.0
            && b.h > 0.0
            && a.x < b.right() - 0.01
            && b.x < a.right() - 0.01
            && a.y < b.bottom() - 0.01
            && b.y < a.bottom() - 0.01
    };

    for scale in [1.0_f32, 1.5] {
        let style = UiStyle::new(&palette, None, scale);
        let mut ctx = DrawContext {
            font_system: &mut fonts,
            family: None,
            palette: &palette,
            metrics: CellMetrics { scale, ..metrics() },
            is_srgb: false,
        };
        for logical_w in [140.0_f32, 158.0, 180.0] {
            let area = Rect::new(200.0, 0.0, (logical_w * scale).round(), 600.0);
            for side in [SidebarPosition::Left, SidebarPosition::Right] {
                for language in [UiLanguage::English, UiLanguage::Arabic] {
                    let cards = cache.cards(&font, ctx.font_system, entries.clone(), area, scale);
                    let view = ScrollState::default().view(&cards, 0, area, scale, side, language);
                    let scene = build(&mut ctx, &view, &mut cache, style, &font);

                    for item in &view.items {
                        assert!(item.rect.x >= area.x && item.rect.right() <= area.right());
                        let parts = item.layout(&view);
                        let row = [parts.unread, parts.title, parts.state, parts.close];
                        for (i, &a) in row.iter().enumerate() {
                            if a.w > 0.0 {
                                assert!(
                                    a.x >= item.rect.x - 0.01
                                        && a.right() <= item.rect.right() + 0.01,
                                    "title row overflow at {logical_w}@{scale} {side:?} {language:?}: {a:?} in {:?}",
                                    item.rect
                                );
                            }
                            for &b in &row[i + 1..] {
                                assert!(
                                    !overlaps(a, b),
                                    "title row overlap at {logical_w}@{scale} {side:?} {language:?}: {a:?} vs {b:?}"
                                );
                            }
                        }
                        assert!(parts.title.w > 0.0);
                        assert!(parts.state.w >= 12.0 * scale);
                        assert!(!overlaps(parts.state, parts.footer));
                        assert!(parts.state.y >= parts.title.bottom());
                        assert!(
                            parts.footer.x >= item.rect.x
                                && parts.footer.right() <= item.rect.right()
                        );
                    }

                    // NeedsInput ("Needs input" / "مستني إدخال") falls back to icon-only at 140 logical px.
                    if logical_w == 140.0 {
                        assert!(
                            !view.items[1].layout(&view).state_has_label,
                            "NeedsInput should be icon-only at 140 logical px ({language:?}@{scale})"
                        );
                    }

                    for label in &scene.labels {
                        assert!(
                            label.bounds.left >= area.x.floor() as i32
                                && label.bounds.right <= area.right().ceil() as i32,
                            "label out of sidebar bounds at {logical_w}@{scale}: {:?} not in {:?}",
                            label.bounds,
                            area
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn narrow_toolbar_and_tab_pills_use_words_and_grapheme_fallbacks() {
    let mut fonts = twitty::render::text::fonts::new_font_system();
    let font = UiFont::default();
    let palette = Palette::default();
    let style = UiStyle::new(&palette, None, 1.0);
    let metrics = metrics();
    let surface = (640.0, 400.0);
    let unbroken = LIVE_TAB_TITLE;
    let words = "Review and merge the extraordinarily long change";
    let tabs = [
        TabLabel {
            title: unbroken.to_owned(),
            active: true,
        },
        TabLabel {
            title: words.to_owned(),
            active: false,
        },
    ];
    for (language, local) in [
        (UiLanguage::English, SEARCH_EN),
        (UiLanguage::Arabic, SEARCH_AR),
    ] {
        let layout = WindowLayout::new(metrics, surface, 2, (SidebarPosition::Hidden, 220.0), true)
            .with_language(language);
        let mut opts = TopBarLayout::new(layout.top_bar, 1.0, None).with_language(language);
        let first = local.graphemes(true).next().unwrap();
        let short = format!("{first}…");
        let width = |fonts: &mut FontSystem, text: &str| {
            label_width(&font.shape(fonts, UiSize::Caption, UiWeight::Regular, 1.0, text)).ceil()
        };
        // Give the placeholder enough space for the first grapheme and ellipsis.
        opts.search.w = width(&mut fonts, &short) + 38.0;
        let view = ChromeView {
            layout: &layout,
            top: opts,
            diff: None,
            unread: 0,
            sidebar_visible: false,
            hover: None,
            hover_tab: None,
            language,
            panes: &[],
            tabs: &tabs,
        };
        let mut ctx = DrawContext {
            font_system: &mut fonts,
            family: None,
            palette: &palette,
            metrics,
            is_srgb: false,
        };
        let options = FrameOptions {
            window_focused: true,
            cursor_style: "block",
            cursor_blink_on: true,
            surface,
        };
        let mut rows = RowCache::default();
        let mut ime = PreeditState::default();
        let mut frame = build_frame(&mut ctx, &options, &mut rows, &mut ime, &[], &[], &[]);
        frame.add_chrome(&mut ctx, style, &font, &view);
        let runs = frame.text_runs(&[], &rows, &ime);
        let labels: Vec<_> = runs.iter().map(|run| run.buffer.lines[0].text()).collect();
        assert_eq!(labels.len(), 3);
        let search_prefix = labels[0].strip_suffix('…').unwrap();
        assert!(
            !search_prefix.is_empty() && local.starts_with(search_prefix),
            "{labels:?}"
        );
        let title_prefix = labels[1].strip_suffix('…').unwrap();
        assert!(
            !title_prefix.is_empty() && unbroken.starts_with(title_prefix),
            "{labels:?}"
        );
        let words_prefix = labels[2].strip_suffix('…').unwrap();
        assert!(words[words_prefix.len()..].starts_with(' '), "{labels:?}");
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
    let entries = [card(Entry::new("محرر", None, false, None, []))];
    let style = UiStyle::new(&palette, None, 1.0);
    let font = UiFont::default();
    let mut cache = LabelCache::default();
    let mut view = ScrollState::default().view(
        &entries,
        0,
        Rect::new(0.0, 0.0, 220.0, 120.0),
        1.0,
        SidebarPosition::Left,
        UiLanguage::English,
    );
    let first = build(&mut ctx, &view, &mut cache, style, &font);
    view.items[0].active = false;
    let second = build(&mut ctx, &view, &mut cache, style, &font);
    assert!(Rc::ptr_eq(
        &first.labels[0].buffer,
        &second.labels[0].buffer
    ));
    view.items[0].card.entry.title = "طرفية".into();
    let third = build(&mut ctx, &view, &mut cache, style, &font);
    assert!(!Rc::ptr_eq(
        &second.labels[0].buffer,
        &third.labels[0].buffer
    ));
    view.items[0].rect.w = 180.0;
    let fourth = build(&mut ctx, &view, &mut cache, style, &font);
    assert!(!Rc::ptr_eq(
        &third.labels[0].buffer,
        &fourth.labels[0].buffer
    ));
    ctx.metrics.font_size += 1.0;
    let fifth = build(&mut ctx, &view, &mut cache, style, &font);
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
            blocks: Vec::new(),
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
        &[card(entry())],
        0,
        Rect::new(580.0, 0.0, 220.0, 600.0),
        1.0,
        SidebarPosition::Right,
        UiLanguage::English,
    );
    frame.add_sidebar(
        &mut ctx,
        &view,
        &mut LabelCache::default(),
        UiStyle::new(&palette, None, 1.0),
        &UiFont::default(),
    );
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

#[test]
fn arabic_title_and_unread_badge_follow_the_title_direction_in_both_languages() {
    use twitty::render::text::ui::label_width;

    let mut fonts = twitty::render::text::fonts::new_font_system();
    let font = UiFont::new(&mut fonts, None);
    let palette = Palette::default();
    let mut cache = LabelCache::default();
    let title = "مراجعة التغييرات";
    let entry = Entry::new(title, None, false, None, [Notice::new(None, "Ready")]);
    for (language, side) in [
        (UiLanguage::English, SidebarPosition::Left),
        (UiLanguage::Arabic, SidebarPosition::Right),
    ] {
        let scale = 1.5;
        let area = Rect::new(200.0, 0.0, 360.0, 400.0);
        let mut ctx = DrawContext {
            font_system: &mut fonts,
            family: None,
            palette: &palette,
            metrics: CellMetrics { scale, ..metrics() },
            is_srgb: false,
        };
        let cards = cache.cards(&font, ctx.font_system, vec![entry.clone()], area, scale);
        let view = ScrollState::default().view(&cards, 0, area, scale, side, language);
        let scene = build(
            &mut ctx,
            &view,
            &mut cache,
            UiStyle::new(&palette, None, scale),
            &font,
        );
        let label = |text| {
            scene
                .labels
                .iter()
                .find(|label| label.buffer.lines[0].text() == text)
                .unwrap()
        };
        let title_label = label(title);
        let badge = label("1");
        let title_edge = title_label.left + label_width(&title_label.buffer);
        assert!((title_edge - view.items[0].layout(&view).title.right()).abs() <= 1.0);
        assert!((title_label.left - badge.bounds.right as f32 - 4.0 * scale).abs() <= 1.0);
    }
}
