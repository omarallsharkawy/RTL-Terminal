use std::collections::HashMap;
use std::time::{Duration, Instant};

use twitty::config::TwittyConfig;
use twitty::ui::motion::{
    ease_out, progress, working_dot, Highlights, Reorder, Tween, HOVER, PANEL,
};

#[test]
fn easing_is_bounded_and_decelerates() {
    assert_eq!(ease_out(-1.0), 0.0);
    assert_eq!(ease_out(2.0), 1.0);
    assert_eq!(progress(Duration::ZERO, Duration::ZERO), 1.0);
    let samples: Vec<_> = (0..=8).map(|i| ease_out(i as f32 / 8.0)).collect();
    let steps: Vec<_> = samples.windows(2).map(|pair| pair[1] - pair[0]).collect();
    assert!(steps.iter().all(|step| *step > 0.0));
    assert!(steps.windows(2).all(|pair| pair[0] > pair[1]));
}

#[test]
fn interrupted_timeline_keeps_position_and_schedules_its_endpoint() {
    let start = Instant::now();
    let mut tween = Tween::default();
    assert_eq!(tween.next_frame(start), None);
    tween.set(1.0, start, PANEL);
    assert_eq!(tween.value(start), 0.0);
    let reverse = start + PANEL / 3;
    let before = tween.value(reverse);
    tween.set(0.0, reverse, PANEL);
    assert_eq!(tween.value(reverse), before);
    assert!(tween.value(reverse + PANEL / 2) < before);
    let end = reverse + PANEL;
    assert_eq!(tween.next_frame(end - Duration::from_millis(1)), Some(end));
    assert_eq!(tween.value(end), 0.0);
    assert_eq!(tween.next_frame(end), None);
    tween.set(1.0, end, Duration::ZERO);
    assert_eq!(tween.value(end), 1.0);
    assert_eq!(tween.next_frame(end), None);
}

#[test]
fn highlights_crossfade_and_snap_when_reduced() {
    let start = Instant::now();
    let mut tabs = Highlights::default();
    tabs.set(Some(5), start, Duration::ZERO);
    tabs.set(Some(9), start, HOVER);
    let mid = start + HOVER / 2;
    assert!(tabs.value(5, mid) > 0.0);
    assert!(tabs.value(9, mid) > tabs.value(5, mid));
    assert_eq!(tabs.value(5, mid) + tabs.value(9, mid), 1.0);
    tabs.set(Some(7), mid, Duration::ZERO);
    assert_eq!(tabs.value(5, mid), 0.0);
    assert_eq!(tabs.value(9, mid), 0.0);
    assert_eq!(tabs.value(7, mid), 1.0);
    assert_eq!(tabs.next_frame(mid), None);
}

#[test]
fn reordering_uses_tab_identity_and_handles_a_second_move() {
    let now = Instant::now();
    let mut reorder = Reorder::default();
    reorder.update(&[1, 2], HashMap::from([(1, 10.0), (2, 80.0)]), now, false);
    assert_eq!(reorder.next_frame(now), None);
    reorder.update(&[2, 1], HashMap::from([(2, 10.0), (1, 65.0)]), now, false);
    assert_eq!(reorder.offset(1, now), -55.0);
    assert_eq!(reorder.offset(2, now), 70.0);
    let mid = now + PANEL / 2;
    let drawn = 65.0 + reorder.offset(1, mid);
    reorder.update(&[1, 2], HashMap::from([(1, 10.0), (2, 80.0)]), mid, false);
    assert_eq!(10.0 + reorder.offset(1, mid), drawn);
    assert_eq!(reorder.offset(1, mid + PANEL), 0.0);
    assert_eq!(reorder.next_frame(mid + PANEL), None);
}

#[test]
fn spinner_blends_across_its_loop_and_config_defaults_to_motion() {
    for i in 0..3 {
        assert_eq!(working_dot(0.0, i), working_dot(3.0, i));
        assert!((working_dot(2.999, i) - working_dot(0.001, i)).abs() < 0.001);
    }
    assert_eq!(working_dot(0.5, 0), working_dot(0.5, 1));
    let defaults: TwittyConfig = serde_json::from_str("{}").unwrap();
    let reduced: TwittyConfig = serde_json::from_str(r#"{"reduce_motion": true}"#).unwrap();
    assert!(!defaults.reduce_motion);
    assert!(reduced.reduce_motion);
}
