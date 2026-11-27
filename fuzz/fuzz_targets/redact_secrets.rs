//! Arbitrary strings passed to `redact()` must never panic and must stay bounded.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = twitty::redact::redact(text);
    }
});
