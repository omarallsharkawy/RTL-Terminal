//! Bounded, timeout-guarded subprocess execution shared by git/generator background workers.

use std::io::{self, Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Guard that kills and reaps a spawned child process on drop unless explicitly disarmed.
pub(crate) struct KillOnDrop(pub(crate) Option<Child>);

impl KillOnDrop {
    pub(crate) fn new(child: Child) -> Self {
        Self(Some(child))
    }

    pub(crate) fn disarm(&mut self) -> Option<Child> {
        self.0.take()
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Suppress console window flashing for short-lived background helpers on Windows.
pub(crate) fn apply_no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

/// Result of running a subprocess with bounded stdout capture and a wall-clock timeout.
pub(crate) struct BoundedOutput {
    pub(crate) stdout: Vec<u8>,
    pub(crate) truncated: bool,
    pub(crate) status: Option<ExitStatus>,
}

/// Read `reader` to EOF or at most `limit` bytes, returning `(bytes, truncated)`.
pub(crate) fn read_bounded(reader: impl Read, limit: usize) -> (Vec<u8>, bool) {
    let mut out = Vec::new();
    let _ = reader.take(limit as u64 + 1).read_to_end(&mut out);
    let truncated = out.len() > limit;
    out.truncate(limit);
    (out, truncated)
}

/// Spawn `cmd` with `stdin=null`, `stdout=piped`, `stderr=null`, read at most `limit` bytes from
/// stdout, and enforce `timeout` via a watchdog thread that kills and reaps the child on timeout
/// or truncation.
pub(crate) fn run_bounded(
    cmd: Command,
    thread_name: &str,
    timeout: Duration,
    poll_interval: Duration,
    limit: usize,
) -> io::Result<BoundedOutput> {
    run_bounded_inner(cmd, None, thread_name, timeout, poll_interval, limit)
}

/// Like [`run_bounded`], but pipes `stdin_payload` to the child off-thread so writing cannot
/// deadlock with stdout capture.
pub(crate) fn run_bounded_with_stdin(
    cmd: Command,
    stdin_payload: &[u8],
    thread_name: &str,
    timeout: Duration,
    poll_interval: Duration,
    limit: usize,
) -> io::Result<BoundedOutput> {
    run_bounded_inner(
        cmd,
        Some(stdin_payload),
        thread_name,
        timeout,
        poll_interval,
        limit,
    )
}

fn run_bounded_inner(
    mut cmd: Command,
    stdin_payload: Option<&[u8]>,
    thread_name: &str,
    timeout: Duration,
    poll_interval: Duration,
    limit: usize,
) -> io::Result<BoundedOutput> {
    apply_no_window(&mut cmd);
    cmd.stdin(if stdin_payload.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::null());
    let mut child = cmd.spawn()?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::other("no output pipe"));
    };
    let stdin_pipe = child.stdin.take();

    let stop = Arc::new(AtomicBool::new(false));
    let mut guard = KillOnDrop::new(child);

    if let (Some(mut stdin), Some(payload)) = (stdin_pipe, stdin_payload) {
        let bytes = payload.to_vec();
        let _ = thread::Builder::new()
            .name(format!("{thread_name}-stdin"))
            .spawn(move || {
                let _ = stdin.write_all(&bytes);
                let _ = stdin.flush();
            });
    }
    let watchdog = {
        let stop = stop.clone();
        thread::Builder::new()
            .name(thread_name.into())
            .spawn(move || {
                let start = Instant::now();
                loop {
                    let child = guard.0.as_mut()?;
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            let _ = guard.disarm();
                            return Some(status);
                        }
                        Ok(None) => {}
                        Err(_) => return None,
                    }
                    if stop.load(Ordering::Relaxed) || start.elapsed() > timeout {
                        return None;
                    }
                    thread::sleep(poll_interval);
                }
            })?
    };

    let (out, truncated) = read_bounded(stdout, limit);
    if truncated {
        stop.store(true, Ordering::Relaxed);
    }
    let status = watchdog.join().ok().flatten();
    Ok(BoundedOutput {
        stdout: out,
        truncated,
        status,
    })
}
