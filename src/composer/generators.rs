//! Dynamic completion generators, bounded TTL cache, and `$PATH` executable scanner.
//!
//! All disk and subprocess generators are designed to run off the UI thread with strict timeouts,
//! bounded output, argv-only process spawning (never a shell), and output sanitization.

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::shell_integration::clean_text;

/// Maximum bytes read from a single config/manifest file (`package.json`, `Makefile`, `Cargo.toml`,
/// `~/.ssh/config`).
const MAX_FILE_BYTES: usize = 512 * 1024;
/// Maximum bytes read from a generator subprocess stdout.
const MAX_CMD_BYTES: usize = 256 * 1024;
/// Maximum items returned by a single generator run.
pub const MAX_GENERATED_ITEMS: usize = 256;
/// Maximum entries kept in [`GeneratorCache`].
const MAX_CACHE_ENTRIES: usize = 64;
/// Default cache TTL for cwd-sensitive generators (git, files, package.json, Cargo.toml, Makefile).
pub const CWD_CACHE_TTL: Duration = Duration::from_secs(8);
/// Default cache TTL for global generators (ssh config, docker, systemctl).
pub const GLOBAL_CACHE_TTL: Duration = Duration::from_secs(20);
/// Timeout for git subprocess generators.
const GIT_TIMEOUT: Duration = Duration::from_millis(750);
/// Short timeout for docker / systemctl so a stopped daemon never stalls completion.
const FAST_CMD_TIMEOUT: Duration = Duration::from_millis(500);

/// Kind of dynamic completion generator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GeneratorKind {
    GitBranches,
    GitRemotes,
    GitTags,
    GitChangedFiles,
    NpmScripts,
    MakeTargets,
    CargoTargets,
    CargoFeatures,
    DockerContainers,
    DockerImages,
    SystemctlUnits { user: bool },
    SshHosts,
}

impl GeneratorKind {
    /// Whether this generator's results depend on the pane's working directory.
    pub fn is_cwd_scoped(self) -> bool {
        matches!(
            self,
            Self::GitBranches
                | Self::GitRemotes
                | Self::GitTags
                | Self::GitChangedFiles
                | Self::NpmScripts
                | Self::MakeTargets
                | Self::CargoTargets
                | Self::CargoFeatures
        )
    }

    pub fn ttl(self) -> Duration {
        if self.is_cwd_scoped() {
            CWD_CACHE_TTL
        } else {
            GLOBAL_CACHE_TTL
        }
    }
}

/// Cache key identifying a generator run.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GeneratorKey {
    pub kind: GeneratorKind,
    pub cwd: Option<PathBuf>,
}

impl GeneratorKey {
    pub fn new(kind: GeneratorKind, cwd: Option<&Path>) -> Self {
        let cwd = if kind.is_cwd_scoped() {
            cwd.map(Path::to_path_buf)
        } else {
            None
        };
        Self { kind, cwd }
    }
}

/// One candidate produced by a dynamic generator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedItem {
    pub value: String,
    pub desc: Option<String>,
}

impl GeneratedItem {
    pub fn new(value: impl Into<String>, desc: Option<String>) -> Self {
        Self {
            value: value.into(),
            desc,
        }
    }
}

/// Bounded in-memory cache for dynamic generator results with per-entry TTL.
#[derive(Clone, Debug, Default)]
pub struct GeneratorCache {
    entries: HashMap<GeneratorKey, (Instant, Vec<GeneratedItem>)>,
}

impl GeneratorCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return cached items if present and not expired at `now`.
    pub fn get(&self, key: &GeneratorKey, now: Instant) -> Option<&[GeneratedItem]> {
        let (ts, items) = self.entries.get(key)?;
        if now.saturating_duration_since(*ts) <= key.kind.ttl() {
            Some(items.as_slice())
        } else {
            None
        }
    }

    /// Insert fresh generator results at `now`, evicting the oldest entry if over capacity.
    pub fn insert(&mut self, key: GeneratorKey, items: Vec<GeneratedItem>, now: Instant) {
        if self.entries.len() >= MAX_CACHE_ENTRIES && !self.entries.contains_key(&key) {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (ts, _))| *ts)
                .map(|(k, _)| k.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(key, (now, items));
    }

    /// Collect all fresh cached entries relevant to `cwd` as a map keyed by [`GeneratorKind`].
    pub fn snapshot_for(
        &self,
        cwd: Option<&Path>,
        now: Instant,
    ) -> HashMap<GeneratorKind, Vec<GeneratedItem>> {
        let mut out = HashMap::new();
        for (key, (ts, items)) in &self.entries {
            if now.saturating_duration_since(*ts) > key.kind.ttl() {
                continue;
            }
            let matches = match (&key.cwd, cwd) {
                (None, _) => true,
                (Some(k_cwd), Some(c)) => k_cwd == c,
                (Some(_), None) => false,
            };
            if matches {
                out.insert(key.kind, items.clone());
            }
        }
        out
    }

    /// Drop cached entries for `cwd` (e.g. after a command finishes).
    pub fn invalidate_cwd(&mut self, cwd: &Path) {
        self.entries.retain(|k, _| k.cwd.as_deref() != Some(cwd));
    }

    /// Drop all cwd-scoped entries.
    pub fn invalidate_all_cwd(&mut self) {
        self.entries.retain(|k, _| !k.kind.is_cwd_scoped());
    }
}

/// Execute a dynamic generator synchronously (call off the UI thread!).
pub fn run_generator(kind: GeneratorKind, cwd: Option<&Path>) -> Vec<GeneratedItem> {
    let mut items = match kind {
        GeneratorKind::GitBranches => cwd.map(git_branches).unwrap_or_default(),
        GeneratorKind::GitRemotes => cwd.map(git_remotes).unwrap_or_default(),
        GeneratorKind::GitTags => cwd.map(git_tags).unwrap_or_default(),
        GeneratorKind::GitChangedFiles => cwd.map(git_changed_files).unwrap_or_default(),
        GeneratorKind::NpmScripts => cwd.map(npm_scripts_from_dir).unwrap_or_default(),
        GeneratorKind::MakeTargets => cwd.map(make_targets_from_dir).unwrap_or_default(),
        GeneratorKind::CargoTargets => cwd.map(cargo_targets_from_dir).unwrap_or_default(),
        GeneratorKind::CargoFeatures => cwd.map(cargo_features_from_dir).unwrap_or_default(),
        GeneratorKind::DockerContainers => docker_containers(),
        GeneratorKind::DockerImages => docker_images(),
        GeneratorKind::SystemctlUnits { user } => systemctl_units(user),
        GeneratorKind::SshHosts => ssh_hosts_from_default(),
    };
    items.truncate(MAX_GENERATED_ITEMS);
    items
}

// ---- Bounded file & subprocess helpers ----

fn read_file_bounded(path: &Path, limit: usize) -> Option<String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.file_type().is_file() {
        return None;
    }
    let mut buf = Vec::new();
    file.take(limit as u64).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Check if `program` exists as an executable file on `$PATH`.
pub fn find_in_path(program: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(program);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// Scan `$PATH` directories for executable names (call off the UI thread).
pub fn scan_path_executables() -> HashSet<String> {
    let Some(path_var) = std::env::var_os("PATH") else {
        return HashSet::new();
    };
    scan_path_dirs(&std::env::split_paths(&path_var).collect::<Vec<_>>())
}

/// Scan the given directories for executable file names (bounded to avoid huge slow mounts).
pub fn scan_path_dirs(dirs: &[PathBuf]) -> HashSet<String> {
    const MAX_PER_DIR: usize = 4_096;
    const MAX_TOTAL: usize = 16_384;
    let mut names = HashSet::new();
    for dir in dirs {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in rd.flatten().take(MAX_PER_DIR) {
            let path = entry.path();
            if !is_executable_file(&path) {
                continue;
            }
            if let Some(name) = entry.file_name().to_str() {
                if !name.is_empty() && !name.starts_with('.') {
                    names.insert(name.to_owned());
                    if names.len() >= MAX_TOTAL {
                        return names;
                    }
                }
            }
        }
    }
    names
}

/// Run a subprocess with argv, a strict wall-clock timeout, and a bounded stdout read.
fn run_command_bounded(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    envs: &[(&str, &str)],
    timeout: Duration,
    limit: usize,
) -> Option<Vec<u8>> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    for &(k, v) in envs {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn().ok()?;
    let stdout = child.stdout.take()?;

    struct KillOnDrop(Option<std::process::Child>);
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            if let Some(mut c) = self.0.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }

    let stop = Arc::new(AtomicBool::new(false));
    let mut guard = KillOnDrop(Some(child));
    let watchdog = {
        let stop = stop.clone();
        thread::Builder::new()
            .name("twitty-gen-watch".into())
            .spawn(move || {
                let start = Instant::now();
                loop {
                    let child = guard.0.as_mut()?;
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            let _ = guard.0.take();
                            return Some(status);
                        }
                        Ok(None) => {}
                        Err(_) => return None,
                    }
                    if stop.load(Ordering::Relaxed) || start.elapsed() > timeout {
                        return None;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            })
            .ok()?
    };

    let mut out = Vec::new();
    let _ = stdout.take(limit as u64 + 1).read_to_end(&mut out);
    if out.len() > limit {
        out.truncate(limit);
        stop.store(true, Ordering::Relaxed);
    }
    let status = watchdog.join().ok().flatten()?;
    status.success().then_some(out)
}

fn run_git(cwd: &Path, args: &[&str]) -> Option<String> {
    let mut full = Vec::with_capacity(args.len() + 4);
    full.extend_from_slice(&["-c", "core.quotepath=false", "-c", "core.fsmonitor=false"]);
    full.extend_from_slice(args);
    let bytes = run_command_bounded(
        "git",
        &full,
        Some(cwd),
        &[
            ("GIT_OPTIONAL_LOCKS", "0"),
            ("GIT_TERMINAL_PROMPT", "0"),
            ("LC_ALL", "C"),
        ],
        GIT_TIMEOUT,
        MAX_CMD_BYTES,
    )?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

// ---- Git generators ----

/// List local and remote branches for the git repository at `cwd`.
pub fn git_branches(cwd: &Path) -> Vec<GeneratedItem> {
    let Some(out) = run_git(
        cwd,
        &[
            "for-each-ref",
            "--format=%(refname:short)\t%(HEAD)",
            "refs/heads",
            "refs/remotes",
        ],
    ) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    for line in out.lines() {
        let (raw_name, head) = line.split_once('\t').unwrap_or((line, ""));
        let name = clean_text(raw_name.trim(), 160);
        if name.is_empty() || name.ends_with("/HEAD") || !seen.insert(name.clone()) {
            continue;
        }
        let desc = if head.trim() == "*" {
            Some("current branch".into())
        } else {
            None
        };
        items.push(GeneratedItem::new(name, desc));
        if items.len() >= MAX_GENERATED_ITEMS {
            break;
        }
    }
    items
}

/// List configured git remotes at `cwd`.
pub fn git_remotes(cwd: &Path) -> Vec<GeneratedItem> {
    let Some(out) = run_git(cwd, &["remote"]) else {
        return Vec::new();
    };
    out.lines()
        .map(|l| clean_text(l.trim(), 120))
        .filter(|s| !s.is_empty())
        .take(MAX_GENERATED_ITEMS)
        .map(|name| GeneratedItem::new(name, Some("remote".into())))
        .collect()
}

/// List git tags at `cwd` (newest first by version/creation).
pub fn git_tags(cwd: &Path) -> Vec<GeneratedItem> {
    let Some(out) = run_git(cwd, &["tag", "--sort=-creatordate"]) else {
        return Vec::new();
    };
    out.lines()
        .map(|l| clean_text(l.trim(), 120))
        .filter(|s| !s.is_empty())
        .take(MAX_GENERATED_ITEMS)
        .map(|name| GeneratedItem::new(name, Some("tag".into())))
        .collect()
}

/// List modified, staged, and untracked files in the git worktree at `cwd`.
pub fn git_changed_files(cwd: &Path) -> Vec<GeneratedItem> {
    let Some(out) = run_git(cwd, &["status", "--porcelain=v1", "-uall"]) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    for line in out.lines() {
        if line.len() < 4 {
            continue;
        }
        let status = line[..2].trim();
        let raw_path = line[3..].trim();
        // Handle renames (`old -> new`) by taking the destination path.
        let path = raw_path
            .rsplit_once(" -> ")
            .map_or(raw_path, |(_, new)| new);
        let cleaned = clean_text(path, 256);
        if cleaned.is_empty() || !seen.insert(cleaned.clone()) {
            continue;
        }
        let desc = match status {
            "??" => Some("untracked".into()),
            "M" | "MM" | "AM" => Some("modified".into()),
            "A" => Some("staged".into()),
            "D" => Some("deleted".into()),
            "R" => Some("renamed".into()),
            _ if !status.is_empty() => Some(status.to_owned()),
            _ => None,
        };
        items.push(GeneratedItem::new(cleaned, desc));
        if items.len() >= MAX_GENERATED_ITEMS {
            break;
        }
    }
    items
}

// ---- package.json scripts ----

/// Parse npm/pnpm/yarn script names and commands from `package.json` content.
pub fn parse_package_json_scripts(json: &str) -> Vec<GeneratedItem> {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(scripts) = val.get("scripts").and_then(|s| s.as_object()) else {
        return Vec::new();
    };
    let mut items: Vec<GeneratedItem> = scripts
        .iter()
        .filter_map(|(name, cmd)| {
            let clean_name = clean_text(name.trim(), 120);
            if clean_name.is_empty() {
                return None;
            }
            let desc = cmd
                .as_str()
                .map(|c| clean_text(c.trim(), 120))
                .filter(|s| !s.is_empty());
            Some(GeneratedItem::new(clean_name, desc))
        })
        .take(MAX_GENERATED_ITEMS)
        .collect();
    items.sort_by(|a, b| a.value.cmp(&b.value));
    items
}

/// Read `<cwd>/package.json` and return its `"scripts"` entries.
pub fn npm_scripts_from_dir(cwd: &Path) -> Vec<GeneratedItem> {
    let Some(text) = read_file_bounded(&cwd.join("package.json"), MAX_FILE_BYTES) else {
        return Vec::new();
    };
    parse_package_json_scripts(&text)
}

// ---- Makefile targets ----

/// Parse target names (and optional `## description` comments) from Makefile text.
pub fn parse_makefile_targets(text: &str) -> Vec<GeneratedItem> {
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    for line in text.lines() {
        // Recipes and indented lines start with tab/space; comments start with '#'.
        if line.is_empty()
            || line.starts_with('\t')
            || line.starts_with(' ')
            || line.starts_with('#')
        {
            continue;
        }
        let Some((lhs, rhs)) = line.split_once(':') else {
            continue;
        };
        // Skip variable assignments (`:=`, `::=`, `+=`, `?=`) and pattern rules (`%`).
        if rhs.starts_with('=') || lhs.contains('=') || lhs.contains('%') || lhs.contains('$') {
            continue;
        }
        let comment = rhs
            .split_once("##")
            .map(|(_, c)| clean_text(c.trim(), 120))
            .filter(|s| !s.is_empty());
        for raw_target in lhs.split_whitespace() {
            if raw_target.starts_with('.') {
                continue;
            }
            if !raw_target
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))
            {
                continue;
            }
            let name = clean_text(raw_target, 120);
            if !name.is_empty() && seen.insert(name.clone()) {
                items.push(GeneratedItem::new(name, comment.clone()));
                if items.len() >= MAX_GENERATED_ITEMS {
                    return items;
                }
            }
        }
    }
    items
}

/// Locate a Makefile in `cwd` (`GNUmakefile`, `makefile`, or `Makefile`) and parse its targets.
pub fn make_targets_from_dir(cwd: &Path) -> Vec<GeneratedItem> {
    for candidate in ["GNUmakefile", "makefile", "Makefile"] {
        let path = cwd.join(candidate);
        if let Some(text) = read_file_bounded(&path, MAX_FILE_BYTES) {
            return parse_makefile_targets(&text);
        }
    }
    Vec::new()
}

// ---- Cargo.toml targets and features ----

/// Simple TOML parser extracting Cargo targets (`[package] name`, `[[bin]]`, `[[example]]`,
/// `[[test]]`, `[[bench]]`) and `[features]` without requiring an external TOML crate.
pub fn parse_cargo_toml(text: &str) -> (Vec<GeneratedItem>, Vec<GeneratedItem>) {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Section {
        Other,
        Package,
        Bin,
        Example,
        Test,
        Bench,
        Features,
    }

    let mut section = Section::Other;
    let mut targets = Vec::new();
    let mut features = Vec::new();
    let mut seen_targets = HashSet::new();
    let mut seen_features = HashSet::new();

    for raw_line in text.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            let header = line.trim_start_matches('[').trim_end_matches(']').trim();
            section = match header {
                "package" => Section::Package,
                "bin" => Section::Bin,
                "example" => Section::Example,
                "test" => Section::Test,
                "bench" => Section::Bench,
                "features" => Section::Features,
                _ => Section::Other,
            };
            continue;
        }
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_matches('"').trim_matches('\'');
        let val = val.trim();
        match section {
            Section::Package | Section::Bin | Section::Example | Section::Test | Section::Bench
                if key == "name" =>
            {
                if let Some(name) = unquote_toml_string(val) {
                    let clean = clean_text(&name, 120);
                    if !clean.is_empty() && seen_targets.insert(clean.clone()) {
                        let kind_label = match section {
                            Section::Package => "package",
                            Section::Bin => "bin",
                            Section::Example => "example",
                            Section::Test => "test",
                            Section::Bench => "bench",
                            _ => "target",
                        };
                        targets.push(GeneratedItem::new(clean, Some(kind_label.into())));
                    }
                }
            }
            Section::Features if !key.is_empty() => {
                let clean = clean_text(key, 120);
                if !clean.is_empty() && seen_features.insert(clean.clone()) {
                    let desc = clean_text(val, 100);
                    features.push(GeneratedItem::new(
                        clean,
                        (!desc.is_empty()).then_some(desc),
                    ));
                }
            }
            _ => {}
        }
    }
    (targets, features)
}

fn strip_toml_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' && in_double {
            escaped = true;
            continue;
        }
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => return &line[..i],
            _ => {}
        }
    }
    line
}

fn unquote_toml_string(val: &str) -> Option<String> {
    if let Some(rest) = val.strip_prefix('"') {
        let end = rest.find('"')?;
        Some(rest[..end].to_owned())
    } else if let Some(rest) = val.strip_prefix('\'') {
        let end = rest.find('\'')?;
        Some(rest[..end].to_owned())
    } else {
        None
    }
}

/// Read Cargo targets from `<cwd>/Cargo.toml` plus conventional directories (`src/bin`,
/// `examples`, `tests`, `benches`).
pub fn cargo_targets_from_dir(cwd: &Path) -> Vec<GeneratedItem> {
    let Some(text) = read_file_bounded(&cwd.join("Cargo.toml"), MAX_FILE_BYTES) else {
        return Vec::new();
    };
    let (mut targets, _) = parse_cargo_toml(&text);
    let mut seen: HashSet<String> = targets.iter().map(|t| t.value.clone()).collect();
    for (subdir, label) in [
        ("src/bin", "bin"),
        ("examples", "example"),
        ("tests", "test"),
        ("benches", "bench"),
    ] {
        let Ok(rd) = std::fs::read_dir(cwd.join(subdir)) else {
            continue;
        };
        for entry in rd.flatten().take(128) {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let clean = clean_text(stem, 120);
            if !clean.is_empty() && seen.insert(clean.clone()) {
                targets.push(GeneratedItem::new(clean, Some(label.into())));
            }
        }
    }
    targets.truncate(MAX_GENERATED_ITEMS);
    targets
}

/// Read Cargo features from `<cwd>/Cargo.toml`.
pub fn cargo_features_from_dir(cwd: &Path) -> Vec<GeneratedItem> {
    let Some(text) = read_file_bounded(&cwd.join("Cargo.toml"), MAX_FILE_BYTES) else {
        return Vec::new();
    };
    let (_, mut features) = parse_cargo_toml(&text);
    features.truncate(MAX_GENERATED_ITEMS);
    features
}

// ---- Docker containers & images ----

/// List Docker containers (only if `docker` is on `$PATH` and responds within 500 ms).
pub fn docker_containers() -> Vec<GeneratedItem> {
    if find_in_path("docker").is_none() {
        return Vec::new();
    }
    let Some(bytes) = run_command_bounded(
        "docker",
        &["ps", "-a", "--format", "{{.Names}}\t{{.Status}}"],
        None,
        &[],
        FAST_CMD_TIMEOUT,
        MAX_CMD_BYTES,
    ) else {
        return Vec::new();
    };
    parse_tab_items(&String::from_utf8_lossy(&bytes))
}

/// List local Docker images (only if `docker` is on `$PATH` and responds within 500 ms).
pub fn docker_images() -> Vec<GeneratedItem> {
    if find_in_path("docker").is_none() {
        return Vec::new();
    }
    let Some(bytes) = run_command_bounded(
        "docker",
        &["images", "--format", "{{.Repository}}:{{.Tag}}\t{{.Size}}"],
        None,
        &[],
        FAST_CMD_TIMEOUT,
        MAX_CMD_BYTES,
    ) else {
        return Vec::new();
    };
    parse_tab_items(&String::from_utf8_lossy(&bytes))
        .into_iter()
        .filter(|it| !it.value.starts_with("<none>"))
        .collect()
}

fn parse_tab_items(text: &str) -> Vec<GeneratedItem> {
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    for line in text.lines() {
        let (val, desc) = line.split_once('\t').unwrap_or((line, ""));
        let val = clean_text(val.trim(), 160);
        if val.is_empty() || !seen.insert(val.clone()) {
            continue;
        }
        let desc = clean_text(desc.trim(), 120);
        items.push(GeneratedItem::new(val, (!desc.is_empty()).then_some(desc)));
        if items.len() >= MAX_GENERATED_ITEMS {
            break;
        }
    }
    items
}

// ---- Systemctl units ----

/// List systemd unit files (only if `systemctl` is on `$PATH` and responds within 500 ms).
pub fn systemctl_units(user: bool) -> Vec<GeneratedItem> {
    if find_in_path("systemctl").is_none() {
        return Vec::new();
    }
    let args: &[&str] = if user {
        &["--user", "list-unit-files", "--no-legend", "--no-pager"]
    } else {
        &["list-unit-files", "--no-legend", "--no-pager"]
    };
    let Some(bytes) = run_command_bounded(
        "systemctl",
        args,
        None,
        &[("LC_ALL", "C")],
        FAST_CMD_TIMEOUT,
        MAX_CMD_BYTES,
    ) else {
        return Vec::new();
    };
    parse_systemctl_unit_files(&String::from_utf8_lossy(&bytes))
}

/// Parse `systemctl list-unit-files --no-legend` output into unit names and states.
pub fn parse_systemctl_unit_files(text: &str) -> Vec<GeneratedItem> {
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(unit) = parts.next() else {
            continue;
        };
        let state = parts.next();
        let unit = clean_text(unit, 160);
        if unit.is_empty() || !seen.insert(unit.clone()) {
            continue;
        }
        let desc = state.map(|s| clean_text(s, 64)).filter(|s| !s.is_empty());
        items.push(GeneratedItem::new(unit, desc));
        if items.len() >= MAX_GENERATED_ITEMS {
            break;
        }
    }
    items
}

// ---- SSH config hosts ----

/// Parse concrete `Host` aliases (ignoring `*`, `?`, `!` patterns) from SSH config text,
/// capturing optional `User` and `HostName` as short descriptions.
pub fn parse_ssh_config(text: &str) -> Vec<GeneratedItem> {
    struct Block {
        hosts: Vec<String>,
        hostname: Option<String>,
        user: Option<String>,
    }

    let mut blocks: Vec<Block> = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        // SSH config allows whitespace or `=` between keyword and arguments.
        let (kw, rest) = split_ssh_directive(line);
        if kw.eq_ignore_ascii_case("host") {
            let hosts: Vec<String> = rest
                .split_whitespace()
                .filter(|h| !h.contains('*') && !h.contains('?') && !h.starts_with('!'))
                .map(|h| clean_text(h, 120))
                .filter(|h| !h.is_empty())
                .collect();
            blocks.push(Block {
                hosts,
                hostname: None,
                user: None,
            });
        } else if let Some(cur) = blocks.last_mut() {
            if kw.eq_ignore_ascii_case("hostname") {
                let val = clean_text(rest.trim(), 120);
                if !val.is_empty() {
                    cur.hostname = Some(val);
                }
            } else if kw.eq_ignore_ascii_case("user") {
                let val = clean_text(rest.trim(), 80);
                if !val.is_empty() {
                    cur.user = Some(val);
                }
            }
        }
    }

    let mut seen = HashSet::new();
    let mut items = Vec::new();
    for block in blocks {
        let desc = match (block.user, block.hostname) {
            (Some(u), Some(h)) => Some(format!("{u}@{h}")),
            (None, Some(h)) => Some(h),
            (Some(u), None) => Some(u),
            (None, None) => None,
        };
        for host in block.hosts {
            if seen.insert(host.clone()) {
                items.push(GeneratedItem::new(host, desc.clone()));
                if items.len() >= MAX_GENERATED_ITEMS {
                    return items;
                }
            }
        }
    }
    items
}

fn split_ssh_directive(line: &str) -> (&str, &str) {
    for (i, c) in line.char_indices() {
        if c.is_whitespace() || c == '=' {
            let rest = line[i + c.len_utf8()..]
                .trim_start_matches(|ch: char| ch.is_whitespace() || ch == '=');
            return (&line[..i], rest);
        }
    }
    (line, "")
}

/// Parse SSH hosts from a specific config file path.
pub fn ssh_hosts_from_file(path: &Path) -> Vec<GeneratedItem> {
    let Some(text) = read_file_bounded(path, MAX_FILE_BYTES) else {
        return Vec::new();
    };
    parse_ssh_config(&text)
}

/// Parse SSH hosts from `~/.ssh/config`.
pub fn ssh_hosts_from_default() -> Vec<GeneratedItem> {
    let Some(home) = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
    else {
        return Vec::new();
    };
    ssh_hosts_from_file(&home.join(".ssh").join("config"))
}
