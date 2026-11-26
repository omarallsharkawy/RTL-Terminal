//! Composer `/` menu: discovery of agent skills, commands and MCP servers, ranking, and what a
//! pick does at a shell prompt versus over a running agent.
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use twitty::composer::complete::CompletionKind;
use twitty::composer::slash::{
    discover, slash_query, suggestions, Agent, SlashCatalog, SlashInputs, SlashSource,
};
use twitty::composer::{ComposerAction, ComposerState};
use winit::keyboard::{Key, ModifiersState, NamedKey};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "twitty-slash-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// A home with Claude skills/commands/plugin/MCP, Codex prompts/MCP and Gemini commands/MCP,
/// plus a project with its own skill and `.mcp.json`.
fn fixture() -> (PathBuf, PathBuf) {
    let home = temp_dir("home");
    let project = temp_dir("project");
    write(
        &home.join(".claude/skills/impeccable/SKILL.md"),
        "---\nname: impeccable\ndescription: Design and polish interfaces.\nenv:\n  TOKEN:\n    description: nested, not the skill's\n---\nbody",
    );
    write(
        &home.join(".claude/skills/hidden/SKILL.md"),
        "---\nname: hidden\ndescription: Model only.\nuser-invocable: false\n---\n",
    );
    write(
        &home.join(".claude/skills/folded/SKILL.md"),
        "---\ndescription: >\n  First line\n  second line\nname: folded\n---\n",
    );
    write(
        &home.join(".claude/commands/git/ship.md"),
        "Commit, push and open a PR\n\nsteps...",
    );
    write(
        &home.join(".claude/settings.json"),
        r#"{"enabledPlugins": {"egm@egm": true, "off@x": false}}"#,
    );
    let plugin = home.join(".claude/plugins/cache/egm/0.1");
    write(
        &home.join(".claude/plugins/installed_plugins.json"),
        &format!(
            r#"{{"version": 2, "plugins": {{"egm@egm": [{{"installPath": {:?}}}], "off@x": [{{"installPath": "/nope"}}]}}}}"#,
            plugin.to_str().unwrap()
        ),
    );
    write(
        &plugin.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: \"Review a document\"\n---\n",
    );
    write(
        &home.join(".claude.json"),
        &format!(
            r#"{{"mcpServers": {{"rea": {{"command": "/usr/bin/rea", "args": []}}}},
                "projects": {{{:?}: {{"mcpServers": {{"local-db": {{"url": "http://127.0.0.1:9000"}}}}}}}}}}"#,
            project.to_str().unwrap()
        ),
    );
    write(
        &home.join(".codex/prompts/fix.md"),
        "---\ndescription: Fix the failing test\n---\n",
    );
    write(
        &home.join(".codex/config.toml"),
        "model = \"x\"\n[mcp_servers.node_repl]\ncommand = \"node\"\n[mcp_servers.node_repl.env]\nA = \"1\"\n",
    );
    write(
        &home.join(".gemini/commands/git/commit.toml"),
        "description = \"Write a commit message\"\nprompt = \"...\"\n",
    );
    write(
        &home.join(".gemini/settings.json"),
        r#"{"mcpServers": {"search": {"command": "npx"}}}"#,
    );
    write(
        &project.join(".claude/skills/deploy/SKILL.md"),
        "---\nname: deploy\ndescription: Deploy this project\n---\n",
    );
    write(
        &project.join(".mcp.json"),
        r#"{"mcpServers": {"playwright": {"command": "npx"}}}"#,
    );
    (home, project)
}

fn names(catalog: &SlashCatalog, source: SlashSource) -> Vec<String> {
    let mut out: Vec<String> = catalog
        .items
        .iter()
        .filter(|i| i.source == source)
        .map(|i| i.name.clone())
        .collect();
    out.sort();
    out
}

#[test]
fn slash_query_is_the_whole_buffer_without_spaces_or_a_second_slash() {
    assert_eq!(slash_query("/", 1), Some(""));
    assert_eq!(slash_query("/rev", 4), Some("rev"));
    assert_eq!(slash_query("/مساعدة", "/مساعدة".len()), Some("مساعدة"));
    assert_eq!(slash_query("/rev", 2), None, "caret must be at the end");
    assert_eq!(slash_query("/usr/bin", 8), None, "a path stays a path");
    assert_eq!(slash_query("/review now", 11), None);
    assert_eq!(slash_query("ls /", 4), None);
}

#[test]
fn agent_is_recognized_from_the_foreground_command_line() {
    let argv = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        Agent::from_argv(&argv(&["/home/u/.local/bin/claude"])),
        Some(Agent::Claude)
    );
    assert_eq!(
        Agent::from_argv(&argv(&[
            "node",
            "/usr/lib/node_modules/@openai/codex/bin/codex.js"
        ])),
        Some(Agent::Codex)
    );
    assert_eq!(
        Agent::from_argv(&argv(&["gemini", "-i"])),
        Some(Agent::Gemini)
    );
    assert_eq!(Agent::from_argv(&argv(&["vim", "claude"])), None);
    assert_eq!(Agent::from_argv(&[]), None);
}

#[test]
fn discovers_skills_commands_plugins_and_mcp_servers_for_each_agent() {
    let (home, project) = fixture();
    let catalog = discover(&home, Some(&project), &[Agent::Claude]);

    assert_eq!(
        names(&catalog, SlashSource::Skill(Agent::Claude)),
        ["deploy", "egm:review", "folded", "impeccable"],
        "user-invocable: false is hidden; plugin skills are namespaced"
    );
    let imp = catalog
        .items
        .iter()
        .find(|i| i.name == "impeccable")
        .unwrap();
    assert_eq!(imp.description, "Design and polish interfaces.");
    let folded = catalog.items.iter().find(|i| i.name == "folded").unwrap();
    assert_eq!(folded.description, "First line second line");
    assert_eq!(
        names(&catalog, SlashSource::Command(Agent::Claude)),
        ["ship"]
    );
    assert_eq!(
        catalog
            .items
            .iter()
            .find(|i| i.name == "ship")
            .unwrap()
            .description,
        "Commit, push and open a PR"
    );
    assert_eq!(
        names(&catalog, SlashSource::Mcp(Agent::Claude)),
        ["local-db", "playwright", "rea"]
    );
    assert_eq!(
        names(&catalog, SlashSource::Command(Agent::Codex)),
        ["prompts:fix"]
    );
    assert_eq!(
        names(&catalog, SlashSource::Mcp(Agent::Codex)),
        ["node_repl"]
    );
    assert_eq!(
        names(&catalog, SlashSource::Command(Agent::Gemini)),
        ["git:commit"]
    );
    assert_eq!(names(&catalog, SlashSource::Mcp(Agent::Gemini)), ["search"]);
    assert!(names(&catalog, SlashSource::BuiltIn(Agent::Claude)).contains(&"compact".into()));
    assert!(
        names(&catalog, SlashSource::BuiltIn(Agent::Codex)).is_empty(),
        "built-ins only for installed agents"
    );
    let _ = fs::remove_dir_all(home);
    let _ = fs::remove_dir_all(project);
}

#[test]
fn at_a_shell_prompt_claude_items_start_claude_and_twitty_commands_run() {
    let (home, project) = fixture();
    let catalog = discover(&home, Some(&project), &[Agent::Claude, Agent::Codex]);
    let inputs = SlashInputs {
        catalog: &catalog,
        workflows: &[],
        foreground: None,
    };

    let imp = suggestions("imp", 4, &inputs);
    assert_eq!(imp[0].label, "/impeccable");
    assert_eq!(imp[0].kind, CompletionKind::Skill);
    assert_eq!(imp[0].replacement, "claude \"/impeccable\"");
    assert_eq!(imp[0].replace_range, (0, 4));

    let settings = suggestions("settings", 9, &inputs);
    assert_eq!(settings[0].kind, CompletionKind::Action);
    assert_eq!(settings[0].replacement, "settings.open");

    // Arabic titles find Twitty commands too.
    let ar = suggestions("الإعدادات", 1 + "الإعدادات".len(), &inputs);
    assert!(ar.iter().any(|i| i.replacement == "settings.open"));

    // Codex/Gemini items and agent built-ins need their agent running.
    let all = suggestions("", 1, &inputs);
    assert!(all.iter().all(|i| !i.label.contains("prompts:")));
    assert!(!all.iter().any(|i| i.label == "/compact"));
    let _ = fs::remove_dir_all(home);
    let _ = fs::remove_dir_all(project);
}

#[test]
fn over_a_running_agent_its_items_are_inserted_for_the_agent() {
    let (home, project) = fixture();
    let catalog = discover(&home, Some(&project), &[Agent::Claude, Agent::Codex]);
    let codex = SlashInputs {
        catalog: &catalog,
        workflows: &[],
        foreground: Some(Agent::Codex),
    };
    let fix = suggestions("fix", 4, &codex);
    assert_eq!(fix[0].label, "/prompts:fix");
    assert_eq!(fix[0].replacement, "/prompts:fix ");
    let mcp = suggestions("node", 5, &codex);
    assert_eq!(mcp[0].kind, CompletionKind::Mcp);
    assert_eq!(mcp[0].replacement, "/mcp");
    assert!(
        suggestions("imp", 4, &codex)
            .iter()
            .all(|i| i.kind != CompletionKind::Skill),
        "Claude skills are not offered to Codex"
    );

    let claude = SlashInputs {
        catalog: &catalog,
        workflows: &[],
        foreground: Some(Agent::Claude),
    };
    let compact = suggestions("comp", 5, &claude);
    assert_eq!(compact[0].label, "/compact");
    assert_eq!(compact[0].replacement, "/compact ");
    assert_eq!(
        compact[0].detail_ar.as_deref(),
        Some("Claude · لخّص المحادثة لتوفير السياق")
    );
    let _ = fs::remove_dir_all(home);
    let _ = fs::remove_dir_all(project);
}

fn type_text(state: &mut ComposerState, text: &str) -> ComposerAction {
    state.key(
        &Key::Character(text.into()),
        None,
        Some(text),
        ModifiersState::empty(),
        &[],
        &[],
        None,
        None,
    )
}

fn press(state: &mut ComposerState, key: NamedKey) -> ComposerAction {
    state.key(
        &Key::Named(key),
        None,
        None,
        ModifiersState::empty(),
        &[],
        &[],
        None,
        None,
    )
}

#[test]
fn typing_slash_opens_the_menu_and_enter_runs_or_inserts() {
    let (home, project) = fixture();
    let catalog = discover(&home, Some(&project), &[Agent::Claude]);
    let mut state = ComposerState::new();
    state.slash = Arc::new(catalog);

    type_text(&mut state, "/");
    assert!(state.popup.is_some(), "the menu opens on `/`");
    for c in ["s", "e", "t", "t", "i", "n", "g", "s", ".", "o"] {
        type_text(&mut state, c);
    }
    assert_eq!(
        press(&mut state, NamedKey::Enter),
        ComposerAction::RunCommand("settings.open".into())
    );
    assert_eq!(
        state.editor.text(),
        "",
        "the composer is cleared after running"
    );

    for c in ["/", "d", "e", "p", "l"] {
        type_text(&mut state, c);
    }
    assert_eq!(press(&mut state, NamedKey::Enter), ComposerAction::Redraw);
    assert_eq!(state.editor.text(), "claude \"/deploy\"");
    assert!(state.popup.is_none());

    // A real path does not open the menu.
    let mut state = ComposerState::new();
    state.slash = Arc::new(SlashCatalog::default());
    for c in ["/", "u", "s", "r", "/"] {
        type_text(&mut state, c);
    }
    assert!(state.popup.is_none());
    let _ = fs::remove_dir_all(home);
    let _ = fs::remove_dir_all(project);
}
