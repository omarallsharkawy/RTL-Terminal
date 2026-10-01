use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use twitty::pty::Pty;
use twitty::terminal::Terminal;

#[test]
fn test_real_shell_prompt_generation() {
    let received_data = Arc::new(Mutex::new(Vec::new()));
    let rec_clone = received_data.clone();

    let mut term = Terminal::new(80, 24, |_| {});

    let pty = Pty::spawn(
        80,
        24,
        move |data| {
            rec_clone.lock().unwrap().extend_from_slice(&data);
        },
        || {},
    );
    assert!(pty.is_ok(), "Failed to spawn PTY");

    // Wait up to 1 second for Bash to output prompt
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(5000) {
        let data = {
            let mut guard = received_data.lock().unwrap();
            let d = guard.clone();
            guard.clear();
            d
        };
        if !data.is_empty() {
            term.process_bytes(&data);
            let (lines, cursor) = term.snapshot();
            println!(
                "Got bytes: len={}, cursor=({},{}) line0='{}'",
                data.len(),
                cursor.col,
                cursor.row,
                lines[0].cells[0..20]
                    .iter()
                    .map(|c| c.c)
                    .collect::<String>()
            );
            if cursor.col > 0 || lines[0].cells.iter().any(|c| c.c != ' ') {
                println!("Success! Prompt captured on line {}", cursor.row);
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let (lines, cursor) = term.snapshot();
    panic!(
        "Timed out waiting for prompt! Cursor at ({},{}), line0: '{}'",
        cursor.col,
        cursor.row,
        lines[0].cells[0..20]
            .iter()
            .map(|c| c.c)
            .collect::<String>()
    );
}
