//! Client for talking to a running Twitty control socket.

#[cfg(unix)]
mod imp {
    use std::io::{self, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use serde_json::Value;

    use crate::control::paths::{connect_newest_in, current_uid, default_runtime_dir};
    use crate::control::protocol::{
        read_bounded_line, ReadLine, Request, Response, MAX_LINE_BYTES,
    };

    const CLIENT_TIMEOUT: Duration = Duration::from_secs(15);

    pub struct Client {
        stream: BufReader<UnixStream>,
        path: PathBuf,
        next_id: u64,
    }

    impl Client {
        /// Connect to an explicit socket path.
        pub fn connect_path(path: impl Into<PathBuf>) -> io::Result<Self> {
            let path = path.into();
            let stream = UnixStream::connect(&path)?;
            Self::from_stream(path, stream)
        }

        /// Connect using `$TWITTY_SOCKET` when set, else the newest live socket in the runtime
        /// directory.
        pub fn connect_default() -> io::Result<Self> {
            if let Some(env_path) = std::env::var_os("TWITTY_SOCKET").filter(|v| !v.is_empty()) {
                let path = PathBuf::from(env_path);
                let stream = UnixStream::connect(&path).map_err(|e| {
                    io::Error::new(
                        e.kind(),
                        format!("could not connect to {}: {e}", path.display()),
                    )
                })?;
                return Self::from_stream(path, stream);
            }
            let dir = default_runtime_dir()?;
            let uid = current_uid()?;
            let (path, stream) = connect_newest_in(&dir, uid)?;
            Self::from_stream(path, stream)
        }

        fn from_stream(path: PathBuf, stream: UnixStream) -> io::Result<Self> {
            let _ = stream.set_read_timeout(Some(CLIENT_TIMEOUT));
            let _ = stream.set_write_timeout(Some(CLIENT_TIMEOUT));
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
pub use imp::Client;
