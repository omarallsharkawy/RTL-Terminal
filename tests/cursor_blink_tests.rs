//! Cursor blink: timing state machine, config default and DECSCUSR handling.
use std::time::{Duration, Instant};
use twitty::blink::{BlinkState, BLINK_INTERVAL};
use twitty::config::TwittyConfig;
use twitty::terminal::Terminal;

const IV: Duration = Duration::from_millis(530);

fn blinking_state(now: Instant) -> BlinkState {
    let mut state = BlinkState::new(IV);
    state.set_enabled(now, true);
    state
}

#[test]
fn interval_is_530ms() {
    assert_eq!(BLINK_INTERVAL, Duration::from_millis(530));
}

#[test]
fn enabling_schedules_first_toggle_one_interval_later() {
    let t0 = Instant::now();
    let state = blinking_state(t0);
    assert!(state.visible());
    assert_eq!(state.next_deadline(), Some(t0 + IV));
}

#[test]
fn toggles_at_each_deadline() {
    let t0 = Instant::now();
    let mut state = blinking_state(t0);

    // Before the deadline nothing changes.
    assert!(!state.tick(t0 + IV - Duration::from_millis(1)));
    assert!(state.visible());

    // First flip: off.
    let t1 = t0 + IV;
    assert!(state.tick(t1));
    assert!(!state.visible());
    assert_eq!(state.next_deadline(), Some(t1 + IV));

    // Second flip: back on.
    let t2 = t1 + IV;
    assert!(state.tick(t2));
    assert!(state.visible());
    assert_eq!(state.next_deadline(), Some(t2 + IV));
}

#[test]
fn late_tick_flips_once_and_reschedules_from_now() {
    let t0 = Instant::now();
    let mut state = blinking_state(t0);
    let late = t0 + Duration::from_secs(5);
    assert!(state.tick(late));
    assert!(!state.visible());
    assert_eq!(state.next_deadline(), Some(late + IV));
}

#[test]
fn reset_makes_cursor_visible_and_restarts_timer() {
    let t0 = Instant::now();
    let mut state = blinking_state(t0);
    let t1 = t0 + IV;
    assert!(state.tick(t1));
    assert!(!state.visible());

    let typed = t1 + Duration::from_millis(100);
    state.reset(typed);
    assert!(state.visible());
    assert_eq!(state.next_deadline(), Some(typed + IV));
    // The old deadline no longer flips the phase.
    assert!(!state.tick(t1 + IV));
    assert!(state.visible());
}

#[test]
fn disabled_is_always_visible_without_deadline() {
    let t0 = Instant::now();
    let mut state = BlinkState::new(IV);
    assert!(state.visible());
    assert_eq!(state.next_deadline(), None);
    assert!(!state.tick(t0 + Duration::from_secs(10)));
    assert!(state.visible());

    state.set_enabled(t0, true);
    state.tick(t0 + IV);
    assert!(!state.visible());
    state.set_enabled(t0 + IV, false);
    assert!(state.visible());
    assert_eq!(state.next_deadline(), None);
    assert!(!state.tick(t0 + Duration::from_secs(10)));
    assert!(state.visible());
}

#[test]
fn unfocused_is_solid_without_deadline_and_resumes_on_focus() {
    let t0 = Instant::now();
    let mut state = blinking_state(t0);
    state.tick(t0 + IV);
    assert!(!state.visible());

    let lost = t0 + IV + Duration::from_millis(200);
    state.set_focused(lost, false);
    assert!(state.visible());
    assert_eq!(state.next_deadline(), None);
    assert!(!state.tick(lost + Duration::from_secs(10)));
    assert!(state.visible());

    let back = lost + Duration::from_secs(10);
    state.set_focused(back, true);
    assert!(state.visible());
    assert_eq!(state.next_deadline(), Some(back + IV));
}

#[test]
fn repeated_set_calls_do_not_restart_the_phase() {
    let t0 = Instant::now();
    let mut state = blinking_state(t0);
    state.tick(t0 + IV);
    // Same value again must not reset the cycle.
    state.set_enabled(t0 + IV + Duration::from_millis(50), true);
    state.set_focused(t0 + IV + Duration::from_millis(50), true);
    assert!(!state.visible());
    assert_eq!(state.next_deadline(), Some(t0 + 2 * IV));
}

#[test]
fn config_cursor_blink_defaults_to_true() {
    assert!(TwittyConfig::default().cursor_blink);
    let parsed: TwittyConfig = serde_json::from_str("{}").unwrap();
    assert!(parsed.cursor_blink);
}

#[test]
fn config_cursor_blink_parses_false() {
    let parsed: TwittyConfig = serde_json::from_str(r#"{"cursor_blink": false}"#).unwrap();
    assert!(!parsed.cursor_blink);
}

#[test]
fn decscusr_blinking_block_requests_blink() {
    let mut term = Terminal::new(80, 24, |_| {});
    term.process_bytes(b"\x1b[2 q");
    assert!(!term.cursor_blinking());
    term.process_bytes(b"\x1b[1 q");
    assert!(term.cursor_blinking());
}

#[test]
fn decscusr_zero_falls_back_to_configured_default() {
    let mut term = Terminal::new(80, 24, |_| {});
    term.set_default_cursor_blink(false);
    assert!(!term.cursor_blinking());
    term.process_bytes(b"\x1b[1 q");
    assert!(term.cursor_blinking());
    term.process_bytes(b"\x1b[0 q");
    assert!(!term.cursor_blinking());
}

#[test]
fn cursor_position_tracks_output() {
    let mut term = Terminal::new(80, 24, |_| {});
    let before = term.cursor_position();
    term.process_bytes(b"abc");
    assert_ne!(term.cursor_position(), before);
    let after = term.cursor_position();
    // Colour changes do not move the cursor.
    term.process_bytes(b"\x1b[31m");
    assert_eq!(term.cursor_position(), after);
}
