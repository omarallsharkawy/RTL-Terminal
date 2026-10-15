//! Edge cases in the terminal core: degenerate sizes and literal search.
use alacritty_terminal::index::Direction;
use twitty::terminal::Terminal;

#[test]
fn zero_sized_terminal_does_not_panic() {
    let mut term = Terminal::new(0, 0, |_| {});
    term.process_bytes(b"hello\r\nworld");
    let _ = term.snapshot();
    term.resize(0, 0);
    term.process_bytes("مرحبا".as_bytes());
    let _ = term.snapshot();
    term.resize(10, 3);
    let (lines, _) = term.snapshot();
    assert_eq!(lines.len(), 3);
}

#[test]
fn search_treats_regex_characters_literally() {
    for query in [
        "a-b", "#1", "~/x", "a&b", "[x]", "c++", "1.5", "a b", "x/y", "\"q\"", "'s", "%d", "@me",
        "a,b", "a=b", "<t>", "!x",
    ] {
        let mut term = Terminal::new(40, 3, |_| {});
        term.process_bytes(format!("zz {query} zz").as_bytes());
        assert!(
            term.search(query, Direction::Left),
            "no match for {query:?}"
        );
    }
}
