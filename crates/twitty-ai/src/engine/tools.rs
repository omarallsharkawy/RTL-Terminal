//! The shared tool schema and strict arguments used by every provider.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Value};

use super::files::{read_text, text_files, Directory};
use super::model::{ContextItem, ContextKind, ReadTool};
use super::policy::{self, Approval};

#[derive(Clone, Deserialize)]
#[serde(
    tag = "name",
    content = "arguments",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(super) enum Tool {
    ReadFile { path: PathBuf },
    ListDir { path: PathBuf },
    Search { path: PathBuf, query: String },
    ReadBlock { label: String },
    RunCommand { command: String, reason: String },
    ApplyPatch { diff: String, reason: String },
    RememberFact { text: String },
}

impl Tool {
    pub fn read_kind(&self) -> Option<(ReadTool, String, Option<&Path>)> {
        match self {
            Self::ReadFile { path } => Some((
                ReadTool::ReadFile,
                path.to_string_lossy().into_owned(),
                Some(path),
            )),
            Self::ListDir { path } => Some((
                ReadTool::ListDir,
                path.to_string_lossy().into_owned(),
                Some(path),
            )),
            Self::Search { path, .. } => Some((
                ReadTool::Search,
                path.to_string_lossy().into_owned(),
                Some(path),
            )),
            Self::ReadBlock { label } => Some((ReadTool::ReadBlock, label.clone(), None)),
            _ => None,
        }
    }
}

pub(super) fn schema() -> Vec<Value> {
    [
        ("read_file", "Read bounded UTF-8 project text", &["path"][..]),
        ("list_dir", "List bounded project directory entries", &["path"][..]),
        ("search", "Search UTF-8 project files for a literal string", &["path", "query"][..]),
        ("read_block", "Read an included terminal block by its exact context label", &["label"][..]),
        ("run_command", "Propose a shell command to run in a visible terminal pane", &["command", "reason"][..]),
        ("apply_patch", "Propose a unified text diff with paths relative to the project root", &["diff", "reason"][..]),
        ("remember_fact", "Suggest a fact for project memory", &["text"][..]),
    ].into_iter().map(|(name, description, fields)| {
        let properties: serde_json::Map<_, _> = fields.iter().map(|field| (field.to_string(), json!({"type": "string"}))).collect();
        json!({"name": name, "description": description, "parameters": {
            "type": "object", "properties": properties, "required": fields, "additionalProperties": false
        }})
    }).collect()
}

pub(super) const MAX_READ_BYTES: usize = 64 * 1024;
const MAX_SEARCH_ENTRIES: usize = 512;
const MAX_MATCHES: usize = 80;

pub(super) fn read(
    tool: &Tool,
    root: &Path,
    resolved: Option<&Path>,
    context: &[ContextItem],
) -> Option<String> {
    match tool {
        Tool::ReadFile { .. } => read_text(resolved?, MAX_READ_BYTES).ok(),
        Tool::ListDir { .. } => {
            let path = resolved?;
            let mut names = Directory::open(path)
                .ok()?
                .entries(MAX_SEARCH_ENTRIES)
                .ok()?;
            names.sort_unstable();
            Some(
                names
                    .into_iter()
                    .filter_map(|name| {
                        let child = path.join(&name);
                        match policy::path_policy(root, &child, false) {
                            // An approved directory listing may show names in that directory. Protected
                            // paths stay hidden, and a listing never exposes file contents.
                            Ok((canonical, _)) if canonical.starts_with(path) => {
                                name.into_string().ok()
                            }
                            _ => None,
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        }
        Tool::Search { query, .. } => search(resolved?, root, query),
        Tool::ReadBlock { label } => context
            .iter()
            .find(|item| item.included && item.kind == ContextKind::Block && item.label == *label)
            .map(|item| item.text.clone()),
        _ => None,
    }
}

fn search(path: &Path, root: &Path, query: &str) -> Option<String> {
    use std::fmt::Write;

    if query.is_empty() || query.len() > 1024 || query.contains(['\r', '\n']) {
        return None;
    }
    // A prompted directory outside the project authorizes only that subtree, not other
    // sensitive files. An explicitly approved single file may be searched directly.
    let search_root = if path.starts_with(root) { root } else { path };
    let mut output = String::new();
    let mut hits = 0;
    text_files(path, search_root, MAX_SEARCH_ENTRIES, |file| {
        if file == path || matches!(policy::read_path(search_root, file), Approval::Allow) {
            if let Ok(text) = read_text(file, MAX_READ_BYTES) {
                for (index, line) in text
                    .lines()
                    .enumerate()
                    .filter(|(_, line)| line.contains(query))
                {
                    let label = file.strip_prefix(root).unwrap_or(file).display();
                    let preview = super::bounded(line, 512);
                    if output.len() + preview.len() + file.as_os_str().len() + 32 > MAX_READ_BYTES {
                        return false;
                    }
                    let _ = writeln!(output, "{label}:{}: {preview}", index + 1);
                    hits += 1;
                    if hits >= MAX_MATCHES {
                        return false;
                    }
                }
            }
        }
        true
    })
    .ok()?;
    Some(output)
}
