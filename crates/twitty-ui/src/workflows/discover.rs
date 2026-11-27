//! Bounded discovery of project `.twitty/commands.json` files.
//!
//! Walks up from a pane's working directory to the first directory containing
//! `.twitty/commands.json`, stopping when reaching `$HOME` (exclusive, since user commands belong
//! in `config.json`) or the filesystem root, after at most [`MAX_DISCOVERY_LEVELS`] levels.
//! Refuses symlinks, non-regular files, and files larger than [`MAX_PROJECT_FILE_BYTES`].

use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::config::warn;
use crate::shell_integration::clean_text;

use super::parse::parse_project_bytes;
use super::{ProjectFile, MAX_DISCOVERY_LEVELS, MAX_PROJECT_FILE_BYTES};
use twitty_store::sha256_hex;

/// Discover and parse the nearest `.twitty/commands.json` starting from `start_cwd`, using
/// `$HOME` (or `%USERPROFILE%` on Windows) as the ceiling directory.
pub fn discover_project_file_default(start_cwd: &Path) -> Option<ProjectFile> {
    let home = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()))
        .map(PathBuf::from);
    discover_project_file(start_cwd, home.as_deref())
}

/// Discover and parse the nearest `.twitty/commands.json` starting from `start_cwd`.
///
/// Stops before ascending into `home` (if `start_cwd` is inside `home`), at the filesystem root,
/// or after checking [`MAX_DISCOVERY_LEVELS`] directories.
pub fn discover_project_file(start_cwd: &Path, home: Option<&Path>) -> Option<ProjectFile> {
    if !start_cwd.is_absolute() {
        return None;
    }
    let canon_start = fs::canonicalize(start_cwd).ok()?;
    let canon_home = home.and_then(|h| fs::canonicalize(h).ok());

    for dir in canon_start.ancestors().take(MAX_DISCOVERY_LEVELS) {
        if canon_home.as_deref() == Some(dir) {
            break;
        }
        // Never treat the filesystem root itself as a project root.
        if dir.parent().is_none() {
            break;
        }
        let dot_twitty = dir.join(".twitty");
        let dir_meta = match fs::symlink_metadata(&dot_twitty) {
            Ok(meta) => meta,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => continue,
        };
        if dir_meta.file_type().is_symlink() || !dir_meta.file_type().is_dir() {
            // If `.twitty` itself is a symlink or non-directory, refuse only if it looks like a
            // commands container or symlinked `.twitty` dir, and stop walking.
            warn(format!(
                "twitty: refusing project commands in {} (.twitty is not a regular directory)",
                dir.display()
            ));
            return None;
        }

        let file_path = dot_twitty.join("commands.json");
        let file_meta = match fs::symlink_metadata(&file_path) {
            Ok(meta) => meta,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                warn(format!("twitty: refusing {} ({err})", file_path.display()));
                return None;
            }
        };

        // First `.twitty/commands.json` found: either load it or refuse and stop walking.
        return load_project_file(dir, &file_path, &file_meta);
    }
    None
}

fn load_project_file(
    canonical_root: &Path,
    file_path: &Path,
    lstat_meta: &fs::Metadata,
) -> Option<ProjectFile> {
    if lstat_meta.file_type().is_symlink() {
        warn(format!(
            "twitty: refusing symlinked project commands file {}",
            file_path.display()
        ));
        return None;
    }
    if !lstat_meta.file_type().is_file() {
        warn(format!(
            "twitty: refusing non-regular project commands file {}",
            file_path.display()
        ));
        return None;
    }
    if lstat_meta.len() > MAX_PROJECT_FILE_BYTES as u64 {
        warn(format!(
            "twitty: refusing project commands file {} (exceeds {} bytes)",
            file_path.display(),
            MAX_PROJECT_FILE_BYTES
        ));
        return None;
    }

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Guard against TOCTOU symlink swap or FIFO substitution between lstat and open.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(file_path) {
        Ok(f) => f,
        Err(err) => {
            warn(format!(
                "twitty: refusing project commands file {} ({err})",
                file_path.display()
            ));
            return None;
        }
    };
    let fd_meta = match file.metadata() {
        Ok(m) => m,
        Err(err) => {
            warn(format!(
                "twitty: refusing project commands file {} ({err})",
                file_path.display()
            ));
            return None;
        }
    };
    if !fd_meta.file_type().is_file() {
        warn(format!(
            "twitty: refusing non-regular project commands file {}",
            file_path.display()
        ));
        return None;
    }
    if fd_meta.len() > MAX_PROJECT_FILE_BYTES as u64 {
        warn(format!(
            "twitty: refusing project commands file {} (exceeds {} bytes)",
            file_path.display(),
            MAX_PROJECT_FILE_BYTES
        ));
        return None;
    }

    let mut bytes = Vec::with_capacity(fd_meta.len().min(MAX_PROJECT_FILE_BYTES as u64) as usize);
    if let Err(err) = file
        .take((MAX_PROJECT_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
    {
        warn(format!(
            "twitty: could not read {} ({err})",
            file_path.display()
        ));
        return None;
    }
    if bytes.len() > MAX_PROJECT_FILE_BYTES {
        warn(format!(
            "twitty: refusing project commands file {} (exceeds {} bytes)",
            file_path.display(),
            MAX_PROJECT_FILE_BYTES
        ));
        return None;
    }

    let sha256 = sha256_hex(&bytes);
    let label = file_path.display().to_string();
    let commands = parse_project_bytes(&bytes, &label).ok()?;
    if commands.is_empty() {
        return None;
    }

    let raw_tag = canonical_root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project");
    let cleaned_tag = clean_text(raw_tag, 48);
    let tag = if cleaned_tag.is_empty() {
        "project".to_owned()
    } else {
        cleaned_tag
    };

    Some(ProjectFile {
        root: canonical_root.to_path_buf(),
        tag,
        sha256,
        commands,
    })
}
