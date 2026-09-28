use twitty::terminal::Terminal;

#[test]
fn test_utf8_arabic_byte_by_byte_stream() {
    let mut term = Terminal::new(80, 24, |_| {});

    let arabic_phrase = "مرحبا بك في تيرمينال تويتي";
    let raw_bytes = arabic_phrase.as_bytes();

    // Feed bytes strictly one byte at a time to test PTY boundary splits on multi-byte characters
    for byte in raw_bytes {
        term.process_bytes(&[*byte]);
    }

    let (lines, _) = term.snapshot();
    assert!(
        lines[0].has_rtl,
        "Streamed Arabic line must be detected as RTL"
    );

    let reconstructed: String = lines[0].cells[0..arabic_phrase.chars().count()]
        .iter()
        .map(|c| c.c)
        .collect();
    assert_eq!(
        reconstructed, arabic_phrase,
        "Reconstructed characters must match original phrase exactly even when streamed 1 byte at a time"
    );
}

#[test]
fn test_ansi_escape_split_across_chunks() {
    let mut term = Terminal::new(80, 24, |_| {});

    // Split an ANSI color sequence and Arabic text across artificial chunk boundaries
    // Chunk 1: "\x1b[3"
    // Chunk 2: "1;1mم" (half of Arabic character 'م' [0xD9, 0x85])
    // Chunk 3: rest of 'م' and "رحبا\x1b[0m"
    let part1 = b"\x1b[3";
    let part2 = [b'1', b';', b'1', b'm', 0xD9];
    let part3 = [
        0x85, 0xD8, 0xB1, 0xD8, 0xAD, 0xD8, 0xA8, 0xD8, 0xA7, 0x1b, b'[', b'0', b'm',
    ];

    term.process_bytes(part1);
    term.process_bytes(&part2);
    term.process_bytes(&part3);

    let (lines, _) = term.snapshot();
    assert!(lines[0].has_rtl);
    let word: String = lines[0].cells[0..5].iter().map(|c| c.c).collect();
    assert_eq!(word, "مرحبا");
}

#[test]
fn test_fuzz_malformed_utf8_recovery() {
    let mut term = Terminal::new(80, 24, |_| {});

    // Random corrupted bytes and invalid UTF-8 codepoints
    let garbage = [0xFF, 0xFE, 0x80, 0xBF, 0xC0, 0xC1, 0xF5, 0xF6];
    term.process_bytes(&garbage);

    // Followed by valid Arabic text
    term.process_bytes("تويتي\r\n".as_bytes());

    // Terminal must not crash and must continue processing valid input cleanly
    let (lines, _) = term.snapshot();
    assert!(lines[0].has_rtl || lines[1].has_rtl);
}
