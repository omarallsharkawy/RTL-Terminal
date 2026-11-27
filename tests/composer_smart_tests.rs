//! Window-free tests for smart completion specs, dynamic generators, syntax highlighting,
//! and pre-run linting in the command composer (lane N14).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cosmic_text::{fontdb, FontSystem};
use twitty::color::Palette;
use twitty::composer::complete::{
    complete_with, extract_path_query, needed_generators, read_dir_entries, CompleteInputs,
    CompletionKind,
};
use twitty::composer::generators::{
    cargo_features_from_dir, cargo_targets_from_dir, git_branches, git_changed_files, git_remotes,
    git_tags, make_targets_from_dir, npm_scripts_from_dir, parse_cargo_toml,
    parse_makefile_targets, parse_package_json_scripts, parse_ssh_config,
    parse_systemctl_unit_files, scan_path_dirs, ssh_hosts_from_file, GeneratedItem, GeneratorCache,
    GeneratorKey, GeneratorKind,
};
use twitty::composer::highlight::{
    extract_candidate_paths, highlight_command, line_highlight_spans, HighlightContext, TokenKind,
};
use twitty::composer::lint::{active_lint_for_caret, lint_command, LintKind, LintSeverity};
use twitty::composer::specs::{default_registry, parse_specs, ArgKind};
use twitty::composer::{ComposerAction, ComposerState};
use twitty::layout::Rect;
use twitty::render::scene::composer::{
    build as build_composer_scene, shape_composer_line, shape_composer_line_highlighted,
    ComposerLayout, ComposerPopupItem, ComposerPopupView, ComposerView,
};
use twitty::render::scene::context::DrawContext;
use twitty::render::shapes::Shape;
use twitty::render::text::ui::UiFont;
use twitty::render::ui_style::UiStyle;
use twitty::render::CellMetrics;
use twitty::ui::chrome::UiLanguage;
use winit::keyboard::{Key, ModifiersState, NamedKey};

const NASKH: &[u8] = include_bytes!("../assets/fonts/NotoNaskhArabic-Regular.ttf");
const MONO: &[u8] = include_bytes!("golden/fonts/NotoSansMono-Subset.ttf");

fn test_font_system() -> FontSystem {
    let mut db = fontdb::Database::new();
    db.load_font_data(MONO.to_vec());
    db.load_font_data(NASKH.to_vec());
    db.set_monospace_family("Noto Sans Mono");
    db.set_sans_serif_family("Noto Sans Mono");
    FontSystem::new_with_locale_and_db("en-US".into(), db)
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "twitty-n14-{prefix}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn known_commands(list: &[&str]) -> HashSet<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

// ---- 1. Spec parsing and built-in coverage ----

#[test]
fn spec_parsing_valid_invalid_and_builtin_coverage() {
    let custom = r#"{
        "commands": [
            {
                "name": "mytool",
                "aliases": ["mt"],
                "desc": "Custom tool",
                "desc_ar": "أداة مخصصة",
                "options": [
                    { "short": "-c", "long": "--config", "value": "file", "desc": "Config file" },
                    { "short": "-v", "long": "--verbose", "desc": "Verbose" }
                ],
                "subcommands": [
                    {
                        "name": "deploy",
                        "desc": "Deploy service",
                        "options": [
                            { "long": "--env", "value": { "values": ["dev", "prod"] }, "desc": "Target env" }
                        ],
                        "args": ["ssh-host"]
                    }
                ]
            }
        ]
    }"#;
    let reg = parse_specs(custom).expect("custom spec must parse");
    let cmd = reg.get("mt").expect("alias lookup works");
    assert_eq!(cmd.name, "mytool");
    assert_eq!(cmd.description(UiLanguage::Arabic), "أداة مخصصة");
    let opt_c = cmd.find_option("-c").unwrap();
    assert!(opt_c.expects_value());
    assert_eq!(opt_c.value_kind(), Some(&ArgKind::File));
    let opt_v = cmd.find_option("--verbose").unwrap();
    assert!(!opt_v.expects_value());
    let sub = cmd.find_subcommand("deploy").unwrap();
    assert_eq!(sub.positional_arg(0), Some(&ArgKind::SshHost));

    // Validation errors on broken specs.
    assert!(parse_specs("not json").is_err());
    assert!(parse_specs(r#"{"commands":[{"name":"","desc":"bad"}]}"#).is_err());
    assert!(parse_specs(
        r#"{"commands":[{"name":"x","desc":"d","options":[{"desc":"missing flag"}]}]}"#
    )
    .is_err());

    // Built-in registry must cover every required command.
    let builtin = default_registry();
    for required in [
        "git",
        "cargo",
        "npm",
        "pnpm",
        "yarn",
        "docker",
        "docker-compose",
        "kubectl",
        "systemctl",
        "journalctl",
        "pacman",
        "apt",
        "ssh",
        "scp",
        "rsync",
        "tar",
        "grep",
        "find",
        "make",
        "python",
        "python3",
        "pip",
        "pip3",
        "go",
        "rustup",
        "gh",
        "sudo",
        "cd",
        "ls",
        "cp",
        "mv",
        "rm",
    ] {
        assert!(
            builtin.get(required).is_some(),
            "built-in spec missing required command `{required}`"
        );
    }
    let docker = builtin.get("docker").unwrap();
    let compose = docker
        .find_subcommand("compose")
        .expect("docker compose subcommand must exist");
    assert!(compose.find_subcommand("up").is_some());
    assert!(compose.find_subcommand("down").is_some());
    assert!(builtin.get("sudo").unwrap().delegates_command());
}

// ---- 2. Smart completion at many cursor positions & ranking ----

#[test]
fn completion_subcommand_option_value_sudo_pipes_and_ranking() {
    let specs = default_registry();
    let mut generated: HashMap<GeneratorKind, Vec<GeneratedItem>> = HashMap::new();
    generated.insert(
        GeneratorKind::GitBranches,
        vec![
            GeneratedItem::new("main", Some("current branch".into())),
            GeneratedItem::new("feat/arabic-rtl", None),
            GeneratedItem::new("fix/bidi-caret", None),
        ],
    );
    generated.insert(
        GeneratorKind::GitRemotes,
        vec![
            GeneratedItem::new("origin", Some("remote".into())),
            GeneratedItem::new("upstream", Some("remote".into())),
        ],
    );
    generated.insert(
        GeneratorKind::CargoFeatures,
        vec![
            GeneratedItem::new("wayland", Some("[\"winit/wayland\"]".into())),
            GeneratedItem::new("x11", None),
        ],
    );
    generated.insert(
        GeneratorKind::SystemctlUnits { user: false },
        vec![
            GeneratedItem::new("nginx.service", Some("enabled".into())),
            GeneratedItem::new("sshd.service", Some("enabled".into())),
        ],
    );
    generated.insert(
        GeneratorKind::MakeTargets,
        vec![
            GeneratedItem::new("build", Some("Compile binary".into())),
            GeneratedItem::new("test", Some("Run tests".into())),
        ],
    );
    let path_cmds = known_commands(&["git", "cargo", "grep", "systemctl", "sudo", "mycustombin"]);
    let history = vec![
        "git checkout main".to_string(),
        "git cherry-pick HEAD~1".to_string(),
        "git check-ignore foo".to_string(),
    ];

    let run = |text: &str, caret: usize| {
        let inputs = CompleteInputs {
            history: &history,
            workflows: &[],
            path_query: None,
            dir_entries: &[],
            specs,
            generated: &generated,
            path_commands: Some(&path_cmds),
        };
        complete_with(text, caret, &inputs)
    };

    // Subcommand completion + ranking: exact prefix > fuzzy > history.
    let sub = run("git ch", 6);
    assert_eq!(sub[0].kind, CompletionKind::Subcommand);
    assert_eq!(sub[0].replacement, "checkout");
    assert_eq!(sub[0].replace_range, (4, 6));
    assert!(sub[0].detail.is_some());
    assert!(sub[0].detail_ar.is_some());
    // History items come strictly after contextual subcommand matches.
    let first_hist = sub
        .iter()
        .position(|i| i.kind == CompletionKind::History)
        .expect("history match present");
    let last_sub = sub
        .iter()
        .rposition(|i| i.kind == CompletionKind::Subcommand)
        .unwrap();
    assert!(last_sub < first_hist);

    // Fuzzy subcommand beats history (`git cko` -> `checkout` via subsequence).
    let fuzzy = run("git cko", 7);
    assert_eq!(fuzzy[0].kind, CompletionKind::Subcommand);
    assert_eq!(fuzzy[0].replacement, "checkout");

    // Exact prefix beats fuzzy (`git st` -> `stash`/`status` before `reset`/`restore`).
    let prefix_vs_fuzzy = run("git st", 6);
    assert!(prefix_vs_fuzzy[0].replacement.starts_with("st"));
    assert!(prefix_vs_fuzzy[1].replacement.starts_with("st"));

    // Nested subcommand (`docker compose u` -> `up`).
    let dc = run("docker compose u", "docker compose u".len());
    assert_eq!(dc[0].kind, CompletionKind::Subcommand);
    assert_eq!(dc[0].replacement, "up");

    // Option completion (`git commit --am` -> `--amend`).
    let opt = run("git commit --am", "git commit --am".len());
    assert_eq!(opt[0].kind, CompletionKind::Option);
    assert_eq!(opt[0].replacement, "--amend");
    assert_eq!(opt[0].replace_range, (11, 15));

    // Option value completion (space-separated and `=`-separated).
    let opt_val = run("kubectl get -o ", "kubectl get -o ".len());
    assert!(opt_val
        .iter()
        .any(|i| i.kind == CompletionKind::Value && i.replacement == "json"));
    assert!(opt_val
        .iter()
        .any(|i| i.kind == CompletionKind::Value && i.replacement == "yaml"));

    let eq_val = run(
        "cargo test --features=way",
        "cargo test --features=way".len(),
    );
    assert_eq!(eq_val[0].kind, CompletionKind::Feature);
    assert_eq!(eq_val[0].replacement, "wayland");
    assert_eq!(eq_val[0].replace_range, (22, 25));

    // Dynamic positional args: `git push <remote> <branch>`.
    let remote = run("git push or", "git push or".len());
    assert_eq!(remote[0].kind, CompletionKind::Remote);
    assert_eq!(remote[0].replacement, "origin");
    let branch = run("git push origin fe", "git push origin fe".len());
    assert_eq!(branch[0].kind, CompletionKind::Branch);
    assert_eq!(branch[0].replacement, "feat/arabic-rtl");

    // After `sudo` (including sudo flags taking values like `-u root`).
    let after_sudo_cmd = run("sudo -u root sys", "sudo -u root sys".len());
    assert_eq!(after_sudo_cmd[0].kind, CompletionKind::Command);
    assert_eq!(after_sudo_cmd[0].replacement, "systemctl");

    let after_sudo_sub = run("sudo systemctl res", "sudo systemctl res".len());
    assert_eq!(after_sudo_sub[0].kind, CompletionKind::Subcommand);
    assert_eq!(after_sudo_sub[0].replacement, "restart");

    let after_sudo_unit = run(
        "sudo systemctl restart ng",
        "sudo systemctl restart ng".len(),
    );
    assert_eq!(after_sudo_unit[0].kind, CompletionKind::Unit);
    assert_eq!(after_sudo_unit[0].replacement, "nginx.service");

    // After pipes and boolean operators.
    let after_pipe = run("cat foo.txt | grep --ig", "cat foo.txt | grep --ig".len());
    assert_eq!(after_pipe[0].kind, CompletionKind::Option);
    assert_eq!(after_pipe[0].replacement, "--ignore-case");
    assert_eq!(after_pipe[0].replace_range, (19, 23));

    let after_and = run("git status && make bu", "git status && make bu".len());
    assert_eq!(after_and[0].kind, CompletionKind::Target);
    assert_eq!(after_and[0].replacement, "build");

    // Mid-line cursor preserves trailing text via exact replace_range.
    let mid = run("git ch main", 6);
    assert_eq!(mid[0].replacement, "checkout");
    assert_eq!(mid[0].replace_range, (4, 6));

    // `needed_generators` detects dynamic sources for cursor positions.
    let cwd = Path::new("/tmp");
    assert_eq!(
        needed_generators("git switch ", 11, specs, Some(cwd)),
        vec![GeneratorKey::new(GeneratorKind::GitBranches, Some(cwd))]
    );
    assert_eq!(
        needed_generators("systemctl --user status ", 24, specs, Some(cwd)),
        vec![GeneratorKey::new(
            GeneratorKind::SystemctlUnits { user: true },
            Some(cwd)
        )]
    );
    assert_eq!(
        needed_generators("ssh ", 4, specs, Some(cwd)),
        vec![GeneratorKey::new(GeneratorKind::SshHosts, Some(cwd))]
    );
}

#[test]
fn cd_completes_only_directories_while_ls_completes_files_and_dirs() {
    let dir = TempDir::new("cd-ls");
    std::fs::create_dir(dir.0.join("alpha_dir")).unwrap();
    std::fs::write(dir.0.join("alpha_file.txt"), b"hi").unwrap();
    let entries = read_dir_entries(&dir.0, 64);
    let empty_gen = HashMap::new();

    let cd_input = "cd alpha";
    let pq_cd = extract_path_query(cd_input, cd_input.len(), Some(&dir.0)).unwrap();
    let cd_items = complete_with(
        cd_input,
        cd_input.len(),
        &CompleteInputs {
            history: &[],
            workflows: &[],
            path_query: Some(&pq_cd),
            dir_entries: &entries,
            specs: default_registry(),
            generated: &empty_gen,
            path_commands: None,
        },
    );
    assert_eq!(cd_items.len(), 1);
    assert_eq!(cd_items[0].replacement, "alpha_dir/");

    let ls_input = "ls alpha";
    let pq_ls = extract_path_query(ls_input, ls_input.len(), Some(&dir.0)).unwrap();
    let ls_items = complete_with(
        ls_input,
        ls_input.len(),
        &CompleteInputs {
            history: &[],
            workflows: &[],
            path_query: Some(&pq_ls),
            dir_entries: &entries,
            specs: default_registry(),
            generated: &empty_gen,
            path_commands: None,
        },
    );
    assert_eq!(ls_items.len(), 2);
}

// ---- 3. Dynamic generators against temp fixtures ----

#[test]
fn generators_against_temp_fixtures_and_cache() {
    let dir = TempDir::new("fixtures");

    // 1. package.json
    std::fs::write(
        dir.0.join("package.json"),
        r#"{
            "name": "demo",
            "scripts": {
                "dev": "vite --port 3000",
                "build": "tsc && vite build",
                "test": "vitest run"
            }
        }"#,
    )
    .unwrap();
    let scripts = npm_scripts_from_dir(&dir.0);
    assert_eq!(scripts.len(), 3);
    assert!(scripts
        .iter()
        .any(|s| s.value == "dev" && s.desc.as_deref() == Some("vite --port 3000")));
    assert!(parse_package_json_scripts("invalid").is_empty());

    // 2. Makefile
    std::fs::write(
        dir.0.join("Makefile"),
        concat!(
            ".PHONY: all clean\n",
            "CC := gcc\n",
            "%.o: %.c\n",
            "\t$(CC) -c $<\n",
            "all: build test ## Build and test everything\n",
            "build: ## Compile release binary\n",
            "\tcargo build --release\n",
            "clean:\n",
            "\trm -rf target\n",
        ),
    )
    .unwrap();
    let targets = make_targets_from_dir(&dir.0);
    let target_names: Vec<&str> = targets.iter().map(|t| t.value.as_str()).collect();
    assert_eq!(target_names, vec!["all", "build", "clean"]);
    assert_eq!(targets[1].desc.as_deref(), Some("Compile release binary"));
    assert!(parse_makefile_targets(".DEFAULT:\n\techo hi\n").is_empty());

    // 3. Cargo.toml + conventional bins/examples/tests
    std::fs::write(
        dir.0.join("Cargo.toml"),
        concat!(
            "[package]\n",
            "name = \"demo-app\"\n",
            "version = \"0.1.0\"\n\n",
            "[[bin]]\n",
            "name = \"helper-cli\"\n",
            "path = \"src/helper.rs\"\n\n",
            "[features]\n",
            "default = [\"wayland\"] # comment\n",
            "wayland = []\n",
            "arabic-extra = [\"dep:icu\"]\n",
        ),
    )
    .unwrap();
    std::fs::create_dir_all(dir.0.join("examples")).unwrap();
    std::fs::write(dir.0.join("examples/showcase.rs"), "fn main() {}").unwrap();
    let cargo_targets = cargo_targets_from_dir(&dir.0);
    assert!(cargo_targets.iter().any(|t| t.value == "demo-app"));
    assert!(cargo_targets.iter().any(|t| t.value == "helper-cli"));
    assert!(cargo_targets.iter().any(|t| t.value == "showcase"));
    let cargo_features = cargo_features_from_dir(&dir.0);
    let feat_names: Vec<&str> = cargo_features.iter().map(|f| f.value.as_str()).collect();
    assert!(feat_names.contains(&"default"));
    assert!(feat_names.contains(&"wayland"));
    assert!(feat_names.contains(&"arabic-extra"));
    let (pt, pf) = parse_cargo_toml("[features]\nfoo = []\n");
    assert!(pt.is_empty());
    assert_eq!(pf.len(), 1);

    // 4. ~/.ssh/config sample
    let ssh_cfg = dir.0.join("ssh_config");
    std::fs::write(
        &ssh_cfg,
        concat!(
            "# Global defaults\n",
            "Host *\n",
            "  ServerAliveInterval 60\n\n",
            "Host prod-web prod-api !prod-legacy\n",
            "  HostName 10.0.0.10\n",
            "  User deploy\n\n",
            "Host staging\n",
            "  HostName=staging.example.com\n",
            "Host *.internal\n",
            "  User ops\n",
        ),
    )
    .unwrap();
    let hosts = ssh_hosts_from_file(&ssh_cfg);
    let host_names: Vec<&str> = hosts.iter().map(|h| h.value.as_str()).collect();
    assert_eq!(host_names, vec!["prod-web", "prod-api", "staging"]);
    assert_eq!(hosts[0].desc.as_deref(), Some("deploy@10.0.0.10"));
    assert_eq!(hosts[2].desc.as_deref(), Some("staging.example.com"));
    assert!(parse_ssh_config("Host *\n  User a\n").is_empty());

    // 5. systemctl output parser
    let units =
        parse_systemctl_unit_files("nginx.service enabled enabled\nsshd.service disabled preset\n");
    assert_eq!(units.len(), 2);
    assert_eq!(units[0].value, "nginx.service");
    assert_eq!(units[0].desc.as_deref(), Some("enabled"));

    // 6. Real temp git repo: branches, remotes, tags, changed files
    if Command::new("git").arg("--version").output().is_ok() {
        let git_dir = TempDir::new("gitrepo");
        let run_git = |args: &[&str]| {
            let status = Command::new("git")
                .current_dir(&git_dir.0)
                .args(args)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .status()
                .expect("git failed to spawn");
            assert!(status.success(), "git {args:?} failed");
        };
        run_git(&["init", "-b", "main"]);
        std::fs::write(git_dir.0.join("README.md"), "hello\n").unwrap();
        run_git(&["add", "README.md"]);
        run_git(&["commit", "-m", "initial"]);
        run_git(&["branch", "feature/rtl"]);
        run_git(&["tag", "v1.0.0"]);
        run_git(&["remote", "add", "origin", "https://example.com/repo.git"]);
        std::fs::write(git_dir.0.join("README.md"), "changed\n").unwrap();
        std::fs::write(git_dir.0.join("new_file.txt"), "untracked\n").unwrap();

        let branches = git_branches(&git_dir.0);
        assert!(branches
            .iter()
            .any(|b| b.value == "main" && b.desc.as_deref() == Some("current branch")));
        assert!(branches.iter().any(|b| b.value == "feature/rtl"));

        let remotes = git_remotes(&git_dir.0);
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].value, "origin");

        let tags = git_tags(&git_dir.0);
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].value, "v1.0.0");

        let changed = git_changed_files(&git_dir.0);
        assert!(changed.iter().any(|f| f.value == "README.md"));
        assert!(changed
            .iter()
            .any(|f| f.value == "new_file.txt" && f.desc.as_deref() == Some("untracked")));
    }

    // 7. GeneratorCache TTL & invalidation + scan_path_dirs
    let mut cache = GeneratorCache::new();
    let now = Instant::now();
    let key = GeneratorKey::new(GeneratorKind::MakeTargets, Some(&dir.0));
    cache.insert(key.clone(), targets.clone(), now);
    assert!(cache.get(&key, now + Duration::from_secs(1)).is_some());
    assert!(cache.get(&key, now + Duration::from_secs(60)).is_none());
    cache.invalidate_cwd(&dir.0);
    assert!(cache.get(&key, now).is_none());

    let bin_dir = TempDir::new("bindir");
    let fake_bin = bin_dir.0.join("twitty-fake-bin");
    std::fs::write(&fake_bin, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let scanned = scan_path_dirs(std::slice::from_ref(&bin_dir.0));
    assert!(scanned.contains("twitty-fake-bin"));
}

// ---- 4. Syntax highlighting spans on mixed Arabic/English commands & bidi ----

#[test]
fn syntax_highlighting_tokens_mixed_arabic_and_bidi_shaping() {
    let cmds = known_commands(&["git", "grep", "sudo", "systemctl"]);
    let existing = HashSet::from(["src/main.rs".to_string()]);
    let path_exists = |p: &str| existing.contains(p);
    let hctx = HighlightContext::with_defaults(Some(&cmds), &path_exists);

    let line = "git commit -m \"إصلاح مشكلة $USER هنا\" && echo 'مرحبا بالعالم' | grep -i foo src/main.rs # تعليق";
    let spans = highlight_command(line, &hctx);

    let kind_of = |needle: &str| -> TokenKind {
        let start = line
            .find(needle)
            .unwrap_or_else(|| panic!("missing `{needle}`"));
        let end = start + needle.len();
        spans
            .iter()
            .find(|s| s.start <= start && s.end >= end)
            .unwrap_or_else(|| panic!("no single span covering `{needle}` in {spans:?}"))
            .kind
    };

    assert_eq!(kind_of("git"), TokenKind::CommandKnown);
    assert_eq!(kind_of("commit"), TokenKind::Subcommand);
    assert_eq!(kind_of("-m"), TokenKind::Option);
    assert_eq!(kind_of("إصلاح مشكلة"), TokenKind::String);
    assert_eq!(kind_of("$USER"), TokenKind::Variable);
    assert_eq!(kind_of("هنا"), TokenKind::String);
    assert_eq!(kind_of("&&"), TokenKind::Operator);
    assert_eq!(kind_of("echo"), TokenKind::CommandBuiltin);
    assert_eq!(kind_of("'مرحبا بالعالم'"), TokenKind::String);
    assert_eq!(kind_of("|"), TokenKind::Operator);
    assert_eq!(kind_of("grep"), TokenKind::CommandKnown);
    assert_eq!(kind_of("-i"), TokenKind::Option);
    assert_eq!(kind_of("src/main.rs"), TokenKind::ExistingPath);
    assert_eq!(kind_of("# تعليق"), TokenKind::Comment);

    // Unknown command and sudo inner command highlighting.
    let sudo_unknown = "sudo nonexistent_xyz --flag";
    let su_spans = highlight_command(sudo_unknown, &hctx);
    assert_eq!(su_spans[0].kind, TokenKind::CommandKnown); // sudo
    assert_eq!(su_spans[1].kind, TokenKind::CommandUnknown); // nonexistent_xyz
    assert_eq!(su_spans[2].kind, TokenKind::Option); // --flag

    // Arabic word glued to `$VAR` inside double quotes stays ONE unbroken String span
    // so cosmic-text never misorders the RTL word.
    let glued = "echo \"مرحبا$USER\"";
    let g_spans = highlight_command(glued, &hctx);
    let q_start = glued.find('"').unwrap();
    let str_span = g_spans.iter().find(|s| s.start == q_start).unwrap();
    assert_eq!(str_span.kind, TokenKind::String);
    assert_eq!(str_span.end, glued.len());

    // Shaping a highlighted mixed Arabic/English string preserves exact RTL bidi order and
    // matches unhighlighted RowShaper caret geometry identically in both dark and light themes.
    let mut font_system = test_font_system();
    let sample = "echo \"مرحبا بالعالم\"";
    let sample_spans = highlight_command(sample, &hctx);
    let line_spans = line_highlight_spans(&sample_spans, 0, sample.len());
    for theme in ["twitty-dark", "twitty-light"] {
        let palette = Palette::named(theme).unwrap();
        let style = UiStyle::new(&palette, None, 1.0);
        let metrics = CellMetrics::measure(&mut font_system, None, 15.0, 1.0);
        let mut ctx = DrawContext {
            font_system: &mut font_system,
            family: None,
            palette: &palette,
            metrics,
            is_srgb: true,
        };
        let ar_start = sample.find("مرحبا").unwrap();
        let ar_second = sample.find("بالعالم").unwrap();
        let mut xs = Vec::new();
        for (off, _) in "مرحبا".char_indices() {
            let byte = ar_start + off;
            let plain = shape_composer_line(&mut ctx, sample, Some(byte), style.text, None, None);
            let shaped = shape_composer_line_highlighted(
                &mut ctx,
                sample,
                Some(byte),
                style.text,
                &line_spans,
                style,
                None,
                None,
            );
            assert!(shaped.extends_left, "Arabic inside string must stay RTL");
            assert_eq!(
                shaped.extends_left, plain.extends_left,
                "highlighting must not alter RTL direction ({theme})"
            );
            assert!(
                (shaped.caret_x - plain.caret_x).abs() < 0.01,
                "highlighting must not break Arabic shaping runs ({theme}): highlighted={} plain={}",
                shaped.caret_x,
                plain.caret_x
            );
            xs.push(shaped.caret_x);
        }
        for pair in xs.windows(2) {
            assert!(
                pair[0] >= pair[1],
                "highlighted Arabic inside string must move right-to-left ({theme}): {xs:?}"
            );
        }
        assert!(
            xs[0] > *xs.last().unwrap(),
            "Arabic word must span right-to-left ({theme}): {xs:?}"
        );
        // Second Arabic word ("بالعالم") must sit visually to the left of the first ("مرحبا").
        let second_word = shape_composer_line_highlighted(
            &mut ctx,
            sample,
            Some(ar_second),
            style.text,
            &line_spans,
            style,
            None,
            None,
        );
        assert!(second_word.extends_left);
        assert!(
            xs[0] > second_word.caret_x,
            "first Arabic word must sit right of second word ({theme}): first={} second={}",
            xs[0],
            second_word.caret_x
        );
    }

    // Candidate path extraction for off-thread stat.
    let cands = extract_candidate_paths(
        "cat src/main.rs --file=Cargo.toml",
        Some(Path::new("/repo")),
    );
    assert!(cands.contains(&PathBuf::from("/repo/src/main.rs")));
    assert!(cands.contains(&PathBuf::from("/repo/Cargo.toml")));
}

// ---- 5. Pre-run lint marks, never blocking execution, and scene rendering ----

#[test]
fn pre_run_lint_marks_never_block_and_render_wavy_underlines_and_tooltip() {
    let cmds = known_commands(&["git", "ls", "grep", "sudo"]);
    let path_exists = |p: &str| p == "./run.sh";
    let hctx = HighlightContext::with_defaults(Some(&cmds), &path_exists);

    // 1. Unknown command (including after pipe and sudo).
    let m1 = lint_command("boguscmd --help | grep foo", &hctx);
    assert_eq!(m1.len(), 1);
    assert_eq!(m1[0].kind, LintKind::UnknownCommand("boguscmd".into()));
    assert_eq!(m1[0].severity, LintSeverity::Warning);
    assert!(m1[0].message_en().contains("boguscmd"));
    assert!(m1[0].message_ar().contains("boguscmd"));

    let m_sudo = lint_command("sudo ghostbin", &hctx);
    assert_eq!(m_sudo.len(), 1);
    assert_eq!(m_sudo[0].kind, LintKind::UnknownCommand("ghostbin".into()));

    // Builtins, existing explicit paths, local functions/aliases, and comments produce zero lints.
    assert!(lint_command("echo مرحبا && ./run.sh", &hctx).is_empty());
    assert!(lint_command("alias ll='ls -la'; ll", &hctx).is_empty());
    assert!(lint_command("myfn() { ls; }\nmyfn", &hctx).is_empty());
    assert!(lint_command("# just a comment with 'unclosed", &hctx).is_empty());

    // When `$PATH` has not finished loading (`None`), never emit false UnknownCommand marks.
    let unloaded = HighlightContext::with_defaults(None, &path_exists);
    assert!(lint_command("not_scanned_yet arg", &unloaded).is_empty());

    // 2. Unclosed quotes and brackets.
    let m_quote = lint_command("echo \"مرحبا", &hctx);
    assert_eq!(m_quote.len(), 1);
    assert_eq!(m_quote[0].kind, LintKind::UnclosedQuote('"'));
    assert_eq!(m_quote[0].severity, LintSeverity::Error);

    let m_bracket = lint_command("echo $(ls -la", &hctx);
    assert_eq!(m_bracket.len(), 1);
    assert_eq!(m_bracket[0].kind, LintKind::UnclosedBracket('('));

    let m_close = lint_command("ls )", &hctx);
    assert_eq!(m_close.len(), 1);
    assert_eq!(m_close[0].kind, LintKind::UnexpectedClose(')'));

    // 3. Trailing pipe and `&&` / `||` (but `;` and `&` are valid).
    let m_pipe = lint_command("ls | ", &hctx);
    assert_eq!(m_pipe.len(), 1);
    assert_eq!(m_pipe[0].kind, LintKind::TrailingOperator("|".into()));

    let m_and = lint_command("git status &&", &hctx);
    assert_eq!(m_and.len(), 1);
    assert_eq!(m_and[0].kind, LintKind::TrailingOperator("&&".into()));

    assert!(lint_command("ls;", &hctx).is_empty());
    assert!(lint_command("ls &", &hctx).is_empty());

    // Active lint selection prefers the mark covering the caret.
    let multi = lint_command("bogus \"unclosed", &hctx);
    assert_eq!(multi.len(), 2);
    assert_eq!(
        active_lint_for_caret(&multi, 2).unwrap().kind,
        LintKind::UnknownCommand("bogus".into())
    );
    assert_eq!(
        active_lint_for_caret(&multi, 10).unwrap().kind,
        LintKind::UnclosedQuote('"')
    );

    // Lints NEVER block running: Enter still submits the exact command text.
    let mut state = ComposerState::new();
    state.editor.insert_text("bogus \"unclosed &&");
    let action = state.key(
        &Key::Named(NamedKey::Enter),
        None,
        None,
        ModifiersState::empty(),
        &[],
        &[],
        None,
        None,
    );
    assert_eq!(action, ComposerAction::Submit("bogus \"unclosed &&".into()));

    // Headless scene rendering draws wavy underline capsules and the lint tooltip bar in both
    // dark/light themes, EN/AR languages, and 1x/2x scale factors.
    let mut font_system = test_font_system();
    let ui_font = UiFont::default();
    let bad_cmd = "bogus \"مرحبا";
    let highlights = highlight_command(bad_cmd, &hctx);
    let lints = lint_command(bad_cmd, &hctx);
    assert_eq!(lints.len(), 2);

    for (theme, lang, scale) in [
        ("twitty-dark", UiLanguage::English, 1.0_f32),
        ("twitty-light", UiLanguage::Arabic, 2.0_f32),
    ] {
        let palette = Palette::named(theme).unwrap();
        let style = UiStyle::new(&palette, None, scale);
        let metrics = CellMetrics::measure(&mut font_system, None, 15.0, scale);
        let pane_rect = Rect::new(0.0, 0.0, 800.0 * scale, 500.0 * scale);
        let mut ctx = DrawContext {
            font_system: &mut font_system,
            family: None,
            palette: &palette,
            metrics,
            is_srgb: true,
        };
        let layout = ComposerLayout::with_lint(pane_rect, metrics, scale, 1, 0, 1, true);
        assert!(layout.lint_bar.is_some());

        let view = ComposerView {
            context: None,
            pane_rect,
            lines: vec![bad_cmd.into()],
            caret: (0, bad_cmd.len()),
            selection: None,
            preedit: None,
            ghost: None,
            highlights: highlights.clone(),
            lints: lints.clone(),
            focused: true,
            cursor_blink_on: true,
            popup: Some(ComposerPopupView {
                items: vec![ComposerPopupItem {
                    kind: CompletionKind::Subcommand,
                    label: "checkout".into(),
                    detail: Some("Switch branches".into()),
                }],
                selected: 0,
            }),
            language: lang,
            ai: Default::default(),
        };
        let scene = build_composer_scene(&mut ctx, style, &ui_font, &view);
        // Wavy underlines + warning icon are emitted as Capsule shapes.
        let capsule_count = scene
            .shapes
            .iter()
            .filter(|s| matches!(s, Shape::Capsule { .. }))
            .count();
        assert!(
            capsule_count >= 4,
            "expected wavy underline + icon capsules, got {capsule_count}"
        );
        // Lint bar label + popup label + description + badge must be present.
        assert!(scene.labels.len() >= 4);
    }
}
