//! Unix-domain control socket server (stub on non-Unix platforms).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::protocol::{Request, Response};

/// Maximum number of clients handled concurrently.
pub const MAX_CLIENTS: usize = 16;

/// Idle read timeout per client connection.
pub const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Timeout waiting for the UI event loop to reply to a single request.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Callback that evaluates one control request and produces a response.
pub type Handler = Arc<dyn Fn(Request) -> Response + Send + Sync + 'static>;

/// Tuning knobs for [`ControlServer`], exposed so tests can override timeouts and limits.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub max_clients: usize,
    pub read_timeout: Duration,
    pub write_timeout: Duration,
    pub max_line_bytes: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            max_clients: MAX_CLIENTS,
            read_timeout: READ_TIMEOUT,
            write_timeout: Duration::from_secs(10),
            max_line_bytes: super::protocol::MAX_LINE_BYTES,
        }
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::fs::{self, Permissions};
    use std::io::{self, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::thread::{self, JoinHandle};

    use serde_json::Value;

    use crate::control::paths::{
        cleanup_stale_sockets, current_uid, default_runtime_dir, ensure_runtime_dir, socket_path_in,
    };
    use crate::control::protocol::{parse_request_line, read_bounded_line, ReadLine};

    /// Running Unix-domain control socket server. Dropping it stops the accept loop and removes
    /// the socket file.
    pub struct ControlServer {
        path: PathBuf,
        shutdown: Arc<AtomicBool>,
        accept_thread: Option<JoinHandle<()>>,
    }

    impl ControlServer {
        /// Start listening on the default runtime socket (`twitty-<pid>.sock`) with default
        /// limits.
        pub fn start(handler: Handler) -> io::Result<Self> {
            let dir = default_runtime_dir()?;
            Self::start_in_dir(&dir, handler, ServerConfig::default())
        }

        /// Start listening inside `dir` (created with mode 0700 and checked for ownership and
        /// permissions) on `twitty-<pid>.sock`.
        pub fn start_in_dir(
            dir: &Path,
            handler: Handler,
            config: ServerConfig,
        ) -> io::Result<Self> {
            let uid = current_uid()?;
            ensure_runtime_dir(dir, uid)?;
            let _ = cleanup_stale_sockets(dir);
            let path = socket_path_in(dir, std::process::id());
            Self::bind_at(path, handler, config)
        }

        /// Start listening at an explicit `path` inside an already-verified directory.
        pub fn start_at_path(
            path: PathBuf,
            handler: Handler,
            config: ServerConfig,
        ) -> io::Result<Self> {
            if let Some(dir) = path.parent() {
                let uid = current_uid()?;
                ensure_runtime_dir(dir, uid)?;
            }
            Self::bind_at(path, handler, config)
        }

        fn bind_at(path: PathBuf, handler: Handler, config: ServerConfig) -> io::Result<Self> {
            let listener = match UnixListener::bind(&path) {
                Ok(l) => l,
                Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
                    // Only remove a pre-existing path if nobody is listening on it.
                    if matches!(
                        UnixStream::connect(&path),
                        Err(ce) if ce.kind() == io::ErrorKind::ConnectionRefused
                    ) {
                        let _ = fs::remove_file(&path);
                        UnixListener::bind(&path)?
                    } else {
                        return Err(e);
                    }
                }
                Err(e) => return Err(e),
            };
            if let Err(err) = fs::set_permissions(&path, Permissions::from_mode(0o600)) {
                let _ = fs::remove_file(&path);
                return Err(err);
            }

            let shutdown = Arc::new(AtomicBool::new(false));
            let shutdown_bg = Arc::clone(&shutdown);
            let accept_thread = thread::Builder::new()
                .name("twitty-control-accept".into())
                .spawn(move || accept_loop(listener, handler, config, shutdown_bg))?;

            Ok(Self {
                path,
                shutdown,
                accept_thread: Some(accept_thread),
            })
        }

        /// Path of the bound Unix domain socket.
        pub fn path(&self) -> Option<&Path> {
            Some(&self.path)
        }
    }

    impl Drop for ControlServer {
        fn drop(&mut self) {
            self.shutdown.store(true, Ordering::Release);
            // Wake the blocking `accept()` call so the accept thread exits cleanly.
            let _ = UnixStream::connect(&self.path);
            if let Some(handle) = self.accept_thread.take() {
                let _ = handle.join();
            }
            let _ = fs::remove_file(&self.path);
        }
    }

    struct ClientSlot(Arc<AtomicUsize>);

    impl Drop for ClientSlot {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::AcqRel);
        }
    }

    fn try_acquire_slot(active: &Arc<AtomicUsize>, max: usize) -> Option<ClientSlot> {
        let mut cur = active.load(Ordering::Acquire);
        loop {
            if cur >= max {
                return None;
            }
            match active.compare_exchange_weak(cur, cur + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some(ClientSlot(Arc::clone(active))),
                Err(actual) => cur = actual,
            }
        }
    }

    fn accept_loop(
        listener: UnixListener,
        handler: Handler,
        config: ServerConfig,
        shutdown: Arc<AtomicBool>,
    ) {
        let active = Arc::new(AtomicUsize::new(0));
        for incoming in listener.incoming() {
            if shutdown.load(Ordering::Acquire) {
                break;
            }
            let mut stream = match incoming {
                Ok(s) => s,
                Err(err) => {
                    if shutdown.load(Ordering::Acquire) {
                        break;
                    }
                    tracing::warn!("control socket accept error: {err}");
                    continue;
                }
            };
            let Some(slot) = try_acquire_slot(&active, config.max_clients) else {
                // Non-blocking error write so a slow extra client cannot stall the accept loop.
                let _ = stream.set_nonblocking(true);
                let _ = write_response(
                    &mut stream,
                    &Response::err(Value::Null, "too many concurrent clients"),
                );
                continue;
            };
            let handler = Arc::clone(&handler);
            let shutdown_client = Arc::clone(&shutdown);
            let cfg = config.clone();
            let spawned = thread::Builder::new()
                .name("twitty-control-client".into())
                .spawn(move || {
                    let _slot = slot;
                    handle_client(stream, handler, cfg, shutdown_client);
                });
            if let Err(err) = spawned {
                tracing::warn!("failed to spawn control client thread: {err}");
            }
        }
    }

    fn handle_client(
        stream: UnixStream,
        handler: Handler,
        config: ServerConfig,
        shutdown: Arc<AtomicBool>,
    ) {
        let _ = stream.set_read_timeout(Some(config.read_timeout));
        let _ = stream.set_write_timeout(Some(config.write_timeout));
        let Ok(mut writer) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(stream);

        while !shutdown.load(Ordering::Acquire) {
            match read_bounded_line(&mut reader, config.max_line_bytes) {
                Ok(ReadLine::Eof) => break,
                Ok(ReadLine::TooLong) => {
                    let _ = write_response(
                        &mut writer,
                        &Response::err(
                            Value::Null,
                            format!("request line exceeds {} byte limit", config.max_line_bytes),
                        ),
                    );
                    break;
                }
                Ok(ReadLine::Line(line)) => {
                    let resp = match parse_request_line(&line) {
                        Ok(req) => handler(req),
                        Err(err_resp) => err_resp,
                    };
                    if write_response(&mut writer, &resp).is_err() {
                        break;
                    }
                }
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(err) if err.kind() == io::ErrorKind::InvalidData => {
                    let _ = write_response(
                        &mut writer,
                        &Response::err(Value::Null, "request is not valid UTF-8"),
                    );
                    break;
                }
                Err(_) => break,
            }
        }
    }

    fn write_response<W: Write>(writer: &mut W, resp: &Response) -> io::Result<()> {
        let mut encoded = serde_json::to_string(resp).map_err(io::Error::other)?;
        encoded.push('\n');
        writer.write_all(encoded.as_bytes())?;
        writer.flush()
    }
}

#[cfg(not(unix))]
mod imp {
    use super::*;
    use std::io;

    /// Stub control server on non-Unix platforms: does nothing.
    pub struct ControlServer;

    impl ControlServer {
        pub fn start(_handler: Handler) -> io::Result<Self> {
            Ok(Self)
        }

        pub fn path(&self) -> Option<&Path> {
            None
        }
    }
}

pub use imp::ControlServer;
