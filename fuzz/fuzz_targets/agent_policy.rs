//! Arbitrary shell proposals and patch texts checked by the agent command policy must never panic.
#![no_main]
use libfuzzer_sys::fuzz_target;
use std::path::Path;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let root = Path::new("/tmp/twitty-fuzz-root");
        let prefixes = vec![vec!["cargo".into(), "test".into()]];
        let _ = twitty_ai::engine::policy::command(root, root, text, &prefixes, false);
        let _ = twitty_ai::engine::policy::command(root, root, text, &prefixes, true);
        let _ = twitty_ai::engine::patch::ValidatedPatch::validate(root, text);
    }
});
