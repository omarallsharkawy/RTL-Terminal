//! Input-method (IME) support on the input side. The inline preedit is drawn by
//! `render::scene::preedit`, re-exported here.
//!
//! Avoiding double input (winit 0.30): a key the IME consumes is never delivered as
//! `KeyboardInput` (Wayland text-input grabs it, X11 drops it via `XFilterEvent`, Windows
//! turns it into `WM_IME_*`). Composed text arrives only as `Ime::Commit`, and plain keys
//! that the IME ignores arrive only as `KeyboardInput`. So the two paths never carry the same
//! character and no text de-duplication is needed. As a safety net, keyboard input is dropped
//! while a preedit is on screen, in case a platform leaks the key that drives composition.

pub use crate::render::scene::preedit::{bounds_outside, preedit_left, PreeditState};

/// Whether a `KeyboardInput` event may reach the shell given the IME state.
pub fn should_forward_key(preedit_active: bool) -> bool {
    !preedit_active
}

/// Normalise winit's preedit: an empty string clears it, and the cursor range is clamped to
/// char boundaries inside the text.
pub fn normalize_preedit(
    text: String,
    cursor: Option<(usize, usize)>,
) -> Option<(String, Option<(usize, usize)>)> {
    if text.is_empty() {
        return None;
    }
    let fix = |mut i: usize| {
        i = i.min(text.len());
        while !text.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let cursor = cursor.map(|(a, b)| (fix(a), fix(b)));
    Some((text, cursor))
}
