//! One-time privacy consent store for built-in AI requests.
//!
//! Stored at `$XDG_STATE_HOME/twitty/ai-consent.json` (fallback
//! `~/.local/state/twitty/ai-consent.json`, or `%LOCALAPPDATA%\twitty\ai-consent.json` on
//! Windows). Refuses symlinks and relative state directories, writes atomically with `0600` file
//! permissions in a `0700` directory, and fails closed (`accepted = false`) on corrupt files.

use std::ffi::OsStr;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use twitty_core::en;
use twitty_core::language::{fill, UiLanguage};
use twitty_core::shell_integration::clean_text;
use twitty_core::warnings::warn;

/// On-disk schema version of `ai-consent.json`.
pub const AI_CONSENT_VERSION: u32 = 1;

/// Maximum `ai-consent.json` size read from disk (64 KiB).
pub const MAX_CONSENT_FILE_BYTES: usize = 64 * 1024;

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ConsentFile {
    version: u32,
    accepted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    #[serde(default)]
    accepted_at: u64,
}

/// Bilingual one-time privacy notice displayed before the first AI request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentNotice {
    pub title_en: String,
    pub title_ar: String,
    pub body_en: String,
    pub body_ar: String,
    pub allow_label_en: &'static str,
    pub allow_label_ar: &'static str,
    pub cancel_label_en: &'static str,
    pub cancel_label_ar: &'static str,
}

const TITLE: &str = en!("Built-in AI Privacy Notice");
const BODY: &str = en!(
    "Selected text, your prompt, working directory, or command output will be passed to your \
     local \"{}\" CLI and sent to its provider. Twitty stores no API keys and makes no network \
     requests itself."
);
const ALLOW: &str = en!("Allow & Continue");
const CANCEL: &str = en!("Cancel");

impl ConsentNotice {
    pub fn for_agent(agent_name: &str) -> Self {
        let agent = clean_text(agent_name, 64);
        let agent = if agent.is_empty() { "agent" } else { &agent };
        Self {
            title_en: TITLE.into(),
            title_ar: UiLanguage::Arabic.t(TITLE).into(),
            body_en: fill(BODY, &[&agent]),
            body_ar: fill(UiLanguage::Arabic.t(BODY), &[&agent]),
            allow_label_en: ALLOW,
            allow_label_ar: UiLanguage::Arabic.t(ALLOW),
            cancel_label_en: CANCEL,
            cancel_label_ar: UiLanguage::Arabic.t(CANCEL),
        }
    }
}

/// Persisted user consent for sending terminal context to the chosen local coding agent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AiConsentStore {
    accepted: bool,
    agent: Option<String>,
    accepted_at: u64,
}

impl AiConsentStore {
    /// Compute the consent file path from `XDG_STATE_HOME` and `HOME` (ignoring relative paths).
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
        Some(base.join("twitty").join("ai-consent.json"))
    }

    /// Default path of the AI consent file for the current user.
    pub fn default_path() -> Option<PathBuf> {
        #[cfg(windows)]
        {
            let non_empty = |key: &str| std::env::var_os(key).filter(|v| !v.is_empty());
            let base = non_empty("LOCALAPPDATA").or_else(|| non_empty("APPDATA"))?;
            Some(PathBuf::from(base).join("twitty").join("ai-consent.json"))
        }
        #[cfg(not(windows))]
        {
            let xdg = std::env::var_os("XDG_STATE_HOME");
            let home = std::env::var_os("HOME");
            Self::path_from(xdg.as_deref(), home.as_deref())
        }
    }

    pub fn load_default() -> Self {
        Self::default_path().map_or_else(Self::default, |p| Self::load_from(&p))
    }

    /// Load consent state from `path`. Missing, symlinked, non-regular, oversized, wrong-version,
    /// or corrupt files fail closed (`accepted = false`).
    pub fn load_from(path: &Path) -> Self {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            match fs::symlink_metadata(parent) {
                Ok(meta) if meta.file_type().is_dir() => {}
                Ok(_) => {
                    warn(format!(
                        "twitty: refusing AI consent store in non-directory or symlinked parent {}",
                        parent.display()
                    ));
                    return Self::default();
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => return Self::default(),
                Err(_) => return Self::default(),
            }
        }
        let meta = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Self::default(),
            Err(_) => return Self::default(),
        };
        if meta.file_type().is_symlink()
            || !meta.file_type().is_file()
            || meta.len() > MAX_CONSENT_FILE_BYTES as u64
        {
            return Self::default();
        }

        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(
                (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
            );
        }
        let Ok(file) = options.open(path) else {
            return Self::default();
        };
        let Ok(fd_meta) = file.metadata() else {
            return Self::default();
        };
        if !fd_meta.file_type().is_file() || fd_meta.len() > MAX_CONSENT_FILE_BYTES as u64 {
            return Self::default();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let uid = rustix::process::getuid().as_raw();
            if fd_meta.uid() != uid || fd_meta.mode() & 0o6022 != 0 {
                return Self::default();
            }
        }

        let mut bytes = Vec::new();
        if file
            .take((MAX_CONSENT_FILE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() > MAX_CONSENT_FILE_BYTES
        {
            return Self::default();
        }

        let Ok(parsed) = serde_json::from_slice::<ConsentFile>(&bytes) else {
            return Self::default();
        };
        if parsed.version != AI_CONSENT_VERSION {
            return Self::default();
        }
        Self {
            accepted: parsed.accepted,
            agent: parsed.agent,
            accepted_at: parsed.accepted_at,
        }
    }

    pub fn is_accepted(&self) -> bool {
        self.accepted
    }

    pub fn accepted_agent(&self) -> Option<&str> {
        self.agent.as_deref()
    }

    pub fn accept(&mut self, agent_name: &str) {
        self.accepted = true;
        let clean = clean_text(agent_name, 64);
        self.agent = (!clean.is_empty()).then_some(clean);
        self.accepted_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
    }

    pub fn save_default(&self) -> io::Result<()> {
        let path = Self::default_path().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "could not determine AI consent state directory",
            )
        })?;
        self.save_to(&path)
    }

    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("AI consent path {} has no parent", path.display()),
                )
            })?;
        ensure_state_dir(parent)?;

        match fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.file_type().is_file() => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "refusing to write non-regular or symlinked {}",
                        path.display()
                    ),
                ));
            }
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }

        let payload = ConsentFile {
            version: AI_CONSENT_VERSION,
            accepted: self.accepted,
            agent: self.agent.clone(),
            accepted_at: self.accepted_at,
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

fn ensure_state_dir(parent: &Path) -> io::Result<()> {
    match fs::symlink_metadata(parent) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "state directory {} is not a real directory",
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
        let tmp_path = parent.join(format!(".ai-consent.json.tmp.{pid}.{seq}"));
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
        "could not allocate temporary file for AI consent store",
    ))
}
