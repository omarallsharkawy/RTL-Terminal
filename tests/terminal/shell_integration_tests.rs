//! Shell signals: OSC 7 working directory, OSC 9/99/777 notifications and OSC 133/633 marks.
use twitty::commands;
use twitty::shell_integration::{parse_osc, MarkKind, Signal, MAX_PENDING_EVENTS};
use twitty::terminal::Terminal;

fn term() -> Terminal {
    Terminal::new(40, 5, |_| {})
}

#[test]
fn cwd_from_osc7_local_only() {
    assert_eq!(
        parse_osc(b"7;file:///home/u/%D9%85%D8%B4%D8%B1%D9%88%D8%B9"),
        Some(Signal::Cwd("/home/u/مشروع".into()))
    );
    assert_eq!(
        parse_osc(b"7;file://localhost/tmp"),
        Some(Signal::Cwd("/tmp".into()))
    );
    assert_eq!(
        parse_osc(b"7;kitty-shell-cwd:///srv"),
        Some(Signal::Cwd("/srv".into()))
    );
    assert_eq!(parse_osc(b"7;file://evil.example/etc"), None);
    assert_eq!(
        parse_osc(b"7;file:///tmp/a%0Ab"),
        None,
        "control char in path"
    );
}

#[test]
fn notifications() {
    assert_eq!(
        parse_osc(b"9;Build done"),
        Some(Signal::Notify {
            title: None,
            body: "Build done".into()
        })
    );
    // ConEmu progress and other numbered OSC 9 commands are not notifications.
    assert_eq!(parse_osc(b"9;4;1;50"), None);
    assert_eq!(parse_osc(b"9;1"), None);
    assert_eq!(
        parse_osc("777;notify;Claude;محتاج موافقتك; يا باشا".as_bytes()),
        Some(Signal::Notify {
            title: Some("Claude".into()),
            body: "محتاج موافقتك; يا باشا".into()
        })
    );
    assert_eq!(parse_osc(b"777;preexec;x"), None);
    assert_eq!(
        parse_osc(b"99;i=1:p=body;Waiting for input"),
        Some(Signal::Notify {
            title: None,
            body: "Waiting for input".into()
        })
    );
    assert_eq!(parse_osc(b"99;d=0;part"), None, "chunked");
    assert_eq!(parse_osc(b"99;e=1;aGk="), None, "base64");
}

#[test]
fn notification_text_is_cleaned() {
    let Some(Signal::Notify { body, .. }) = parse_osc("9;a\u{202E}b\u{7}c\u{2066}d".as_bytes())
    else {
        panic!("not a notification");
    };
    assert_eq!(body, "abcd");
    let long = format!("9;{}", "x".repeat(10_000));
    let Some(Signal::Notify { body, .. }) = parse_osc(long.as_bytes()) else {
        panic!();
    };
    assert_eq!(body.chars().count(), 512);
}

#[test]
fn command_marks() {
    let mark = |s: &[u8]| parse_osc(s);
    assert_eq!(mark(b"133;A"), Some(Signal::Mark(MarkKind::PromptStart)));
    assert_eq!(
        mark(b"133;A;cl=m;aid=7"),
        Some(Signal::Mark(MarkKind::PromptStart))
    );
    assert_eq!(mark(b"133;B"), Some(Signal::Mark(MarkKind::CommandStart)));
    assert_eq!(mark(b"133;C"), Some(Signal::Mark(MarkKind::OutputStart)));
    assert_eq!(
        mark(b"133;D;127"),
        Some(Signal::Mark(MarkKind::CommandEnd { exit: Some(127) }))
    );
    assert_eq!(
        mark(b"133;D"),
        Some(Signal::Mark(MarkKind::CommandEnd { exit: None }))
    );
    assert_eq!(mark(b"133;Z"), None);
    assert_eq!(mark(b"133;AB"), None);
    assert_eq!(
        mark(b"633;E;ls\\x3b echo \\\\ok;nonce"),
        Some(Signal::CommandLine("ls; echo \\ok".into()))
    );
    assert_eq!(mark(b"133;E;ls"), None, "E is VS Code only");
}

#[test]
fn terminal_records_events_with_cursor_position() {
    let mut t = term();
    t.process_bytes(
        b"\x1b]133;A\x07$ \x1b]133;B\x1b\\ls\r\n\x1b]133;C\x07a\r\nb\r\n\x1b]133;D;0\x07",
    );
    let events = t.take_shell_events();
    let got: Vec<_> = events
        .iter()
        .map(|e| (e.signal.clone(), e.line, e.column))
        .collect();
    assert_eq!(
        got,
        vec![
            (Signal::Mark(MarkKind::PromptStart), 0, 0),
            (Signal::Mark(MarkKind::CommandStart), 0, 2),
            // No OSC 633 E came: the command line is read from the grid, B up to C.
            (Signal::CommandLine("ls".into()), 1, 0),
            (Signal::Mark(MarkKind::OutputStart), 1, 0),
            (Signal::Mark(MarkKind::CommandEnd { exit: Some(0) }), 3, 0),
        ]
    );
    assert!(t.take_shell_events().is_empty(), "drained");
}

#[test]
fn signal_split_across_reads() {
    let mut t = term();
    t.process_bytes(b"x\x1b]9;hel");
    assert!(t.take_shell_events().is_empty());
    t.process_bytes(b"lo\x1b");
    t.process_bytes(b"\\y");
    let events = t.take_shell_events();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].signal,
        Signal::Notify {
            title: None,
            body: "hello".into()
        }
    );
    let (lines, _) = t.snapshot();
    let text: String = lines[0].cells.iter().map(|c| c.c).collect();
    assert!(text.starts_with("xy"), "{text:?}");
}

#[test]
fn cancelled_and_oversized_oscs_are_not_signals() {
    let mut t = term();
    t.process_bytes(b"\x1b]9;nope\x18\x1b]9;");
    t.process_bytes(&vec![b'a'; 5000]);
    t.process_bytes(b"\x07");
    assert!(t.take_shell_events().is_empty());
}

#[test]
fn pending_events_are_bounded() {
    let mut t = term();
    let mut flood = Vec::new();
    for i in 0..1000 {
        flood.extend_from_slice(format!("\x1b]9;n{i}\x07").as_bytes());
    }
    t.process_bytes(&flood);
    let events = t.take_shell_events();
    assert_eq!(events.len(), MAX_PENDING_EVENTS);
    assert_eq!(
        events.last().unwrap().signal,
        Signal::Notify {
            title: None,
            body: "n999".into()
        }
    );
}

#[test]
fn other_oscs_still_work() {
    let mut t = term();
    t.process_bytes(b"\x1b]8;;https://example.com\x07link\x1b]8;;\x07");
    assert!(t.take_shell_events().is_empty());
    let (lines, _) = t.snapshot();
    let text: String = lines[0].cells.iter().map(|c| c.c).collect();
    assert!(text.starts_with("link"));
}

#[test]
fn command_names_are_unique_and_resolve() {
    let all = commands::all();
    let mut names: Vec<_> = all.iter().map(|c| c.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), all.len());
    for c in &all {
        assert!(commands::action(c.name).is_some(), "{}", c.name);
        assert!(!c.title_ar.is_empty() && !c.title_en.is_empty());
    }
    assert_eq!(commands::action("no.such"), None);
}
