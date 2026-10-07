use alacritty_terminal::term::cell::Flags as CellFlags;
use twitty::terminal::Terminal;

#[test]
fn test_inverse_and_decorations() {
    let mut term = Terminal::new(80, 24, |_| {});

    // Print inverted text: SGR 7
    term.process_bytes(b"\x1b[7mINVERTED\x1b[0m");
    let (lines, _) = term.snapshot();
    assert!(lines[0].cells[0].flags.contains(CellFlags::INVERSE));
    assert_eq!(lines[0].cells[0].c, 'I');

    // Print underlined text: SGR 4
    term.process_bytes(b"\r\n\x1b[4mUNDERLINED\x1b[0m");
    let (lines, _) = term.snapshot();
    assert!(lines[1].cells[0].flags.contains(CellFlags::UNDERLINE));

    // Print strikeout text: SGR 9
    term.process_bytes(b"\r\n\x1b[9mSTRIKEOUT\x1b[0m");
    let (lines, _) = term.snapshot();
    assert!(lines[2].cells[0].flags.contains(CellFlags::STRIKEOUT));

    // Print hidden text: SGR 8
    term.process_bytes(b"\r\n\x1b[8mSECRET\x1b[0m");
    let (lines, _) = term.snapshot();
    assert!(lines[3].cells[0].flags.contains(CellFlags::HIDDEN));
}
