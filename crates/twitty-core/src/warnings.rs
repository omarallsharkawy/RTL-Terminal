//! Configuration warnings shared with the app, AI, and renderer.

use std::sync::Mutex;

static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Report a configuration problem: printed to stderr and kept for `take_warnings`, so the
/// app can also show it in the terminal (a Windows GUI app has no visible stderr).
/// Control and bidi-override characters are stripped so paths/errors cannot inject escapes.
#[doc(hidden)]
pub fn warn(message: String) {
    let clean = crate::shell_integration::clean_text(&message, 512);
    if clean.is_empty() {
        return;
    }
    tracing::warn!(target: "twitty::config", "{clean}");
    if let Ok(mut warnings) = WARNINGS.lock() {
        warnings.push(clean);
    }
}

/// Problems reported since the last call, oldest first.
pub fn take_warnings() -> Vec<String> {
    WARNINGS
        .lock()
        .map(|mut w| std::mem::take(&mut *w))
        .unwrap_or_default()
}
