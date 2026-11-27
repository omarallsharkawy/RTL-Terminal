//! The SSH-side attach process. The session and its PTY live in a separate process.

use std::io::{self, IsTerminal, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::io::Errno;
use rustix::termios::{tcgetattr, tcgetwinsize, tcsetattr, OptionalActions, Termios};

use super::protocol::{self, byte, Size, ATTACH, CHUNK, EXIT, INPUT, OUTPUT, RESIZE};
use super::runtime::Runtime;

const START_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) fn measured_size() -> Option<Size> {
    let size = tcgetwinsize(io::stdin()).ok()?;
    Size::new(size.ws_col, size.ws_row).ok()
}

fn connect_or_start(runtime: &Runtime, name: &str, size: Size) -> Result<UnixStream> {
    match runtime.connect(name) {
        Ok(stream) => return Ok(stream),
        Err(err) if needs_server(&err) => {}
        Err(err) => return Err(err.into()),
    }
    // The versioned file may have been atomically replaced by another bootstrap.
    let mut child = Command::new("/proc/self/exe")
        .args([
            "__serve",
            name,
            &size.cols.to_string(),
            &size.rows.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("starting the remote session server")?;

    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        match runtime.connect(name) {
            Ok(stream) => {
                // The server owns the shell. Reap it if it exits while we're attached.
                let _ = thread::Builder::new()
                    .name("twitty-remote-reap".into())
                    .spawn(move || {
                        let _ = child.wait();
                    });
                return Ok(stream);
            }
            Err(err) if needs_server(&err) => {}
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(err.into());
            }
        }
        if let Some(status) = child.try_wait()? {
            let mut error = String::new();
            if !status.success() {
                if let Some(mut stderr) = child.stderr.take() {
                    stderr.read_to_string(&mut error)?;
                }
            }
            // Another caller may have won the bind race just before we checked.
            let error = error.trim();
            return match runtime.connect(name) {
                Ok(stream) => Ok(stream),
                Err(err) if error.is_empty() => {
                    Err(err).context("remote session server could not start")
                }
                Err(_) => bail!("{}", error.strip_prefix("twitty-remote: ").unwrap_or(error)),
            };
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("timed out waiting for the remote session server");
        }
        thread::sleep(Duration::from_millis(25));
    }
}

pub(super) fn needs_server(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    )
}

struct RawTerminal<F: AsFd> {
    tty: F,
    original: Termios,
}

impl<F: AsFd> RawTerminal<F> {
    fn enter(tty: F) -> io::Result<Self> {
        let original = tcgetattr(&tty)?;
        let mut raw = original.clone();
        raw.make_raw();
        tcsetattr(&tty, OptionalActions::Now, &raw)?;
        Ok(Self { tty, original })
    }
}

impl<F: AsFd> Drop for RawTerminal<F> {
    fn drop(&mut self) {
        let _ = tcsetattr(&self.tty, OptionalActions::Now, &self.original);
    }
}

pub(super) fn attach(runtime: &Runtime, name: &str, size: Size) -> Result<u8> {
    let mut stream = connect_or_start(runtime, name, size)?;
    let stdin = io::stdin();
    let tty = stdin.is_terminal();
    let _raw = tty.then(|| RawTerminal::enter(&stdin)).transpose()?;
    let mut stdout = io::stdout().lock();
    stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    stream.set_write_timeout(Some(REQUEST_TIMEOUT))?;
    size.send(ATTACH, &mut stream)?;
    if byte(&mut stream)? != ATTACH {
        bail!("remote session server rejected attach");
    }
    stream.set_read_timeout(None)?;

    if std::env::var_os("TWITTY_REMOTE_MARKER").is_some_and(|value| value == "1") {
        // Restore a known screen/parser state before replaying the tail into Twitty.
        stdout.write_all(b"\x18\x1bc")?;
        stdout.write_all(super::super::ATTACH_MARKER)?;
        stdout.flush()?;
    }

    let mut current = size;
    let mut buffer = [0; CHUNK];
    let tick = Timespec {
        tv_sec: 0,
        tv_nsec: 150_000_000,
    };
    loop {
        if tty {
            if let Some(size) = measured_size().filter(|size| *size != current) {
                size.send(RESIZE, &mut stream)?;
                current = size;
            }
        }
        let (out_ready, in_ready) = {
            let flags = PollFlags::IN;
            let mut fds = [PollFd::new(&stream, flags), PollFd::new(&stdin, flags)];
            match poll(&mut fds, tty.then_some(&tick)) {
                Ok(_) => (!fds[0].revents().is_empty(), !fds[1].revents().is_empty()),
                Err(Errno::INTR) => continue,
                Err(err) => return Err(err.into()),
            }
        };
        if out_ready {
            match byte(&mut stream)? {
                OUTPUT => {
                    stdout.write_all(protocol::read_data(&mut stream, &mut buffer)?)?;
                    stdout.flush()?;
                }
                EXIT => return Ok(byte(&mut stream)?),
                _ => bail!("invalid remote session response"),
            }
        }
        if in_ready {
            match rustix::io::read(&stdin, &mut buffer) {
                Ok(0) => return Ok(0),
                Ok(n) => protocol::write_data(&mut stream, INPUT, &buffer[..n])?,
                Err(Errno::INTR) => {}
                // SSH closed the PTY, so release the attachment and leave the shell running.
                Err(Errno::IO) if tty => return Ok(0),
                Err(err) => return Err(err.into()),
            }
        }
    }
}
