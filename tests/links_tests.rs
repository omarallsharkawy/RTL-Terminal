//! Link detection (plain URLs and OSC 8), hover cells, and the scheme allow-list.
use twitty::links::{find_url_at, is_allowed_scheme};
use twitty::terminal::Terminal;

fn url(text: &str, i: usize) -> Option<String> {
    find_url_at(text, i).map(|(_, _, u)| u)
}

#[test]
fn url_found_from_its_middle() {
    let t = "see https://example.com/path now";
    assert_eq!(url(t, 10).as_deref(), Some("https://example.com/path"));
    assert_eq!(find_url_at(t, 4).map(|r| (r.0, r.1)), Some((4, 28)));
    assert_eq!(url(t, 0), None);
    assert_eq!(url(t, 29), None);
}

#[test]
fn trailing_punctuation_is_excluded() {
    assert_eq!(
        url("go to http://a.org/x.", 10).as_deref(),
        Some("http://a.org/x")
    );
    assert_eq!(
        url("(http://a.org/x), ok", 5).as_deref(),
        Some("http://a.org/x")
    );
}

#[test]
fn balanced_parens_are_kept() {
    let t = "https://en.wikipedia.org/wiki/Foo_(bar)";
    assert_eq!(url(t, 3).as_deref(), Some(t));
    let t2 = "(see https://en.wikipedia.org/wiki/Foo_(bar))";
    assert_eq!(
        url(t2, 10).as_deref(),
        Some("https://en.wikipedia.org/wiki/Foo_(bar)")
    );
}

#[test]
fn url_inside_arabic_text() {
    let t = "افتح https://example.com/a?b=1 الآن";
    assert_eq!(url(t, 8).as_deref(), Some("https://example.com/a?b=1"));
    assert_eq!(url(t, 1), None);
}

#[test]
fn no_url_or_disallowed_scheme() {
    assert_eq!(url("just words here", 3), None);
    assert_eq!(url("javascript:alert(1)", 3), None);
    assert_eq!(url("xhttps://nope.com", 5), None);
    assert_eq!(url("mailto:a@b.co ok", 2).as_deref(), Some("mailto:a@b.co"));
}

#[test]
fn scheme_allow_list() {
    for ok in [
        "http://a",
        "https://a",
        "file:///tmp/x",
        "mailto:a@b",
        "ftp://h/f",
        "HTTPS://A",
    ] {
        assert!(is_allowed_scheme(ok), "{ok}");
    }
    for bad in [
        "javascript:alert(1)",
        "data:text/html,x",
        "vscode://open",
        "ssh://h",
        "nocolon",
        "http:",
        "http://a b",
        "https://a\n",
    ] {
        assert!(!is_allowed_scheme(bad), "{bad}");
    }
}

#[test]
fn terminal_plain_url_link() {
    let mut term = Terminal::new(40, 3, |_| {});
    term.process_bytes(b"go https://example.com/x. ok");
    let link = term.link_at(10, 0).expect("link");
    assert_eq!(link.uri, "https://example.com/x");
    assert_eq!((link.row, link.cols.clone()), (0, 3..24));
    assert!(term.link_at(1, 0).is_none());
    assert!(term.link_at(25, 0).is_none());

    assert!(term.set_link_hover(Some((link.row, link.cols))));
    let (lines, _) = term.snapshot();
    assert!(lines[0].cells[3].link_hover && lines[0].cells[23].link_hover);
    assert!(!lines[0].cells[2].link_hover && !lines[0].cells[24].link_hover);
    assert!(term.set_link_hover(None));
}

#[test]
fn terminal_url_after_wide_chars() {
    let mut term = Terminal::new(40, 3, |_| {});
    term.process_bytes("日本 https://a.io/z".as_bytes());
    // Two wide chars take 4 columns, a space, then the URL from column 5.
    let link = term.link_at(8, 0).expect("link");
    assert_eq!(link.uri, "https://a.io/z");
    assert_eq!(link.cols, 5..19);
}

#[test]
fn terminal_osc8_link() {
    let mut term = Terminal::new(40, 3, |_| {});
    term.process_bytes(b"a \x1b]8;;https://x.y\x1b\\text\x1b]8;;\x1b\\ b");
    let link = term.link_at(3, 0).expect("link");
    assert_eq!(link.uri, "https://x.y");
    assert_eq!(link.cols, 2..6);
    assert!(term.link_at(0, 0).is_none());
    assert!(term.link_at(7, 0).is_none());
}
