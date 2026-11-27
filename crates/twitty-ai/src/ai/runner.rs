//! Non-interactive subprocess runner for local coding-agent CLIs.
//!
//! Runs blocking off the UI thread with:
//! - Configurable timeout (default 60 s via `config.ai_timeout_secs`)
//! - Cooperative cancellation (`AiCancel`, triggered by Esc or a newer request)
//! - Bounded stdout and stderr capture (no pipe deadlocks, no unbounded allocation)
//! - Guaranteed kill and reap on cancel, timeout, output-bound overflow, or drop
//! - Environment stripped of all `TWITTY_*`, `CMUX_*`, `__TWITTY_*`, and inherited terminal vars
//! - Fresh empty `0700` temporary working directory (never the user's pane cwd), removed after exit

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use twitty_core::language::fill;

use twitty_core::language::UiLanguage;
use twitty_core::shell_integration::clean_text;

/// Maximum bytes read from the agent's stdout (128 KiB).
pub const MAX_AI_OUTPUT_BYTES: usize = 128 * 1024;

/// Maximum bytes read from the agent's stderr for diagnostics (16 KiB).
pub const MAX_AI_STDERR_BYTES: usize = 16 * 1024;

/// Cooperative cancellation token shared between the UI thread and the AI runner thread.
#[derive(Clone, Debug, Default)]
pub struct AiCancel(Arc<AtomicBool>);

impl AiCancel {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Failure modes of a built-in AI request, with bilingual user-facing messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AiError {
    Disabled,
    NoAgent,
    SpawnFailed(String),
    TimedOut(u64),
    Cancelled,
    NonZeroExit { code: Option<i32>, stderr: String },
    OutputTooLarge,
    EmptyReply,
    UnparseableReply,
}

impl AiError {
    /// The error in `language`.
    pub fn message_in(&self, language: UiLanguage) -> String {
        match self {
            Self::Disabled => language.t("Built-in AI is disabled in config.json (\"ai\": false).").into(),
            Self::NoAgent => {
                language.t("No coding agent found (claude, codex, gemini, opencode). Set \"ai_agent\" in config.json.")
                    .into()
            }
            Self::SpawnFailed(err) => fill(language.t("Failed to start agent: {}"), &[&err]),
            Self::TimedOut(secs) => fill(language.t("AI timed out after {}s."), &[&secs]),
            Self::Cancelled => language.t("AI request cancelled.").into(),
            Self::NonZeroExit { code, stderr } => {
                let detail = first_non_empty_line(stderr);
                match (code, detail) {
                    (Some(c), Some(msg)) => fill(language.t("Agent failed (exit {}): {}"), &[&c, &msg]),
                    (Some(c), None) => fill(language.t("Agent failed with exit code {}."), &[&c]),
                    (None, Some(msg)) => fill(language.t("Agent failed: {}"), &[&msg]),
                    (None, None) => language.t("Agent failed unexpectedly.").into(),
                }
            }
            Self::OutputTooLarge => language.t("Agent output exceeded size limit.").into(),
            Self::EmptyReply => language.t("Agent returned an empty reply.").into(),
            Self::UnparseableReply => language.t("Could not extract a command from the agent reply.").into(),
        }
    }

    pub fn message_en(&self) -> String {
        self.message_in(UiLanguage::English)
    }

    pub fn message_ar(&self) -> String {
        self.message_in(UiLanguage::Arabic)
    }

    pub fn message_for(&self, language: UiLanguage) -> String {
        match language {
            UiLanguage::Arabic => self.message_ar(),
            UiLanguage::English => self.message_en(),
        }
    }

    /// Compact bilingual message (English + Arabic) for errors shown in both languages.
    /// The message in the current interface language.
    pub fn message_localized(&self) -> String {
        match twitty_core::language::UiLanguage::current() {
            twitty_core::language::UiLanguage::English => self.message_en(),
            twitty_core::language::UiLanguage::Arabic => self.message_ar(),
        }
    }
}

/// Whether an environment variable must be stripped before spawning a local coding agent.
///
/// Strips all `TWITTY_*`, `CMUX_*`, `__TWITTY_*` variables (including control sockets and pane
/// identifiers) as well as inherited parent-terminal variables.
pub fn is_stripped_ai_env_var(name: &str) -> bool {
    name.starts_with("TWITTY_")
        || name.starts_with("CMUX_")
        || name.starts_with("__TWITTY_")
        || twitty_core::pty::is_stripped_env_var(name)
}

enum WatchOutcome {
    Exited(ExitStatus),
    TimedOut,
    Cancelled,
    OutputTooLarge,
    Error(String),
}

struct KillOnDrop(Option<std::process::Child>);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            kill_tree(&mut child);
            let _ = child.wait();
        }
    }
}

fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        use rustix::process::{kill_process_group, Pid, Signal};
        let _ = kill_process_group(Pid::from_child(child), Signal::KILL);
    }
    let _ = child.kill();
}

fn credential_env(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    [
        "API_KEY",
        "AUTH_TOKEN",
        "ACCESS_TOKEN",
        "OAUTH_TOKEN",
        "SECRET_ACCESS_KEY",
    ]
    .iter()
    .any(|suffix| {
        name == *suffix
            || name
                .strip_suffix(suffix)
                .is_some_and(|prefix| prefix.ends_with('_'))
    })
}

struct SandboxTempDir(PathBuf);

impl SandboxTempDir {
    fn create() -> std::io::Result<Self> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir();
        let pid = std::process::id();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);

        for attempt in 0..64u32 {
            let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = base.join(format!("twitty-ai-sandbox-{pid}-{nanos}-{seq}-{attempt}"));
            #[cfg_attr(not(unix), allow(unused_mut))]
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ =
                            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700));
                    }
                    return Ok(Self(path));
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate unique AI sandbox directory",
        ))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for SandboxTempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run `argv` non-interactively in a fresh empty `0700` temporary directory with bounded
/// stdout/stderr, timeout, cancellation, and stripped Twitty/cmux environment variables.
/// Always kills and reaps the child and removes the temporary directory before returning.
pub fn run_command_blocking(
    argv: &[String],
    timeout: Duration,
    max_output_bytes: usize,
    cancel: &AiCancel,
) -> Result<String, AiError> {
    run_isolated(argv, timeout, max_output_bytes, cancel, false, None)
}

pub(crate) fn run_agent_command_blocking(
    argv: &[String],
    timeout: Duration,
    max_output_bytes: usize,
    cancel: &AiCancel,
) -> Result<String, AiError> {
    run_isolated(argv, timeout, max_output_bytes, cancel, true, None)
}

pub(crate) fn run_agent_command_with_stdin(
    argv: &[String],
    stdin_payload: &[u8],
    timeout: Duration,
    max_output_bytes: usize,
    cancel: &AiCancel,
) -> Result<String, AiError> {
    run_isolated(
        argv,
        timeout,
        max_output_bytes,
        cancel,
        true,
        Some(stdin_payload),
    )
}

fn run_isolated(
    argv: &[String],
    timeout: Duration,
    max_output_bytes: usize,
    cancel: &AiCancel,
    strip_credentials: bool,
    stdin_payload: Option<&[u8]>,
) -> Result<String, AiError> {
    if cancel.is_cancelled() {
        return Err(AiError::Cancelled);
    }
    let Some((program, args)) = argv.split_first() else {
        return Err(AiError::NoAgent);
    };

    // Declared before `guard` so Rust drops `guard` (kill + reap child) before removing this dir.
    let sandbox_dir = SandboxTempDir::create()
        .map_err(|e| AiError::SpawnFailed(clean_text(&e.to_string(), 160)))?;

    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(sandbox_dir.path())
        .stdin(if stdin_payload.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    for (key, _) in std::env::vars_os() {
        if let Some(name) = key.to_str() {
            if is_stripped_ai_env_var(name) || strip_credentials && credential_env(name) {
                cmd.env_remove(name);
            }
        }
    }
    for key in twitty_core::pty::INHERITED_TERMINAL_VARS {
        cmd.env_remove(key);
    }

    // Retry briefly on Linux ETXTBSY (transient when another thread forks while a newly written
    // helper script fd is still open in the child before exec).
    let mut child = {
        let mut attempt = 0u8;
        loop {
            match cmd.spawn() {
                Ok(child) => break child,
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempt < 4 => {
                    attempt += 1;
                    thread::sleep(Duration::from_millis(5 * u64::from(attempt)));
                }
                Err(e) => {
                    return Err(match e.kind() {
                        std::io::ErrorKind::NotFound => AiError::NoAgent,
                        _ => AiError::SpawnFailed(clean_text(&e.to_string(), 160)),
                    });
                }
            }
        }
    };

    let Some(stdout_pipe) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(AiError::SpawnFailed("missing stdout pipe".into()));
    };
    let Some(stderr_pipe) = child.stderr.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(AiError::SpawnFailed("missing stderr pipe".into()));
    };
    let stdin_pipe = child.stdin.take();

    let overflow = Arc::new(AtomicBool::new(false));
    let mut guard = KillOnDrop(Some(child));

    // Write large prompts off-thread so filling the OS pipe buffer never deadlocks stdout reading.
    if let (Some(mut stdin), Some(payload)) = (stdin_pipe, stdin_payload) {
        let bytes = payload.to_vec();
        let _ = thread::Builder::new()
            .name("twitty-ai-stdin".into())
            .spawn(move || {
                let _ = stdin.write_all(&bytes);
                let _ = stdin.flush();
            });
    }

    let stdout_thread = {
        let overflow = overflow.clone();
        thread::Builder::new()
            .name("twitty-ai-stdout".into())
            .spawn(move || {
                let (bytes, truncated) = read_bounded(stdout_pipe, max_output_bytes);
                if truncated {
                    overflow.store(true, Ordering::SeqCst);
                }
                (bytes, truncated)
            })
            .map_err(|e| AiError::SpawnFailed(e.to_string()))?
    };

    let stderr_thread = thread::Builder::new()
        .name("twitty-ai-stderr".into())
        .spawn(move || read_bounded_and_drain(stderr_pipe, MAX_AI_STDERR_BYTES))
        .map_err(|e| AiError::SpawnFailed(e.to_string()))?;

    let start = Instant::now();
    let outcome = loop {
        if cancel.is_cancelled() {
            drop(guard);
            break WatchOutcome::Cancelled;
        }
        if overflow.load(Ordering::SeqCst) {
            drop(guard);
            break WatchOutcome::OutputTooLarge;
        }
        let Some(child) = guard.0.as_mut() else {
            break WatchOutcome::Error("child missing".into());
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                // A descendant can retain the pipe after its parent has exited.
                #[cfg(unix)]
                {
                    use rustix::process::{kill_process_group, Pid, Signal};
                    let _ = kill_process_group(Pid::from_child(child), Signal::KILL);
                }
                let _ = guard.0.take();
                break WatchOutcome::Exited(status);
            }
            Ok(None) => {}
            Err(err) => {
                drop(guard);
                break WatchOutcome::Error(clean_text(&err.to_string(), 160));
            }
        }
        if start.elapsed() >= timeout {
            drop(guard);
            break WatchOutcome::TimedOut;
        }
        thread::sleep(Duration::from_millis(15));
    };

    // An escaped descendant can retain a pipe even after the process group is killed.
    // Cancellation and timeout must not wait for that pipe.
    let status = match outcome {
        WatchOutcome::Cancelled => return Err(AiError::Cancelled),
        WatchOutcome::TimedOut => return Err(AiError::TimedOut(timeout.as_secs().max(1))),
        WatchOutcome::OutputTooLarge => return Err(AiError::OutputTooLarge),
        WatchOutcome::Error(msg) => return Err(AiError::SpawnFailed(msg)),
        WatchOutcome::Exited(status) => status,
    };
    let (stdout_bytes, stdout_truncated) = stdout_thread.join().unwrap_or_default();
    let stderr_bytes = stderr_thread.join().unwrap_or_default();
    if stdout_truncated {
        return Err(AiError::OutputTooLarge);
    }
    if !status.success() {
        let stderr = clean_text(&String::from_utf8_lossy(&stderr_bytes), 240);
        return Err(AiError::NonZeroExit {
            code: status.code(),
            stderr,
        });
    }
    let text = String::from_utf8_lossy(&stdout_bytes).into_owned();
    if text.trim().is_empty() {
        return Err(AiError::EmptyReply);
    }
    Ok(text)
}

fn read_bounded(reader: impl Read, limit: usize) -> (Vec<u8>, bool) {
    let mut out = Vec::new();
    let _ = reader.take(limit as u64 + 1).read_to_end(&mut out);
    let truncated = out.len() > limit;
    out.truncate(limit);
    (out, truncated)
}

/// Keep up to `limit` bytes of stderr and drain the remainder to EOF so verbose child processes
/// do not receive `SIGPIPE`/`EPIPE`.
fn read_bounded_and_drain(mut reader: impl Read, limit: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    while let Ok(n) = reader.read(&mut buf) {
        if n == 0 {
            break;
        }
        if out.len() < limit {
            let take = (limit - out.len()).min(n);
            out.extend_from_slice(&buf[..take]);
        }
    }
    out
}

fn first_non_empty_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| clean_text(l, 120))
        .filter(|l| !l.is_empty())
}
