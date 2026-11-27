//! Arbitrary git diff bytes must never panic or exceed parser bounds.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = twitty::review::diff::Limits::default();
    let _ = twitty::review::diff::parse(data, false, &limits);
    let _ = twitty::review::diff::parse(data, true, &limits);
});
