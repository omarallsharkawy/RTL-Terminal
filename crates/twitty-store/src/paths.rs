//! Database path resolution, Unix permission hardening (`0700` dir / `0600` file), and bounded
//! process-free Git root discovery.

use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use crate::error::StoreError;

/// Default database path for the current user:
/// - Unix: `$XDG_DATA_HOME/twitty/twitty.db` (fallback `~/.local/share/twitty/twitty.db`)
/// - Windows: `%LOCALAPPDATA%\twitty\twitty.db` (fallback `%APPDATA%\twitty\twitty.db`)
///
/// Relative environment paths are ignored.
pub fn default_db_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        db_path_windows(
            std::env::var_os("LOCALAPPDATA").as_deref(),
            std::env::var_os("APPDATA").as_deref(),
        )
    }
    #[cfg(not(windows))]
    {
        db_path_unix(
            std::env::var_os("XDG_DATA_HOME").as_deref(),
            std::env::var_os("HOME").as_deref(),
        )
    }
}

/// Resolve the Unix DB path from explicit `XDG_DATA_HOME` and `HOME` values (testable without
/// mutating process environment variables).
pub fn db_path_unix(xdg_data_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    let absolute = |v: Option<&OsStr>| {
        v.filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let base = absolute(xdg_data_home).or_else(|| Some(absolute(home)?.join(".local/share")))?;
    Some(base.join("twitty").join("twitty.db"))
}

/// Resolve the Windows DB path from explicit `LOCALAPPDATA` and `APPDATA` values.
pub fn db_path_windows(local_appdata: Option<&OsStr>, appdata: Option<&OsStr>) -> Option<PathBuf> {
    let non_empty = |v: Option<&OsStr>| v.filter(|s| !s.is_empty()).map(PathBuf::from);
    let base = non_empty(local_appdata).or_else(|| non_empty(appdata))?;
    Some(base.join("twitty").join("twitty.db"))
}

/// Create the database parent directory (`0700` on Unix) and pre-create the database file
/// (`0600` on Unix) so the database is never created with world- or group-readable permissions.
pub(crate) fn prepare_db_path(path: &Path) -> Result<(), StoreError> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
        let mut created_parent = false;
        match fs::symlink_metadata(parent) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(StoreError::InvalidInput(
                        "database parent directory must not be a symlink",
                    ));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let mut builder = fs::DirBuilder::new();
                builder.recursive(true).mode(0o700);
                builder.create(parent)?;
                created_parent = true;
            }
            Err(err) => return Err(err.into()),
        }
        // Only tighten the parent directory to 0700 when Twitty created it or it is a dedicated
        // `twitty` directory — never silently chmod shared directories such as `$HOME` or `/tmp`.
        let dedicated_dir = parent.file_name() == Some(OsStr::new("twitty"));
        if (created_parent || dedicated_dir)
            && parent != Path::new(".")
            && parent.parent().is_some()
        {
            if let Ok(meta) = fs::symlink_metadata(parent) {
                if !meta.file_type().is_symlink() {
                    let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
                }
            }
        }
        match fs::symlink_metadata(path) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(StoreError::InvalidInput(
                        "database path must not be a symlink",
                    ));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let mut opts = OpenOptions::new();
                opts.write(true).create_new(true).mode(0o600);
                match opts.open(path) {
                    Ok(_) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                        if fs::symlink_metadata(path)?.file_type().is_symlink() {
                            return Err(StoreError::InvalidInput(
                                "database path must not be a symlink",
                            ));
                        }
                    }
                    Err(err) => return Err(err.into()),
                }
            }
            Err(err) => return Err(err.into()),
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }

    #[cfg(not(unix))]
    {
        if !parent.exists() {
            fs::create_dir_all(parent)?;
        }
        let _ = OpenOptions::new();
    }

    Ok(())
}

/// Ensure SQLite companion WAL/SHM files also stay `0600` on Unix when present.
pub(crate) fn tighten_sidecar_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut wal = path.as_os_str().to_os_string();
        wal.push("-wal");
        let mut shm = path.as_os_str().to_os_string();
        shm.push("-shm");
        for sidecar in [PathBuf::from(wal), PathBuf::from(shm)] {
            if fs::symlink_metadata(&sidecar)
                .ok()
                .is_some_and(|meta| !meta.file_type().is_symlink())
            {
                let _ = fs::set_permissions(sidecar, fs::Permissions::from_mode(0o600));
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Resolve the project root for `cwd`: the nearest ancestor containing `.git` (directory or
/// worktree/submodule file, bounded to 64 levels), falling back to `cwd` itself.
pub fn resolve_project_root(cwd: &Path) -> PathBuf {
    if !cwd.is_absolute() {
        return cwd.to_path_buf();
    }
    for dir in cwd.ancestors().take(64) {
        if dir.join(".git").symlink_metadata().is_ok() {
            return dir.to_path_buf();
        }
    }
    cwd.to_path_buf()
}
