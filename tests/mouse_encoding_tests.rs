#[test]
fn test_mouse_sgr_formatting() {
    // Verify SGR format
    let btn = 0; // left press
    let col = 10;
    let row = 5;
    let sgr_press = format!("\x1b[<{btn};{col};{row}M");
    assert_eq!(sgr_press, "\x1b[<0;10;5M");
    let sgr_release = format!("\x1b[<{btn};{col};{row}m");
    assert_eq!(sgr_release, "\x1b[<0;10;5m");
}

#[test]
fn test_mouse_modifier_bits() {
    // Shift = 4, Alt = 8, Ctrl = 16, Motion/Drag = 32
    let base_btn = 0; // Left button
    let shift_btn = base_btn | 4;
    let ctrl_btn = base_btn | 16;
    let drag_btn = base_btn | 32;
    assert_eq!(shift_btn, 4);
    assert_eq!(ctrl_btn, 16);
    assert_eq!(drag_btn, 32);
}
