//! Searching every terminal at once (`search.all`): matching across panes in English and
//! Arabic, bounds, ordering, trimming around the match, the jump target, and the hint.
use std::time::{Duration, Instant};

use alacritty_terminal::term::TermMode;
use twitty::commands;
use twitty::input::{handle_key_mode_raw, InputAction};
use twitty::layout::PaneId;
use twitty::terminal::{Terminal, TextChar};
use twitty::ui::palette::{PaletteCmd, PaletteState};
use twitty::ui::search_all::{
    fair_shares, jump_target, palette_item, scan_terminal, search_all, snippet, PaneInfo, Query,
    SearchAll, SearchJob, DEBOUNCE, MAX_HITS, MAX_LINES_PER_PANE,
};
use winit::event::ElementState;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey};

fn term_with(lines: &[&str]) -> Terminal {
    let mut term = Terminal::new(60, 8, |_| {});
    for line in lines {
        term.process_bytes(format!("{line}\r\n").as_bytes());
    }
    term
}

fn info(pane: u64, tab: usize, title: &str) -> PaneInfo {
    PaneInfo {
        pane: PaneId(pane),
        tab,
        tab_title: title.into(),
        pane_title: None,
    }
}

fn query(text: &str) -> Query {
    Query::new(text).unwrap_or_else(|| panic!("{text:?} should search"))
}

/// The text of the focused search match's row on screen.
fn focused_row(term: &Terminal) -> Option<String> {
    let (lines, _) = term.snapshot();
    lines
        .iter()
        .find(|l| l.cells.iter().any(|c| c.search_match == 2))
        .map(|l| {
            let mut s = String::new();
            for c in l.cells.iter() {
                c.push_text(&mut s);
            }
            s.trim_end().to_owned()
        })
}

#[test]
fn finds_lines_in_every_pane_case_insensitively_newest_first() {
    let mut a = term_with(&["cargo build", "Error: first", "ok", "error: second"]);
    let mut b = term_with(&["nothing here"]);
    let mut c = term_with(&["ERROR third"]);
    let hits = search_all(
        [
            (info(1, 0, "one"), &mut a),
            (info(2, 0, "one"), &mut b),
            (info(3, 1, "two"), &mut c),
        ],
        &query("error"),
    );
    let got: Vec<(u64, &str)> = hits
        .iter()
        .map(|h| (h.pane.0, h.line.snippet.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            (1, "error: second"),
            (1, "Error: first"),
            (3, "ERROR third")
        ]
    );
    assert_eq!(hits[0].tag, "1 one");
    assert_eq!(hits[2].tag, "2 two");
    // The needle is the text as stored, for the scrollback search to highlight.
    assert_eq!(hits[1].line.needle, "Error");
    assert_eq!(hits[1].line.column, 0);
}

#[test]
fn arabic_matches_without_tashkeel_and_with_other_alef_forms() {
    let mut term = term_with(&["قال: مَرْحَبًا بِكُم", "إسلام ومدرسة", "مستشفى كبير", "شيء آخر"]);
    let found = |term: &mut Terminal, q: &str| -> Vec<String> {
        scan_terminal(term, &query(q), MAX_HITS, MAX_LINES_PER_PANE)
            .into_iter()
            .map(|h| h.snippet)
            .collect()
    };
    // Typed without harakat.
    assert_eq!(found(&mut term, "مرحبا"), ["قال: مَرْحَبًا بِكُم"]);
    // Typed with harakat the text does not have, and the other way round.
    assert_eq!(found(&mut term, "بِكم"), ["قال: مَرْحَبًا بِكُم"]);
    // Alef with hamza below typed as a bare alef, teh marbuta as heh.
    assert_eq!(found(&mut term, "اسلام"), ["إسلام ومدرسة"]);
    assert_eq!(found(&mut term, "مدرسه"), ["إسلام ومدرسة"]);
    // Alef maqsura as yeh, madda as a bare alef.
    assert_eq!(found(&mut term, "مستشفي"), ["مستشفى كبير"]);
    assert_eq!(found(&mut term, "اخر"), ["شيء آخر"]);
    // The needle keeps the stored letters and harakat-free cells, so the highlight is exact.
    let hit = &scan_terminal(&mut term, &query("مرحبا"), 10, 100)[0];
    assert_eq!(hit.needle, "مرحبا");
    assert_eq!(hit.column, 5);
}

#[test]
fn presentation_forms_from_vim_match_plain_letters() {
    // "سلام" as vim's arabicshape sends it: pre-joined forms in logical order.
    let mut term = term_with(&["\u{FEB3}\u{FEFC}\u{FEE1} x"]);
    let hits = scan_terminal(&mut term, &query("سلام"), 10, 100);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].snippet, "سلام x");
}

#[test]
fn results_are_bounded_and_shared_between_panes() {
    let mut terms: Vec<Terminal> = (0..20)
        .map(|_| {
            let mut t = Terminal::new(40, 5, |_| {});
            for i in 0..300 {
                t.process_bytes(format!("match {i}\r\n").as_bytes());
            }
            t
        })
        .collect();
    let mut quiet = term_with(&["one match only"]);
    let panes = terms
        .iter_mut()
        .enumerate()
        .map(|(i, t)| (info(i as u64, i, "t"), t))
        .chain([(info(99, 20, "q"), &mut quiet)]);
    let hits = search_all(panes, &query("match"));
    assert_eq!(hits.len(), MAX_HITS);
    // The quiet pane keeps its one line; the others share the rest about equally.
    assert_eq!(hits.iter().filter(|h| h.pane == PaneId(99)).count(), 1);
    for i in 0..20 {
        let n = hits.iter().filter(|h| h.pane == PaneId(i)).count();
        assert!((9..=10).contains(&n), "pane {i}: {n}");
    }
    // Each pane's lines newest first, panes in the order given.
    let first: Vec<_> = hits
        .iter()
        .take(3)
        .map(|h| h.line.snippet.clone())
        .collect();
    assert_eq!(first, ["match 299", "match 298", "match 297"]);
    let panes: Vec<u64> = hits.iter().map(|h| h.pane.0).collect();
    let mut sorted = panes.clone();
    sorted.sort_by_key(|&p| if p == 99 { 20 } else { p });
    assert_eq!(panes, sorted);
}

#[test]
fn fair_shares_give_unused_room_to_busy_panes() {
    assert_eq!(fair_shares(&[500, 1, 0, 500], 200), [100, 1, 0, 99]);
    assert_eq!(fair_shares(&[3, 4], 200), [3, 4]);
    assert_eq!(fair_shares(&[], 200), Vec::<usize>::new());
    assert_eq!(fair_shares(&[5, 5, 5], 2), [1, 1, 0]);
    let shares = fair_shares(&[1000; 7], 200);
    assert_eq!(shares.iter().sum::<usize>(), 200);
}

#[test]
fn per_pane_scan_reads_a_bounded_number_of_rows() {
    let mut term = Terminal::new(40, 5, |_| {});
    term.process_bytes(b"needle at the very top\r\n");
    for i in 0..200 {
        term.process_bytes(format!("filler {i}\r\n").as_bytes());
    }
    assert_eq!(scan_terminal(&mut term, &query("needle"), 10, 50).len(), 0);
    assert_eq!(
        scan_terminal(&mut term, &query("needle"), 10, 1000).len(),
        1
    );
    assert_eq!(scan_terminal(&mut term, &query("filler"), 7, 1000).len(), 7);
}

fn chars(text: &str) -> Vec<TextChar> {
    let mut out = Vec::new();
    let mut col = 0u16;
    for c in text.chars() {
        let mark = unicode_width_is_zero(c);
        let wide = !mark && is_wide(c);
        out.push(TextChar {
            c,
            mark,
            row: 0,
            col,
            wide,
        });
        if !mark {
            col += if wide { 2 } else { 1 };
        }
    }
    out
}

fn unicode_width_is_zero(c: char) -> bool {
    matches!(c, '\u{064B}'..='\u{065F}' | '\u{0670}')
}

fn is_wide(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}')
}

/// Columns `text` takes (marks none, CJK two).
fn cols(text: &str) -> usize {
    text.chars()
        .filter(|&c| !unicode_width_is_zero(c))
        .map(|c| if is_wide(c) { 2 } else { 1 })
        .sum()
}

#[test]
fn long_lines_are_cut_around_the_match() {
    let line = format!("{} NEEDLE {}", "a".repeat(100), "b".repeat(100));
    let c = chars(&line);
    let at = line.find("NEEDLE").unwrap();
    let s = snippet(&c, at, at + 6, 40);
    assert!(s.starts_with('…') && s.ends_with('…'), "{s}");
    assert!(s.contains("NEEDLE"));
    assert!(cols(&s) <= 40, "{s}");
    // Some text before the match, more after it.
    let before = s.find("NEEDLE").unwrap();
    assert!(before > 3 && before < 20, "{s}");

    // Near the start: nothing cut there.
    let line = format!("NEEDLE {}", "b".repeat(100));
    let s = snippet(&chars(&line), 0, 6, 40);
    assert!(s.starts_with("NEEDLE") && s.ends_with('…'), "{s}");
    assert_eq!(cols(&s), 40);
    // Near the end: the room goes before it.
    let line = format!("{} NEEDLE", "a".repeat(100));
    let at = line.find("NEEDLE").unwrap();
    let s = snippet(&chars(&line), at, at + 6, 40);
    assert!(s.starts_with('…') && s.ends_with("NEEDLE"), "{s}");
    assert_eq!(cols(&s), 40);
    // Short lines stay whole, without the blanks around them.
    assert_eq!(snippet(&chars("   short  "), 3, 5, 40), "short");
    // A match longer than the room keeps its start.
    let line = "x".repeat(10) + &"M".repeat(80);
    let s = snippet(&chars(&line), 10, 90, 20);
    assert!(s.starts_with("…M") || s.starts_with('M'), "{s}");
    assert!(cols(&s) <= 20);
}

#[test]
fn arabic_and_wide_lines_are_cut_by_columns_keeping_marks() {
    // Harakat take no column and stay with their letter.
    let word = "مَرْحَبًا";
    let line = format!("{} {word} {}", "كلام ".repeat(20), "تاني ".repeat(20));
    let c = chars(&line);
    let start = c.iter().position(|t| t.c == 'م' && t.col > 90).unwrap();
    let end = start + word.chars().count();
    let s = snippet(&c, start, end, 30);
    assert!(s.contains(word), "{s}");
    assert!(s.starts_with('…') && s.ends_with('…'), "{s}");
    assert!(cols(&s) <= 30, "{s}");

    let line = format!("{}目标{}", "汉".repeat(40), "字".repeat(40));
    let c = chars(&line);
    let s = snippet(&c, 40, 42, 21);
    assert!(s.contains("目标"), "{s}");
    assert!(cols(&s) <= 21, "{s}");
}

#[test]
fn arabic_results_go_on_the_right() {
    let mut term = term_with(&["مرحبا يا عالم", "hello world"]);
    let hits = search_all([(info(1, 0, "shell"), &mut term)], &query("hello"));
    let row = palette_item(&hits[0]);
    assert_eq!(
        (row.title_en.as_str(), row.title_ar.as_str()),
        ("hello world", "1 shell")
    );
    let hits = search_all([(info(1, 0, "shell"), &mut term)], &query("عالم"));
    let row = palette_item(&hits[0]);
    assert_eq!(
        (row.title_en.as_str(), row.title_ar.as_str()),
        ("1 shell", "مرحبا يا عالم")
    );
}

#[test]
fn tags_name_the_pane_in_split_tabs_and_stay_short() {
    let mut a = term_with(&["needle"]);
    let mut b = term_with(&["needle"]);
    let mut c = term_with(&["needle"]);
    let mut split = info(1, 0, "build");
    split.pane_title = Some("vim".into());
    let mut same = info(2, 0, "build");
    same.pane_title = Some("build".into());
    let long = info(3, 1, &"x".repeat(80));
    let hits = search_all(
        [(split, &mut a), (same, &mut b), (long, &mut c)],
        &query("needle"),
    );
    assert_eq!(hits[0].tag, "1 build › vim");
    assert_eq!(hits[1].tag, "1 build");
    assert!(hits[2].tag.ends_with('…') && hits[2].tag.chars().count() <= 28);
}

#[test]
fn jump_highlights_the_line_after_more_output_scrolled_in() {
    let mut term = Terminal::new(40, 5, |_| {});
    for i in 0..30 {
        term.process_bytes(format!("item {i} Target\r\n").as_bytes());
    }
    let hits = scan_terminal(&mut term, &query("target"), MAX_HITS, MAX_LINES_PER_PANE);
    // Newest first: item 29 .. item 0.
    let hit = hits
        .iter()
        .find(|h| h.snippet == "item 7 Target")
        .expect("item 7")
        .clone();
    for i in 30..80 {
        term.process_bytes(format!("item {i} Target\r\n").as_bytes());
    }
    assert!(term.focus_search_at(&hit.needle, hit.pos, hit.row, hit.column));
    assert!(term.display_offset() > 0);
    assert_eq!(focused_row(&term).as_deref(), Some("item 7 Target"));
    // The scrollback search goes on from there (older first), as its bar does.
    let (index, total) = term.search_position().unwrap();
    assert_eq!(total, 80);
    assert_eq!(index, 8);
    assert!(term.search(&hit.needle, alacritty_terminal::index::Direction::Left));
    assert_eq!(focused_row(&term).as_deref(), Some("item 6 Target"));
}

#[test]
fn jump_finds_a_match_inside_the_line_and_across_a_wrap() {
    let mut term = Terminal::new(20, 5, |_| {});
    // 26 columns: wraps after "abcdefghijklmnopqrst".
    term.process_bytes(b"abcdefghijklmnopqrstUVWxyz\r\n");
    for _ in 0..10 {
        term.process_bytes(b"other\r\n");
    }
    let hits = scan_terminal(&mut term, &query("stuv"), 10, 1000);
    assert_eq!(hits.len(), 1, "the two rows are one line");
    let hit = &hits[0];
    assert_eq!((hit.row, hit.column), (0, 18));
    assert_eq!(hit.needle, "stUV");
    assert!(term.focus_search_at(&hit.needle, hit.pos, hit.row, hit.column));
    let (lines, _) = term.snapshot();
    let marked: Vec<(usize, usize)> = lines
        .iter()
        .enumerate()
        .flat_map(|(r, l)| {
            l.cells
                .iter()
                .enumerate()
                .filter(|(_, c)| c.search_match == 2)
                .map(move |(col, _)| (r, col))
        })
        .collect();
    assert_eq!(marked.len(), 4, "{marked:?}");
    assert_eq!(marked[0].1, 18);
    assert_eq!(marked[2].1, 0);
}

#[test]
fn jump_falls_back_to_the_newest_match_when_the_line_is_gone() {
    let mut term = Terminal::with_scrollback(30, 4, 20);
    term.process_bytes(b"old needle\r\n");
    let hit = scan_terminal(&mut term, &query("needle"), 10, 1000).remove(0);
    // The line leaves the scrollback; a newer one says the same.
    for i in 0..40 {
        term.process_bytes(format!("line {i}\r\n").as_bytes());
    }
    term.process_bytes(b"new needle\r\nprompt");
    assert!(term.focus_search_at(&hit.needle, hit.pos, hit.row, hit.column));
    assert_eq!(focused_row(&term).as_deref(), Some("new needle"));
    // Nothing left to match: no highlight.
    let mut term = Terminal::with_scrollback(30, 4, 20);
    term.process_bytes(b"gone\r\n");
    let hit = scan_terminal(&mut term, &query("gone"), 10, 1000).remove(0);
    term.process_bytes(b"\x1b[2J\x1b[3J");
    assert!(!term.focus_search_at(&hit.needle, hit.pos, hit.row, hit.column));
    assert_eq!(focused_row(&term), None);
}

#[test]
fn alternate_screen_lines_are_searched_and_jumped_to() {
    let mut term = Terminal::new(30, 6, |_| {});
    term.process_bytes(b"main line\r\n\x1b[?1049h\x1b[Hvim buffer\r\nsecond row here");
    assert!(term.mode().contains(TermMode::ALT_SCREEN));
    let hits = scan_terminal(&mut term, &query("row"), 10, 1000);
    assert_eq!(hits.len(), 1);
    assert!(term.focus_search_at(&hits[0].needle, hits[0].pos, hits[0].row, hits[0].column));
    assert_eq!(focused_row(&term).as_deref(), Some("second row here"));
    // The main screen is not shown, so its lines are not listed.
    assert!(scan_terminal(&mut term, &query("main"), 10, 1000).is_empty());
}

/// Run `job` over `terms` (pane ids 0..) a minimal slice at a time, calling `between` after
/// each slice. Returns how many slices it took.
fn run_in_slices(
    job: &mut SearchJob,
    terms: &mut [Option<Terminal>],
    mut between: impl FnMut(&mut [Option<Terminal>]),
) -> usize {
    let mut slices = 0;
    while let Some(id) = job.current_pane() {
        // A deadline already passed: each slice reads one batch of lines.
        let past = Some(Instant::now());
        job.step(terms[id.0 as usize].as_mut(), past);
        slices += 1;
        between(terms);
        assert!(slices < 10_000);
    }
    slices
}

fn numbered(n: usize) -> Terminal {
    let mut t = Terminal::new(40, 5, |_| {});
    for i in 0..n {
        t.process_bytes(format!("row {i} {}\r\n", if i % 10 == 0 { "hit" } else { "" }).as_bytes());
    }
    t
}

#[test]
fn a_scan_in_slices_finds_what_one_scan_finds() {
    let infos = || vec![info(0, 0, "a"), info(1, 1, "b")];
    let mut whole = [numbered(1000), numbered(300)];
    let [a, b] = &mut whole;
    let expected = search_all(infos().into_iter().zip([a, b]), &query("hit"));
    let mut terms = [Some(numbered(1000)), Some(numbered(300))];
    let mut job = SearchJob::new(query("hit"), infos());
    let slices = run_in_slices(&mut job, &mut terms, |_| {});
    assert!(slices > 5, "{slices}");
    assert!(job.is_done());
    let hits = job.hits();
    assert_eq!(hits.len(), 130);
    let text = |h: &[twitty::ui::search_all::SearchHit]| -> Vec<(u64, String)> {
        h.iter()
            .map(|h| (h.pane.0, h.line.snippet.clone()))
            .collect()
    };
    assert_eq!(text(&hits), text(&expected));
}

#[test]
fn output_between_slices_keeps_the_scan_and_the_jump_on_the_right_lines() {
    let mut terms = [Some(numbered(2000))];
    let mut job = SearchJob::new(query("hit"), vec![info(0, 0, "a")]);
    let mut n = 2000;
    run_in_slices(&mut job, &mut terms, |terms| {
        // New lines (without "hit") scroll in while the scan goes on.
        let t = terms[0].as_mut().unwrap();
        for _ in 0..7 {
            t.process_bytes(format!("later {n}\r\n").as_bytes());
            n += 1;
        }
    });
    let hits = job.hits();
    // Every original "hit" line once, newest first, none twice or skipped.
    let rows: Vec<String> = hits.iter().map(|h| h.line.snippet.clone()).collect();
    let want: Vec<String> = (0..200)
        .rev()
        .map(|i| format!("row {} hit", i * 10))
        .collect();
    assert_eq!(rows, want);
    let hit = hits
        .iter()
        .find(|h| h.line.snippet == "row 1500 hit")
        .unwrap();
    let term = terms[0].as_mut().unwrap();
    let l = &hit.line;
    assert!(term.focus_search_at(&l.needle, l.pos, l.row, l.column));
    assert_eq!(focused_row(term).as_deref(), Some("row 1500 hit"));
}

#[test]
fn a_pane_closed_during_the_scan_is_left_out() {
    let mut terms = [Some(numbered(500)), Some(numbered(500))];
    let mut job = SearchJob::new(query("hit"), vec![info(0, 0, "a"), info(1, 1, "b")]);
    let mut closed = false;
    run_in_slices(&mut job, &mut terms, |terms| {
        if !closed {
            terms[0] = None;
            closed = true;
        }
    });
    let hits = job.hits();
    assert_eq!(hits.len(), 50);
    assert!(hits.iter().all(|h| h.pane == PaneId(1)));
}

#[test]
fn a_new_query_drops_the_scan_in_progress() {
    let mut state = SearchAll::default();
    let now = Instant::now();
    state.query_changed("hit", now);
    state.start(query("hit"), vec![info(0, 0, "a")]);
    assert!(state.is_pending() && state.due().is_none());
    // The same query again keeps it.
    state.query_changed("HIT ", now);
    assert!(state.job_mut().is_some() && state.due().is_none());
    state.query_changed("hits", now);
    assert!(state.job_mut().is_none());
    assert_eq!(state.due(), Some(now + DEBOUNCE));
    // The job's results land through `update`.
    state.start(query("hits"), vec![info(0, 0, "a")]);
    let mut term = term_with(&["two hits here"]);
    state.job_mut().unwrap().step(Some(&mut term), None);
    state.update();
    assert!(!state.is_pending());
    assert_eq!(state.hits().len(), 1);
}

#[test]
fn closed_pane_results_are_ignored() {
    let mut a = term_with(&["shared text"]);
    let mut b = term_with(&["shared text"]);
    let q = query("shared");
    let hits = search_all([(info(1, 0, "a"), &mut a), (info(2, 1, "b"), &mut b)], &q);
    let mut state = SearchAll::default();
    state.finish(&q, hits);
    let open = |id: PaneId| id == PaneId(2);
    assert!(jump_target(&state.hits()[0], open).is_none());
    assert!(jump_target(&state.hits()[1], open).is_some());
    state.retain_open(open);
    assert_eq!(state.hits().len(), 1);
    assert_eq!(state.hits()[0].pane, PaneId(2));
    assert_eq!(state.rows("shared").len(), 1);
}

#[test]
fn short_queries_show_a_hint_instead_of_searching() {
    assert!(Query::new("").is_none());
    assert!(Query::new(" a ").is_none());
    // A letter and a haraka is still one letter.
    assert!(Query::new("بَ").is_none());
    assert!(Query::new("ab").is_some());
    assert!(Query::new("با").is_some());
    let mut state = SearchAll::default();
    let now = Instant::now();
    state.query_changed("a", now);
    assert!(!state.is_pending());
    let rows = state.rows("a");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].dimmed);
    assert!(rows[0].title_en.contains("2 characters"));
    assert_eq!(rows[0].title_ar, "اكتب حرفين على الأقل");
}

#[test]
fn typing_schedules_one_scan_after_a_pause() {
    let mut state = SearchAll::default();
    let t0 = Instant::now();
    state.query_changed("er", t0);
    assert_eq!(state.due(), Some(t0 + DEBOUNCE));
    assert!(state.rows("er")[0].title_en.starts_with("Searching"));
    let t1 = t0 + Duration::from_millis(50);
    state.query_changed("err", t1);
    assert_eq!(state.due(), Some(t1 + DEBOUNCE));
    let mut term = term_with(&["no"]);
    let q = query("err");
    let hits = search_all([(info(1, 0, "a"), &mut term)], &q);
    state.finish(&q, hits);
    assert!(!state.is_pending());
    let rows = state.rows("err");
    assert_eq!(
        (rows[0].title_en.as_str(), rows[0].dimmed),
        ("No matches", true)
    );
    // The same query again (case or spaces aside) needs no new scan.
    state.query_changed(" ERR ", t1);
    assert!(!state.is_pending());
}

#[test]
fn list_keys_choose_among_the_results() {
    let mut state = PaletteState::default();
    let none = ModifiersState::empty();
    let down = Key::Named(NamedKey::ArrowDown);
    let enter = Key::Named(NamedKey::Enter);
    assert_eq!(state.key_list(3, &down, None, none), PaletteCmd::None);
    assert_eq!(state.key_list(3, &enter, None, none), PaletteCmd::Choose(1));
    assert_eq!(state.key_list(0, &enter, None, none), PaletteCmd::None);
    let a = Key::Character("ع".into());
    state.key_list(3, &a, Some("ع"), none);
    assert_eq!((state.query.as_str(), state.selected), ("ع", 0));
    assert_eq!(
        state.key_list(3, &Key::Named(NamedKey::Escape), None, none),
        PaletteCmd::Close
    );
}

#[test]
fn command_and_shortcut() {
    assert_eq!(commands::action("search.all"), Some(InputAction::SearchAll));
    let c = commands::all()
        .into_iter()
        .find(|c| c.name == "search.all")
        .unwrap();
    assert_eq!(
        (c.title_en, c.title_ar, c.shortcut),
        (
            "Search all terminals",
            "دوّر في كل الترمينالات",
            Some("Ctrl+Shift+G")
        )
    );
    let cs = ModifiersState::CONTROL | ModifiersState::SHIFT;
    for mode in [TermMode::empty(), TermMode::DISAMBIGUATE_ESC_CODES] {
        // US layout, then Arabic (the key under G types ل).
        for logical in ["G", "ل"] {
            assert_eq!(
                handle_key_mode_raw(
                    Some(KeyCode::KeyG),
                    &Key::Character(logical.into()),
                    None,
                    None,
                    ElementState::Pressed,
                    false,
                    cs,
                    mode,
                ),
                Some(InputAction::SearchAll),
                "{logical}"
            );
        }
    }
    // Plain Ctrl+G still reaches the shell (BEL).
    assert_eq!(
        handle_key_mode_raw(
            Some(KeyCode::KeyG),
            &Key::Character("g".into()),
            None,
            Some("\u{7}"),
            ElementState::Pressed,
            false,
            ModifiersState::CONTROL,
            TermMode::empty(),
        ),
        Some(InputAction::Bytes(vec![7]))
    );
}

/// Scan 20 full panes for a word on every line and a word on no line. Log timings for diagnosis.
#[test]
fn scanning_twenty_full_panes_caps_hits_and_handles_no_match() {
    let mut terms: Vec<Terminal> = (0..20)
        .map(|p| {
            let mut t = Terminal::new(100, 40, |_| {});
            let mut chunk = String::new();
            for i in 0..MAX_LINES_PER_PANE {
                chunk.push_str(&format!(
                    "pane {p} line {i}: cargo build --release مرحبا بالعالم {i:08x} done\r\n"
                ));
            }
            t.process_bytes(chunk.as_bytes());
            t
        })
        .collect();
    let run = |terms: &mut Vec<Terminal>, q: &str| {
        let start = Instant::now();
        let panes = terms
            .iter_mut()
            .enumerate()
            .map(|(i, t)| (info(i as u64, i, "t"), t));
        let hits = search_all(panes, &query(q));
        (hits.len(), start.elapsed())
    };
    let (n, common) = run(&mut terms, "بالعالم");
    // The app reads them a slice at a time: no slice takes much longer than its budget.
    let mut job = SearchJob::new(
        query("no such text"),
        (0..20).map(|i| info(i, i as usize, "t")).collect(),
    );
    let mut longest = Duration::ZERO;
    while let Some(id) = job.current_pane() {
        let start = Instant::now();
        job.step(
            Some(&mut terms[id.0 as usize]),
            Some(start + twitty::ui::search_all::SLICE),
        );
        longest = longest.max(start.elapsed());
    }
    assert!(job.is_done());
    assert!(job.hits().is_empty());
    eprintln!("search.all longest slice: {longest:?}");
    assert_eq!(n, MAX_HITS);
    let (n, rare) = run(&mut terms, "no such text");
    assert_eq!(n, 0);
    eprintln!("search.all 20 x {MAX_LINES_PER_PANE} lines: common {common:?}, no match {rare:?}");
}
