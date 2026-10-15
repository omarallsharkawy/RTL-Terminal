//! Arbitrary program output: parsing, resizing, selecting, searching and snapshotting must
//! never panic.
#![no_main]
use libfuzzer_sys::fuzz_target;
use twitty::terminal::Terminal;

fuzz_target!(|data: &[u8]| {
    let mut term = Terminal::new(40, 10, |_| {});
    let (head, tail) = data.split_at(data.len() / 2);
    term.process_bytes(head);
    let cols = 1 + data.first().copied().unwrap_or(0) as usize % 120;
    let rows = 1 + data.last().copied().unwrap_or(0) as usize % 40;
    term.resize(cols, rows);
    term.process_bytes(tail);
    let _ = term.snapshot();
    let _ = term.link_at(cols / 2, rows / 2);
});
