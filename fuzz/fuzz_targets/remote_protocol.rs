//! Arbitrary remote target names, attach markers, and wire-protocol frames must never panic.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut marker = twitty_remote::AttachMarker::default();
    let _ = marker.push(data);
    #[cfg(target_os = "linux")]
    twitty_remote::daemon::fuzz_protocol_stream(data);
    if let Ok(text) = std::str::from_utf8(data) {
        let mut parts = text.splitn(2, '\n');
        let dest = parts.next().unwrap_or("");
        let name = parts.next().unwrap_or("");
        if let Ok(target) = twitty_remote::Target::new(dest, name) {
            let _ = twitty_remote::bootstrap::attach_args(&target, 80, 24);
        }
        let _ = twitty_remote::Target::for_tab(dest, (!name.is_empty()).then_some(name));
    }
});
