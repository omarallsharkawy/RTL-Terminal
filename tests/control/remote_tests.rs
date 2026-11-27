use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;
use twitty::control::cli::{build_params, parse_args};
use twitty::control::{MethodCall, Request};
use twitty::remote::bootstrap::{attach_args, bootstrap, Error, SshRunner};
use twitty::remote::{
    retry_delay, validate_destination, validate_name, AttachMarker, Target, ATTACH_MARKER,
};

#[test]
fn validates_ssh_destinations_and_session_names() {
    for destination in ["user@example.org", "my-alias", "user@[::1]", "خادم"] {
        assert!(validate_destination(destination).is_ok(), "{destination}");
    }
    for destination in [
        "",
        "-oProxyCommand=bad",
        "user@-host",
        "user@host another",
        "host\u{00a0}name",
        "host\nname",
        "host\u{7f}",
        "host\0",
        "host$(bad)",
        "host;bad",
        "host\u{202e}name",
    ] {
        assert!(
            validate_destination(destination).is_err(),
            "{destination:?}"
        );
    }

    for name in ["twitty-1a2b3c", "-_abc09"] {
        assert!(validate_name(name).is_ok());
    }
    assert!(validate_name(&"a".repeat(64)).is_ok());
    assert!(validate_name(&"a".repeat(65)).is_err());
    for name in [
        "",
        "../abc",
        "a/b",
        "a b",
        "a\nb",
        "a\0b",
        "جلسة",
        "\u{202e}a",
    ] {
        assert!(validate_name(name).is_err(), "{name:?}");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn generates_a_six_character_default_name() {
    let target = Target::for_tab("alias", None).unwrap();
    let suffix = target.name().strip_prefix("twitty-").unwrap();
    assert_eq!(suffix.len(), 6);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}

#[test]
fn cli_and_control_agree_on_the_ssh_request() {
    let args = ["ssh", "خادم", "--name", "editor"].map(str::to_owned);
    let action = parse_args(&args, None).unwrap();
    let (method, params) = build_params(&action).unwrap();
    assert_eq!(method, "tab.new-ssh");
    assert_eq!(params, json!({"destination": "خادم", "name": "editor"}));
    let parse = |params| {
        MethodCall::parse(&Request {
            id: json!(1),
            method: method.into(),
            params,
        })
    };
    assert_eq!(
        parse(params).unwrap(),
        MethodCall::TabNewSsh {
            destination: "خادم".into(),
            name: Some("editor".into())
        }
    );
    assert!(parse(json!({"destination": "server"})).is_ok());
    assert!(parse(json!({"destination": "-X", "name": "editor"})).is_err());
    assert!(parse(json!({"destination": "server", "name": "جلسة"})).is_err());
    assert!(parse(json!({"destination": "server", "name": 1})).is_err());
    assert!(parse_args(&["ssh".into(), "-X".into()], None).is_err());
}

#[test]
fn builds_ssh_argv_with_one_quoted_remote_command() {
    let target = Target::new("user@خادم", "-work_2").unwrap();
    let args = attach_args(&target, 122, 37);
    assert_eq!(args[..3], ["-t", "--", "user@خادم"]);
    assert_eq!(
        args[3],
        format!(
            "env TWITTY_REMOTE_MARKER=1 \"$HOME\"/'.cache/twitty/bin/twitty-remote-{}' attach '-work_2' --cols 122 --rows 37",
            env!("CARGO_PKG_VERSION")
        )
    );
    assert_eq!(args.len(), 4);
}

#[derive(Default)]
struct FakeSsh {
    replies: VecDeque<String>,
    calls: Vec<(String, String, Option<PathBuf>)>,
}

impl FakeSsh {
    fn replying(replies: &[&str]) -> Self {
        Self {
            replies: replies.iter().map(|reply| (*reply).into()).collect(),
            ..Self::default()
        }
    }
}

impl SshRunner for FakeSsh {
    fn run(
        &mut self,
        target: &Target,
        command: &str,
        upload: Option<&Path>,
    ) -> Result<String, Error> {
        self.calls.push((
            target.destination().into(),
            command.into(),
            upload.map(Path::to_path_buf),
        ));
        Ok(self.replies.pop_front().expect("unexpected SSH call"))
    }
}

#[test]
fn bootstrap_probes_hashes_installs_and_verifies() {
    let hash = "a".repeat(64);
    let wrong_hash = "b".repeat(64);
    let hash_line = format!("{hash}  -\n");
    let target = Target::new("example", "session").unwrap();
    let binary = Path::new("/path with spaces/twitty-remote");
    let mut cached = FakeSsh::replying(&["Linux x86_64\n", &hash_line]);
    bootstrap(&mut cached, &target, binary, &hash, "x86_64").unwrap();
    assert_eq!(cached.calls.len(), 2);
    assert_eq!(
        cached.calls[0],
        ("example".into(), "uname -sm".into(), None)
    );
    assert!(cached.calls[1].1.contains("sha256sum < \"$HOME\"/"));
    assert!(cached.calls[1]
        .1
        .contains(&format!("twitty-remote-{}", env!("CARGO_PKG_VERSION"))));

    for installed in ["", &wrong_hash] {
        let mut runner = FakeSsh::replying(&["Linux x86_64\n", installed, "", &hash_line]);
        bootstrap(&mut runner, &target, binary, &hash, "x86_64").unwrap();
        assert_eq!(runner.calls.len(), 4);
        let (_, upload_command, upload) = &runner.calls[2];
        assert_eq!(upload.as_deref(), Some(binary));
        assert!(upload_command.contains("mktemp"));
        assert!(upload_command.contains(&hash));
        assert!(upload_command.contains("mv -fT"));
        assert_eq!(runner.calls[1], runner.calls[3]);
    }

    let mut failed = FakeSsh::replying(&["Linux x86_64\n", "", "", &wrong_hash]);
    assert!(bootstrap(&mut failed, &target, binary, &hash, "x86_64")
        .unwrap_err()
        .to_string()
        .contains("verification failed"));
}

#[test]
fn bootstrap_rejects_wrong_platform_before_touching_cache() {
    let target = Target::new("example", "session").unwrap();
    for platform in ["Linux aarch64", "Darwin x86_64"] {
        let mut runner = FakeSsh::replying(&[platform]);
        let error = bootstrap(
            &mut runner,
            &target,
            Path::new("helper"),
            &"a".repeat(64),
            "x86_64",
        )
        .unwrap_err();
        assert!(error.to_string().contains(platform));
        assert_eq!(runner.calls.len(), 1);
    }
}

#[test]
fn reconnect_uses_five_exponential_delays() {
    for (attempt, secs) in [1, 2, 4, 8, 16].into_iter().enumerate() {
        assert_eq!(retry_delay(attempt), Some(Duration::from_secs(secs)));
    }
    assert_eq!(retry_delay(5), None);
    assert_eq!(retry_delay(usize::MAX), None);
}

#[test]
fn detects_attach_marker_split_across_reads_once() {
    let mut detector = AttachMarker::default();
    assert!(!detector.push(b"first banner \x1b]not the marker"));
    for byte in &ATTACH_MARKER[..ATTACH_MARKER.len() - 1] {
        assert!(!detector.push(&[*byte]));
    }
    assert!(detector.push(&ATTACH_MARKER[ATTACH_MARKER.len() - 1..]));
    assert!(!detector.push(ATTACH_MARKER));
}

#[cfg(target_os = "linux")]
mod remote_process {
    use std::fs;
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{self, Receiver};
    use std::time::{Duration, Instant};

    use twitty::remote::daemon::{ReplayBuffer, REPLAY_LIMIT};
    use twitty::remote::ATTACH_MARKER;

    const TIMEOUT: Duration = Duration::from_secs(8);

    #[test]
    fn replay_buffer_keeps_only_the_most_recent_bytes() {
        let mut tail = ReplayBuffer::default();
        tail.push(&vec![1; REPLAY_LIMIT]);
        tail.push(&[2, 3]);
        let bytes = tail.snapshot();
        assert_eq!(bytes.len(), REPLAY_LIMIT);
        assert_eq!(&bytes[REPLAY_LIMIT - 2..], &[2, 3]);
        assert!(bytes[..REPLAY_LIMIT - 2].iter().all(|byte| *byte == 1));

        let mut big = vec![4; REPLAY_LIMIT + 10];
        big[..10].fill(5);
        tail.push(&big);
        assert_eq!(tail.snapshot(), &big[10..]);
    }

    struct RemoteTemp {
        dir: PathBuf,
        name: String,
        use_xdg: bool,
    }

    impl RemoteTemp {
        fn new(use_xdg: bool) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "twitty-remote-test-{}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
                "long-path-".repeat(8),
            ));
            fs::create_dir(&dir).unwrap();
            Self {
                dir,
                name: "s".repeat(64),
                use_xdg,
            }
        }

        fn cmd(&self) -> Command {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_twitty-remote"));
            cmd.env_remove("XDG_RUNTIME_DIR")
                .env_remove("SSH_CONNECTION")
                .env("HOME", &self.dir)
                .env("SHELL", "/bin/sh")
                .env("TERM", "xterm-256color")
                .env("TWITTY_REMOTE_MARKER", "1")
                .env_remove("ENV");
            if self.use_xdg {
                cmd.env("XDG_RUNTIME_DIR", &self.dir);
            } else {
                // An SSH login must keep its socket even when logind removes this runtime dir.
                cmd.env("SSH_CONNECTION", "127.0.0.1 12345 127.0.0.1 22")
                    .env("XDG_RUNTIME_DIR", self.dir.join("transient"));
            }
            cmd
        }

        fn runtime(&self) -> PathBuf {
            self.dir.join(if self.use_xdg {
                "twitty-remote"
            } else {
                ".cache/twitty/remote"
            })
        }

        fn attach(&self, cols: &str, rows: &str) -> Attachment {
            Attachment::spawn(
                self.cmd()
                    .args(["attach", &self.name, "--cols", cols, "--rows", rows]),
            )
        }

        fn expect_stopped(&self) {
            let deadline = Instant::now() + TIMEOUT;
            while self.runtime().join(format!("{}.sock", self.name)).exists() {
                assert!(Instant::now() < deadline, "remote socket still exists");
                std::thread::sleep(Duration::from_millis(20));
            }
            let list = self.cmd().arg("list").output().unwrap();
            assert!(list.status.success());
            assert!(list.stdout.is_empty());
        }
    }

    impl Drop for RemoteTemp {
        fn drop(&mut self) {
            let _ = self.cmd().args(["kill", &self.name]).output();
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    struct Attachment {
        child: Child,
        chunks: Receiver<Vec<u8>>,
        seen: Vec<u8>,
    }

    impl Attachment {
        fn spawn(command: &mut Command) -> Self {
            let mut child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut stdout = child.stdout.take().unwrap();
            let (tx, chunks) = mpsc::channel();
            std::thread::spawn(move || {
                let mut bytes = [0; 4096];
                while let Ok(n @ 1..) = stdout.read(&mut bytes) {
                    if tx.send(bytes[..n].to_vec()).is_err() {
                        break;
                    }
                }
            });
            Self {
                child,
                chunks,
                seen: Vec::new(),
            }
        }

        fn wait_for(&mut self, needle: &[u8]) {
            let deadline = Instant::now() + TIMEOUT;
            while !self.seen.windows(needle.len()).any(|bytes| bytes == needle) {
                let next = self
                    .chunks
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .unwrap_or_else(|err| {
                        panic!(
                            "expected {:?} in {:?}: {err}",
                            String::from_utf8_lossy(needle),
                            String::from_utf8_lossy(&self.seen),
                        )
                    });
                self.seen.extend(next);
            }
        }

        fn send(&mut self, bytes: &[u8]) {
            self.child.stdin.as_mut().unwrap().write_all(bytes).unwrap();
        }

        fn exit_code(&mut self) -> i32 {
            let deadline = Instant::now() + TIMEOUT;
            loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    return status.code().unwrap_or(-1);
                }
                assert!(Instant::now() < deadline, "attachment did not exit");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    impl Drop for Attachment {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    #[test]
    fn a_real_login_shell_survives_detach_and_replays_then_exits() {
        let remote = RemoteTemp::new(true);
        let mut first = remote.attach("100", "30");
        first.wait_for(ATTACH_MARKER);
        first.send(b"state=still-here; printf 'twitty-%s\\n' replay\n");
        first.wait_for(b"twitty-replay");
        let path = remote.runtime();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let socket = path.join(format!("{}.sock", remote.name));
        assert_eq!(
            fs::metadata(socket).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(first);

        let list = remote.cmd().arg("list").output().unwrap();
        assert!(list.status.success());
        assert_eq!(String::from_utf8(list.stdout).unwrap().trim(), remote.name);

        let mut second = remote.attach("93", "27");
        second.wait_for(ATTACH_MARKER);
        second.wait_for(b"twitty-replay");
        second.send(b"printf 'value=%s\\n' \"$state\"; printf 'size=%s\\n' \"$(stty size)\"\n");
        second.wait_for(b"value=still-here");
        second.wait_for(b"size=27 93");
        second.send(b"exit 7\n");
        assert_eq!(second.exit_code(), 7);
        remote.expect_stopped();
    }

    #[test]
    fn kill_notifies_an_attachment_and_removes_the_fallback_socket() {
        let remote = RemoteTemp::new(false);
        let mut first = remote.attach("80", "24");
        first.wait_for(ATTACH_MARKER);
        let kill = remote.cmd().args(["kill", &remote.name]).output().unwrap();
        assert!(kill.status.success());
        assert_ne!(first.exit_code(), 0);
        remote.expect_stopped();
    }
}
