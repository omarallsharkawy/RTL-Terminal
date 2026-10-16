//! The queue between a PTY reader thread and the UI: batching, the size limit and closing.
use std::thread;
use std::time::Duration;

use twitty::pty::OutputQueue;

#[test]
fn only_output_into_an_empty_queue_wakes_the_ui() {
    let q = OutputQueue::default();
    assert!(q.push(b"ab"));
    assert!(!q.push(b"cd"), "the UI was already told");
    assert_eq!(q.take(), b"abcd");
    assert!(q.push(b"e"), "taken: the next output wakes it again");
}

#[test]
fn a_full_queue_holds_the_reader_until_the_ui_takes_it() {
    let q = OutputQueue::default();
    q.push(&vec![b'x'; OutputQueue::LIMIT]);
    let reader = {
        let q = q.clone();
        thread::spawn(move || q.push(b"y"))
    };
    thread::sleep(Duration::from_millis(50));
    assert!(!reader.is_finished(), "the reader waits for room");
    assert_eq!(q.take().len(), OutputQueue::LIMIT);
    assert!(reader.join().unwrap());
    assert_eq!(q.take(), b"y");
}

#[test]
fn closing_releases_a_waiting_reader() {
    let q = OutputQueue::default();
    q.push(&vec![b'x'; OutputQueue::LIMIT]);
    let reader = {
        let q = q.clone();
        thread::spawn(move || q.push(b"y"))
    };
    thread::sleep(Duration::from_millis(50));
    q.close();
    assert!(!reader.join().unwrap());
    assert!(q.take().is_empty());
}
