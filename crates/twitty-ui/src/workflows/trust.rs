//! Persistent trust store for project `.twitty/commands.json` files.
//!
//! Stored at `$XDG_STATE_HOME/twitty/trust.json` (fallback `~/.local/state/twitty/trust.json`),
//! keyed by canonical project directory path and SHA-256 digest of the file's raw bytes.
//! Any corrupt, oversized, or symlinked store fails closed (nothing trusted).

use std::ffi::OsStr;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::warn;

use super::MAX_TRUST_ENTRIES;

/// Current on-disk schema version of `trust.json`.
pub const TRUST_STORE_VERSION: u32 = 1;

/// Maximum `trust.json` size read from disk (512 KiB).
pub const MAX_TRUST_FILE_BYTES: usize = 512 * 1024;

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustEntry {
    pub dir: String,
    pub sha256: String,
    pub trusted_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct TrustStoreFile {
    version: u32,
    entries: Vec<TrustEntry>,
}

/// Trusted project command files, oldest entry first (bounded to [`MAX_TRUST_ENTRIES`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrustStore {
    entries: Vec<TrustEntry>,
}

impl TrustStore {
    /// Compute the trust store path from `XDG_STATE_HOME` and `HOME`.
    /// Relative paths are ignored so a repository or untrusted environment cannot redirect trust.
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
        Some(base.join("twitty").join("trust.json"))
    }

    /// Default path of the trust store for the current user.
    pub fn default_path() -> Option<PathBuf> {
        #[cfg(windows)]
        {
            let non_empty = |key: &str| std::env::var_os(key).filter(|v| !v.is_empty());
            let base = non_empty("LOCALAPPDATA").or_else(|| non_empty("APPDATA"))?;
            Some(PathBuf::from(base).join("twitty").join("trust.json"))
        }
        #[cfg(not(windows))]
        {
            let xdg = std::env::var_os("XDG_STATE_HOME");
            let home = std::env::var_os("HOME");
            Self::path_from(xdg.as_deref(), home.as_deref())
        }
    }

    /// Load from the default location; missing or corrupt files yield an empty store.
    pub fn load_default() -> Self {
        Self::default_path().map_or_else(Self::default, |p| Self::load_from(&p))
    }

    /// Load from `path`. Missing, symlinked, non-regular, oversized, wrong-version, or corrupt
    /// files fail closed and return an empty store (nothing trusted).
    pub fn load_from(path: &Path) -> Self {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            match fs::symlink_metadata(parent) {
                Ok(parent_meta) if parent_meta.file_type().is_dir() => {}
                Ok(_) => {
                    warn(format!(
                        "twitty: refusing trust store in non-directory or symlinked parent {}; treating all project commands as untrusted",
                        parent.display()
                    ));
                    return Self::default();
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => return Self::default(),
                Err(err) => {
                    warn(format!(
                        "twitty: ignoring trust store {} ({err}); treating all project commands as untrusted",
                        path.display()
                    ));
                    return Self::default();
                }
            }
        }
        let meta = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Self::default(),
            Err(err) => {
                warn(format!(
                    "twitty: ignoring trust store {} ({err}); treating all project commands as untrusted",
                    path.display()
                ));
                return Self::default();
            }
        };
        if meta.file_type().is_symlink() || !meta.file_type().is_file() {
            warn(format!(
                "twitty: refusing non-regular or symlinked trust store {}; treating all project commands as untrusted",
                path.display()
            ));
            return Self::default();
        }
        if meta.len() > MAX_TRUST_FILE_BYTES as u64 {
            warn(format!(
                "twitty: refusing oversized trust store {}; treating all project commands as untrusted",
                path.display()
            ));
            return Self::default();
        }

        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = match options.open(path) {
            Ok(f) => f,
            Err(err) => {
                warn(format!(
                    "twitty: could not open trust store {} ({err}); treating all project commands as untrusted",
                    path.display()
                ));
                return Self::default();
            }
        };
        let Ok(fd_meta) = file.metadata() else {
            return Self::default();
        };
        if !fd_meta.file_type().is_file() || fd_meta.len() > MAX_TRUST_FILE_BYTES as u64 {
            warn(format!(
                "twitty: refusing trust store {}; treating all project commands as untrusted",
                path.display()
            ));
            return Self::default();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let Ok(uid) = twitty_control::paths::current_uid() else {
                return Self::default();
            };
            if fd_meta.uid() != uid || fd_meta.mode() & 0o6002 != 0 {
                warn(format!(
                    "twitty: refusing trust store {} with unsafe ownership or permissions; treating all project commands as untrusted",
                    path.display()
                ));
                return Self::default();
            }
        }

        let mut bytes = Vec::with_capacity(fd_meta.len().min(MAX_TRUST_FILE_BYTES as u64) as usize);
        if file
            .take((MAX_TRUST_FILE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() > MAX_TRUST_FILE_BYTES
        {
            warn(format!(
                "twitty: could not read trust store {}; treating all project commands as untrusted",
                path.display()
            ));
            return Self::default();
        }

        let parsed: TrustStoreFile = match serde_json::from_slice(&bytes) {
            Ok(file) => file,
            Err(err) => {
                warn(format!(
                    "twitty: corrupt trust store {} ({err}); treating all project commands as untrusted",
                    path.display()
                ));
                return Self::default();
            }
        };
        if parsed.version != TRUST_STORE_VERSION {
            warn(format!(
                "twitty: unsupported trust store version {} in {}; treating all project commands as untrusted",
                parsed.version,
                path.display()
            ));
            return Self::default();
        }
        if !parsed.entries.iter().all(valid_entry) {
            warn(format!(
                "twitty: invalid entry in trust store {}; treating all project commands as untrusted",
                path.display()
            ));
            return Self::default();
        }

        let mut entries = parsed.entries;
        if entries.len() > MAX_TRUST_ENTRIES {
            let excess = entries.len() - MAX_TRUST_ENTRIES;
            entries.drain(0..excess);
        }
        Self { entries }
    }

    /// Number of stored trust entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Current entries, oldest first.
    pub fn entries(&self) -> &[TrustEntry] {
        &self.entries
    }

    /// Whether `canonical_dir` with exact file digest `sha256` is trusted.
    pub fn is_trusted(&self, canonical_dir: &Path, sha256: &str) -> bool {
        let Some(dir) = canonical_dir.to_str() else {
            return false;
        };
        if !is_sha256_hex(sha256) {
            return false;
        }
        self.entries
            .iter()
            .any(|e| e.dir == dir && e.sha256 == sha256)
    }

    /// Record trust for `(canonical_dir, sha256)`, replacing any older digest for the same
    /// directory and dropping the oldest entries when exceeding [`MAX_TRUST_ENTRIES`].
    pub fn trust(&mut self, canonical_dir: &Path, sha256: &str) -> bool {
        let Some(dir) = canonical_dir.to_str() else {
            return false;
        };
        if dir.is_empty() || !is_sha256_hex(sha256) {
            return false;
        }
        self.entries.retain(|e| e.dir != dir);
        let trusted_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        self.entries.push(TrustEntry {
            dir: dir.to_owned(),
            sha256: sha256.to_owned(),
            trusted_at,
        });
        if self.entries.len() > MAX_TRUST_ENTRIES {
            let excess = self.entries.len() - MAX_TRUST_ENTRIES;
            self.entries.drain(0..excess);
        }
        true
    }

    /// Save to the default path.
    pub fn save_default(&self) -> io::Result<()> {
        let path = Self::default_path().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "could not determine trust store directory",
            )
        })?;
        self.save_to(&path)
    }

    /// Atomically write the trust store to `path` (temp file + rename), enforcing directory mode
    /// `0700`, file mode `0600`, and refusing a symlinked target file or symlinked state directory.
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "trust store path {} has no parent directory",
                        path.display()
                    ),
                )
            })?;
        ensure_state_dir(parent)?;

        match fs::symlink_metadata(path) {
            Ok(meta) => {
                if meta.file_type().is_symlink() || !meta.file_type().is_file() {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!(
                            "refusing to write non-regular or symlinked trust store {}",
                            path.display()
                        ),
                    ));
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }

        let payload = TrustStoreFile {
            version: TRUST_STORE_VERSION,
            entries: self.entries.clone(),
        };
        let json = serde_json::to_string_pretty(&payload).map_err(io::Error::other)?;

        let pid = std::process::id();
        let (tmp_path, mut tmp_file) = create_temp_file(parent, pid)?;
        let write_result = (|| -> io::Result<()> {
            tmp_file.write_all(json.as_bytes())?;
            tmp_file.write_all(b"\n")?;
            tmp_file.sync_all()?;
            Ok(())
        })();
        drop(tmp_file);
        if let Err(err) = write_result {
            let _ = fs::remove_file(&tmp_path);
            return Err(err);
        }

        if let Err(err) = fs::rename(&tmp_path, path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(err);
        }
        Ok(())
    }
}

fn valid_entry(entry: &TrustEntry) -> bool {
    !entry.dir.is_empty() && is_sha256_hex(&entry.sha256)
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn ensure_state_dir(parent: &Path) -> io::Result<()> {
    match fs::symlink_metadata(parent) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "trust store directory {} is not a real directory",
                        parent.display()
                    ),
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
            }
            Ok(())
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            if let Some(grandparent) = parent.parent() {
                if !grandparent.as_os_str().is_empty() {
                    fs::create_dir_all(grandparent)?;
                }
            }
            #[cfg_attr(not(unix), allow(unused_mut))]
            let mut builder = DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(parent) {
                Ok(()) => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
                    }
                    Ok(())
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => ensure_state_dir(parent),
                Err(err) => Err(err),
            }
        }
        Err(err) => Err(err),
    }
}

fn create_temp_file(parent: &Path, pid: u32) -> io::Result<(PathBuf, fs::File)> {
    for _ in 0..64 {
        let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_path = parent.join(format!(".trust.json.tmp.{pid}.{seq}"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&tmp_path) {
            Ok(file) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
                }
                return Ok((tmp_path, file));
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate temporary file for trust store",
    ))
}
