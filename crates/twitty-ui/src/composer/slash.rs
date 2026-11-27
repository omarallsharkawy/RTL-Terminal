//! Slash suggestions for the composer (Warp-style `/` menu).
//!
//! Typing `/` at the start of an empty composer lists what is installed on this machine and in
//! this project: Twitty commands and saved workflows, plus the coding agents' slash commands,
//! skills, custom commands and MCP servers (Claude Code, Codex, Gemini CLI).
//!
//! What a pick does depends on what runs in the pane:
//! - At a shell prompt, Twitty commands run directly, and a Claude skill or command is inserted
//!   as `claude "/name"` for review (Enter starts Claude Code with it).
//! - Over a running agent TUI (composer opened with `Ctrl+Shift+Space`), that agent's items are
//!   inserted as `/name ` for the agent to handle.
//!
//! Discovery reads small files only (frontmatter of `SKILL.md` and command files, MCP config
//! files), bounded in count and size, and runs off the UI thread.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::composer::complete::{CompletionItem, CompletionKind, MAX_COMPLETIONS};
use crate::ui::palette::fold;
use crate::workflows::Workflow;
use twitty_core::en;
use twitty_core::language::{fill, UiLanguage};

/// Bytes read from the top of a skill/command file to find its frontmatter.
const MAX_HEADER_BYTES: usize = 8 * 1024;
/// Upper bound on files read per directory, so a huge folder cannot stall discovery.
const MAX_FILES_PER_DIR: usize = 256;
/// Descriptions are cut to this many characters in the popup.
const MAX_DESCRIPTION_CHARS: usize = 120;

/// A coding-agent CLI whose slash commands Twitty knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Agent {
    Claude,
    Codex,
    Gemini,
    /// Google Antigravity's CLI (`agy`).
    Antigravity,
}

impl Agent {
    pub const ALL: [Agent; 4] = [
        Agent::Claude,
        Agent::Codex,
        Agent::Gemini,
        Agent::Antigravity,
    ];

    pub fn program(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Gemini => "gemini",
            Agent::Antigravity => "agy",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude",
            Agent::Codex => "Codex",
            Agent::Gemini => "Gemini",
            Agent::Antigravity => "Antigravity",
        }
    }

    /// Recognize an agent from a process command line (`argv[0]`, or `argv[1]` when the agent
    /// is a script run by `node`/`bun`/`deno`/`python`).
    pub fn from_argv(argv: &[String]) -> Option<Agent> {
        let base = |s: &str| {
            Path::new(s)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(s)
                .to_owned()
        };
        let first = argv.first().map(|s| base(s))?;
        let name = if matches!(
            first.as_str(),
            "node" | "bun" | "deno" | "python" | "python3"
        ) {
            argv.get(1).map(|s| base(s))?
        } else {
            first
        };
        let name = name.strip_suffix(".js").unwrap_or(&name);
        Agent::ALL.into_iter().find(|a| a.program() == name)
    }
}

/// What runs in the pane, as far as the slash menu cares.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Foreground {
    /// The shell prompt (nothing else in the foreground).
    #[default]
    Shell,
    /// A coding agent whose slash commands Twitty knows.
    Agent(Agent),
    /// Some other program (an editor, a pager, an agent Twitty does not know): `/` is its
    /// business, so the slash menu stays closed and inserts nothing.
    Other,
}

impl Foreground {
    /// From the pane's foreground command line (`None`: the shell itself).
    pub fn from_argv(argv: Option<&[String]>) -> Self {
        match argv {
            None => Foreground::Shell,
            Some(argv) => Agent::from_argv(argv).map_or(Foreground::Other, Foreground::Agent),
        }
    }

    pub fn agent(self) -> Option<Agent> {
        match self {
            Foreground::Agent(agent) => Some(agent),
            Foreground::Shell | Foreground::Other => None,
        }
    }
}

/// Where a slash item comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlashSource {
    /// A built-in slash command of the agent itself (`/compact`, `/model`, ...).
    BuiltIn(Agent),
    /// A skill (`SKILL.md`), user-level, project-level or from an enabled plugin.
    Skill(Agent),
    /// A custom command or prompt file.
    Command(Agent),
    /// A configured MCP server (picking it opens the agent's `/mcp` view).
    Mcp(Agent),
}

impl SlashSource {
    pub fn agent(&self) -> Agent {
        match self {
            SlashSource::BuiltIn(a)
            | SlashSource::Skill(a)
            | SlashSource::Command(a)
            | SlashSource::Mcp(a) => *a,
        }
    }
}

/// One discovered agent item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlashItem {
    /// Name without the leading `/`, e.g. `review`, `egm:review`, `prompts:fix`, or an MCP
    /// server name.
    pub name: String,
    pub source: SlashSource,
    pub description: String,
    /// Arabic description for built-ins; files only carry their own text.
    pub description_ar: Option<String>,
}

/// Everything found by [`discover`], plus the inputs it was built for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SlashCatalog {
    pub items: Vec<SlashItem>,
    pub project_root: Option<PathBuf>,
}

/// The composer token that opens the slash menu: the whole buffer is `/` plus an optional
/// query without spaces or a second `/` (`/usr/bin` stays a path), and the caret is at its end.
pub fn slash_query(text: &str, caret: usize) -> Option<&str> {
    let query = text.strip_prefix('/')?;
    if caret != text.len()
        || query
            .chars()
            .any(|c| c.is_whitespace() || c == '/' || c == '\'' || c == '"')
    {
        return None;
    }
    Some(query)
}

/// Rank `candidate` against the folded `query`: prefix < word prefix < substring < subsequence.
fn match_score(candidate: &str, query: &str) -> Option<usize> {
    if query.is_empty() {
        return Some(500);
    }
    let cand = fold(candidate);
    if cand.starts_with(query) {
        return Some(100 + cand.len().min(99));
    }
    if cand
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| w.starts_with(query))
    {
        return Some(200 + cand.len().min(99));
    }
    if cand.contains(query) {
        return Some(300 + cand.len().min(99));
    }
    // Scattered letters only count once the query is specific enough.
    if query.chars().count() < 3 {
        return None;
    }
    let mut chars = cand.chars();
    query
        .chars()
        .all(|q| chars.any(|c| c == q))
        .then(|| 400 + cand.len().min(99))
}

/// Score a match on the shown name, or else on `others` (titles, descriptions) ranked
/// `penalty` lower, so `/re` lists names starting with `re` before titles that do.
fn name_first<'a>(
    query: &str,
    name: &str,
    others: impl IntoIterator<Item = &'a str>,
    penalty: usize,
) -> Option<usize> {
    match_score(name, query).or_else(|| {
        others
            .into_iter()
            .filter_map(|f| match_score(f, query))
            .min()
            .map(|s| s + penalty)
    })
}

fn truncate(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= MAX_DESCRIPTION_CHARS {
        return one_line;
    }
    let mut out: String = one_line.chars().take(MAX_DESCRIPTION_CHARS - 1).collect();
    out.push('…');
    out
}

/// Inputs for [`suggestions`].
pub struct SlashInputs<'a> {
    pub catalog: &'a SlashCatalog,
    pub workflows: &'a [Workflow],
    /// What runs in the pane.
    pub foreground: Foreground,
}

/// Popup items for `/query` (the whole composer buffer, `text_len` bytes long).
pub fn suggestions(query: &str, text_len: usize, inputs: &SlashInputs<'_>) -> Vec<CompletionItem> {
    if inputs.foreground == Foreground::Other {
        return Vec::new();
    }
    let foreground = inputs.foreground.agent();
    let q = fold(query);
    let range = (0, text_len);
    let mut out: Vec<CompletionItem> = Vec::new();

    // Match quality decides first; among equal matches, the running agent's items come first
    // (that is what the user is talking to), then Twitty's, then agents started from the shell.
    // Match class (hundreds), then source band, then shorter names.
    let rank = |quality: usize, band: usize| (quality / 100) * 1_000 + band * 100 + quality % 100;
    let agent_band = if foreground.is_some() { 0 } else { 2 };
    let mut seen = HashSet::new();
    for item in &inputs.catalog.items {
        let agent = item.source.agent();
        let (replacement, label) = match (&item.source, foreground) {
            (SlashSource::Mcp(_), Some(fg)) if fg == agent => {
                ("/mcp".to_owned(), item.name.clone())
            }
            (_, Some(fg)) if fg == agent => (format!("/{} ", item.name), format!("/{}", item.name)),
            // At a shell prompt, Claude and Antigravity skills start a session with the command.
            (SlashSource::Skill(Agent::Claude) | SlashSource::Command(Agent::Claude), None) => (
                format!("claude \"/{}\"", item.name),
                format!("/{}", item.name),
            ),
            (SlashSource::Skill(Agent::Antigravity), None) => (
                format!("agy -i \"/{}\"", item.name),
                format!("/{}", item.name),
            ),
            (SlashSource::Mcp(a), None) => (
                match a {
                    Agent::Gemini | Agent::Antigravity => format!("{} mcp list", a.program()),
                    _ => format!("{} mcp get {}", a.program(), item.name),
                },
                item.name.clone(),
            ),
            _ => continue,
        };
        let Some(score) = name_first(&q, &item.name, [item.description.as_str()], 600) else {
            continue;
        };
        if !seen.insert((label.clone(), replacement.clone())) {
            continue;
        }
        let kind = match item.source {
            SlashSource::BuiltIn(_) | SlashSource::Command(_) => CompletionKind::AgentCommand,
            SlashSource::Skill(_) => CompletionKind::Skill,
            SlashSource::Mcp(_) => CompletionKind::Mcp,
        };
        let detail_for = |desc: &str| {
            if desc.is_empty() {
                agent.label().to_owned()
            } else {
                format!("{} · {}", agent.label(), truncate(desc))
            }
        };
        out.push(CompletionItem {
            kind,
            label,
            detail: Some(detail_for(&item.description)),
            detail_ar: item.description_ar.as_deref().map(detail_for),
            replacement,
            replace_range: range,
            // MCP servers are context, not commands: they go after commands of equal match.
            score: rank(
                score,
                if kind == CompletionKind::Mcp {
                    3
                } else {
                    agent_band
                },
            ),
        });
    }

    // Twitty's own commands and the user's saved workflows run directly.
    let twitty_band = 1;
    for command in crate::commands::all() {
        let Some(score) = name_first(&q, command.name, [command.title_en, command.title_ar], 200)
        else {
            continue;
        };
        let detail_for = |title: &str| match command.shortcut {
            Some(keys) => format!("Twitty · {title} · {keys}"),
            None => format!("Twitty · {title}"),
        };
        out.push(CompletionItem {
            kind: CompletionKind::Action,
            label: format!("/{}", command.name),
            detail: Some(detail_for(command.title_en)),
            detail_ar: Some(detail_for(command.title_ar)),
            replacement: command.name.to_owned(),
            replace_range: range,
            score: rank(score, twitty_band),
        });
    }
    for workflow in inputs.workflows.iter().filter(|w| w.trusted) {
        let spec = &workflow.spec;
        let Some(score) = name_first(
            &q,
            &spec.name,
            [spec.name_ar.as_str(), spec.command.as_str()],
            200,
        ) else {
            continue;
        };
        out.push(CompletionItem {
            kind: CompletionKind::Workflow,
            label: spec.name.clone(),
            detail: Some(spec.command.clone()),
            detail_ar: (!spec.name_ar.is_empty())
                .then(|| format!("{} · {}", spec.name_ar, spec.command)),
            replacement: spec.name.clone(),
            replace_range: range,
            score: rank(score, twitty_band),
        });
    }

    out.sort_by(|a, b| a.score.cmp(&b.score).then_with(|| a.label.cmp(&b.label)));
    out.truncate(MAX_COMPLETIONS);
    out
}

// ---- Discovery (off the UI thread) ----

/// Read the frontmatter `name`, `description` and `user-invocable` of a Markdown file.
fn read_frontmatter(path: &Path) -> (Option<String>, Option<String>, bool) {
    let Ok(bytes) = read_head(path) else {
        return (None, None, true);
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        // No frontmatter: the first non-empty line is the description (Claude commands).
        let first = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(|l| l.trim_start_matches('#').trim().to_owned());
        return (None, first, true);
    }
    let (mut name, mut description, mut invocable) = (None, None, true);
    let mut block: Option<String> = None;
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if let Some(acc) = block.as_mut() {
            if line.starts_with(' ') || line.starts_with('\t') || line.trim().is_empty() {
                acc.push(' ');
                acc.push_str(line.trim());
                continue;
            }
            description = block.take();
        }
        // Only top-level keys: nested maps (e.g. env vars) may have their own `description`.
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = unquote(value.trim());
        match key.trim() {
            "name" => name = Some(value),
            "description" if matches!(value.as_str(), "|" | ">" | "|-" | ">-") => {
                block = Some(String::new());
            }
            "description" => description = Some(value),
            "user-invocable" => invocable = value != "false",
            _ => {}
        }
    }
    if let Some(acc) = block {
        description = Some(acc);
    }
    (
        name.filter(|n| !n.is_empty()),
        description.map(|d| d.trim().to_owned()),
        invocable,
    )
}

fn unquote(value: &str) -> String {
    let v = value.trim();
    for q in ['"', '\''] {
        if let Some(inner) = v.strip_prefix(q).and_then(|s| s.strip_suffix(q)) {
            return inner.to_owned();
        }
    }
    v.to_owned()
}

fn read_head(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let mut buf = Vec::with_capacity(1024);
    file.take(MAX_HEADER_BYTES as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .take(MAX_FILES_PER_DIR)
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// `<dir>/<skill>/SKILL.md` skills, named `<prefix><skill>`.
fn skills_in(dir: &Path, agent: Agent, prefix: &str, out: &mut Vec<SlashItem>) {
    for skill_dir in sorted_entries(dir) {
        let file = skill_dir.join("SKILL.md");
        if !file.is_file() {
            continue;
        }
        let (name, description, invocable) = read_frontmatter(&file);
        if !invocable {
            continue;
        }
        let Some(dir_name) = skill_dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        out.push(SlashItem {
            name: format!("{prefix}{}", name.unwrap_or_else(|| dir_name.to_owned())),
            source: SlashSource::Skill(agent),
            description: description.unwrap_or_default(),
            description_ar: None,
        });
    }
}

/// Markdown command files under `dir` (one level of sub-folders), named by file stem.
fn md_commands_in(dir: &Path, agent: Agent, prefix: &str, out: &mut Vec<SlashItem>) {
    let mut files = Vec::new();
    for entry in sorted_entries(dir) {
        if entry.is_dir() {
            files.extend(
                sorted_entries(&entry)
                    .into_iter()
                    .filter(|p| p.extension().is_some_and(|e| e == "md")),
            );
        } else if entry.extension().is_some_and(|e| e == "md") {
            files.push(entry);
        }
    }
    for file in files.into_iter().take(MAX_FILES_PER_DIR) {
        let Some(stem) = file.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let (_, description, _) = read_frontmatter(&file);
        out.push(SlashItem {
            name: format!("{prefix}{stem}"),
            source: SlashSource::Command(agent),
            description: description.unwrap_or_default(),
            description_ar: None,
        });
    }
}

/// Gemini TOML commands: `<dir>/git/commit.toml` is `/git:commit`.
fn gemini_commands_in(dir: &Path, out: &mut Vec<SlashItem>) {
    fn walk(dir: &Path, ns: &str, depth: usize, out: &mut Vec<SlashItem>) {
        for entry in sorted_entries(dir) {
            let Some(stem) = entry.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let name = if ns.is_empty() {
                stem.to_owned()
            } else {
                format!("{ns}:{stem}")
            };
            if entry.is_dir() && depth < 3 {
                walk(&entry, &name, depth + 1, out);
            } else if entry.extension().is_some_and(|e| e == "toml") {
                let text = read_head(&entry)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default();
                let description = text
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once('=')?;
                        (k.trim() == "description").then(|| unquote(v))
                    })
                    .unwrap_or_default();
                out.push(SlashItem {
                    name,
                    source: SlashSource::Command(Agent::Gemini),
                    description,
                    description_ar: None,
                });
            }
        }
    }
    walk(dir, "", 0, out);
}

fn mcp_item(agent: Agent, name: &str, detail: String) -> SlashItem {
    SlashItem {
        name: name.to_owned(),
        source: SlashSource::Mcp(agent),
        description: format!("MCP · {detail}"),
        description_ar: Some(fill(UiLanguage::Arabic.t("MCP · {}"), &[&detail])),
    }
}

/// `{"mcpServers": {"name": {"command": ..} | {"url": ..}}}` in a JSON value.
fn mcp_from_json(agent: Agent, value: &serde_json::Value, out: &mut Vec<SlashItem>) {
    let Some(servers) = value.get("mcpServers").and_then(|v| v.as_object()) else {
        return;
    };
    for (name, server) in servers {
        let detail = server
            .get("command")
            .and_then(|c| c.as_str())
            .map(|c| {
                Path::new(c)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(c)
                    .to_owned()
            })
            .or_else(|| {
                server
                    .get("url")
                    .and_then(|u| u.as_str())
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        out.push(mcp_item(agent, name, detail));
    }
}

fn read_json(path: &Path, limit: u64) -> Option<serde_json::Value> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > limit {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Codex `config.toml`: top-level `[mcp_servers.<name>]` tables.
fn codex_mcp(path: &Path, out: &mut Vec<SlashItem>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for line in text.lines() {
        let Some(table) = line
            .trim()
            .strip_prefix("[mcp_servers.")
            .and_then(|s| s.strip_suffix(']'))
        else {
            continue;
        };
        let name = unquote(table);
        if !name.contains('.') {
            out.push(mcp_item(Agent::Codex, &name, String::new()));
        }
    }
}

/// Enabled Claude Code plugins: `(plugin name, install path)`.
fn claude_plugins(home: &Path) -> Vec<(String, PathBuf)> {
    let enabled: HashSet<String> = read_json(&home.join(".claude/settings.json"), 1 << 20)
        .and_then(|v| v.get("enabledPlugins").cloned())
        .and_then(|v| v.as_object().cloned())
        .map(|m| {
            m.into_iter()
                .filter(|(_, on)| on.as_bool() == Some(true))
                .map(|(k, _)| k)
                .collect()
        })
        .unwrap_or_default();
    let Some(installed) = read_json(
        &home.join(".claude/plugins/installed_plugins.json"),
        4 << 20,
    ) else {
        return Vec::new();
    };
    let Some(plugins) = installed.get("plugins").and_then(|p| p.as_object()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, entries) in plugins {
        if !enabled.contains(key) {
            continue;
        }
        let entry = entries
            .as_array()
            .and_then(|a| a.first())
            .unwrap_or(entries);
        let Some(path) = entry.get("installPath").and_then(|p| p.as_str()) else {
            continue;
        };
        let name = key.split('@').next().unwrap_or(key).to_owned();
        out.push((name, PathBuf::from(path)));
    }
    out.sort();
    out
}

/// Enabled Antigravity plugins (`~/.gemini/config/config.json` → `plugins.<name>.enabled`).
fn agy_plugins(config_dir: &Path) -> Vec<String> {
    let Some(v) = read_json(&config_dir.join("config.json"), 1 << 20) else {
        return Vec::new();
    };
    let Some(plugins) = v.get("plugins").and_then(|p| p.as_object()) else {
        return Vec::new();
    };
    let mut names: Vec<String> = plugins
        .iter()
        .filter(|(name, entry)| {
            entry.get("enabled").and_then(|e| e.as_bool()) != Some(false)
                && !name.contains(['/', '\\'])
                && name.as_str() != ".."
        })
        .map(|(name, _)| name.clone())
        .take(MAX_FILES_PER_DIR)
        .collect();
    names.sort();
    names
}

fn builtins(agent: Agent) -> &'static [(&'static str, &'static str)] {
    match agent {
        Agent::Claude => &[
            ("help", en!("Show help")),
            ("clear", en!("Clear the conversation")),
            ("compact", en!("Compact the conversation")),
            ("context", en!("Show context usage")),
            ("model", en!("Choose the model")),
            ("init", en!("Create CLAUDE.md for this project")),
            ("memory", en!("Edit memory files")),
            ("mcp", en!("Manage MCP servers")),
            ("agents", en!("Manage subagents")),
            ("permissions", en!("View or change permissions")),
            ("hooks", en!("Manage hooks")),
            ("plugin", en!("Manage plugins")),
            ("resume", en!("Resume a conversation")),
            ("review", en!("Review a pull request")),
            ("cost", en!("Show cost and duration")),
            ("status", en!("Show status")),
            ("config", en!("Open settings")),
            ("doctor", en!("Check the installation")),
            ("export", en!("Export the conversation")),
            ("add-dir", en!("Add a working directory")),
        ],
        Agent::Codex => &[
            ("model", en!("Choose the model and effort")),
            ("approvals", en!("Choose what runs without asking")),
            ("review", en!("Review the current changes")),
            ("new", en!("Start a new chat")),
            ("init", en!("Create AGENTS.md")),
            ("compact", en!("Summarize the conversation")),
            ("diff", en!("Show the git diff")),
            ("mention", en!("Mention a file")),
            ("status", en!("Show session status")),
            ("mcp", en!("List MCP tools")),
            ("quit", en!("Exit Codex")),
        ],
        Agent::Antigravity => &[
            ("help", en!("Show help")),
            ("clear", en!("Clear the conversation")),
            ("compact", en!("Summarize the conversation")),
            ("context", en!("Show context usage")),
            ("model", en!("Choose the model")),
            ("effort", en!("Choose reasoning effort")),
            ("skills", en!("List skills")),
            ("mcp", en!("Manage MCP servers")),
            ("agents", en!("Manage agents")),
            ("plugin", en!("Manage plugins")),
            ("permissions", en!("View or change permissions")),
            ("resume", en!("Resume a conversation")),
            ("rewind", en!("Go back to an earlier point")),
            ("diff", en!("Show the changes")),
            ("plan", en!("Plan before acting")),
            ("usage", en!("Show usage")),
            ("settings", en!("Open settings")),
            ("quit", en!("Exit Antigravity")),
        ],
        Agent::Gemini => &[
            ("help", en!("Show help")),
            ("chat", en!("Save or resume chats")),
            ("clear", en!("Clear the screen and context")),
            ("compress", en!("Summarize the context")),
            ("memory", en!("Manage memory")),
            ("mcp", en!("List MCP servers")),
            ("tools", en!("List tools")),
            ("stats", en!("Show session stats")),
            ("settings", en!("Open settings")),
            ("quit", en!("Exit Gemini CLI")),
        ],
    }
}

/// Find agent slash items for `project_root` (if any) and the user's home.
///
/// `installed` limits built-ins to agents present on `PATH`; files are read for every agent
/// whose config folder exists. Safe to call off the UI thread.
pub fn discover(home: &Path, project_root: Option<&Path>, installed: &[Agent]) -> SlashCatalog {
    let mut items = Vec::new();

    // Claude Code.
    if let Some(root) = project_root {
        skills_in(&root.join(".claude/skills"), Agent::Claude, "", &mut items);
        md_commands_in(
            &root.join(".claude/commands"),
            Agent::Claude,
            "",
            &mut items,
        );
    }
    skills_in(&home.join(".claude/skills"), Agent::Claude, "", &mut items);
    md_commands_in(
        &home.join(".claude/commands"),
        Agent::Claude,
        "",
        &mut items,
    );
    for (plugin, path) in claude_plugins(home) {
        let prefix = format!("{plugin}:");
        skills_in(&path.join("skills"), Agent::Claude, &prefix, &mut items);
        md_commands_in(&path.join("commands"), Agent::Claude, &prefix, &mut items);
    }
    if let Some(root) = project_root {
        if let Some(v) = read_json(&root.join(".mcp.json"), 1 << 20) {
            mcp_from_json(Agent::Claude, &v, &mut items);
        }
    }
    if let Some(v) = read_json(&home.join(".claude.json"), 64 << 20) {
        mcp_from_json(Agent::Claude, &v, &mut items);
        if let Some(project) = project_root
            .and_then(|r| r.to_str())
            .and_then(|r| v.get("projects").and_then(|p| p.get(r)))
        {
            mcp_from_json(Agent::Claude, project, &mut items);
        }
    }

    // Codex.
    md_commands_in(
        &home.join(".codex/prompts"),
        Agent::Codex,
        "prompts:",
        &mut items,
    );
    codex_mcp(&home.join(".codex/config.toml"), &mut items);

    // Gemini CLI.
    if let Some(root) = project_root {
        gemini_commands_in(&root.join(".gemini/commands"), &mut items);
    }
    gemini_commands_in(&home.join(".gemini/commands"), &mut items);
    if let Some(v) = read_json(&home.join(".gemini/settings.json"), 1 << 20) {
        mcp_from_json(Agent::Gemini, &v, &mut items);
    }

    // Antigravity (`agy`): project `.agents/skills` (or `.agent`, `_agents`, `_agent`), global
    // `~/.gemini/config/skills`, enabled plugins; MCP servers in `mcp_config.json`.
    if let Some(root) = project_root {
        for dir in [".agents", ".agent", "_agents", "_agent"] {
            skills_in(
                &root.join(dir).join("skills"),
                Agent::Antigravity,
                "",
                &mut items,
            );
        }
    }
    let agy_config = home.join(".gemini/config");
    skills_in(
        &agy_config.join("skills"),
        Agent::Antigravity,
        "",
        &mut items,
    );
    if let Some(v) = read_json(&agy_config.join("mcp_config.json"), 1 << 20) {
        mcp_from_json(Agent::Antigravity, &v, &mut items);
    }
    for plugin in agy_plugins(&agy_config) {
        let path = agy_config.join("plugins").join(&plugin);
        skills_in(&path.join("skills"), Agent::Antigravity, "", &mut items);
        if let Some(v) = read_json(&path.join("mcp_config.json"), 1 << 20) {
            mcp_from_json(Agent::Antigravity, &v, &mut items);
        }
    }

    for agent in installed {
        for (name, en) in builtins(*agent) {
            items.push(SlashItem {
                name: (*name).to_owned(),
                source: SlashSource::BuiltIn(*agent),
                description: (*en).to_owned(),
                description_ar: Some(UiLanguage::Arabic.t(en).to_owned()),
            });
        }
    }

    // A project item shadows a user item of the same name and kind (project first above).
    let mut seen = HashSet::new();
    items.retain(|i| {
        seen.insert((
            i.name.clone(),
            std::mem::discriminant(&i.source),
            i.source.agent(),
        ))
    });
    SlashCatalog {
        items,
        project_root: project_root.map(Path::to_path_buf),
    }
}

/// Agents found on `PATH`.
pub fn installed_agents() -> Vec<Agent> {
    let Some(path) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    Agent::ALL
        .into_iter()
        .filter(|a| dirs.iter().any(|d| d.join(a.program()).is_file()))
        .collect()
}
