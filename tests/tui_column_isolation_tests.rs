use twitty::terminal::Terminal;

#[test]
fn test_multi_column_spaces_separate_cells() {
    let mut term = Terminal::new(120, 24, |_| {});

    // Simulate OpenCode chat line with Arabic on the left and Sidebar on the right
    // Left: "Robustness: لو هيكراش" (~25 cols)
    // Gap: 30 spaces
    // Right: "$0.00 spent" (~11 cols)
    let left_text = "Robustness: لو هيكراش";
    let right_text = "$0.00 spent";
    let gap_spaces = " ".repeat(30);
    let full_line = format!("{}{}{}\r\n", left_text, gap_spaces, right_text);

    term.process_bytes(full_line.as_bytes());
    let (lines, _) = term.snapshot();
    let row0 = &lines[0];

    // The left text should reside starting at column 0
    let reconstructed_left: String = row0.cells[0..left_text.chars().count()]
        .iter()
        .map(|c| c.c)
        .collect();
    assert_eq!(reconstructed_left, left_text);

    // The space gap should be strictly empty spaces
    let gap_start = left_text.chars().count();
    let gap_end = gap_start + 30;
    for col in gap_start..gap_end {
        assert_eq!(
            row0.cells[col].c, ' ',
            "Gap cells must be spaces at col {}",
            col
        );
    }

    // The right text should start at gap_end (col 51)
    let reconstructed_right: String = row0.cells[gap_end..gap_end + right_text.chars().count()]
        .iter()
        .map(|c| c.c)
        .collect();
    assert_eq!(reconstructed_right, right_text);
}
