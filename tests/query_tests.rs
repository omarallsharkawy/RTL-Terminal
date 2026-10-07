use std::sync::{Arc, Mutex};
use twitty::terminal::Terminal;

#[test]
fn test_color_request_query() {
    let responses = Arc::new(Mutex::new(Vec::new()));
    let resp_clone = responses.clone();
    let mut term = Terminal::new(80, 24, move |resp| {
        resp_clone.lock().unwrap().push(resp);
    });

    // Send OSC 11;? BEL to query background color
    term.process_bytes(b"]11;?");

    let received = responses.lock().unwrap().clone();
    assert!(
        !received.is_empty(),
        "Terminal should respond to OSC 11 color query"
    );
    assert!(
        received[0].starts_with("]11;rgb:"),
        "Response should be an OSC 11 rgb sequence: {}",
        received[0]
    );
}

#[test]
fn test_text_area_size_request() {
    let responses = Arc::new(Mutex::new(Vec::new()));
    let resp_clone = responses.clone();
    let mut term = Terminal::new(80, 24, move |resp| {
        resp_clone.lock().unwrap().push(resp);
    });
    term.update_cell_size(12.0, 24.0);

    // Send CSI 14 t to query text area size in pixels
    term.process_bytes(b"[14t");

    let received = responses.lock().unwrap().clone();
    assert!(
        !received.is_empty(),
        "Terminal should respond to CSI 14t size query"
    );
    assert_eq!(received[0], "[4;576;960t");
}
