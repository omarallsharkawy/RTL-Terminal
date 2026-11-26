//! Client for talking to a running Twitty control socket.

#[cfg(unix)]
mod imp {
    use std::io::{self, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use serde_json::Value;

    use crate::paths::{connect_newest_in, current_uid, default_runtime_dir, verify_socket};
    use crate::protocol::{read_bounded_line, ReadLine, Request, Response, MAX_LINE_BYTES};

    const CLIENT_TIMEOUT: Duration = Duration::from_secs(15);
    const MIN_CLIENT_TIMEOUT: Duration = Duration::from_millis(50);
    const MAX_CLIENT_TIMEOUT: Duration = Duration::from_secs(60);

    /// Read an optional client I/O timeout override from `TWITTY_CLI_TIMEOUT_MS` (milliseconds)
    /// or `CMUXTERM_CLI_RESPONSE_TIMEOUT_SEC` (seconds), clamped to `[50 ms, 60 s]`.
    pub fn timeout_from_env() -> Duration {
        if let Ok(raw) = std::env::var("TWITTY_CLI_TIMEOUT_MS") {
            if let Ok(ms) = raw.trim().parse::<u64>() {
                return Duration::from_millis(ms).clamp(MIN_CLIENT_TIMEOUT, MAX_CLIENT_TIMEOUT);
            }
        }
        if let Ok(raw) = std::env::var("CMUXTERM_CLI_RESPONSE_TIMEOUT_SEC") {
            if let Ok(secs) = raw.trim().parse::<f64>() {
                if secs.is_finite() && secs > 0.0 {
                    let dur = Duration::from_secs_f64(secs.min(60.0));
                    return dur.clamp(MIN_CLIENT_TIMEOUT, MAX_CLIENT_TIMEOUT);
                }
            }
        }
        CLIENT_TIMEOUT
    }

    pub struct Client {
        stream: BufReader<UnixStream>,
        path: PathBuf,
        next_id: u64,
    }

    impl Client {
        /// Connect to an explicit socket path after verifying ownership and permissions.
        pub fn connect_path(path: impl Into<PathBuf>) -> io::Result<Self> {
            Self::connect_path_with_timeout(path, timeout_from_env())
        }

        /// Connect to an explicit socket path with an explicit read/write timeout.
        pub fn connect_path_with_timeout(
            path: impl Into<PathBuf>,
            timeout: Duration,
        ) -> io::Result<Self> {
            let path = path.into();
            verify_socket(&path, current_uid()?)?;
            let stream = UnixStream::connect(&path)?;
            Self::from_stream(path, stream, timeout)
        }

        /// Connect using `$TWITTY_SOCKET` or `$CMUX_SOCKET_PATH` when set, else the newest live
        /// socket in the runtime directory.
        pub fn connect_default() -> io::Result<Self> {
            let timeout = timeout_from_env();
            let env_path = std::env::var_os("TWITTY_SOCKET")
                .filter(|v| !v.is_empty())
                .or_else(|| std::env::var_os("CMUX_SOCKET_PATH").filter(|v| !v.is_empty()));
            if let Some(env_path) = env_path {
                let path = PathBuf::from(env_path);
                verify_socket(&path, current_uid()?)?;
                let stream = UnixStream::connect(&path).map_err(|e| {
                    io::Error::new(
                        e.kind(),
                        format!("could not connect to {}: {e}", path.display()),
                    )
                })?;
                return Self::from_stream(path, stream, timeout);
            }
            let dir = default_runtime_dir()?;
            let uid = current_uid()?;
            let (path, stream) = connect_newest_in(&dir, uid)?;
            Self::from_stream(path, stream, timeout)
        }

        fn from_stream(path: PathBuf, stream: UnixStream, timeout: Duration) -> io::Result<Self> {
            let _ = stream.set_read_timeout(Some(timeout));
            let _ = stream.set_write_timeout(Some(timeout));
            Ok(Self {
                stream: BufReader::new(stream),
                path,
                next_id: 1,
            })
        }

        pub fn path(&self) -> &Path {
            &self.path
        }

        /// Send a request and wait for its matching response.
        pub fn call(&mut self, method: impl Into<String>, params: Value) -> io::Result<Response> {
            let id = self.next_id;
            self.next_id += 1;
            let req = Request {
                id: Value::from(id),
                method: method.into(),
                params,
            };
            self.send_request(&req)
        }

        /// Send an already-built [`Request`] and read one [`Response`].
        pub fn send_request(&mut self, req: &Request) -> io::Result<Response> {
            let mut line = serde_json::to_string(req).map_err(io::Error::other)?;
            line.push('\n');
            self.stream.get_mut().write_all(line.as_bytes())?;
            self.stream.get_mut().flush()?;

            match read_bounded_line(&mut self.stream, MAX_LINE_BYTES)? {
                ReadLine::Line(raw) => serde_json::from_str(&raw)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
                ReadLine::Eof => Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "server closed connection before replying",
                )),
                ReadLine::TooLong => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "response exceeded maximum line size",
                )),
            }
        }
    }
}

#[cfg(unix)]
pub use imp::{timeout_from_env, Client};
