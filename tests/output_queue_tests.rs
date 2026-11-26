//! The queue between a PTY reader thread and the UI: batching, the size limit and closing.
use std::thread;
use std::time::{Duration, Instant};

use twitty::layout::PaneId;
use twitty::pty::schedule::{OutputSchedule, CHUNK_BYTES};
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

#[test]
fn partial_drains_keep_fifo_order_and_need_no_extra_wake() {
    let q = OutputQueue::default();
    assert!(q.push(b"abcdefgh"));
    let mut chunk = Vec::new();

    assert!(q.take_into(&mut chunk, 5));
    assert_eq!(chunk, b"abcde");
    assert!(!q.push(b"ijkl"));

    assert!(q.take_into(&mut chunk, 4));
    assert_eq!(chunk, b"fghi");
    assert!(!q.take_into(&mut chunk, 4));
    assert_eq!(chunk, b"jkl");
    assert!(q.push(b"m"));
    assert_eq!(q.take(), b"m");
}

#[test]
fn a_full_queue_can_drain_in_one_budget() {
    let q = OutputQueue::default();
    assert!(q.push(&vec![b'x'; OutputQueue::LIMIT]));
    let pane = PaneId(1);
    let mut schedule = OutputSchedule::default();
    schedule.queue(pane);
    let mut now = Instant::now();
    let mut chunk = Vec::new();
    let mut drained = 0;

    while let Some(id) = schedule.next_pane(|| now) {
        assert_eq!(id, pane);
        let more = q.take_into(&mut chunk, CHUNK_BYTES);
        drained += chunk.len();
        schedule.chunk_drained(id, more);
        now += Duration::from_micros(100);
    }

    assert_eq!(drained, OutputQueue::LIMIT);
    assert_eq!(schedule.end_turn(now), None);
}

#[test]
fn a_busy_turn_yields_then_resumes_immediately_in_round_robin_order() {
    let mut schedule = OutputSchedule::default();
    let busy = PaneId(1);
    let other = PaneId(2);
    schedule.queue(busy);
    schedule.queue(busy);
    schedule.queue(other);
    assert!(schedule.defer_exit(busy, Some(7)));

    let start = Instant::now();
    assert_eq!(schedule.next_pane(|| start), Some(busy));
    assert_eq!(schedule.chunk_drained(busy, true), None);

    let end = start + Duration::from_millis(12);
    assert_eq!(schedule.next_pane(|| end), None);
    assert_eq!(schedule.end_turn(end), Some(end));
    assert_eq!(schedule.next_pane(|| end), Some(other));
    assert_eq!(schedule.chunk_drained(other, false), None);
    assert_eq!(schedule.next_pane(|| end), Some(busy));
    assert_eq!(schedule.chunk_drained(busy, false), Some(Some(7)));
    assert_eq!(schedule.end_turn(end), None);
}
