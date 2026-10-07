use alacritty_terminal::vte::ansi::CursorShape;
use twitty::terminal::Terminal;

#[test]
fn test_cursor_shapes() {
    let mut term = Terminal::new(80, 24, |_| {});

    // Default cursor
    let (_, cursor) = term.snapshot();
    assert_eq!(cursor.shape, CursorShape::Block);

    // DECSCUSR 2 -> Block
    term.process_bytes(b"[2 q");
    let (_, cursor) = term.snapshot();
    assert_eq!(cursor.shape, CursorShape::Block);

    // DECSCUSR 4 -> Underline
    term.process_bytes(b"[4 q");
    let (_, cursor) = term.snapshot();
    assert_eq!(cursor.shape, CursorShape::Underline);

    // DECSCUSR 6 -> Beam
    term.process_bytes(b"[6 q");
    let (_, cursor) = term.snapshot();
    assert_eq!(cursor.shape, CursorShape::Beam);
}
