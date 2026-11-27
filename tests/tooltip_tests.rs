//! Hover tooltips: the resting delay, restarting on a new target, and the wording.
use std::time::{Duration, Instant};

use twitty::blocks::cards::BlockButton;
use twitty::ui::chrome::ChromeAction;
use twitty::ui::tooltip::{tip_text, TipTarget, TooltipState, DELAY};

#[test]
fn tooltip_waits_for_the_pointer_to_rest() {
    let start = Instant::now();
    let new_tab = Some(TipTarget::Chrome(ChromeAction::NewTab));
    let mut state = TooltipState::default();
    assert_eq!(state.update(new_tab, start), None);
    assert_eq!(state.pending(new_tab, start), Some(start + DELAY));
    let later = start + DELAY;
    assert_eq!(state.update(new_tab, later), new_tab);
    assert_eq!(state.pending(new_tab, later), None);
    assert!(state.tracking(new_tab));
}

#[test]
fn moving_to_another_button_starts_the_wait_again() {
    let start = Instant::now();
    let mut state = TooltipState::default();
    state.update(Some(TipTarget::Chrome(ChromeAction::NewTab)), start);
    let copy = Some(TipTarget::Block(BlockButton::CopyOutput));
    let moved = start + DELAY - Duration::from_millis(50);
    assert_eq!(state.update(copy, moved), None);
    assert_eq!(state.update(copy, start + DELAY), None);
    assert_eq!(state.update(copy, moved + DELAY), copy);
    // Leaving every button forgets the target.
    assert_eq!(state.update(None, moved + DELAY), None);
    assert!(!state.tracking(copy));
    assert_eq!(state.pending(copy, moved + DELAY), None);
}

#[test]
fn tooltip_names_the_button_and_its_shortcut() {
    let (label, command) = TipTarget::Chrome(ChromeAction::NewTab).label().unwrap();
    assert_eq!(command, Some("tab.new"));
    assert_eq!(
        tip_text(label, Some("Ctrl+Shift+T")),
        format!("{label}  Ctrl+Shift+T")
    );
    assert_eq!(tip_text(label, None), label);
    assert!(TipTarget::Chrome(ChromeAction::Search).label().is_none());
    assert_eq!(
        TipTarget::Block(BlockButton::Rerun).label().map(|(_, c)| c),
        Some(None)
    );
}
