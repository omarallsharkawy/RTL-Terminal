//! Bounded file I/O. Unix operations walk canonical path components without following symlinks.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
pub(super) struct Directory(File);
#[cfg(not(unix))]
pub(super) struct Directory(PathBuf);

impl Directory {
    #[cfg(unix)]
    pub fn open(path: &Path) -> io::Result<Self> {
        use rustix::fs::{openat, Mode, OFlags};
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let mut directory = File::open("/")?;
        for component in path.components() {
            match component {
                std::path::Component::Normal(name) => {
                    directory = openat(&directory, name, flags, Mode::empty())?.into();
                }
                std::path::Component::RootDir => {}
                _ => return Err(io::ErrorKind::InvalidInput.into()),
            }
        }
        Ok(Self(directory))
    }

    #[cfg(not(unix))]
    pub fn open(path: &Path) -> io::Result<Self> {
        if path.canonicalize()? != path || !path.is_dir() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(Self(path.into()))
    }

    pub fn parent(path: &Path) -> io::Result<(Self, &OsStr)> {
        let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
        let name = path.file_name().ok_or(io::ErrorKind::InvalidInput)?;
        Ok((Self::open(parent)?, name))
    }

    #[cfg(unix)]
    fn file(&self, name: &OsStr) -> io::Result<File> {
        use rustix::fs::{openat, Mode, OFlags};
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
        Ok(openat(&self.0, name, flags, Mode::empty())?.into())
    }

    #[cfg(not(unix))]
    fn file(&self, name: &OsStr) -> io::Result<File> {
        let path = self.0.join(name);
        if path.symlink_metadata()?.file_type().is_symlink() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        File::open(path)
    }

    pub fn read(&self, name: &OsStr, max_bytes: usize) -> io::Result<Option<Snapshot>> {
        let file = match self.file(name) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > max_bytes as u64 {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let mut bytes = Vec::new();
        file.take(max_bytes as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > max_bytes || bytes.contains(&0) {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let text = String::from_utf8(bytes).map_err(|_| io::ErrorKind::InvalidData)?;
        Ok(Some(Snapshot {
            text,
            permissions: metadata.permissions(),
        }))
    }

    pub fn entries(&self, max: usize) -> io::Result<Vec<OsString>> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let dir = rustix::fs::Dir::read_from(&self.0)?;
            dir.filter_map(|entry| match entry {
                Ok(entry) => {
                    let name = entry.file_name().to_bytes();
                    (!matches!(name, b"." | b"..")).then(|| Ok(OsStr::from_bytes(name).to_owned()))
                }
                Err(err) => Some(Err(err.into())),
            })
            .take(max)
            .collect()
        }
        #[cfg(not(unix))]
        {
            fs::read_dir(&self.0)?
                .take(max)
                .map(|entry| entry.map(|entry| entry.file_name()))
                .collect()
        }
    }

    #[cfg(unix)]
    fn create_temp(&self, name: &OsStr) -> io::Result<File> {
        use rustix::fs::{openat, Mode, OFlags};
        let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC;
        Ok(openat(&self.0, name, flags, Mode::RUSR | Mode::WUSR)?.into())
    }

    #[cfg(not(unix))]
    fn create_temp(&self, name: &OsStr) -> io::Result<File> {
        File::options()
            .write(true)
            .create_new(true)
            .open(self.0.join(name))
    }

    #[cfg(unix)]
    fn remove(&self, name: &OsStr) -> io::Result<()> {
        Ok(rustix::fs::unlinkat(
            &self.0,
            name,
            rustix::fs::AtFlags::empty(),
        )?)
    }

    #[cfg(not(unix))]
    fn remove(&self, name: &OsStr) -> io::Result<()> {
        fs::remove_file(self.0.join(name))
    }

    #[cfg(unix)]
    fn install(&self, from: &OsStr, to: &OsStr, new: bool) -> io::Result<()> {
        if new {
            rustix::fs::linkat(&self.0, from, &self.0, to, rustix::fs::AtFlags::empty())?;
        } else {
            rustix::fs::renameat(&self.0, from, &self.0, to)?;
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn install(&self, from: &OsStr, to: &OsStr, new: bool) -> io::Result<()> {
        if new {
            fs::hard_link(self.0.join(from), self.0.join(to))
        } else {
            fs::rename(self.0.join(from), self.0.join(to))
        }
    }

    pub fn stage<'a>(&'a self, snapshot: Option<&Snapshot>) -> io::Result<Staged<'a>> {
        let Some(snapshot) = snapshot else {
            return Ok(Staged {
                dir: self,
                name: None,
            });
        };
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        for _ in 0..64 {
            let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
            let name = OsString::from(format!(".twitty-patch-{}-{seq}", std::process::id()));
            match self.create_temp(&name) {
                Ok(mut file) => {
                    let staged = Staged {
                        dir: self,
                        name: Some(name),
                    };
                    file.write_all(snapshot.text.as_bytes())?;
                    set_permissions(&file, &snapshot.permissions)?;
                    file.sync_all()?;
                    return Ok(staged);
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                Err(err) => return Err(err),
            }
        }
        Err(io::ErrorKind::AlreadyExists.into())
    }
}

#[cfg(unix)]
fn set_permissions(file: &File, permissions: &fs::Permissions) -> io::Result<()> {
    file.set_permissions(permissions.clone())
}

/// Windows permissions are file attributes; copying a folder's (or another file's) attributes
/// can carry flags such as "directory" that make the call fail. Only the read-only flag counts.
#[cfg(not(unix))]
fn set_permissions(file: &File, permissions: &fs::Permissions) -> io::Result<()> {
    let mut current = file.metadata()?.permissions();
    if current.readonly() == permissions.readonly() {
        return Ok(());
    }
    current.set_readonly(permissions.readonly());
    file.set_permissions(current)
}

pub(super) struct Staged<'a> {
    dir: &'a Directory,
    name: Option<OsString>,
}

impl Staged<'_> {
    pub fn commit(&self, target: &OsStr, new: bool) -> io::Result<()> {
        match &self.name {
            Some(name) => self.dir.install(name, target, new),
            None => self.dir.remove(target),
        }
    }
}

impl Drop for Staged<'_> {
    fn drop(&mut self) {
        if let Some(name) = &self.name {
            let _ = self.dir.remove(name);
        }
    }
}

pub(super) struct Snapshot {
    pub text: String,
    pub permissions: fs::Permissions,
}

impl Snapshot {
    pub fn mode(&self) -> u32 {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            self.permissions.mode() & 0o7777
        }
        #[cfg(not(unix))]
        {
            u32::from(self.permissions.readonly())
        }
    }
}

impl PartialEq for Snapshot {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text && self.mode() == other.mode()
    }
}

pub(super) fn read_text(path: &Path, max_bytes: usize) -> io::Result<String> {
    let (dir, name) = Directory::parent(path)?;
    dir.read(name, max_bytes)?
        .map(|file| file.text)
        .ok_or_else(|| io::ErrorKind::NotFound.into())
}

pub(super) fn text_files(
    start: &Path,
    root: &Path,
    max_entries: usize,
    mut visit: impl FnMut(&Path) -> bool,
) -> io::Result<()> {
    let mut pending = vec![PathBuf::from(start)];
    let mut remaining = max_entries;
    while let Some(path) = pending.pop() {
        if let Ok(dir) = Directory::open(&path) {
            let mut entries = dir.entries(remaining)?;
            remaining = remaining.saturating_sub(entries.len());
            entries.sort_unstable();
            for entry in entries.into_iter().rev() {
                if matches!(
                    entry.to_str(),
                    Some(
                        ".git"
                            | ".hg"
                            | ".svn"
                            | "target"
                            | "node_modules"
                            | "dist"
                            | "__pycache__"
                    )
                ) {
                    continue;
                }
                let child = path.join(entry);
                if matches!(super::policy::path_policy(root, &child, false), Ok((canonical, super::policy::Approval::Allow)) if canonical == child)
                {
                    pending.push(child);
                }
            }
        } else if !visit(&path) {
            break;
        }
    }
    Ok(())
}
