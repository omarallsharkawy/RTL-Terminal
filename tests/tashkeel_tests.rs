use twitty::terminal::Terminal;

#[test]
fn test_arabic_tashkeel_zerowidth_preservation() {
    let mut term = Terminal::new(80, 24, |_| {});

    // "مَرحَباً" with Fatha (U+064E) and Fathatan (U+064B)
    let tashkeel_text = "مَرحَباً\r\n";
    term.process_bytes(tashkeel_text.as_bytes());

    let (lines, _) = term.snapshot();
    let row0 = &lines[0];
    assert!(row0.has_rtl, "Tashkeel line must be detected as RTL");

    // Cell 0 is 'م' with zerowidth mark Fatha (U+064E)
    assert_eq!(row0.cells[0].c, 'م');
    assert!(
        !row0.cells[0].zerowidth.is_empty(),
        "First Arabic cell must retain zerowidth Fatha combining mark"
    );
    assert_eq!(row0.cells[0].zerowidth[0], '\u{064E}');

    // Cell 4 is 'ا' with zerowidth mark Fathatan (U+064B)
    assert_eq!(row0.cells[4].c, 'ا');
    assert!(
        !row0.cells[4].zerowidth.is_empty(),
        "Alef cell must retain zerowidth Fathatan combining mark"
    );
    assert_eq!(row0.cells[4].zerowidth[0], '\u{064B}');
}
