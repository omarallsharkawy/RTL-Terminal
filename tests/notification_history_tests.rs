//! Notification history and the palette's notifications mode: bounding and order, Arabic
//! filtering, the Enter target, closed panes, the rows and their drawing.
use std::time::{Duration, SystemTime};

use cosmic_text::FontSystem;
use twitty::color::Palette;
use twitty::commands;
use twitty::input::InputAction;
use twitty::layout::PaneId;
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::frame::{build_frame, FrameOptions};
use twitty::render::scene::palette::{label_width, panel_rect, PaletteItem, PaletteView};
use twitty::render::scene::preedit::PreeditState;
use twitty::render::{CellMetrics, RowCache};
use twitty::ui::notifications::{
    age_label, jump_target, palette_item, HistoryEntry, NotificationHistory, MAX_HISTORY,
};
use twitty::ui::palette::{
    matches, visible_window, PaletteCmd, PaletteEntry, PaletteId, PaletteState, MAX_RESULTS,
};
use winit::keyboard::{Key, ModifiersState, NamedKey};

fn t(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + secs)
}

fn history() -> NotificationHistory {
    let mut h = NotificationHistory::default();
    h.push(
        PaneId(1),
        Some("Build"),
        "finished in 3s",
        "cargo",
        t(0),
        false,
    );
    h.push(
        PaneId(2),
        Some("الوكيل"),
        "خلّص الشغل ومستني ردّك",
        "كلود",
        t(10),
        false,
    );
    h.push(PaneId(3), None, "tests passed", "agent", t(20), true);
    h.push(PaneId(2), None, "محتاج إذن", "كلود", t(30), false);
    h
}

fn enter(state: &mut PaletteState, list: &[&HistoryEntry]) -> PaletteCmd {
    state.key_in(
        list,
        &Key::Named(NamedKey::Enter),
        None,
        ModifiersState::empty(),
    )
}

#[test]
fn history_is_bounded_and_newest_first() {
    let mut h = NotificationHistory::default();
    for i in 0..(MAX_HISTORY as u64 + 25) {
        assert!(h.push(PaneId(i), None, &format!("n{i}"), "tab", t(i), false));
    }
    assert_eq!(h.len(), MAX_HISTORY);
    let list = h.newest_first();
    assert_eq!(list[0].body, format!("n{}", MAX_HISTORY + 24));
    assert_eq!(list[MAX_HISTORY - 1].body, "n25", "the oldest were dropped");
    assert!(list.windows(2).all(|w| w[0].at > w[1].at));
}

#[test]
fn history_cleans_and_bounds_text_and_skips_empty_ones() {
    let mut h = NotificationHistory::default();
    assert!(!h.push(PaneId(1), Some("  "), "\u{7}", "tab", t(0), false));
    assert!(h.is_empty());
    let huge = "ب".repeat(100_000);
    assert!(h.push(
        PaneId(1),
        Some("\u{202e}تم"),
        &huge,
        &"x".repeat(1000),
        t(0),
        false
    ));
    let e = h.newest_first()[0].clone();
    assert_eq!(e.title.as_deref(), Some("تم"));
    assert!(e.body.chars().count() <= 512);
    assert!(e.tab_title.chars().count() <= 64);
}

#[test]
fn seen_follows_the_pane() {
    let mut h = history();
    assert_eq!(h.unseen(), 3);
    h.mark_seen(PaneId(2));
    assert_eq!(h.unseen(), 1);
    assert!(h
        .newest_first()
        .iter()
        .filter(|e| e.pane == PaneId(2))
        .all(|e| e.seen));
}

#[test]
fn arabic_filter_ignores_hamza_and_tashkeel_and_matches_tab_titles() {
    let h = history();
    let list = h.newest_first();
    // "خلص" without the shadda finds "خلّص"; "اذن" finds "إذن".
    assert_eq!(matches(&list, "خلص"), vec![2]);
    assert_eq!(matches(&list, "اذن"), vec![0]);
    // The tab title is searchable too, and words combine.
    let mut by_tab = matches(&list, "كلود");
    by_tab.sort_unstable();
    assert_eq!(by_tab, vec![0, 2]);
    assert_eq!(matches(&list, "BUILD finished"), vec![3]);
    assert!(matches(&list, "zzz").is_empty());
    assert_eq!(matches(&list, "").len(), 4);
}

#[test]
fn enter_chooses_the_selected_match_and_escape_closes() {
    let h = history();
    let list = h.newest_first();
    let mut state = PaletteState::default();
    // Arabic typed through the IME.
    state.push_text("الوكيل");
    assert_eq!(enter(&mut state, &list), PaletteCmd::Choose(2));
    assert_eq!(list[2].pane, PaneId(2));
    // Down then Enter with no query: the second newest.
    let mut state = PaletteState::default();
    state.key_in(
        &list,
        &Key::Named(NamedKey::ArrowDown),
        None,
        ModifiersState::empty(),
    );
    assert_eq!(enter(&mut state, &list), PaletteCmd::Choose(1));
    // No match: Enter does nothing.
    state.push_text("zzz");
    assert_eq!(enter(&mut state, &list), PaletteCmd::None);
    assert_eq!(
        state.key_in(
            &list,
            &Key::Named(NamedKey::Escape),
            None,
            ModifiersState::empty()
        ),
        PaletteCmd::Close
    );
}

#[test]
fn closed_panes_stay_listed_dimmed_and_cannot_be_jumped_to() {
    let h = history();
    let list = h.newest_first();
    let open = |id: PaneId| id != PaneId(3);
    assert_eq!(jump_target(list[1], open), None, "pane 3 was closed");
    assert_eq!(jump_target(list[0], open), Some(PaneId(2)));
    let now = t(60);
    let closed = palette_item(list[1], open(list[1].pane), now);
    assert!(closed.dimmed);
    assert!(!palette_item(list[0], true, now).dimmed);
    // Closed or not, it still matches the filter.
    assert_eq!(matches(&list, "passed"), vec![1]);
}

#[test]
fn rows_put_arabic_on_the_right_and_mark_unseen_ones() {
    let h = history();
    let list = h.newest_first();
    let now = t(90);
    // Arabic text: on the Arabic (right) side, marked unseen, the tab and age on the left.
    let ar = palette_item(list[0], true, now);
    assert_eq!(ar.title_ar, "● محتاج إذن");
    assert_eq!(ar.title_en, "كلود · 1m");
    // English text: on the left, its tab on the right.
    let en = palette_item(list[3], true, now);
    assert_eq!(en.title_en, "● Build: finished in 3s");
    assert_eq!(en.title_ar, "cargo · 1m");
    // Seen: no mark.
    let seen = palette_item(list[1], true, now);
    assert_eq!(seen.title_en, "tests passed");
    assert_eq!(seen.shortcut, None);
}

#[test]
fn ages() {
    assert_eq!(age_label(t(0), t(3)), "now");
    assert_eq!(age_label(t(0), t(42)), "42s");
    assert_eq!(age_label(t(0), t(5 * 60 + 9)), "5m");
    assert_eq!(age_label(t(0), t(3 * 3600)), "3h");
    assert_eq!(age_label(t(0), t(2 * 86_400 + 5)), "2d");
    // A clock that went backwards.
    assert_eq!(age_label(t(10), t(0)), "now");
}

#[test]
fn long_lists_scroll_with_the_selection() {
    assert_eq!(visible_window(0, 0), 0..0);
    assert_eq!(visible_window(4, 2), 0..4);
    assert_eq!(visible_window(50, 0), 0..MAX_RESULTS);
    assert_eq!(visible_window(50, 9), 0..MAX_RESULTS);
    assert_eq!(visible_window(50, 10), 1..11);
    assert_eq!(visible_window(50, 49), 40..50);
    assert_eq!(visible_window(50, 99), 40..50);
    // Arrow keys reach every item, not only the first page, and wrap.
    let mut h = NotificationHistory::default();
    for i in 0..30 {
        h.push(PaneId(i), None, &format!("n{i}"), "tab", t(i), false);
    }
    let list = h.newest_first();
    let mut state = PaletteState::default();
    let up = Key::Named(NamedKey::ArrowUp);
    state.key_in(&list, &up, None, ModifiersState::empty());
    assert_eq!(state.selected, 29);
    assert_eq!(enter(&mut state, &list), PaletteCmd::Choose(29));
}

#[test]
fn notifications_command_and_shortcut() {
    assert_eq!(
        commands::action("notifications.list"),
        Some(InputAction::NotificationList)
    );
    let c = commands::all()
        .into_iter()
        .find(|c| c.name == "notifications.list")
        .unwrap();
    assert_eq!((c.title_en, c.title_ar), ("Notifications", "الإشعارات"));
    assert_eq!(c.shortcut, Some("Ctrl+Shift+I"));
    // Findable from the command palette in Arabic without the hamza.
    let all = commands::all();
    assert_eq!(all[matches(&all, "الاشعارات")[0]].name, "notifications.list");
    // The commands palette still runs commands by name.
    let entries = PaletteEntry::builtins();
    let mut state = PaletteState::default();
    state.push_text("notifications");
    assert_eq!(
        state.key(
            &entries,
            &Key::Named(NamedKey::Enter),
            None,
            ModifiersState::empty()
        ),
        PaletteCmd::Run(PaletteId::Builtin("notifications.list"))
    );
}

#[test]
fn shortcut_is_ctrl_shift_i_on_any_layout_and_keyboard_mode() {
    use alacritty_terminal::term::TermMode;
    use twitty::input::handle_key_mode_raw;
    use winit::event::ElementState;
    use winit::keyboard::KeyCode;
    let cs = ModifiersState::CONTROL | ModifiersState::SHIFT;
    for mode in [TermMode::empty(), TermMode::DISAMBIGUATE_ESC_CODES] {
        // US layout, then Arabic (the key under I types ه): the physical key decides.
        for logical in ["I", "ه"] {
            assert_eq!(
                handle_key_mode_raw(
                    Some(KeyCode::KeyI),
                    &Key::Character(logical.into()),
                    None,
                    None,
                    ElementState::Pressed,
                    false,
                    cs,
                    mode,
                ),
                Some(InputAction::NotificationList),
                "{logical}"
            );
        }
    }
}

#[test]
fn dimmed_rows_and_long_text_are_drawn_inside_the_panel() {
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
    let mut cache = RowCache::default();
    let mut ime = PreeditState::default();
    let mut frame = build_frame(&mut ctx, &opts, &mut cache, &mut ime, &[], &[], &[]);
    let long_ar = format!("● {}", "الوكيل خلّص الشغل ".repeat(20));
    let view = PaletteView {
        query: String::new(),
        items: vec![
            PaletteItem {
                title_en: "كلود · 2m".into(),
                title_ar: long_ar.clone(),
                ..Default::default()
            },
            PaletteItem {
                title_en: "tests passed".into(),
                title_ar: "agent · 5m".into(),
                dimmed: true,
                ..Default::default()
            },
        ],
        selected: 0,
        placeholder: Some("Filter notifications… / دوّر في الإشعارات…".into()),
    };
    frame.add_palette(&mut ctx, &view);
    let runs = frame.text_runs(&[], &cache, &ime);
    let overlay = &runs[runs.len() - 5..];
    assert!(overlay[0]
        .buffer
        .lines
        .iter()
        .any(|l| l.text().contains("دوّر في الإشعارات")));
    let panel = panel_rect(9.0, 23.0, surface, 2);
    // The long Arabic text was cut to fit beside its tab and age, and still reads RTL.
    let (meta, arabic) = (&overlay[1], &overlay[2]);
    let arabic_text = arabic.buffer.lines[0].text();
    assert!(arabic_text.ends_with('…'), "{arabic_text}");
    assert!(arabic_text.chars().count() < long_ar.chars().count());
    assert!(arabic.buffer.layout_runs().next().unwrap().rtl);
    let meta_right = meta.left + label_width(meta.buffer);
    assert!(meta_right < arabic.left, "the two sides do not overlap");
    let arabic_right = arabic.left + label_width(arabic.buffer);
    assert!((arabic_right - (panel.right() - 9.0)).abs() < 1.0);
    // The dimmed row is fainter than the normal unselected text.
    let (dim_en, dim_ar) = (&overlay[3], &overlay[4]);
    assert_eq!(dim_en.color, dim_ar.color);
    assert_ne!(dim_ar.color, arabic.color);
}
