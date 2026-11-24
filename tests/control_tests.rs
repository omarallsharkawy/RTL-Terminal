//! Window-free tests for the control socket protocol, path/permission checks, server round trips,
//! size limits, CLI argument parsing and `Terminal::recent_lines`.

use std::ffi::OsString;
use std::io::Cursor;
use std::path::PathBuf;

use serde_json::{json, Value};
use twitty::control::cli::{build_params, format_list, help_text, parse_args, CliAction};
use twitty::control::paths::{parse_socket_pid, runtime_dir_from, socket_filename};
use twitty::control::protocol::{
    parse_request_line, read_bounded_line, MethodCall, ReadLine, Request, Response,
    DEFAULT_READ_LINES, MAX_COMMAND_ARGS, MAX_COMMAND_BYTES, MAX_READ_LINES, MAX_SEND_TEXT_BYTES,
};
use twitty::layout::SplitDir;
use twitty::terminal::Terminal;

fn s(items: &[&str]) -> Vec<String> {
    items.iter().map(|v| (*v).to_owned()).collect()
}

// ---- protocol parsing and validation ----

#[test]
fn request_line_parsing_and_errors() {
    let req = parse_request_line(r#"{"id": 42, "method": "system.ping"}"#).unwrap();
    assert_eq!(req.id, json!(42));
    assert_eq!(req.method, "system.ping");
    assert_eq!(req.params, json!({}));

    let req = parse_request_line(r#"{"id": "abc", "method": "tab.list", "params": null}"#).unwrap();
    assert_eq!(req.id, json!("abc"));
    assert_eq!(req.params, json!({}));

    let err = parse_request_line("").unwrap_err();
    assert!(!err.ok);
    assert_eq!(err.id, Value::Null);

    let err = parse_request_line("{not json").unwrap_err();
    assert!(!err.ok);
    assert!(err.error.unwrap().contains("invalid JSON"));

    let err = parse_request_line("[1, 2, 3]").unwrap_err();
    assert!(!err.ok);

    let err = parse_request_line(r#"{"id": 7}"#).unwrap_err();
    assert_eq!(err.id, json!(7));
    assert!(!err.ok);

    let err = parse_request_line(r#"{"id": 8, "method": "tab.list", "params": [1]}"#).unwrap_err();
    assert_eq!(err.id, json!(8));
    assert!(!err.ok);
}

#[test]
fn method_call_parsing_covers_all_methods_and_limits() {
    let make = |method: &str, params: Value| Request {
        id: json!(1),
        method: method.into(),
        params,
    };

    assert_eq!(
        MethodCall::parse(&make("system.ping", json!({}))).unwrap(),
        MethodCall::SystemPing
    );
    assert_eq!(
        MethodCall::parse(&make("command.list", json!({}))).unwrap(),
        MethodCall::CommandList
    );
    assert_eq!(
        MethodCall::parse(&make(
            "command.run",
            json!({"name": "split.right", "pane": 3})
        ))
        .unwrap(),
        MethodCall::CommandRun {
            name: "split.right".into(),
            pane: Some(3),
        }
    );
    assert_eq!(
        MethodCall::parse(&make("tab.list", json!({}))).unwrap(),
        MethodCall::TabList
    );
    assert_eq!(
        MethodCall::parse(&make("pane.list", json!({}))).unwrap(),
        MethodCall::PaneList
    );
    assert_eq!(
        MethodCall::parse(&make("pane.focus", json!({"pane": 5}))).unwrap(),
        MethodCall::PaneFocus { pane: 5 }
    );
    assert!(MethodCall::parse(&make("pane.focus", json!({}))).is_err());

    // pane.send-text size limit (64 KiB)
    let max_text = "a".repeat(MAX_SEND_TEXT_BYTES);
    assert!(MethodCall::parse(&make("pane.send-text", json!({"text": max_text}))).is_ok());
    let too_big = "a".repeat(MAX_SEND_TEXT_BYTES + 1);
    assert!(MethodCall::parse(&make("pane.send-text", json!({"text": too_big}))).is_err());

    // pane.read default and max lines
    assert_eq!(
        MethodCall::parse(&make("pane.read", json!({}))).unwrap(),
        MethodCall::PaneRead {
            pane: None,
            lines: DEFAULT_READ_LINES,
        }
    );
    assert_eq!(
        MethodCall::parse(&make(
            "pane.read",
            json!({"pane": 2, "lines": MAX_READ_LINES})
        ))
        .unwrap(),
        MethodCall::PaneRead {
            pane: Some(2),
            lines: MAX_READ_LINES,
        }
    );
    assert!(MethodCall::parse(&make("pane.read", json!({"lines": MAX_READ_LINES + 1}))).is_err());

    // tab.new and pane.split
    assert_eq!(
        MethodCall::parse(&make(
            "tab.new",
            json!({"cwd": "/tmp", "command": ["echo", "مرحبا"]})
        ))
        .unwrap(),
        MethodCall::TabNew {
            cwd: Some(PathBuf::from("/tmp")),
            command: s(&["echo", "مرحبا"]),
        }
    );
    assert!(MethodCall::parse(&make("tab.new", json!({"command": []}))).is_err());
    assert!(MethodCall::parse(&make("tab.new", json!({"command": [""]}))).is_err());
    assert!(MethodCall::parse(&make("tab.new", json!({"command": ["a\0b"]}))).is_err());
    assert!(MethodCall::parse(&make("tab.new", json!({"cwd": "/tmp/a\nb"}))).is_err());

    let too_many_args: Vec<String> = (0..=MAX_COMMAND_ARGS).map(|_| "x".into()).collect();
    assert!(MethodCall::parse(&make("tab.new", json!({"command": too_many_args}))).is_err());
    let huge_arg = "x".repeat(MAX_COMMAND_BYTES + 1);
    assert!(MethodCall::parse(&make("tab.new", json!({"command": [huge_arg]}))).is_err());

    assert_eq!(
        MethodCall::parse(&make("pane.split", json!({"pane": 1, "direction": "down"}))).unwrap(),
        MethodCall::PaneSplit {
            pane: Some(1),
            direction: SplitDir::Vertical,
            cwd: None,
            command: vec![],
        }
    );
    assert_eq!(
        MethodCall::parse(&make("pane.split", json!({"direction": "right"}))).unwrap(),
        MethodCall::PaneSplit {
            pane: None,
            direction: SplitDir::Horizontal,
            cwd: None,
            command: vec![],
        }
    );
    assert!(MethodCall::parse(&make("pane.split", json!({"direction": "up"}))).is_err());

    // notify cleans control and bidi-override characters and preserves Arabic
    assert_eq!(
        MethodCall::parse(&make(
            "notify",
            json!({"pane": 4, "title": "Claude\u{202E}", "body": "خلصت\x07 الشغل"})
        ))
        .unwrap(),
        MethodCall::Notify {
            pane: Some(4),
            title: Some("Claude".into()),
            body: "خلصت الشغل".into(),
        }
    );
    assert!(MethodCall::parse(&make("notify", json!({"body": "   \x07"}))).is_err());
    assert!(MethodCall::parse(&make("no.such", json!({}))).is_err());
}

#[test]
fn bounded_line_reader_enforces_limit() {
    let mut exact = Cursor::new(b"12345\n".to_vec());
    assert_eq!(
        read_bounded_line(&mut exact, 5).unwrap(),
        ReadLine::Line("12345".into())
    );
    assert_eq!(read_bounded_line(&mut exact, 5).unwrap(), ReadLine::Eof);

    let mut crlf = Cursor::new(b"123\r\n".to_vec());
    assert_eq!(
        read_bounded_line(&mut crlf, 5).unwrap(),
        ReadLine::Line("123".into())
    );

    let mut over = Cursor::new(b"123456\n".to_vec());
    assert_eq!(read_bounded_line(&mut over, 5).unwrap(), ReadLine::TooLong);

    let mut no_nl_over = Cursor::new(b"1234567890".to_vec());
    assert_eq!(
        read_bounded_line(&mut no_nl_over, 5).unwrap(),
        ReadLine::TooLong
    );
}

// ---- path selection ----

#[test]
fn runtime_dir_and_socket_filename_selection() {
    assert_eq!(
        runtime_dir_from(Some(OsString::from("/run/user/1000")), 1000),
        PathBuf::from("/run/user/1000/twitty")
    );
    assert_eq!(
        runtime_dir_from(Some(OsString::from("")), 1000),
        PathBuf::from("/tmp/twitty-1000")
    );
    assert_eq!(
        runtime_dir_from(None, 501),
        PathBuf::from("/tmp/twitty-501")
    );

    assert_eq!(socket_filename(42), "twitty-42.sock");
    assert_eq!(parse_socket_pid("twitty-42.sock"), Some(42));
    assert_eq!(parse_socket_pid("twitty-.sock"), None);
    assert_eq!(parse_socket_pid("twitty-abc.sock"), None);
    assert_eq!(parse_socket_pid("twitty-42.sock.bak"), None);
    assert_eq!(parse_socket_pid("other-42.sock"), None);
}

// ---- CLI argument parsing and formatting ----

#[test]
fn cli_arg_parsing() {
    assert_eq!(parse_args(&[], None).unwrap(), CliAction::RunGui);
    assert_eq!(parse_args(&s(&["--help"]), None).unwrap(), CliAction::Help);
    assert_eq!(parse_args(&s(&["-h"]), None).unwrap(), CliAction::Help);
    assert_eq!(
        parse_args(&s(&["--version"]), None).unwrap(),
        CliAction::Version
    );
    assert!(help_text().contains("twitty ctl"));
    assert!(help_text().contains("twitty notify"));
    assert!(help_text().contains("twitty send"));
    assert!(help_text().contains("twitty list"));
    assert!(help_text().contains("twitty read"));

    assert_eq!(
        parse_args(&s(&["ctl", "system.ping"]), None).unwrap(),
        CliAction::Ctl {
            method: "system.ping".into(),
            params: json!({}),
        }
    );
    assert_eq!(
        parse_args(&s(&["ctl", "pane.focus", r#"{"pane": 2}"#]), None).unwrap(),
        CliAction::Ctl {
            method: "pane.focus".into(),
            params: json!({"pane": 2}),
        }
    );
    assert!(parse_args(&s(&["ctl"]), None).is_err());
    assert!(parse_args(&s(&["ctl", "system.ping", "[1]"]), None).is_err());
    assert!(parse_args(&s(&["ctl", "system.ping", "{bad"]), None).is_err());

    // notify uses TWITTY_PANE_ID by default, overridden by --pane
    assert_eq!(
        parse_args(
            &s(&["notify", "--title", "Agent", "تم", "التنفيذ"]),
            Some("7")
        )
        .unwrap(),
        CliAction::Notify {
            pane: Some(7),
            title: Some("Agent".into()),
            body: "تم التنفيذ".into(),
        }
    );
    assert_eq!(
        parse_args(&s(&["notify", "--pane=9", "hi"]), Some("7")).unwrap(),
        CliAction::Notify {
            pane: Some(9),
            title: None,
            body: "hi".into(),
        }
    );
    assert!(parse_args(&s(&["notify"]), None).is_err());
    assert!(parse_args(&s(&["notify", "--title"]), None).is_err());

    // send
    assert_eq!(
        parse_args(&s(&["send", "ls -la\n"]), Some("3")).unwrap(),
        CliAction::Send {
            pane: Some(3),
            text: "ls -la\n".into(),
        }
    );
    assert_eq!(
        parse_args(&s(&["send", "--pane", "5", "--", "-v"]), None).unwrap(),
        CliAction::Send {
            pane: Some(5),
            text: "-v".into(),
        }
    );
    assert!(parse_args(&s(&["send"]), None).is_err());

    // list
    assert_eq!(parse_args(&s(&["list"]), None).unwrap(), CliAction::List);
    assert!(parse_args(&s(&["list", "extra"]), None).is_err());

    // read
    assert_eq!(
        parse_args(&s(&["read", "--lines", "100"]), Some("4")).unwrap(),
        CliAction::Read {
            pane: Some(4),
            lines: Some(100),
        }
    );
    assert_eq!(
        parse_args(&s(&["read", "-p", "2", "-n", "25"]), Some("4")).unwrap(),
        CliAction::Read {
            pane: Some(2),
            lines: Some(25),
        }
    );
    assert!(parse_args(&s(&["read", "--lines", "nope"]), None).is_err());
    assert!(parse_args(&s(&["unknown"]), None).is_err());

    // build_params
    let notify_action = CliAction::Notify {
        pane: Some(2),
        title: Some("T".into()),
        body: "B".into(),
    };
    let (method, params) = build_params(&notify_action).unwrap();
    assert_eq!(method, "notify");
    assert_eq!(params, json!({"pane": 2, "title": "T", "body": "B"}));
}

#[test]
fn format_list_renders_tabs_panes_and_arabic() {
    let tabs = json!([
        {"index": 0, "title": "مشروع", "active": true, "panes": [1, 2]},
        {"index": 1, "title": "Tab 2", "active": false, "panes": [3]}
    ]);
    let panes = json!([
        {
            "id": 1,
            "tab": 0,
            "focused": true,
            "title": "bash",
            "cwd": "/home/u/مشروع",
            "running": true,
            "last_exit": null,
            "attention": null,
            "size": {"cols": 80, "rows": 24}
        },
        {
            "id": 2,
            "tab": 0,
            "focused": false,
            "title": "claude",
            "cwd": "/tmp",
            "running": false,
            "last_exit": 0,
            "attention": {"title": "Claude", "body": "جاهز"},
            "size": {"cols": 80, "rows": 24}
        },
        {
            "id": 3,
            "tab": 1,
            "focused": false,
            "title": null,
            "cwd": null,
            "running": false,
            "last_exit": 1,
            "attention": null,
            "size": {"cols": 100, "rows": 30}
        }
    ]);
    let rendered = format_list(&tabs, &panes);
    assert!(rendered.contains("Tab 0 [active]: مشروع"));
    assert!(rendered.contains("* pane 1  80x24  /home/u/مشروع  [running]  bash"));
    assert!(rendered.contains("  pane 2  80x24  /tmp  [exit 0]  claude  [!] Claude: جاهز"));
    assert!(rendered.contains("Tab 1: Tab 2"));
    assert!(rendered.contains("  pane 3  100x30  [exit 1]"));
}

// ---- Terminal::recent_lines ----

#[test]
fn terminal_recent_lines_reads_logical_scrollback_and_screen() {
    let mut term = Terminal::new(40, 4, |_| {});
    assert_eq!(term.recent_lines(50), "");
    assert_eq!(term.recent_lines(0), "");

    // Write more lines than the 4-row screen so older lines move into scrollback.
    term.process_bytes(
        "line 1\r\nline 2\r\nمَرْحَباً بالعالم 123\r\nline 4\r\nline 5\r\nline 6".as_bytes(),
    );
    assert_eq!(
        term.recent_lines(50),
        "line 1\nline 2\nمَرْحَباً بالعالم 123\nline 4\nline 5\nline 6"
    );
    assert_eq!(term.recent_lines(3), "line 4\nline 5\nline 6");
    assert_eq!(term.recent_lines(1), "line 6");
}

// ---- Unix-specific permission, stale cleanup, and real socket round-trip tests ----

#[cfg(unix)]
mod unix_tests {
    use super::*;
    use std::fs::{self, Permissions};
    use std::io::{BufReader, Write};
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use twitty::control::client::Client;
    use twitty::control::paths::{
        cleanup_stale_sockets, current_uid, ensure_runtime_dir, find_newest_socket_in,
        socket_path_in, verify_dir,
    };
    use twitty::control::server::{ControlServer, ServerConfig};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "twitty-ctl-test-{}-{}-{n}",
                std::process::id(),
                label
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn dir_permission_and_ownership_checks() {
        let tmp = TempDir::new("perms");
        let uid = current_uid().unwrap();

        let good = tmp.0.join("good");
        ensure_runtime_dir(&good, uid).unwrap();
        let mode = fs::symlink_metadata(&good).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o700);
        assert!(verify_dir(&good, uid).is_ok());

        // Wrong expected UID must be rejected.
        assert!(verify_dir(&good, uid.wrapping_add(1)).is_err());

        // Group or world accessible directories must be rejected.
        for bad_mode in [0o750, 0o755, 0o705, 0o777, 0o710, 0o701] {
            let d = tmp.0.join(format!("bad-{bad_mode:o}"));
            fs::create_dir(&d).unwrap();
            fs::set_permissions(&d, Permissions::from_mode(bad_mode)).unwrap();
            assert!(
                verify_dir(&d, uid).is_err(),
                "mode {bad_mode:o} should be refused"
            );
            assert!(ensure_runtime_dir(&d, uid).is_err());
        }

        // Symlink to a 0700 directory must be rejected.
        let link = tmp.0.join("link");
        symlink(&good, &link).unwrap();
        assert!(verify_dir(&link, uid).is_err());

        // Regular file must be rejected.
        let file = tmp.0.join("file");
        fs::write(&file, b"hi").unwrap();
        assert!(verify_dir(&file, uid).is_err());
    }

    #[test]
    fn stale_socket_cleanup_removes_dead_pid_sockets() {
        let tmp = TempDir::new("stale");
        let dir = tmp.0.join("run");
        ensure_runtime_dir(&dir, current_uid().unwrap()).unwrap();

        // Bind then drop a listener with a non-existent PID so the socket file stays behind.
        let dead_sock = socket_path_in(&dir, 999_999_999);
        {
            let _listener = UnixListener::bind(&dead_sock).unwrap();
        }
        assert!(dead_sock.exists());

        // Non-socket file matching the name pattern should not be removed.
        let regular = dir.join("not-a-socket.txt");
        fs::write(&regular, b"keep").unwrap();

        let removed = cleanup_stale_sockets(&dir).unwrap();
        assert_eq!(removed, 1);
        assert!(!dead_sock.exists());
        assert!(regular.exists());
    }

    #[test]
    fn real_socket_round_trip_with_fake_handler_and_limits() {
        let tmp = TempDir::new("roundtrip");
        let dir = tmp.0.join("run");
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = Arc::clone(&calls);

        let handler = Arc::new(move |req: Request| {
            calls_clone.fetch_add(1, Ordering::Relaxed);
            match req.method.as_str() {
                "system.ping" => Response::ok(req.id, json!({"version": "test", "pid": 123})),
                "echo" => Response::ok(req.id, req.params),
                other => Response::err(req.id, format!("unknown: {other}")),
            }
        });

        let cfg = ServerConfig {
            max_clients: 2,
            read_timeout: Duration::from_millis(300),
            write_timeout: Duration::from_secs(2),
            max_line_bytes: 256,
        };

        let server = ControlServer::start_in_dir(&dir, handler, cfg).unwrap();
        let sock_path = server.path().unwrap().to_path_buf();
        assert!(sock_path.exists());

        // Socket file itself must have mode 0600.
        let sock_mode = fs::symlink_metadata(&sock_path).unwrap().mode() & 0o777;
        assert_eq!(sock_mode, 0o600);

        // Discovery finds the live socket.
        let found = find_newest_socket_in(&dir, current_uid().unwrap()).unwrap();
        assert_eq!(found, sock_path);

        // Multiple requests over a single client connection, including Arabic UTF-8.
        let mut client = Client::connect_path(&sock_path).unwrap();
        let r1 = client.call("system.ping", json!({})).unwrap();
        assert!(r1.ok);
        assert_eq!(r1.result, Some(json!({"version": "test", "pid": 123})));

        let r2 = client.call("echo", json!({"text": "أهلاً بالعالم"})).unwrap();
        assert!(r2.ok);
        assert_eq!(r2.result, Some(json!({"text": "أهلاً بالعالم"})));

        let r3 = client.call("missing.method", json!({})).unwrap();
        assert!(!r3.ok);
        assert_eq!(r3.error.as_deref(), Some("unknown: missing.method"));
        drop(client);

        // Oversized request line returns an error and closes the connection.
        {
            let mut raw = UnixStream::connect(&sock_path).unwrap();
            raw.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let big = format!("{}\n", "x".repeat(400));
            raw.write_all(big.as_bytes()).unwrap();
            let mut reader = BufReader::new(raw);
            let ReadLine::Line(resp_line) = read_bounded_line(&mut reader, 4096).unwrap() else {
                panic!("expected error response for oversized line");
            };
            let resp: Response = serde_json::from_str(&resp_line).unwrap();
            assert!(!resp.ok);
            assert!(resp.error.unwrap().contains("exceeds"));
            assert_eq!(
                read_bounded_line(&mut reader, 4096).unwrap(),
                ReadLine::Eof,
                "server must close connection after oversized line"
            );
        }

        // Concurrent client limit: 2 held connections, 3rd is refused immediately.
        let hold1 = UnixStream::connect(&sock_path).unwrap();
        let hold2 = UnixStream::connect(&sock_path).unwrap();
        // Send a ping on each so both threads are definitely spawned and holding slots.
        let mut hold1_client = {
            let mut h = hold1;
            h.write_all(b"{\"id\":1,\"method\":\"system.ping\"}\n")
                .unwrap();
            let mut r = BufReader::new(h);
            assert!(matches!(
                read_bounded_line(&mut r, 4096).unwrap(),
                ReadLine::Line(_)
            ));
            r
        };
        let mut hold2_client = {
            let mut h = hold2;
            h.write_all(b"{\"id\":2,\"method\":\"system.ping\"}\n")
                .unwrap();
            let mut r = BufReader::new(h);
            assert!(matches!(
                read_bounded_line(&mut r, 4096).unwrap(),
                ReadLine::Line(_)
            ));
            r
        };

        // 3rd client must be refused with an error response.
        {
            let extra = UnixStream::connect(&sock_path).unwrap();
            extra
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut reader = BufReader::new(extra);
            let ReadLine::Line(line) = read_bounded_line(&mut reader, 4096).unwrap() else {
                panic!("expected refusal JSON for 3rd client");
            };
            let resp: Response = serde_json::from_str(&line).unwrap();
            assert!(!resp.ok);
            assert!(resp.error.unwrap().contains("too many"));
        }

        // Idle timeout (300 ms) frees slots even while the idle sockets remain open on client side.
        std::thread::sleep(Duration::from_millis(450));
        assert_eq!(
            read_bounded_line(&mut hold1_client, 4096).unwrap(),
            ReadLine::Eof
        );
        assert_eq!(
            read_bounded_line(&mut hold2_client, 4096).unwrap(),
            ReadLine::Eof
        );
        drop(hold1_client);
        drop(hold2_client);

        // After idle slots are freed, a new client succeeds again.
        let mut after = Client::connect_path(&sock_path).unwrap();
        assert!(after.call("system.ping", json!({})).unwrap().ok);
        drop(after);

        assert!(calls.load(Ordering::Relaxed) >= 6);

        // Dropping the server removes the socket file.
        drop(server);
        assert!(!sock_path.exists());
    }

    #[test]
    fn newest_live_socket_skips_dead_sockets_and_picks_newest() {
        let tmp = TempDir::new("newest");
        let dir = tmp.0.join("run");
        let uid = current_uid().unwrap();
        ensure_runtime_dir(&dir, uid).unwrap();

        let noop = Arc::new(|req: Request| Response::ok(req.id, json!({})));
        let pid = std::process::id();

        // Bind a live socket for our PID (alive in /proc), plus a dead socket with a dead PID.
        let live =
            ControlServer::start_at_path(socket_path_in(&dir, pid), noop, ServerConfig::default())
                .unwrap();
        let dead_path = socket_path_in(&dir, 999_999_998);
        {
            let _dead = UnixListener::bind(&dead_path).unwrap();
        }

        let found = find_newest_socket_in(&dir, uid).unwrap();
        assert_eq!(found, live.path().unwrap());

        drop(live);
        assert!(find_newest_socket_in(&dir, uid).is_err());
    }
}
