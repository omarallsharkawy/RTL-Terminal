use twitty::ime::{normalize_preedit, preedit_left, should_forward_key};

#[test]
fn keys_dropped_only_while_composing() {
    assert!(should_forward_key(false));
    assert!(!should_forward_key(true));
}

#[test]
fn empty_preedit_clears() {
    assert_eq!(normalize_preedit(String::new(), Some((0, 0))), None);
}

#[test]
fn preedit_cursor_is_clamped_to_char_boundaries() {
    // The first character is 3 bytes, so offset 1 is mid-character.
    let (text, cur) = normalize_preedit("啊b".into(), Some((1, 99))).unwrap();
    assert_eq!(text, "啊b");
    assert_eq!(cur, Some((0, 4)));
    assert_eq!(normalize_preedit("a".into(), None).unwrap().1, None);
}

#[test]
fn preedit_grows_right_after_latin_and_left_after_arabic() {
    assert_eq!(preedit_left(100.0, false, 40.0), 100.0);
    assert_eq!(preedit_left(100.0, true, 40.0), 60.0);
    // Never runs off the left edge of the window.
    assert_eq!(preedit_left(10.0, true, 40.0), 0.0);
}
