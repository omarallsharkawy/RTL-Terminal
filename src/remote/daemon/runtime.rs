//! Per-user directory and serialized socket publication, including recovery after a crash.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};

pub(super) struct Runtime {
    dir: File,
}

impl Runtime {
    pub fn open() -> Result<Self> {
        // A host can remove its XDG runtime directory when the last SSH login ends.
        let xdg = std::env::var_os("SSH_CONNECTION")
            .filter(|connection| !connection.is_empty())
            .is_none()
            .then(|| absolute_env("XDG_RUNTIME_DIR"))
            .flatten();
        let path = xdg
            .map(|path| path.join("twitty-remote"))
            .or_else(|| Some(absolute_env("HOME")?.join(".cache/twitty/remote")))
            .context("neither XDG_RUNTIME_DIR nor HOME is an absolute path")?;
        fs::create_dir_all(path.parent().expect("runtime has a parent"))?;
        match DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err).with_context(|| format!("creating {}", path.display())),
        }
        let dir = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        let metadata = dir.metadata()?;
        ensure!(
            metadata.uid() == rustix::process::geteuid().as_raw()
                && metadata.mode() & 0o777 == 0o700,
            "{} must be owned by this user and have mode 0700",
            path.display()
        );
        Ok(Self { dir })
    }

    // Keep paths beneath the same directory descriptor and under sockaddr_un's length limit,
    // even when HOME is long or the session name uses all 64 characters.
    fn directory(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.dir.as_raw_fd()))
    }

    fn socket(&self, name: &str) -> PathBuf {
        self.directory().join(format!("{name}.sock"))
    }

    pub fn names(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(self.directory())? {
            let name = entry?.file_name();
            if let Some(name) = name.to_str().and_then(|name| name.strip_suffix(".sock")) {
                if super::super::validate_name(name).is_ok() {
                    names.push(name.to_owned());
                }
            }
        }
        names.sort();
        Ok(names)
    }

    pub(super) fn lock(&self) -> io::Result<File> {
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.directory().join(".lock"))?;
        if !lock.metadata()?.is_file() {
            return Err(io::Error::other("socket lock must be a regular file"));
        }
        lock.lock()?;
        Ok(lock)
    }

    pub fn connect(&self, name: &str) -> io::Result<UnixStream> {
        let _lock = self.lock()?;
        let path = self.socket(name);
        let metadata = socket_metadata(&path)?;
        if metadata.mode() & 0o777 != 0o600 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "session socket must have mode 0600",
            ));
        }
        UnixStream::connect(path)
    }

    pub fn bind(&self, name: &str) -> io::Result<Option<BoundSocket>> {
        let path = self.socket(name);
        match socket_metadata(&path) {
            Ok(_) => match UnixStream::connect(&path) {
                Ok(_) => return Ok(None),
                Err(err) if err.kind() == io::ErrorKind::ConnectionRefused => {
                    match fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                        Err(err) => return Err(err),
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            },
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        // This runs in the daemon before it starts any threads. Restore the caller's umask
        // before starting the shell.
        let previous = rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o177));
        let listener = UnixListener::bind(&path);
        rustix::process::umask(previous);
        let listener = listener?;
        let socket = SocketFile(path);
        listener.set_nonblocking(true)?;
        Ok(Some(BoundSocket {
            _file: socket,
            listener,
        }))
    }
}

pub(super) struct BoundSocket {
    // Unlink before closing the listener so another server cannot take over early.
    _file: SocketFile,
    pub listener: UnixListener,
}

struct SocketFile(PathBuf);

impl Drop for SocketFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn absolute_env(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

fn socket_metadata(path: &Path) -> io::Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket() || metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "session socket must be a real socket owned by this user",
        ));
    }
    Ok(metadata)
}
