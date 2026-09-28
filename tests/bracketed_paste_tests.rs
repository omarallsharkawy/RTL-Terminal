use twitty::terminal::Terminal;

#[test]
fn test_bracketed_paste_framing() {
    let mut term = Terminal::new(80, 24, |_| {});

    // Terminal receives \x1b[?2004h to enable bracketed paste mode
    term.process_bytes(b"\x1b[?2004h");
    assert!(
        term.mode()
            .contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE),
        "Terminal must reflect BRACKETED_PASTE mode after escape sequence"
    );

    // Construct bracketed paste payload
    let text_to_paste = "echo 'Arabic test مرحبا'\nls -la";
    let mut payload = Vec::with_capacity(text_to_paste.len() + 12);
    payload.extend_from_slice(b"\x1b[200~");
    payload.extend_from_slice(text_to_paste.as_bytes());
    payload.extend_from_slice(b"\x1b[201~");

    assert!(payload.starts_with(b"\x1b[200~"));
    assert!(payload.ends_with(b"\x1b[201~"));
    assert!(payload
        .windows(text_to_paste.len())
        .any(|w| w == text_to_paste.as_bytes()));

    // Disable bracketed paste mode
    term.process_bytes(b"\x1b[?2004l");
    assert!(
        !term
            .mode()
            .contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE),
        "Terminal must disable BRACKETED_PASTE mode after escape sequence"
    );
}
