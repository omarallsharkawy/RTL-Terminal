//! Strict unified diff validation and reversible file transactions.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::files::{Directory, Snapshot};
use super::model::{DenyReason, PatchFile};
use super::policy::{self, relative_patch_path};

pub const MAX_PATCH_BYTES: usize = 512 * 1024;
const MAX_FILES: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchError {
    Invalid,
    Policy(DenyReason),
    Conflict,
    Io,
    Rollback,
}

struct Change {
    path: PathBuf,
    before: Option<Snapshot>,
    after: Option<Snapshot>,
    summary: PatchFile,
}

/// A validated diff and the exact file states expected at apply and undo time.
pub struct Patch {
    changes: Vec<Change>,
}

impl Patch {
    pub fn validate(root: &Path, diff: &str) -> Result<Self, PatchError> {
        if diff.len() > MAX_PATCH_BYTES || diff.contains('\0') {
            return Err(PatchError::Invalid);
        }
        let diff = parse(diff)?;
        let mut changes = Vec::new();
        let mut seen = HashSet::new();
        let mut bytes = 0usize;
        for entry in diff {
            let relative = entry
                .old
                .as_ref()
                .or(entry.new.as_ref())
                .ok_or(PatchError::Invalid)?;
            let (path, _) =
                policy::path_policy(root, relative, true).map_err(PatchError::Policy)?;
            // Preserve the identity shown on the approval card. An alias could be retargeted.
            if path != root.join(relative) || !seen.insert(path.clone()) {
                return Err(PatchError::Invalid);
            }
            let (dir, name) = Directory::parent(&path).map_err(|_| PatchError::Io)?;
            let before = dir
                .read(name, MAX_PATCH_BYTES)
                .map_err(|_| PatchError::Invalid)?;
            if before.is_some() != entry.old.is_some() {
                return Err(PatchError::Conflict);
            }
            let original = before.as_ref().map_or("", |file| file.text.as_str());
            let (text, added, removed) = apply_hunks(original, entry.hunks)?;
            bytes = bytes
                .saturating_add(original.len())
                .saturating_add(text.len());
            if bytes > MAX_PATCH_BYTES || (entry.new.is_none() && !text.is_empty()) {
                return Err(PatchError::Invalid);
            }
            let after = if entry.new.is_some() {
                let permissions = match &before {
                    Some(snapshot) => snapshot.permissions.clone(),
                    None => new_permissions(root).map_err(|_| PatchError::Io)?,
                };
                Some(Snapshot { text, permissions })
            } else {
                None
            };
            changes.push(Change {
                path,
                before,
                after,
                summary: PatchFile {
                    path: relative.clone(),
                    added,
                    removed,
                    new_file: entry.old.is_none(),
                    deleted: entry.new.is_none(),
                },
            });
        }
        Ok(Self { changes })
    }

    pub fn files(&self) -> Vec<PatchFile> {
        self.changes
            .iter()
            .map(|change| change.summary.clone())
            .collect()
    }

    pub fn apply(&self) -> Result<(), PatchError> {
        self.transact(false)
    }

    pub fn undo(&self) -> Result<(), PatchError> {
        self.transact(true)
    }

    pub(super) fn undo_record(&self) -> Value {
        Value::Array(
            self.changes
                .iter()
                .map(|change| {
                    json!({
                        "path": change.summary.path.to_string_lossy(),
                        "before": change.before.as_ref().map(|file| &file.text),
                        "after": change.after.as_ref().map(|file| &file.text),
                        "before_mode": change.before.as_ref().map(Snapshot::mode),
                        "after_mode": change.after.as_ref().map(Snapshot::mode),
                    })
                })
                .collect(),
        )
    }

    fn transact(&self, undo: bool) -> Result<(), PatchError> {
        let dirs = self
            .changes
            .iter()
            .map(|change| Directory::parent(&change.path))
            .collect::<io::Result<Vec<_>>>()
            .map_err(|_| PatchError::Conflict)?;
        let states: Vec<_> = self
            .changes
            .iter()
            .map(|change| {
                if undo {
                    (&change.after, &change.before)
                } else {
                    (&change.before, &change.after)
                }
            })
            .collect();
        let mut staged = Vec::with_capacity(dirs.len());
        for ((dir, name), (expected, next)) in dirs.iter().zip(&states) {
            check(dir, name, expected)?;
            let forward = dir.stage(next.as_ref()).map_err(|_| PatchError::Io)?;
            let reverse = dir.stage(expected.as_ref()).map_err(|_| PatchError::Io)?;
            staged.push((forward, reverse));
        }

        for (index, ((dir, name), (expected, _))) in dirs.iter().zip(&states).enumerate() {
            let result = check(dir, name, expected).and_then(|()| {
                staged[index]
                    .0
                    .commit(name, expected.is_none())
                    .map_err(|_| PatchError::Io)
            });
            if let Err(err) = result {
                let mut rollback_failed = false;
                for previous in (0..index).rev() {
                    let (dir, name) = &dirs[previous];
                    let after = states[previous].1;
                    rollback_failed |= check(dir, name, after)
                        .and_then(|()| {
                            staged[previous]
                                .1
                                .commit(name, after.is_none())
                                .map_err(|_| PatchError::Rollback)
                        })
                        .is_err();
                }
                return Err(if rollback_failed {
                    PatchError::Rollback
                } else {
                    err
                });
            }
        }
        Ok(())
    }
}

fn check(
    dir: &Directory,
    name: &std::ffi::OsStr,
    expected: &Option<Snapshot>,
) -> Result<(), PatchError> {
    if dir.read(name, MAX_PATCH_BYTES).ok().as_ref() == Some(expected) {
        Ok(())
    } else {
        Err(PatchError::Conflict)
    }
}

#[cfg(unix)]
fn new_permissions(_: &Path) -> io::Result<fs::Permissions> {
    use std::os::unix::fs::PermissionsExt;
    Ok(fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
// Only compiled off Unix, where the lint's concern (a world-writable file) does not apply:
// Windows has no mode bits and access comes from the folder's ACL.
#[allow(clippy::permissions_set_readonly_false)]
fn new_permissions(root: &Path) -> io::Result<fs::Permissions> {
    let mut permissions = fs::metadata(root)?.permissions();
    permissions.set_readonly(false);
    Ok(permissions)
}

struct FileDiff<'a> {
    old: Option<PathBuf>,
    new: Option<PathBuf>,
    hunks: Vec<Hunk<'a>>,
}

struct Hunk<'a> {
    position: usize,
    next_position: usize,
    lines: Vec<(u8, &'a str)>,
}

fn header_path(line: &str, marker: &str, prefix: &str) -> Result<Option<PathBuf>, PatchError> {
    let path = line.strip_prefix(marker).ok_or(PatchError::Invalid)?;
    let path = path.split('\t').next().ok_or(PatchError::Invalid)?;
    if path == "/dev/null" {
        return Ok(None);
    }
    let path = path.strip_prefix(prefix).unwrap_or(path);
    if path.contains(['\\', '"', ':'])
        || path
            .chars()
            .any(|ch| ch.is_control() || twitty_core::terminal::is_bidi_control(ch))
        || path.split('/').any(|c| matches!(c, "" | "." | ".."))
        || !relative_patch_path(Path::new(path))
    {
        return Err(PatchError::Invalid);
    }
    Ok(Some(path.into()))
}

fn parse(diff: &str) -> Result<Vec<FileDiff<'_>>, PatchError> {
    let mut lines = diff.split_inclusive('\n').peekable();
    let mut files = Vec::new();
    while let Some(line) = lines.next() {
        if files.len() == MAX_FILES {
            return Err(PatchError::Invalid);
        }
        let mut old_line = line;
        if line.starts_with("diff --git ") {
            while let Some(line) = lines.peek() {
                if ["index ", "new file mode ", "deleted file mode "]
                    .iter()
                    .any(|prefix| line.starts_with(prefix))
                {
                    lines.next();
                } else {
                    break;
                }
            }
            old_line = lines.next().ok_or(PatchError::Invalid)?;
        }
        let new_line = lines.next().ok_or(PatchError::Invalid)?;
        let old_line = old_line.trim_end_matches(['\r', '\n']);
        let new_line = new_line.trim_end_matches(['\r', '\n']);
        let git_paths = (old_line.starts_with("--- a/")
            && (new_line.starts_with("+++ b/") || new_line == "+++ /dev/null"))
            || (old_line == "--- /dev/null" && new_line.starts_with("+++ b/"));
        let old = header_path(old_line, "--- ", if git_paths { "a/" } else { "" })?;
        let new = header_path(new_line, "+++ ", if git_paths { "b/" } else { "" })?;
        if old.is_some() && new.is_some() && old != new || old.is_none() && new.is_none() {
            return Err(PatchError::Invalid);
        }
        let mut hunks = Vec::new();
        while lines.peek().is_some_and(|line| line.starts_with("@@ ")) {
            let header = lines.next().ok_or(PatchError::Invalid)?;
            let (old, new) = header
                .strip_prefix("@@ -")
                .and_then(|h| h.split_once(" +"))
                .and_then(|(old, rest)| rest.split_once(" @@").map(|(new, _)| (old, new)))
                .ok_or(PatchError::Invalid)?;
            let (position, mut old_count) = range(old)?;
            let (next_position, mut new_count) = range(new)?;
            let mut hunk = Hunk {
                position,
                next_position,
                lines: Vec::new(),
            };
            while old_count > 0 || new_count > 0 {
                let line = lines.next().ok_or(PatchError::Invalid)?;
                let (kind, text) = match line.as_bytes() {
                    [kind @ (b' ' | b'+' | b'-'), ..] => (*kind, &line[1..]),
                    _ => return Err(PatchError::Invalid),
                };
                old_count = old_count
                    .checked_sub(usize::from(kind != b'+'))
                    .ok_or(PatchError::Invalid)?;
                new_count = new_count
                    .checked_sub(usize::from(kind != b'-'))
                    .ok_or(PatchError::Invalid)?;
                let text = if lines
                    .peek()
                    .is_some_and(|next| next.trim_end() == "\\ No newline at end of file")
                {
                    lines.next();
                    text.strip_suffix('\n').ok_or(PatchError::Invalid)?
                } else if text.ends_with('\n') {
                    text
                } else {
                    return Err(PatchError::Invalid);
                };
                hunk.lines.push((kind, text));
            }
            if hunk.lines.is_empty() {
                return Err(PatchError::Invalid);
            }
            hunks.push(hunk);
        }
        if hunks.is_empty() {
            return Err(PatchError::Invalid);
        }
        files.push(FileDiff { old, new, hunks });
    }
    if files.is_empty() {
        Err(PatchError::Invalid)
    } else {
        Ok(files)
    }
}

fn range(text: &str) -> Result<(usize, usize), PatchError> {
    let (start, count) = text.split_once(',').unwrap_or((text, "1"));
    let start = start.parse::<usize>().map_err(|_| PatchError::Invalid)?;
    let count = count.parse::<usize>().map_err(|_| PatchError::Invalid)?;
    let position = start
        .checked_sub(usize::from(count != 0))
        .ok_or(PatchError::Invalid)?;
    Ok((position, count))
}

fn apply_hunks(original: &str, hunks: Vec<Hunk<'_>>) -> Result<(String, u32, u32), PatchError> {
    let input: Vec<_> = original.split_inclusive('\n').collect();
    let mut output = Vec::new();
    let mut cursor = 0;
    let mut added = 0;
    let mut removed = 0;
    for hunk in hunks {
        output.extend(
            input
                .get(cursor..hunk.position)
                .ok_or(PatchError::Conflict)?,
        );
        if output.len() != hunk.next_position {
            return Err(PatchError::Invalid);
        }
        cursor = hunk.position;
        for (kind, line) in hunk.lines {
            if kind != b'+' {
                if input.get(cursor) != Some(&line) {
                    return Err(PatchError::Conflict);
                }
                cursor += 1;
            }
            if kind != b'-' {
                output.push(line);
            }
            added += u32::from(kind == b'+');
            removed += u32::from(kind == b'-');
        }
    }
    output.extend(&input[cursor..]);
    if output
        .iter()
        .rev()
        .skip(1)
        .any(|line| !line.ends_with('\n'))
    {
        return Err(PatchError::Invalid);
    }
    Ok((output.concat(), added, removed))
}
