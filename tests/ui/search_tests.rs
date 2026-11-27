use alacritty_terminal::index::Direction;
use twitty::terminal::Terminal;

fn filled() -> Terminal {
    let mut term = Terminal::new(40, 10, |_| {});
    for i in 0..50 {
        let line = if i % 5 == 0 {
            format!("سطر {i} مرحبا\r\n")
        } else {
            format!("Line {i} Hello World a.b(\r\n")
        };
        term.process_bytes(line.as_bytes());
    }
    term
}

fn row_text(term: &Terminal, row: usize) -> String {
    let (lines, _) = term.snapshot();
    lines[row].cells.iter().map(|c| c.c).collect()
}

fn marked(term: &Terminal, level: u8) -> Vec<usize> {
    let (lines, _) = term.snapshot();
    lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.cells.iter().any(|c| c.search_match == level))
        .map(|(r, _)| r)
        .collect()
}

#[test]
fn finds_arabic_and_scrolls_it_into_view() {
    let mut term = filled();
    assert!(term.search("مرحبا", Direction::Left));
    let rows = marked(&term, 2);
    assert_eq!(rows.len(), 1);
    assert!(row_text(&term, rows[0]).contains("مرحبا"));
}

#[test]
fn next_moves_to_older_match() {
    let mut term = filled();
    assert!(term.search("مرحبا", Direction::Left));
    let first = term.display_offset();
    assert!(term.search("مرحبا", Direction::Left));
    assert!(term.search("مرحبا", Direction::Left));
    assert!(term.display_offset() > first);
    // Previous goes back towards newer output (higher index = newer).
    let (older, total) = term.search_position().unwrap();
    assert_eq!(total, 10);
    assert!(term.search("مرحبا", Direction::Right));
    let (newer, _) = term.search_position().unwrap();
    assert_eq!(newer, older + 1);
}

#[test]
fn case_insensitive_ascii() {
    let mut term = filled();
    assert!(term.search("hello WORLD", Direction::Left));
    assert_eq!(marked(&term, 2).len(), 1);
}

#[test]
fn no_match_returns_false() {
    let mut term = filled();
    assert!(!term.search("zzzzqq", Direction::Left));
    assert!(!term.has_search_match());
    assert!(marked(&term, 1).is_empty() && marked(&term, 2).is_empty());
}

#[test]
fn snapshot_marks_focused_and_other_matches() {
    let mut term = filled();
    assert!(term.search("hello", Direction::Left));
    assert_eq!(marked(&term, 2).len(), 1);
    assert!(marked(&term, 1).len() >= 2);
    let (lines, _) = term.snapshot();
    let row = marked(&term, 2)[0];
    let focused: String = lines[row]
        .cells
        .iter()
        .filter(|c| c.search_match == 2)
        .map(|c| c.c)
        .collect();
    assert_eq!(focused.to_lowercase(), "hello");
    term.clear_search();
    assert!(marked(&term, 1).is_empty() && marked(&term, 2).is_empty());
}

#[test]
fn regex_metacharacters_are_literal() {
    let mut term = filled();
    assert!(term.search("a.b(", Direction::Left));
    assert!(!term.search("a.b)", Direction::Left));
    // "." must not match any character.
    assert!(!term.search("aXb(", Direction::Left));
}
