use std::sync::{Arc, Mutex};
use twitty::terminal::Terminal;

#[test]
fn test_osc52_clipboard_store() {
    let stored_clipboard = Arc::new(Mutex::new(None));
    let stored_clone = stored_clipboard.clone();

    let mut term = Terminal::new_with_clipboard(
        80,
        24,
        |_| {},
        move |text| {
            *stored_clone.lock().unwrap() = Some(text);
        },
    );

    // OSC 52 sequence: ESC ] 52 ; c ; <base64> BEL
    // "dGVzdF9vc2M1Ml9jb3B5" is "test_osc52_copy" in base64
    let osc52_sequence = b"\x1b]52;c;dGVzdF9vc2M1Ml9jb3B5\x07";
    term.process_bytes(osc52_sequence);

    let result = stored_clipboard.lock().unwrap().clone();
    assert_eq!(
        result,
        Some("test_osc52_copy".to_string()),
        "OSC 52 sequence must dispatch decoded string to clipboard store callback"
    );
}
