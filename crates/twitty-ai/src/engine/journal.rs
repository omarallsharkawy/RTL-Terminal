//! Private per-process session journals. Every value is scrubbed before serialization.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use twitty_core::redact::redact;

use super::model::SessionId;

pub(super) fn default_dir() -> PathBuf {
    crate::ai::consent::AiConsentStore::default_path()
        .and_then(|path| path.parent().map(|parent| parent.join("agent")))
        .unwrap_or_default()
}

pub(super) struct Journal(File);

impl Journal {
    pub fn open(dir: &Path, session: SessionId) -> io::Result<Self> {
        if !dir.is_absolute() || dir.parent().is_none() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(dir)?;
        if dir.symlink_metadata()?.file_type().is_symlink() {
            return Err(io::ErrorKind::InvalidInput.into());
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }

        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let seq = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name = format!("{now}-{}-{seq}-{}.jsonl", std::process::id(), session.0);
        Ok(Self(options.open(dir.join(name))?))
    }

    pub fn append(&mut self, mut record: Value) -> io::Result<()> {
        scrub(&mut record, None);
        let mut line = serde_json::to_vec(&record)?;
        line.push(b'\n');
        self.0.write_all(&line)?;
        self.0.sync_data()
    }
}

/// Apply redaction to structured values before the serializer escapes newlines and quotes.
/// Field names in model-supplied arguments also participate in redaction.
pub(super) fn scrub(value: &mut Value, credential: Option<&str>) {
    match value {
        Value::String(text) => *text = sanitized(text, credential),
        Value::Array(values) => {
            for value in values {
                scrub(value, credential);
            }
        }
        Value::Object(fields) => {
            for (key, mut value) in std::mem::take(fields) {
                // A structured environment value loses its assignment syntax when serialized.
                // Run the shared redactor over that assignment before processing the standalone string.
                if let Some(text) = value.as_str() {
                    let assignment = format!("{key}={text}");
                    if redact(&assignment) != assignment {
                        value = Value::String("[REDACTED]".into());
                    }
                }
                scrub(&mut value, credential);
                fields.insert(sanitized(&key, credential), value);
            }
        }
        _ => {}
    }
}

pub(super) fn sanitized(text: &str, credential: Option<&str>) -> String {
    let text = credential
        .filter(|key| !key.is_empty() && text.contains(key))
        .map_or(std::borrow::Cow::Borrowed(text), |key| {
            text.replace(key, "[REDACTED]").into()
        });
    redact(&text).into_owned()
}
