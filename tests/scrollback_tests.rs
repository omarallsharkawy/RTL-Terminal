use twitty::terminal::Terminal;

#[test]
fn test_scrollback_display_offset_viewport_mapping() {
    let mut term = Terminal::new(80, 24, |_| {});

    // Generate 60 lines of output (scrollback history of 36 lines)
    for i in 0..60 {
        let line = format!("Line {:03} of output history\r\n", i);
        term.process_bytes(line.as_bytes());
    }

    // 1. At bottom (display_offset == 0): screen shows lines 36 to 59
    let (lines_bottom, cursor) = term.snapshot();
    assert_eq!(lines_bottom.len(), 24);
    let first_line_str: String = lines_bottom[0].cells[0..7].iter().map(|c| c.c).collect();
    assert_eq!(first_line_str, "Line 03");
    assert!(
        cursor.is_visible,
        "Cursor must be visible at bottom of scrollback"
    );

    // 2. Scroll up by 10 lines
    term.scroll_display(10);
    let (lines_scrolled, cursor_scrolled) = term.snapshot();
    assert_eq!(lines_scrolled.len(), 24);

    // Top line should now be 10 lines earlier (Line 026)
    let scrolled_top_str: String = lines_scrolled[0].cells[0..7].iter().map(|c| c.c).collect();
    assert_eq!(
        scrolled_top_str, "Line 02",
        "Top line after scrolling up by 10 must map to earlier history row without disappearing"
    );

    // No cells should be empty or erased across all 24 viewport lines
    for r in 0..24 {
        let row_prefix: String = lines_scrolled[r].cells[0..4].iter().map(|c| c.c).collect();
        assert_eq!(
            row_prefix, "Line",
            "Row {} must contain valid history content, not blank/erased cells",
            r
        );
    }

    // Cursor has scrolled off the bottom of the viewport
    assert!(
        !cursor_scrolled.is_visible,
        "Cursor must be hidden while reviewing scrollback history above the bottom"
    );

    // 3. Scroll back to bottom
    term.scroll_display(-10);
    let (lines_restored, cursor_restored) = term.snapshot();
    assert!(cursor_restored.is_visible);
    let restored_top: String = lines_restored[0].cells[0..7].iter().map(|c| c.c).collect();
    assert_eq!(restored_top, "Line 03");
}
