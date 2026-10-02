use twitty::terminal::SelectionType;
use twitty::terminal::Terminal;

#[test]
fn test_terminal_selection_lifecycle() {
    let mut term = Terminal::new(80, 24, |_| {});

    term.process_bytes(b"Twitty Terminal v2.0 Native Test\r\nLine 2 of output\r\n");

    // Initially no selection
    assert_eq!(term.selection_text(), None);

    // Select "Twitty" from column 0 to 5 on row 0
    term.start_selection(0, 0);
    term.update_selection(6, 0);

    let text = term.selection_text();
    assert!(text.is_some(), "Selection text should be present");
    let unwrapped = text.unwrap();
    assert!(
        unwrapped.contains("Twitty"),
        "Selected text should contain 'Twitty', got: {}",
        unwrapped
    );

    // Verify snapshot sets is_selected on affected cells
    let (lines, _) = term.snapshot();
    let row0 = &lines[0];
    assert!(row0.cells[0].is_selected, "Cell 0 should be selected");
    assert!(row0.cells[1].is_selected, "Cell 1 should be selected");
    assert!(row0.cells[2].is_selected, "Cell 2 should be selected");

    // Clear selection
    term.clear_selection();
    assert_eq!(term.selection_text(), None);

    let (lines_cleared, _) = term.snapshot();
    assert!(
        !lines_cleared[0].cells[0].is_selected,
        "Cell 0 should no longer be selected after clear"
    );
}

#[test]
fn test_terminal_select_all() {
    let mut term = Terminal::new(80, 24, |_| {});
    term.process_bytes(b"Line Alpha\r\nLine Beta\r\n");

    term.select_all();
    let selected = term.selection_text();
    assert!(selected.is_some(), "Select all should produce text");
    let content = selected.unwrap();
    assert!(content.contains("Line Alpha"));
    assert!(content.contains("Line Beta"));
}

#[test]
fn test_semantic_word_and_lines_selection() {
    let mut term = Terminal::new(80, 24, |_| {});
    term.process_bytes(b"hello opencode_user_test\r\n");

    // Double-click (Semantic) selection on the word "opencode_user_test"
    term.start_selection_type(8, 0, SelectionType::Semantic);
    let text = term.selection_text();
    assert!(text.is_some());
    let unwrapped = text.unwrap();
    assert!(
        unwrapped.contains("opencode_user_test"),
        "Semantic selection should select whole word, got: {}",
        unwrapped
    );

    // Triple-click (Lines) selection
    term.start_selection_type(3, 0, SelectionType::Lines);
    let line_text = term.selection_text().unwrap();
    assert!(line_text.contains("hello opencode_user_test"));
}

#[test]
fn test_selection_in_scrollback() {
    let mut term = Terminal::new(80, 24, |_| {});
    for i in 0..60 {
        let line = format!("Line {:03} of output history\r\n", i);
        term.process_bytes(line.as_bytes());
    }
    // Scroll up by 10
    term.scroll_display(10);
    assert_eq!(term.display_offset(), 10);

    // Select on row 0 in current viewport (which is Line 026)
    term.start_selection(0, 0);
    term.update_selection(8, 0);

    let text = term.selection_text();
    assert!(text.is_some());
    let unwrapped = text.unwrap();
    assert!(
        unwrapped.contains("Line 027"),
        "Selected scrolled text should contain 'Line 027', got: {}",
        unwrapped
    );
}
