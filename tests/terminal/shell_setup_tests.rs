//! Automatic shell integration: how bash, zsh and fish are started, the script directory, and
//! the scripts themselves, run in real shells (those installed) on a pipe.
use std::fs;
use std::path::{Path, PathBuf};

use twitty::session::shell_setup::{install, integrate, Integration, Shell};

/// A fresh directory under the system temp dir, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("twitty-shell-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn no_env(_: &str) -> Option<String> {
    None
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn env_of(integration: &Integration, key: &str) -> Option<String> {
    integration
        .env
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

#[test]
fn which_shells_and_arguments_are_integrated() {
    let dir = Path::new("/run/user/1/twitty/shell");
    assert_eq!(Shell::of("/usr/bin/bash"), Some(Shell::Bash));
    assert_eq!(Shell::of("zsh"), Some(Shell::Zsh));
    assert_eq!(Shell::of("/opt/fish/bin/fish"), Some(Shell::Fish));
    assert_eq!(Shell::of("/bin/sh"), None);
    assert_eq!(Shell::of("nu"), None);

    let bash = integrate("/bin/bash", &[], dir, no_env).unwrap();
    assert_eq!(
        bash.command,
        args(&[
            "/bin/bash",
            "--rcfile",
            "/run/user/1/twitty/shell/twitty.bash"
        ])
    );
    assert_eq!(
        env_of(&bash, "TWITTY_SHELL_INTEGRATION").as_deref(),
        Some("1")
    );
    assert_eq!(env_of(&bash, "__TWITTY_BASH_LOGIN"), None);
    // A login bash ignores --rcfile: the script reads the login files itself.
    let login = integrate("bash", &args(&["-l"]), dir, no_env).unwrap();
    assert_eq!(
        login.command,
        args(&["bash", "--rcfile", "/run/user/1/twitty/shell/twitty.bash"])
    );
    assert_eq!(env_of(&login, "__TWITTY_BASH_LOGIN").as_deref(), Some("1"));
    // Anything else is left alone.
    assert_eq!(integrate("bash", &args(&["--norc"]), dir, no_env), None);
    assert_eq!(integrate("bash", &args(&["-c", "ls"]), dir, no_env), None);
    assert_eq!(integrate("sh", &[], dir, no_env), None);

    let zsh = integrate("zsh", &args(&["-l"]), dir, |k| {
        (k == "ZDOTDIR").then(|| "/home/u/.config/zsh".to_string())
    })
    .unwrap();
    assert_eq!(zsh.command, args(&["zsh", "-l"]));
    assert_eq!(
        env_of(&zsh, "ZDOTDIR").as_deref(),
        Some("/run/user/1/twitty/shell/zsh")
    );
    assert_eq!(
        env_of(&zsh, "__TWITTY_ZSH_ZDOTDIR").as_deref(),
        Some("/home/u/.config/zsh")
    );
    let zsh = integrate("zsh", &[], dir, no_env).unwrap();
    assert_eq!(
        env_of(&zsh, "__TWITTY_ZSH_ZDOTDIR"),
        None,
        "unset stays unset"
    );

    let fish = integrate("fish", &[], dir, no_env).unwrap();
    assert_eq!(fish.command, args(&["fish"]));
    assert_eq!(
        env_of(&fish, "XDG_DATA_DIRS").as_deref(),
        Some("/run/user/1/twitty/shell/fish:/usr/local/share:/usr/share")
    );
    assert_eq!(
        env_of(&fish, "__TWITTY_FISH_XDG_DATA_DIRS").as_deref(),
        Some("")
    );
    let fish = integrate("fish", &[], dir, |k| {
        (k == "XDG_DATA_DIRS").then(|| "/nix/share".to_string())
    })
    .unwrap();
    assert_eq!(
        env_of(&fish, "XDG_DATA_DIRS").as_deref(),
        Some("/run/user/1/twitty/shell/fish:/nix/share")
    );
}

#[test]
fn scripts_are_written_privately_and_atomically() {
    let tmp = TempDir::new("install");
    let dir = tmp.0.join("twitty").join("shell");
    install(&dir).unwrap();
    for file in [
        "twitty.bash",
        "twitty.zsh",
        "zsh/.zshenv",
        "zsh/.zprofile",
        "zsh/.zshrc",
        "zsh/.zlogin",
        "fish/fish/vendor_conf.d/twitty.fish",
    ] {
        let text = fs::read_to_string(dir.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert!(text.contains("Twitty"), "{file}");
    }
    let zshenv = fs::read_to_string(dir.join("zsh/.zshenv")).unwrap();
    assert!(zshenv.contains("twitty.zsh\" .zshenv"), "{zshenv}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        // An existing directory others can read is made private.
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        install(&dir).unwrap();
        let mode = fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        // A symlink in its place is refused.
        let link = tmp.0.join("link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert!(install(&link).is_err());
    }
    // A changed file is rewritten; no temporary files stay behind.
    fs::write(dir.join("twitty.bash"), "old").unwrap();
    install(&dir).unwrap();
    assert!(fs::read_to_string(dir.join("twitty.bash"))
        .unwrap()
        .contains("PROMPT_COMMAND"));
    let leftovers: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty());
}

#[cfg(unix)]
mod unix_shell_tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    use twitty::blocks::{BlockList, BlockStatus};
    use twitty::shell_integration::{parse_osc, Signal};
    use twitty::terminal::Terminal;

    /// The program's full path, if it is installed.
    fn find(program: &str) -> Option<PathBuf> {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|dir| dir.join(program))
            .find(|p| p.is_file())
    }

    /// Run an integrated shell on a pipe with `home` as HOME, feeding it `input`. Returns
    /// everything it wrote (stdout and stderr together, as a terminal would show them).
    fn run_shell(program: &Path, shell_args: &[&str], home: &Path, input: &str) -> String {
        let scripts = home.join("scripts");
        install(&scripts).unwrap();
        let integration = integrate(
            program.to_str().unwrap(),
            &args(shell_args),
            &scripts,
            no_env,
        )
        .expect("integrated");
        // Interactive on a pipe: `-i` (bash wants it before nothing else).
        let mut command = integration.command.clone();
        if !command.iter().any(|a| a == "-i") {
            command.push("-i".into());
        }
        let out_path = home.join("out.txt");
        let out = fs::File::create(&out_path).unwrap();
        let mut child = Command::new(&command[0])
            .args(&command[1..])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", home)
            .env("TERM", "xterm-256color")
            .envs(integration.env.iter().map(|(k, v)| (k, v)))
            .current_dir(home)
            .stdin(Stdio::piped())
            .stdout(Stdio::from(out.try_clone().unwrap()))
            .stderr(Stdio::from(out))
            .spawn()
            .unwrap();
        {
            use std::io::Write;
            let mut stdin = child.stdin.take().unwrap();
            stdin.write_all(input.as_bytes()).unwrap();
        }
        let start = Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if start.elapsed().as_secs() > 20 {
                let _ = child.kill();
                panic!(
                    "shell did not exit: {}",
                    fs::read_to_string(&out_path).unwrap_or_default()
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        String::from_utf8_lossy(&fs::read(&out_path).unwrap()).into_owned()
    }

    /// Run an integrated shell inside a real PTY, sending each command line after the shell
    /// prints its prompt/mark so interactive-only shells (like `fish`, which skips prompt and
    /// preexec/postexec hooks when stdin is a pipe) emit full OSC 133 / 633 / 7 sequences.
    fn run_shell_pty(
        program: &Path,
        shell_args: &[&str],
        home: &Path,
        commands: &[&str],
    ) -> String {
        use portable_pty::{native_pty_system, CommandBuilder, PtySize};
        use std::io::{Read, Write};
        use std::sync::{Arc, Condvar, Mutex};
        use std::time::Duration;

        let scripts = home.join("scripts");
        install(&scripts).unwrap();
        let integration = integrate(
            program.to_str().unwrap(),
            &args(shell_args),
            &scripts,
            no_env,
        )
        .expect("integrated");

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");

        let mut cmd = CommandBuilder::new(&integration.command[0]);
        for arg in &integration.command[1..] {
            cmd.arg(arg);
        }
        cmd.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            cmd.env("PATH", path);
        }
        cmd.env("HOME", home);
        cmd.env("TERM", "xterm-256color");
        for (k, v) in &integration.env {
            cmd.env(k, v);
        }
        cmd.cwd(home);

        let mut child = pair.slave.spawn_command(cmd).expect("spawn shell in pty");
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().expect("clone pty reader");
        let mut writer = pair.master.take_writer().expect("take pty writer");

        let shared = Arc::new((Mutex::new((Vec::<u8>::new(), false)), Condvar::new()));
        let bg = Arc::clone(&shared);
        let reader_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let (lock, cv) = &*bg;
                        let mut guard = lock.lock().unwrap();
                        guard.0.extend_from_slice(&buf[..n]);
                        cv.notify_all();
                    }
                }
            }
            let (lock, cv) = &*bg;
            let mut guard = lock.lock().unwrap();
            guard.1 = true;
            cv.notify_all();
        });

        let wait_for = |pred: &dyn Fn(&[u8]) -> bool, timeout: Duration| {
            let (lock, cv) = &*shared;
            let deadline = Instant::now() + timeout;
            let mut guard = lock.lock().unwrap();
            while !guard.1 && !pred(&guard.0) {
                let Some(rem) = deadline.checked_duration_since(Instant::now()) else {
                    break;
                };
                let (g, _) = cv.wait_timeout(guard, rem).unwrap();
                guard = g;
            }
        };

        let count_bytes = |haystack: &[u8], needle: &[u8]| -> usize {
            haystack
                .windows(needle.len())
                .filter(|w| *w == needle)
                .count()
        };

        // Wait for the initial prompt mark (OSC 133;B) before feeding commands.
        let mut expected_b = 1usize;
        wait_for(
            &|buf| count_bytes(buf, b"\x1b]133;B") >= expected_b,
            Duration::from_secs(4),
        );

        let mut expected_d = 0usize;
        for line in commands {
            let _ = writer.write_all(line.as_bytes());
            let _ = writer.write_all(b"\r");
            let _ = writer.flush();
            if *line != "exit" {
                expected_d += 1;
                expected_b += 1;
                wait_for(
                    &|buf| {
                        count_bytes(buf, b"\x1b]133;D;") >= expected_d
                            && count_bytes(buf, b"\x1b]133;B") >= expected_b
                    },
                    Duration::from_secs(4),
                );
            }
        }
        drop(writer);

        let start = Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if start.elapsed().as_secs() > 20 {
                let _ = child.kill();
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(pair.master);
        let _ = reader_thread.join();

        let guard = shared.0.lock().unwrap();
        String::from_utf8_lossy(&guard.0).into_owned()
    }

    /// The shell signals in `output`, in order.
    fn signals(output: &str) -> Vec<Signal> {
        output
            .split("\x1b]")
            .skip(1)
            .filter_map(|osc| {
                let end = osc.find(['\x07', '\x1b'])?;
                parse_osc(&osc.as_bytes()[..end])
            })
            .collect()
    }

    fn marks(output: &str) -> Vec<String> {
        use twitty::shell_integration::MarkKind;
        signals(output)
            .into_iter()
            .filter_map(|s| match s {
                Signal::Mark(MarkKind::PromptStart) => Some("A".to_string()),
                Signal::Mark(MarkKind::CommandStart) => Some("B".to_string()),
                Signal::Mark(MarkKind::OutputStart) => Some("C".to_string()),
                Signal::Mark(MarkKind::CommandEnd { exit }) => {
                    Some(format!("D{}", exit.unwrap_or(-1)))
                }
                _ => None,
            })
            .collect()
    }

    /// The blocks a terminal builds from `output`.
    fn blocks_of(output: &str) -> (Terminal, BlockList) {
        let mut term = Terminal::new(80, 24, |_| {});
        let mut blocks = BlockList::new();
        // A pipe has no terminal to turn \n into \r\n.
        term.process_bytes(output.replace('\n', "\r\n").as_bytes());
        for event in term.take_shell_events() {
            blocks.on_event(&event, &term, Instant::now());
        }
        (term, blocks)
    }

    fn assert_cwd(output: &str, expected: &Path) {
        let cwds: Vec<_> = signals(output)
            .into_iter()
            .filter_map(|s| match s {
                Signal::Cwd(path) => Some(path),
                _ => None,
            })
            .collect();
        assert!(
            cwds.iter().any(|c| c == expected),
            "no OSC 7 for {expected:?} in {cwds:?}"
        );
    }

    #[test]
    fn bash_reports_prompts_commands_and_directories() {
        let Some(bash) = find("bash") else {
            eprintln!("bash not installed, skipped");
            return;
        };
        let home = TempDir::new("bash");
        // The user's own prompt and PROMPT_COMMAND (ending in `;`) keep working.
        fs::write(
            home.0.join(".bashrc"),
            "PS1='mine> '\nPROMPT_COMMAND='echo -n PC;'\necho RC-LOADED\n",
        )
        .unwrap();
        let arabic = home.0.join("مجلد فيه مسافة");
        fs::create_dir(&arabic).unwrap();
        let out = run_shell(
            &bash,
            &[],
            &home.0,
            "echo hi\nfalse\ncd 'مجلد فيه مسافة'\necho $TWITTY_SHELL_INTEGRATION\nexit\n",
        );
        assert!(out.contains("RC-LOADED"), "{out:?}");
        assert!(out.contains("mine> "), "{out:?}");
        assert!(out.contains("PC"), "user PROMPT_COMMAND ran: {out:?}");
        let marks = marks(&out);
        let joined = marks.join(" ");
        assert!(joined.contains("A B C D0 A B C D1 A B C D0"), "{joined}");
        assert_cwd(&out, &arabic);

        let (term, blocks) = blocks_of(&out);
        let commands: Vec<_> = blocks.blocks().filter_map(|b| b.command.clone()).collect();
        assert_eq!(
            &commands[..3],
            ["echo hi", "false", "cd 'مجلد فيه مسافة'"],
            "{out:?}"
        );
        let first = blocks.blocks().next().unwrap();
        assert_eq!(blocks.text(first, &term).unwrap().output, "hi");
        assert_eq!(
            blocks.blocks().nth(1).unwrap().status(),
            BlockStatus::Failed(1)
        );
        assert!(
            out.contains("C\x071\n") || out.contains("\x071\n"),
            "env var set: {out:?}"
        );
    }

    #[test]
    fn bash_login_reads_the_profile() {
        let Some(bash) = find("bash") else {
            return;
        };
        let home = TempDir::new("bash-login");
        fs::write(home.0.join(".bash_profile"), "echo PROFILE-LOADED\n").unwrap();
        fs::write(home.0.join(".bashrc"), "echo RC-LOADED\n").unwrap();
        let out = run_shell(&bash, &["-l"], &home.0, "true\nexit\n");
        assert!(out.contains("PROFILE-LOADED"), "{out:?}");
        assert!(!out.contains("RC-LOADED"), "{out:?}");
        assert!(marks(&out).join(" ").contains("A B C D0"), "{out:?}");
    }

    #[test]
    fn bash_with_its_own_marks_is_not_doubled() {
        let Some(bash) = find("bash") else {
            return;
        };
        let home = TempDir::new("bash-own");
        fs::write(home.0.join(".bashrc"), "PS1='\\[\\e]133;A\\a\\]own> '\n").unwrap();
        let out = run_shell(&bash, &[], &home.0, "true\nexit\n");
        let marks = marks(&out);
        assert!(
            marks.iter().all(|m| m == "A"),
            "only the user's marks: {marks:?}"
        );
        assert!(!marks.is_empty());
    }

    #[test]
    fn zsh_reports_prompts_commands_and_directories() {
        let Some(zsh) = find("zsh") else {
            eprintln!("zsh not installed, skipped");
            return;
        };
        let home = TempDir::new("zsh");
        fs::write(home.0.join(".zshenv"), "echo ENV-LOADED\n").unwrap();
        fs::write(
            home.0.join(".zshrc"),
            "PS1='mine> '\necho RC-LOADED $ZDOTDIR\nprecmd() { echo -n PC }\n",
        )
        .unwrap();
        let dir = home.0.join("dir x");
        fs::create_dir(&dir).unwrap();
        let out = run_shell(&zsh, &[], &home.0, "echo hi\nfalse\ncd 'dir x'\nexit\n");
        assert!(out.contains("ENV-LOADED"), "{out:?}");
        assert!(out.contains("RC-LOADED"), "{out:?}");
        assert!(
            !out.contains("twitty/shell") && !out.contains("scripts/zsh"),
            "ZDOTDIR given back: {out:?}"
        );
        let joined = marks(&out).join(" ");
        assert!(
            joined.contains("C D0") && joined.contains("C D1"),
            "{joined}"
        );
        assert!(
            signals(&out).contains(&Signal::CommandLine("echo hi".into())),
            "{out:?}"
        );
        assert_cwd(&out, &dir);
    }

    #[test]
    fn fish_reports_commands_and_directories() {
        let Some(fish) = find("fish") else {
            eprintln!("fish not installed, skipped");
            return;
        };
        let home = TempDir::new("fish");
        let dir = home.0.join("dir x");
        fs::create_dir(&dir).unwrap();
        let out = run_shell_pty(
            &fish,
            &[],
            &home.0,
            &["echo hi", "false", "cd 'dir x'", "exit"],
        );
        let joined = marks(&out).join(" ");
        assert!(
            joined.contains("C D0") && joined.contains("C D1"),
            "marks={joined:?} out={out:?}"
        );
        assert!(
            signals(&out).contains(&Signal::CommandLine("echo hi".into())),
            "{out:?}"
        );
        assert_cwd(&out, &dir);
    }

    /// Startup files that prepend their own directories (`~/.local/bin`) must not push Twitty's
    /// `claude` wrapper directory back: the integration puts it first again, exactly once.
    #[test]
    fn claude_wrapper_dir_stays_first_on_path_after_startup_files() {
        let rc =
            "export TWITTY_CLAUDE_DIR=/twitty-wrap\nexport PATH=\"/user-bin:/twitty-wrap:$PATH\"\n";
        let check = "printf '\\nTWITTY_PATH_CHECK=%s\\n' \"$PATH\"\nexit\n";
        for (shell, rcfile) in [("bash", ".bashrc"), ("zsh", ".zshrc")] {
            let Some(program) = find(shell) else {
                eprintln!("{shell} not installed, skipped");
                continue;
            };
            let home = TempDir::new(&format!("{shell}-claude-path"));
            fs::write(home.0.join(rcfile), rc).unwrap();
            let out = run_shell(&program, &[], &home.0, check);
            let line = out
                .lines()
                .filter_map(|l| l.trim_end_matches('\r').strip_prefix("TWITTY_PATH_CHECK="))
                .next_back()
                .unwrap_or_else(|| panic!("{shell}: no PATH line in {out:?}"));
            let dirs: Vec<&str> = line.trim().split(':').collect();
            assert_eq!(dirs[0], "/twitty-wrap", "{shell}: {line}");
            assert_eq!(dirs[1], "/user-bin", "{shell}: {line}");
            assert_eq!(
                dirs.iter().filter(|d| **d == "/twitty-wrap").count(),
                1,
                "{shell}: {line}"
            );
        }
    }
}

/// Scripts live in a subdirectory named by their content, so an updated Twitty never runs
/// an older copy left in the runtime directory.
#[test]
fn script_directory_is_named_by_the_scripts_content() {
    let version = twitty::session::shell_setup::scripts_version();
    assert_eq!(version.len(), 12);
    assert!(version.chars().all(|c| c.is_ascii_hexdigit()), "{version}");
    assert_eq!(version, twitty::session::shell_setup::scripts_version());
}
