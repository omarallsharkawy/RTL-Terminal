use twitty::quad::try_render_box_or_block;

#[test]
fn test_box_and_block_rendering() {
    let mut quads = Vec::new();
    let color = [1.0, 1.0, 1.0, 1.0];

    // 1. Full block
    let is_block = try_render_box_or_block('█', 10.0, 20.0, 9.0, 22.0, color, &mut quads);
    assert!(is_block);
    assert_eq!(quads.len(), 1);
    assert_eq!(quads[0], (10.0, 20.0, 9.0, 22.0, color));

    // 2. Vertical line (box drawing)
    quads.clear();
    let is_vline = try_render_box_or_block('│', 10.0, 20.0, 9.0, 22.0, color, &mut quads);
    assert!(is_vline);
    assert_eq!(quads.len(), 1);

    // 3. Normal letters should not be intercepted as box/block
    quads.clear();
    let is_letter = try_render_box_or_block('A', 10.0, 20.0, 9.0, 22.0, color, &mut quads);
    assert!(!is_letter);
    assert!(quads.is_empty());
}
