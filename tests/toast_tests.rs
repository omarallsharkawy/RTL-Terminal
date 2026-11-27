//! Notices: bounded stack, same text restarts, errors stay longer, control text is cleaned.
use std::time::{Duration, Instant};
use twitty::ui::toast::{ToastKind, Toasts, MAX_TOASTS};

#[test]
fn toasts_are_bounded_deduplicated_and_expire() {
    let now = Instant::now();
    let mut toasts = Toasts::default();
    for i in 0..5 {
        toasts.push(ToastKind::Info, format!("n{i}"), now);
    }
    let shown = toasts.visible();
    assert_eq!(shown.len(), MAX_TOASTS);
    assert_eq!(shown[0].text, "n2");

    toasts.push(ToastKind::Error, "boom", now);
    toasts.push(ToastKind::Error, "boom", now);
    assert_eq!(
        toasts.visible().iter().filter(|t| t.text == "boom").count(),
        1
    );

    assert!(toasts.prune(now + Duration::from_secs(3)));
    let left = toasts.visible();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].kind, ToastKind::Error);
    assert!(toasts.next_deadline().is_some());
    toasts.prune(now + Duration::from_secs(10));
    assert!(toasts.is_empty());
    assert_eq!(toasts.next_deadline(), None);
}

#[test]
fn toast_text_is_cleaned_and_empty_text_is_ignored() {
    let now = Instant::now();
    let mut toasts = Toasts::default();
    toasts.push(ToastKind::Error, "bad\x1b[31m red\x07", now);
    assert!(!toasts.visible()[0].text.contains('\x1b'));
    let mut empty = Toasts::default();
    empty.push(ToastKind::Info, "", now);
    assert!(empty.is_empty());
}
