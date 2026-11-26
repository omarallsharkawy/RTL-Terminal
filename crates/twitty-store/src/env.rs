//! Bounded, process-free detection of project environment notes (`kind = 'env'`).
//!
//! Inspects local configuration files inside the project root:
//! - Rust toolchain (`rust-toolchain.toml` or `rust-toolchain`)
//! - Tool version managers (`mise.toml`, `.mise.toml`, `.tool-versions`)
//! - Present package managers / build systems (`Cargo.toml`, `pnpm-lock.yaml`, `yarn.lock`,
//!   `bun.lock*`, `package.json`, `uv.lock`, `poetry.lock`, `pyproject.toml`, `go.mod`, etc.)
//!
//! Runs off the UI thread on the `twitty-store` worker thread; never spawns subprocesses.

use std::fs;
use std::path::Path;

use twitty_core::redact::redact;

use crate::rules::read_bounded_project_file;

/// Source tag stored on auto-detected environment memory rows so refreshes update them cleanly
/// without overwriting user-authored environment notes.
pub const AUTO_ENV_SOURCE: &str = "auto:env";

/// Maximum size read from any single environment config file (16 KiB).
const MAX_ENV_FILE_BYTES: usize = 16 * 1024;

/// Maximum number of tool entries listed per config file.
const MAX_TOOLS: usize = 16;

/// One detected environment note ready to store as `MemoryKind::Env`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvNote {
    pub text: String,
    pub source: String,
}

/// Detect environment notes for `project_root` without spawning any processes.
///
/// Output order is deterministic: Rust toolchain, mise tools, `.tool-versions`, package managers.
pub fn detect_project_env(project_root: &Path) -> Vec<EnvNote> {
    let Ok(canon_root) = fs::canonicalize(project_root) else {
        return Vec::new();
    };
    let mut notes = Vec::new();

    if let Some(rust_note) = detect_rust_toolchain(&canon_root) {
        notes.push(EnvNote {
            text: redact(&rust_note).into_owned(),
            source: AUTO_ENV_SOURCE.to_owned(),
        });
    }

    if let Some(mise_note) = detect_mise_tools(&canon_root) {
        notes.push(EnvNote {
            text: redact(&mise_note).into_owned(),
            source: AUTO_ENV_SOURCE.to_owned(),
        });
    }

    if let Some(tv_note) = detect_tool_versions(&canon_root) {
        notes.push(EnvNote {
            text: redact(&tv_note).into_owned(),
            source: AUTO_ENV_SOURCE.to_owned(),
        });
    }

    if let Some(pm_note) = detect_package_managers(&canon_root) {
        notes.push(EnvNote {
            text: pm_note,
            source: AUTO_ENV_SOURCE.to_owned(),
        });
    }

    notes
}

fn read_text(canon_root: &Path, rel: &str) -> Option<String> {
    let bytes = read_bounded_project_file(canon_root, rel, MAX_ENV_FILE_BYTES)?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn detect_rust_toolchain(canon_root: &Path) -> Option<String> {
    if let Some(toml) = read_text(canon_root, "rust-toolchain.toml") {
        let mut channel = None;
        let mut profile = None;
        let mut targets = Vec::new();
        let mut components = Vec::new();
        let mut in_toolchain = true; // top-level or [toolchain]

        for raw_line in toml.lines() {
            let line = strip_comment(raw_line).trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                let section = line[1..line.len() - 1].trim();
                in_toolchain = section == "toolchain";
                continue;
            }
            if !in_toolchain {
                continue;
            }
            let Some((key, val)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let val = val.trim();
            match key {
                "channel" => {
                    if let Some(s) = parse_toml_string(val) {
                        channel = Some(s);
                    }
                }
                "profile" => {
                    if let Some(s) = parse_toml_string(val) {
                        profile = Some(s);
                    }
                }
                "targets" => {
                    targets = parse_toml_string_array(val);
                }
                "components" => {
                    components = parse_toml_string_array(val);
                }
                _ => {}
            }
        }
        if let Some(ch) = channel {
            let mut parts = vec![format!("Rust toolchain: {ch}")];
            if let Some(p) = profile {
                parts.push(format!("profile={p}"));
            }
            if !targets.is_empty() {
                parts.push(format!("targets={}", targets.join(", ")));
            }
            if !components.is_empty() {
                parts.push(format!("components={}", components.join(", ")));
            }
            return Some(parts.join(" · "));
        }
    }

    if let Some(plain) = read_text(canon_root, "rust-toolchain") {
        for raw_line in plain.lines() {
            let line = strip_comment(raw_line).trim();
            if !line.is_empty() {
                // Handle legacy TOML inside `rust-toolchain` without `.toml` extension.
                if let Some((_, val)) = line.split_once('=') {
                    if line.trim_start().starts_with("channel") {
                        if let Some(ch) = parse_toml_string(val.trim()) {
                            return Some(format!("Rust toolchain: {ch}"));
                        }
                    }
                    continue;
                }
                if !line.starts_with('[') {
                    return Some(format!("Rust toolchain: {line}"));
                }
            }
        }
    }
    None
}

fn detect_mise_tools(canon_root: &Path) -> Option<String> {
    let content =
        read_text(canon_root, "mise.toml").or_else(|| read_text(canon_root, ".mise.toml"))?;
    let mut in_tools = false;
    let mut entries = Vec::new();

    for raw_line in content.lines() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let section = line[1..line.len() - 1].trim();
            in_tools = section == "tools";
            continue;
        }
        if !in_tools {
            continue;
        }
        if entries.len() >= MAX_TOOLS {
            break;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim().trim_matches('"').trim_matches('\'');
        if key.is_empty() {
            continue;
        }
        let val = v.trim();
        let parsed = if let Some(s) = parse_toml_string(val) {
            s
        } else if val.starts_with('[') {
            let arr = parse_toml_string_array(val);
            if arr.is_empty() {
                continue;
            }
            arr.join("/")
        } else if val.starts_with('{') {
            // Inline table e.g. `{ version = "20" }`: extract `version` if present.
            extract_inline_version(val).unwrap_or_default()
        } else {
            val.to_owned()
        };
        if !parsed.is_empty() {
            entries.push(format!("{key} {parsed}"));
        }
    }

    if entries.is_empty() {
        None
    } else {
        Some(format!("mise tools: {}", entries.join(", ")))
    }
}

fn detect_tool_versions(canon_root: &Path) -> Option<String> {
    let content = read_text(canon_root, ".tool-versions")?;
    let mut entries = Vec::new();
    for raw_line in content.lines() {
        if entries.len() >= MAX_TOOLS {
            break;
        }
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(tool) = parts.next() else {
            continue;
        };
        let versions: Vec<&str> = parts.collect();
        if versions.is_empty() {
            continue;
        }
        entries.push(format!("{tool} {}", versions.join("/")));
    }
    if entries.is_empty() {
        None
    } else {
        Some(format!(".tool-versions: {}", entries.join(", ")))
    }
}

fn detect_package_managers(canon_root: &Path) -> Option<String> {
    let exists = |rel: &str| canon_root.join(rel).symlink_metadata().is_ok();

    let mut managers: Vec<&'static str> = Vec::new();
    if exists("Cargo.toml") {
        managers.push("cargo");
    }
    let has_pnpm = exists("pnpm-lock.yaml") || exists("pnpm-workspace.yaml");
    let has_yarn = exists("yarn.lock");
    let has_bun = exists("bun.lockb") || exists("bun.lock");
    let has_npm_lock = exists("package-lock.json") || exists("npm-shrinkwrap.json");
    if has_pnpm {
        managers.push("pnpm");
    }
    if has_yarn {
        managers.push("yarn");
    }
    if has_bun {
        managers.push("bun");
    }
    if has_npm_lock || (exists("package.json") && !has_pnpm && !has_yarn && !has_bun) {
        managers.push("npm");
    }

    let has_uv = exists("uv.lock");
    let has_poetry = exists("poetry.lock");
    if has_uv {
        managers.push("uv");
    }
    if has_poetry {
        managers.push("poetry");
    }
    if !has_uv && !has_poetry && (exists("pyproject.toml") || exists("requirements.txt")) {
        managers.push("pip");
    }
    if exists("go.mod") {
        managers.push("go");
    }
    if exists("Gemfile") {
        managers.push("bundler");
    }
    if exists("composer.json") {
        managers.push("composer");
    }
    if exists("flake.nix") {
        managers.push("nix");
    }
    if exists("justfile") || exists("Justfile") {
        managers.push("just");
    }
    if exists("Makefile") || exists("GNUmakefile") {
        managers.push("make");
    }
    if exists("CMakeLists.txt") {
        managers.push("cmake");
    }

    if managers.is_empty() {
        None
    } else {
        Some(format!("Package managers: {}", managers.join(", ")))
    }
}

fn strip_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    for (idx, ch) in line.char_indices() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => return &line[..idx],
            _ => {}
        }
    }
    line
}

fn parse_toml_string(val: &str) -> Option<String> {
    let trimmed = val.trim();
    if let Some(inner) = trimmed.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        let s = inner.trim();
        return (!s.is_empty()).then(|| s.to_owned());
    }
    if let Some(inner) = trimmed
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
    {
        let s = inner.trim();
        return (!s.is_empty()).then(|| s.to_owned());
    }
    None
}

fn parse_toml_string_array(val: &str) -> Vec<String> {
    let Some(inner) = val
        .trim()
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
    else {
        return Vec::new();
    };
    inner
        .split(',')
        .filter_map(parse_toml_string)
        .take(MAX_TOOLS)
        .collect()
}

fn extract_inline_version(val: &str) -> Option<String> {
    let inner = val
        .trim()
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))?;
    for part in inner.split(',') {
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        if k.trim() == "version" {
            return parse_toml_string(v);
        }
    }
    None
}
