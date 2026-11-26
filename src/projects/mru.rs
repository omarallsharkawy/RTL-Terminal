//! Bounded MRU store of recent Twitty pane directories, persisted next to `session.json` with
//! atomic writes and `0600` permissions.

use std::ffi::OsStr;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::config::warn;

/// Current on-disk schema version for `recent_projects.json`.
pub const MRU_VERSION: u32 = 1;

/// Maximum number of recent directories retained.
pub const MAX_MRU_ENTRIES: usize = 64;

/// Maximum `recent_projects.json` file size read from disk (512 KiB).
pub const MAX_MRU_FILE_BYTES: usize = 512 * 1024;

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Deserialize, Serialize)]
struct MruFile {
    version: u32,
    entries: Vec<String>,
}

/// Recent project directories, ordered most-recent first and bounded to [`MAX_MRU_ENTRIES`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectMru {
    entries: Vec<PathBuf>,
}

impl ProjectMru {
    /// Compute the MRU path from `XDG_STATE_HOME` and `HOME` (relative paths are ignored).
    pub fn path_from(xdg_state_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
        let absolute = |v: Option<&OsStr>| {
            v.filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        };
        let base = if let Some(xdg) = absolute(xdg_state_home) {
            xdg
        } else {
            absolute(home)?.join(".local").join("state")
        };
        Some(base.join("twitty").join("recent_projects.json"))
    }

    /// Default path next to `session.json`.
    pub fn default_path() -> Option<PathBuf> {
        #[cfg(windows)]
        {
            let base = std::env::var_os("LOCALAPPDATA")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())?;
            Some(base.join("twitty").join("recent_projects.json"))
        }
        #[cfg(not(windows))]
        {
            let xdg = std::env::var_os("XDG_STATE_HOME");
            let home = std::env::var_os("HOME");
            Self::path_from(xdg.as_deref(), home.as_deref())
        }
    }

    /// Load from the default state path; missing or invalid files yield an empty MRU list.
    pub fn load_default() -> Self {
        Self::default_path().map_or_else(Self::default, |path| Self::load_from(&path))
    }

    /// Load from `path`, refusing symlinks, non-regular files, oversized files, and unsupported
    /// schema versions.
    pub fn load_from(path: &Path) -> Self {
        let file = match open_state_file(path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Self::default(),
            Err(err) => {
                warn(format!(
                    "twitty: ignoring recent projects {} ({err})",
                    path.display()
                ));
                return Self::default();
            }
        };
        let mut bytes = Vec::new();
        if let Err(err) = file
            .take(MAX_MRU_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
        {
            warn(format!(
                "twitty: could not read recent projects {} ({err})",
                path.display()
            ));
            return Self::default();
        }
        if bytes.len() > MAX_MRU_FILE_BYTES {
            warn(format!(
                "twitty: ignoring oversized recent projects {}",
                path.display()
            ));
            return Self::default();
        }
        let parsed: MruFile = match serde_json::from_slice(&bytes) {
            Ok(parsed) => parsed,
            Err(err) => {
                warn(format!(
                    "twitty: ignoring corrupt recent projects {} ({err})",
                    path.display()
                ));
                return Self::default();
            }
        };
        if parsed.version != MRU_VERSION {
            warn(format!(
                "twitty: ignoring unsupported recent projects version {} in {}",
                parsed.version,
                path.display()
            ));
            return Self::default();
        }
        let mut entries = Vec::new();
        for raw in parsed.entries {
            let p = PathBuf::from(raw);
            if !p.is_absolute() || entries.contains(&p) {
                continue;
            }
            entries.push(p);
            if entries.len() >= MAX_MRU_ENTRIES {
                break;
            }
        }
        Self { entries }
    }

    /// Record `dir` as the most recently used directory. Returns `true` when the list changed.
    pub fn record(&mut self, dir: &Path) -> bool {
        if !dir.is_absolute() || dir.to_str().is_none() {
            return false;
        }
        if self.entries.first().is_some_and(|first| first == dir) {
            return false;
        }
        self.entries.retain(|existing| existing != dir);
        self.entries.insert(0, dir.to_path_buf());
        if self.entries.len() > MAX_MRU_ENTRIES {
            self.entries.truncate(MAX_MRU_ENTRIES);
        }
        true
    }

    /// Recent directories, most recent first.
    pub fn entries(&self) -> &[PathBuf] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Atomically save to the default state path.
    pub fn save_default(&self) -> io::Result<()> {
        let path = Self::default_path().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "could not determine state directory for recent projects",
            )
        })?;
        self.save_to(&path)
    }

    /// Atomically write to `path` via a unique temp file (`0600` in a `0700` dir) + rename,
    /// refusing symlinked directories or target files.
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        let payload = MruFile {
            version: MRU_VERSION,
            entries: self
                .entries
                .iter()
                .take(MAX_MRU_ENTRIES)
                .filter_map(|p| p.to_str().map(str::to_owned))
                .collect(),
        };
        let bytes = serde_json::to_vec_pretty(&payload).map_err(io::Error::other)?;
        if bytes.len() > MAX_MRU_FILE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "recent projects file exceeds size bound",
            ));
        }
        atomic_write_state_file(path, ".recent-projects", &bytes)
    }
}

pub(crate) fn open_state_file(path: &Path) -> io::Result<File> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "state path has no parent"))?;
    let parent_meta = fs::symlink_metadata(parent)?;
    if parent_meta.file_type().is_symlink() || !parent_meta.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "state directory must be a real directory",
        ));
    }
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || !meta.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "state file must be a regular file",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "state file must be a regular file",
        ));
    }
    Ok(file)
}

pub(crate) fn atomic_write_state_file(
    path: &Path,
    tmp_prefix: &str,
    bytes: &[u8],
) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "state path has no parent"))?;
    ensure_state_dir(parent)?;
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.file_type().is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "refusing to overwrite symlink or non-regular state file",
                ));
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }

    let (tmp_path, mut file) = create_temp_file(parent, tmp_prefix)?;
    let write_res = (|| -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()
    })();
    drop(file);
    if let Err(err) = write_res {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    if let Err(err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    Ok(())
}

fn ensure_state_dir(dir: &Path) -> io::Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "state directory is not a real directory",
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if meta.permissions().mode() & 0o7777 != 0o700 {
                    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
                }
            }
            Ok(())
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            if let Some(grandparent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
                fs::create_dir_all(grandparent)?;
            }
            #[cfg_attr(not(unix), allow(unused_mut))]
            let mut builder = DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(dir) {
                Ok(()) => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
                    }
                    Ok(())
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => ensure_state_dir(dir),
                Err(err) => Err(err),
            }
        }
        Err(err) => Err(err),
    }
}

fn create_temp_file(parent: &Path, prefix: &str) -> io::Result<(PathBuf, File)> {
    let pid = std::process::id();
    for _ in 0..64 {
        let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_path = parent.join(format!("{prefix}-{pid}-{seq}.tmp"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&tmp_path) {
            Ok(file) => return Ok((tmp_path, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate temporary state file",
    ))
}
