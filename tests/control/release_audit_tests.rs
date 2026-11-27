//! Regression tests for every in-scope release audit fix:
//! trust store & config safety, shell_setup atomic permissions, control socket split reasons
//! and clipboard-read blocking, link symlink-to-launcher checks, SSH validation/hardening,
//! terminal arithmetic bounds, workflow error sanitization, PTY bounds, and README/packaging sync.

use std::ffi::OsString;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;
use twitty::config::{take_warnings, TwittyConfig, MAX_CONFIG_BYTES};
use twitty::control::cmux::HeadlessHost;
use twitty::control::paths::runtime_dir_from;
use twitty::control::protocol::{read_bounded_line, MethodCall, ReadLine, Request};
use twitty::remote::bootstrap::{bootstrap, Error as BootstrapError, SshRunner};
use twitty::remote::{validate_destination, Target};
use twitty::terminal::Terminal;
#[cfg(unix)]
use twitty::workflows::sha256_hex;
use twitty::workflows::{resolve_project_cwd, TrustStore};

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("twitty-audit-{label}-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn trust_store_ignores_relative_env_paths_and_refuses_unsafe_store_files() {
    use std::ffi::OsStr;

    let home_dir = if cfg!(windows) {
        r"C:\Users\u"
    } else {
        "/home/u"
    };
    let expected_home = PathBuf::from(home_dir)
        .join(".local")
        .join("state")
        .join("twitty")
        .join("trust.json");

    // Relative XDG_STATE_HOME must be ignored in favor of absolute HOME.
    assert_eq!(
        TrustStore::path_from(
            Some(OsStr::new("relative/state")),
            Some(OsStr::new(home_dir))
        ),
        Some(expected_home)
    );
    // Relative HOME yields None.
    assert_eq!(
        TrustStore::path_from(None, Some(OsStr::new("relative/home"))),
        None
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let tmp = TempDir::new("trust-perms");
        let dir = tmp.path().join("state/twitty");
        let store_path = dir.join("trust.json");
        let hash = sha256_hex(b"commands");

        let mut store = TrustStore::default();
        assert!(store.trust(Path::new("/work/repo"), &hash));
        store.save_to(&store_path).unwrap();
        assert!(TrustStore::load_from(&store_path).is_trusted(Path::new("/work/repo"), &hash));

        // World-writable trust.json must fail closed.
        fs::set_permissions(&store_path, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(TrustStore::load_from(&store_path).is_empty());

        // Setgid trust.json must fail closed.
        fs::set_permissions(&store_path, fs::Permissions::from_mode(0o2600)).unwrap();
        assert!(TrustStore::load_from(&store_path).is_empty());

        // Restore safe permissions, then test symlinked parent directory.
        fs::set_permissions(&store_path, fs::Permissions::from_mode(0o600)).unwrap();
        let symlink_parent = tmp.path().join("state-link");
        symlink(&dir, &symlink_parent).unwrap();
        assert!(TrustStore::load_from(&symlink_parent.join("trust.json")).is_empty());
    }
}

#[test]
fn config_bounds_size_refuses_non_regular_and_sanitizes_warnings() {
    let tmp = TempDir::new("config-safety");

    // Oversized config file returns defaults and refuses to overwrite.
    let big_path = tmp.path().join("big.json");
    let oversized = format!(
        "{{\"font_size\": 20, \"pad\": \"{}\"}}",
        "x".repeat(MAX_CONFIG_BYTES + 128)
    );
    fs::write(&big_path, &oversized).unwrap();
    let mut cfg = TwittyConfig::load_from(&big_path);
    assert_eq!(cfg.font_size, TwittyConfig::default().font_size);
    cfg.font_size = 24.0;
    cfg.save_to(&big_path).unwrap();
    assert_eq!(
        fs::metadata(&big_path).unwrap().len(),
        oversized.len() as u64,
        "oversized config must not be overwritten"
    );

    // Non-regular path (directory) returns defaults and does not panic.
    let dir_path = tmp.path().join("config-dir");
    fs::create_dir(&dir_path).unwrap();
    let dir_cfg = TwittyConfig::load_from(&dir_path);
    assert_eq!(dir_cfg.font_size, TwittyConfig::default().font_size);

    // Atomic save leaves no temporary files behind.
    let ok_path = tmp.path().join("ok/config.json");
    let mut good = TwittyConfig::default();
    good.font_size = 17.0;
    good.save_to(&ok_path).unwrap();
    assert_eq!(TwittyConfig::load_from(&ok_path).font_size, 17.0);
    let leftovers: Vec<_> = fs::read_dir(ok_path.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "leftover temp files: {leftovers:?}");

    // Warnings strip escape sequences and bidi overrides before reaching the terminal.
    let _ = take_warnings();
    let mut hostile = TwittyConfig::default();
    hostile.theme = Some("bad\x1b]52;c;aGk=\x07\u{202E}theme".into());
    let _ = hostile.palette();
    let warnings = take_warnings();
    assert!(
        warnings.iter().any(|w| w.contains("bad")
            && !w.contains('\x1b')
            && !w.contains('\x07')
            && !w.contains('\u{202E}')),
        "expected cleaned warning without escapes or bidi overrides: {warnings:?}"
    );
}

#[cfg(unix)]
#[test]
fn shell_setup_enforces_private_modes_and_refuses_wrapper_symlinks() {
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
    use twitty::session::shell_setup::{install, install_claude_wrapper};

    let tmp = TempDir::new("shell-setup-sec");
    let dir = tmp.path().join("shell");
    fs::create_dir_all(&dir).unwrap();
    // Setgid bit on directory must be cleared to 0700.
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o2700)).unwrap();

    // Symlink placed at twitty.bash must be replaced with a regular 0600 file.
    let decoy = tmp.path().join("decoy.bash");
    fs::write(&decoy, "decoy").unwrap();
    let target_script = dir.join("twitty.bash");
    symlink(&decoy, &target_script).unwrap();

    install(&dir).unwrap();
    assert_eq!(fs::symlink_metadata(&dir).unwrap().mode() & 0o7777, 0o700);
    let script_meta = fs::symlink_metadata(&target_script).unwrap();
    assert!(script_meta.file_type().is_file());
    assert_eq!(script_meta.mode() & 0o7777, 0o600);
    assert_eq!(
        fs::read_to_string(&decoy).unwrap(),
        "decoy",
        "symlink target must never be overwritten"
    );

    // Symlink at claude wrapper path must be refused.
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::set_permissions(&bin_dir, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&decoy, bin_dir.join("claude")).unwrap();
    assert!(install_claude_wrapper(&bin_dir).is_err());
    fs::remove_file(bin_dir.join("claude")).unwrap();

    let installed = install_claude_wrapper(&bin_dir).unwrap();
    assert_eq!(
        fs::symlink_metadata(&installed).unwrap().mode() & 0o7777,
        0o700
    );
    let agy_installed = bin_dir.join("agy");
    assert_eq!(
        fs::symlink_metadata(&agy_installed).unwrap().mode() & 0o7777,
        0o700
    );
}

#[test]
fn control_split_pane_too_small_reports_the_reason() {
    let mut host = HeadlessHost::new();

    // Initial pane is 100x30 (HeadlessGrid is 1000x600, 10x20 cells).
    // Split horizontally twice: 100 -> 50 -> 24 cols (< 48 cols minimum).
    for _ in 0..2 {
        let (resp, _) = host.dispatch(Request {
            id: json!(1),
            method: "pane.split".into(),
            params: json!({"direction": "right"}),
        });
        assert!(resp.ok, "initial split should succeed: {resp:?}");
    }

    // 3rd horizontal split via Twitty native pane.split must explain WHY.
    let (native_err, _) = host.dispatch(Request {
        id: json!(2),
        method: "pane.split".into(),
        params: json!({"direction": "right"}),
    });
    assert!(!native_err.ok);
    let msg = native_err.error.as_deref().unwrap_or("");
    assert!(
        msg.contains("too narrow"),
        "expected 'too narrow' in {msg:?}"
    );
    assert!(msg.contains("48"), "expected minimum columns in {msg:?}");

    // Same via cmux surface.split returns structured code "pane_too_small".
    let (cmux_err, _) = host.dispatch(Request {
        id: json!(3),
        method: "surface.split".into(),
        params: json!({"orientation": "horizontal"}),
    });
    assert!(!cmux_err.ok);
    assert_eq!(cmux_err.error_code.as_deref(), Some("pane_too_small"));
    let cmsg = cmux_err.error.as_deref().unwrap_or("");
    assert!(cmsg.contains("too narrow"), "{cmsg:?}");

    // Vertical split: 30 rows -> 14 rows (< 20 rows minimum) -> 2nd vertical split fails with reason.
    let (v1, _) = host.dispatch(Request {
        id: json!(4),
        method: "pane.split".into(),
        params: json!({"direction": "down"}),
    });
    assert!(v1.ok);
    let (v2, _) = host.dispatch(Request {
        id: json!(5),
        method: "pane.split".into(),
        params: json!({"direction": "down"}),
    });
    assert!(!v2.ok);
    let vmsg = v2.error.as_deref().unwrap_or("");
    assert!(
        vmsg.contains("too short") && vmsg.contains("20"),
        "{vmsg:?}"
    );
}

#[test]
fn control_blocks_edit_paste_bounds_mods_handles_exact_crlf_and_relative_xdg() {
    // edit.paste must be blocked over the control socket to match OSC 52 clipboard-read block.
    let paste_req = Request {
        id: json!(1),
        method: "command.run".into(),
        params: json!({"name": "edit.paste"}),
    };
    let err = MethodCall::parse(&paste_req).unwrap_err();
    assert!(
        err.contains("edit.paste") && err.contains("clipboard"),
        "{err}"
    );

    let mut host = HeadlessHost::new();
    let (resp, _) = host.dispatch(paste_req);
    assert!(!resp.ok);

    // Excessive mods array in surface.send_key is rejected.
    let many_mods: Vec<&str> = vec!["ctrl"; 32];
    let (mods_resp, _) = host.dispatch(Request {
        id: json!(2),
        method: "surface.send_key".into(),
        params: json!({"key": "c", "mods": many_mods}),
    });
    assert!(!mods_resp.ok);
    assert_eq!(mods_resp.error_code.as_deref(), Some("invalid_params"));

    // read_bounded_line accepts exact max_bytes line ending in \r\n without over-reading next line.
    let mut exact_crlf = Cursor::new(b"12345\r\nnext\n".to_vec());
    assert_eq!(
        read_bounded_line(&mut exact_crlf, 5).unwrap(),
        ReadLine::Line("12345".into())
    );
    assert_eq!(
        read_bounded_line(&mut exact_crlf, 5).unwrap(),
        ReadLine::Line("next".into())
    );
    let mut over_crlf = Cursor::new(b"123456\r\n".to_vec());
    assert_eq!(
        read_bounded_line(&mut over_crlf, 5).unwrap(),
        ReadLine::TooLong
    );

    // Relative XDG_RUNTIME_DIR falls back to /tmp/twitty-<uid>.
    assert_eq!(
        runtime_dir_from(Some(OsString::from("relative/run")), 1000),
        PathBuf::from("/tmp/twitty-1000")
    );
}

#[cfg(unix)]
#[test]
fn links_refuse_symlink_pointing_to_launchable_file() {
    use std::os::unix::fs::symlink;
    use twitty::links::file_link_target;

    let tmp = TempDir::new("link-symlink");
    let launcher = tmp.path().join("evil.desktop");
    fs::write(&launcher, "[Desktop Entry]\nType=Application\n").unwrap();
    let disguise = tmp.path().join("notes.txt");
    symlink(&launcher, &disguise).unwrap();

    let url = format!("file://{}", disguise.display());
    assert_eq!(
        file_link_target(&url),
        Some(tmp.path().to_path_buf()),
        "symlink to a .desktop launcher must open its folder, not the launcher"
    );
}

#[test]
fn remote_validation_and_bootstrap_hardening() {
    assert!(validate_destination("@example.org").is_err());

    struct CaptureSsh {
        replies: std::collections::VecDeque<String>,
        commands: Vec<String>,
    }
    impl SshRunner for CaptureSsh {
        fn run(
            &mut self,
            _target: &Target,
            command: &str,
            _upload: Option<&Path>,
        ) -> Result<String, BootstrapError> {
            self.commands.push(command.to_owned());
            Ok(self.replies.pop_front().unwrap_or_default())
        }
    }

    let hash = "a".repeat(64);
    let mut runner = CaptureSsh {
        replies: [
            "Linux x86_64\n".into(),
            String::new(),
            String::new(),
            format!("{hash}  -\n"),
        ]
        .into(),
        commands: Vec::new(),
    };
    let target = Target::new("example", "sess").unwrap();
    bootstrap(&mut runner, &target, Path::new("helper"), &hash, "x86_64").unwrap();
    assert!(
        runner.commands[2].contains("chmod 700"),
        "bootstrap upload must tighten remote bin directory permissions: {}",
        runner.commands[2]
    );
}

#[test]
fn terminal_text_scan_and_scroll_never_overflow_on_extreme_bounds() {
    let mut term = Terminal::new(40, 4, |_| {});
    term.process_bytes(b"\x1b]133;A\x07hello\r\nworld\r\n");

    // max_rows = 0, i32::MAX, and usize::MAX must never underflow/panic.
    let zero = term.for_each_text_line(None, 0, |_, _| true);
    assert_eq!(zero.rows, 0);
    assert_eq!(zero.next, None);

    for huge in [i32::MAX as usize, usize::MAX] {
        let mut seen = 0usize;
        let scan = term.for_each_text_line(None, huge, |_, _| {
            seen += 1;
            true
        });
        assert!(seen >= 2);
        assert_eq!(scan.next, None);
    }

    // Extreme absolute line numbers must not overflow or panic.
    assert_eq!(term.text_between((i64::MIN, 0), (i64::MIN, 0)), None);
    assert_eq!(term.text_between((i64::MAX, 0), (i64::MAX, 0)), None);
    term.scroll_to_line(i64::MIN);
    term.scroll_to_line(i64::MAX);
}

#[test]
fn workflows_cwd_errors_strip_escape_sequences() {
    let tmp = TempDir::new("wf-cwd-clean");
    let err = resolve_project_cwd(tmp.path(), "../\x1b]52;c;aGk=\x07\u{202E}evil").unwrap_err();
    assert!(!err.contains('\x1b'), "ESC leaked into error: {err:?}");
    assert!(!err.contains('\x07'), "BEL leaked into error: {err:?}");
    assert!(!err.contains('\u{202E}'), "RLO leaked into error: {err:?}");
}

#[test]
fn readme_documents_all_config_keys_and_install_rejects_empty_prefix() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let readme = fs::read_to_string(root.join("README.md")).unwrap();

    for key in [
        "font_size",
        "background_opacity",
        "cursor_style",
        "theme",
        "colors",
        "font_family",
        "ui_font_family",
        "ui_accent",
        "top_bar",
        "pane_headers",
        "cursor_blink",
        "sidebar",
        "sidebar_width",
        "desktop_notifications",
        "control_socket",
        "cmux_compat",
        "claude_hooks",
        "shell_integration",
        "restore_session",
        "agent_command",
        "commands",
    ] {
        assert!(
            readme.contains(&format!("`{key}`")),
            "README.md is missing config key `{key}`"
        );
    }

    #[cfg(unix)]
    {
        let status = std::process::Command::new("sh")
            .arg(root.join("packaging/linux/install.sh"))
            .args(["--prefix", ""])
            .status()
            .unwrap();
        assert_eq!(
            status.code(),
            Some(2),
            "install.sh must reject empty --prefix"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn pty_handles_zero_dimensions_cleanly() {
    use twitty::pty::{Pty, SpawnOptions};

    let options = SpawnOptions {
        command: vec!["true".into()],
        ..SpawnOptions::default()
    };
    let pty = Pty::spawn_with(&options, 0, 0, || {}, || {}).unwrap();
    pty.resize(0, 0).unwrap();
}
