//! Arbitrary IPC wire lines and v1 text commands must never panic during parsing or key encoding.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(line) = std::str::from_utf8(data) {
        let _ = twitty::control::protocol::parse_wire_line(line);
        let _ = twitty::control::protocol::parse_v1_line(line);
    }
});
