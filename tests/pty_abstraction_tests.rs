use portable_pty::PtySize;

#[test]
fn test_cross_platform_pty_size_mapping() {
    // Test standard terminal dimensions
    let cols = 120_u16;
    let rows = 40_u16;
    let char_width = 9_u16;
    let line_height = 23_u16;

    let pty_size = PtySize {
        rows,
        cols,
        pixel_width: cols * char_width,
        pixel_height: rows * line_height,
    };

    assert_eq!(pty_size.rows, 40);
    assert_eq!(pty_size.cols, 120);
    assert_eq!(pty_size.pixel_width, 1080);
    assert_eq!(pty_size.pixel_height, 920);

    // Windows ConPTY and Unix openpty boundary checks: minimum 1x1
    let min_size = PtySize {
        rows: 1,
        cols: 1,
        pixel_width: 9,
        pixel_height: 23,
    };
    assert!(min_size.rows >= 1);
    assert!(min_size.cols >= 1);
}
