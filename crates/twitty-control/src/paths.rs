//! Socket path selection, runtime directory permission checks, stale socket cleanup and
//! discovery of live Twitty sockets.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Socket filename for process `pid`.
pub fn socket_filename(pid: u32) -> String {
    format!("twitty-{pid}.sock")
}

/// Extract the PID from a `twitty-<pid>.sock` filename.
pub fn parse_socket_pid(name: &str) -> Option<u32> {
    let inner = name.strip_prefix("twitty-")?.strip_suffix(".sock")?;
    if inner.is_empty() || !inner.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    inner.parse().ok()
}

/// Choose the runtime directory from an optional `XDG_RUNTIME_DIR` value and the user's UID:
/// `$XDG_RUNTIME_DIR/twitty` when set, non-empty, and absolute, else `/tmp/twitty-<uid>`.
pub fn runtime_dir_from(xdg_runtime_dir: Option<OsString>, uid: u32) -> PathBuf {
    match xdg_runtime_dir
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        Some(xdg) => xdg.join("twitty"),
        None => PathBuf::from(format!("/tmp/twitty-{uid}")),
    }
}

/// Full path of the socket for `pid` inside `dir`.
pub fn socket_path_in(dir: &Path, pid: u32) -> PathBuf {
    dir.join(socket_filename(pid))
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::fs::{self, DirBuilder, OpenOptions};
    use std::io;
    use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt};
    use std::os::unix::net::UnixStream;
    use std::time::SystemTime;

    /// Current user's UID, obtained without `unsafe` code.
    pub fn current_uid() -> io::Result<u32> {
        #[cfg(target_os = "linux")]
        if let Ok(meta) = fs::metadata("/proc/self") {
            return Ok(meta.uid());
        }

        let pid = std::process::id();
        let dir = std::env::temp_dir();
        for attempt in 0u32..64 {
            let probe = dir.join(format!(".twitty-uid-{pid}-{attempt}"));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&probe)
            {
                Ok(file) => {
                    let uid = file.metadata().map(|m| m.uid());
                    let _ = fs::remove_file(&probe);
                    return uid;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("could not determine current user id"))
    }

    /// Default runtime directory for the current process.
    pub fn default_runtime_dir() -> io::Result<PathBuf> {
        Ok(runtime_dir_from(
            std::env::var_os("XDG_RUNTIME_DIR"),
            current_uid()?,
        ))
    }

    /// Verify that `dir` is a real directory (not a symlink), owned by `expected_uid`, and has no
    /// setuid, setgid, group, or world permission bits set (`mode & 0o6077 == 0`).
    pub fn verify_dir(dir: &Path, expected_uid: u32) -> io::Result<()> {
        let meta = fs::symlink_metadata(dir)?;
        if !meta.file_type().is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("control socket path {} is not a directory", dir.display()),
            ));
        }
        if meta.uid() != expected_uid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "control socket directory {} is owned by uid {}, expected {expected_uid}",
                    dir.display(),
                    meta.uid()
                ),
            ));
        }
        if meta.mode() & 0o6077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "control socket directory {} has unsafe permissions {:04o} (must be 0700)",
                    dir.display(),
                    meta.mode() & 0o7777
                ),
            ));
        }
        Ok(())
    }

    /// Verify that `path` is a real Unix socket (not a symlink), owned by `expected_uid`, has no
    /// group or world permission bits (`mode & 0o077 == 0`), and resides in a verified directory.
    pub fn verify_socket(path: &Path, expected_uid: u32) -> io::Result<()> {
        let parent = match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        verify_dir(parent, expected_uid)?;
        let meta = fs::symlink_metadata(path)?;
        if !meta.file_type().is_socket() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("control socket path {} is not a socket", path.display()),
            ));
        }
        if meta.uid() != expected_uid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "control socket {} is owned by uid {}, expected {expected_uid}",
                    path.display(),
                    meta.uid()
                ),
            ));
        }
        if meta.mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "control socket {} has unsafe permissions {:04o} (must be 0600)",
                    path.display(),
                    meta.mode() & 0o7777
                ),
            ));
        }
        Ok(())
    }

    /// Create `dir` with mode 0700 if it does not exist, and verify its ownership and mode.
    pub fn ensure_runtime_dir(dir: &Path, expected_uid: u32) -> io::Result<()> {
        match DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        verify_dir(dir, expected_uid)
    }

    /// Whether a socket file belonging to `pid` appears dead and safe to remove.
    fn is_stale_socket(path: &Path, pid: u32) -> bool {
        #[cfg(target_os = "linux")]
        {
            if !Path::new(&format!("/proc/{pid}")).exists() {
                return true;
            }
            // If the PID is alive (e.g. recycled or our own PID before bind), leave live
            // sockets alone; only treat our own pre-bind path as stale when nobody listens.
            if pid == std::process::id() {
                return matches!(
                    UnixStream::connect(path),
                    Err(e) if e.kind() == io::ErrorKind::ConnectionRefused
                );
            }
            false
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = pid;
            matches!(
                UnixStream::connect(path),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                    )
            )
        }
    }

    /// Remove stale `twitty-<pid>.sock` files in `dir` whose owning process is no longer alive.
    /// Returns the number of sockets removed.
    pub fn cleanup_stale_sockets(dir: &Path) -> io::Result<usize> {
        let uid = current_uid()?;
        verify_dir(dir, uid)?;
        let mut removed = 0;
        for entry in fs::read_dir(dir)? {
            let Ok(entry) = entry else { continue };
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else {
                continue;
            };
            let Some(pid) = parse_socket_pid(name_str) else {
                continue;
            };
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.file_type().is_socket() || meta.uid() != uid {
                continue;
            }
            if is_stale_socket(&path, pid) && fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Connect to the newest live `twitty-<pid>.sock` in `dir`, returning `(path, stream)`.
    pub fn connect_newest_in(dir: &Path, expected_uid: u32) -> io::Result<(PathBuf, UnixStream)> {
        verify_dir(dir, expected_uid)?;
        let mut candidates: Vec<(SystemTime, u32, PathBuf)> = Vec::new();
        for entry in fs::read_dir(dir)? {
            let Ok(entry) = entry else { continue };
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else {
                continue;
            };
            let Some(pid) = parse_socket_pid(name_str) else {
                continue;
            };
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.file_type().is_socket()
                || meta.uid() != expected_uid
                || meta.mode() & 0o077 != 0
            {
                continue;
            }
            #[cfg(target_os = "linux")]
            if !Path::new(&format!("/proc/{pid}")).exists() {
                continue;
            }
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            candidates.push((mtime, pid, path));
        }
        // Newest modified first; break ties deterministically by higher PID.
        candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));

        for (_, _, path) in candidates {
            if let Ok(stream) = UnixStream::connect(&path) {
                return Ok((path, stream));
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no running Twitty instance found",
        ))
    }

    /// Find the path of the newest live socket in `dir`.
    pub fn find_newest_socket_in(dir: &Path, expected_uid: u32) -> io::Result<PathBuf> {
        connect_newest_in(dir, expected_uid).map(|(path, _)| path)
    }
}

#[cfg(unix)]
pub use unix::{
    cleanup_stale_sockets, connect_newest_in, current_uid, default_runtime_dir, ensure_runtime_dir,
    find_newest_socket_in, verify_dir, verify_socket,
};
