use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use twitty_ai::context::{
    assemble, estimate_tokens, included_items, reassemble_with_bundle, tail_trim_to_tokens,
    total_included_tokens, AssembleInput, SelectedBlock,
};
use twitty_ai::engine::model::ContextKind;
use twitty_store::{sha256_hex, MemoryKind, NewCommand, NewMemory, Store};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "twitty-context-test-{tag}-{}-{id}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn token_estimator_weights_latin_and_arabic_and_tail_trim_bounds() {
    assert_eq!(estimate_tokens(""), 0);
    assert_eq!(estimate_tokens("a"), 1);
    // 40 ASCII chars -> ~4 chars/token = 10 tokens.
    assert_eq!(estimate_tokens(&"a".repeat(40)), 10);
    // 40 Arabic chars -> ~2 chars/token = 20 tokens.
    assert_eq!(estimate_tokens(&"م".repeat(40)), 20);
    // Mixed: 20 Latin (5) + 20 Arabic (10) = 15 tokens.
    let mixed = format!("{}{}", "a".repeat(20), "ب".repeat(20));
    assert_eq!(estimate_tokens(&mixed), 15);

    // Tail trimming keeps the tail (error line) and strictly respects max_tokens.
    let mut log = String::new();
    for i in 0..40 {
        log.push_str(&format!("line {i}: compiling crate module\n"));
    }
    log.push_str("error[E0308]: mismatched types in final line");
    let trimmed = tail_trim_to_tokens(&log, 30).expect("trimmed output");
    assert!(
        estimate_tokens(&trimmed) <= 30,
        "estimated tokens {} exceeded 30",
        estimate_tokens(&trimmed)
    );
    assert!(trimmed.contains("mismatched types in final line"));
    assert!(!trimmed.contains("line 0:"));
}

#[test]
fn assemble_order_approvals_redaction_and_determinism() {
    let dir = TempDir::new("order");
    let repo = dir.path().join("repo");
    let sub = repo.join("src");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(&sub).unwrap();

    // Rules files: AGENTS.md approved, CLAUDE.md changed after approval, TWITTY.md unapproved.
    let agents_body = format!(
        "Always run clippy. Token {}",
        concat!("gh", "p_1234567890abcdef1234567890abcdef1234")
    );
    fs::write(repo.join("AGENTS.md"), &agents_body).unwrap();
    fs::write(repo.join("CLAUDE.md"), "original claude rules").unwrap();
    fs::write(repo.join("TWITTY.md"), "unapproved secret rules").unwrap();

    let store = Store::open_in_memory().unwrap();
    let key = repo.to_string_lossy().into_owned();
    store
        .approve_rules_file(&key, "AGENTS.md", &sha256_hex(agents_body.as_bytes()))
        .unwrap();
    store
        .approve_rules_file(&key, "CLAUDE.md", &sha256_hex(b"original claude rules"))
        .unwrap();
    // Mutate CLAUDE.md so its approval silently lapses.
    fs::write(repo.join("CLAUDE.md"), "tampered claude rules").unwrap();

    // User rule.
    let rule_id = store
        .create_memory(&NewMemory {
            project_root: key.clone(),
            kind: MemoryKind::Rule,
            text: "Use Arabic and English labels everywhere".into(),
            source: "user".into(),
            approved: true,
        })
        .unwrap();

    // Approved fact matching query "bidi", plus an unapproved pending fact matching "bidi".
    let fact_id = store
        .create_memory(&NewMemory {
            project_root: key.clone(),
            kind: MemoryKind::Fact,
            text: "Bidi shaping tests live in tests/bidi_layout_tests.rs".into(),
            source: "agent".into(),
            approved: true,
        })
        .unwrap();
    store
        .create_memory(&NewMemory {
            project_root: key.clone(),
            kind: MemoryKind::Fact,
            text: "Pending unapproved bidi note".into(),
            source: "agent".into(),
            approved: false,
        })
        .unwrap();

    // Env note.
    let env_id = store
        .create_memory(&NewMemory {
            project_root: key.clone(),
            kind: MemoryKind::Env,
            text: "Rust toolchain: 1.90.0".into(),
            source: "auto:env".into(),
            approved: true,
        })
        .unwrap();

    // Frequent command in history.
    for i in 0..3 {
        store
            .record_command(&NewCommand {
                command: "cargo test --test bidi_layout_tests".into(),
                exit_code: Some(0),
                duration_ms: Some(150),
                cwd: sub.to_string_lossy().into_owned(),
                project_root: Some(key.clone()),
                shell: Some("zsh".into()),
                started_at: 1000 + i,
                finished_at: Some(1150 + i),
            })
            .unwrap();
    }

    let secret_block = SelectedBlock {
        command: Some("cargo test".into()),
        exit_code: Some(101),
        output: format!(
            "Authorization: Bearer {}\ntest failed: bidi_join",
            concat!("top", "secret_bearer_token_999")
        ),
    };

    let input = AssembleInput {
        cwd: Some(&sub),
        project_root: None,
        request: "fix bidi shaping failure",
        budget_tokens: 4096,
        memory_exclude: &[],
        store: Some(&store),
        selected_blocks: std::slice::from_ref(&secret_block),
        included_overrides: &[],
    };

    let bundle = assemble(&input);
    let bundle2 = assemble(&input);
    assert_eq!(bundle, bundle2, "context assembly must be deterministic");
    assert_eq!(bundle.project_root.as_deref(), Some(repo.as_path()));

    // Verify exact priority order:
    // 0: AGENTS.md (approved rules file)
    // 1: rule #<rule_id> (user rule)
    // 2: fact #<fact_id> (FTS hit)
    // 3: env #<env_id> (env note)
    // 4: cmd: cargo test --test bidi_layout_tests (frequent command)
    // 5: block: cargo test (selected block)
    let labels: Vec<&str> = bundle.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(
        labels,
        vec![
            "AGENTS.md",
            &format!("rule #{rule_id}"),
            &format!("fact #{fact_id}"),
            &format!("env #{env_id}"),
            "cmd: cargo test --test bidi_layout_tests",
            "block: cargo test",
        ]
    );
    assert_eq!(bundle.items[0].kind, ContextKind::Rule);
    assert_eq!(bundle.items[1].kind, ContextKind::Rule);
    assert_eq!(bundle.items[2].kind, ContextKind::Memory);
    assert_eq!(bundle.items[3].kind, ContextKind::Memory);
    assert_eq!(bundle.items[4].kind, ContextKind::Memory);
    assert_eq!(bundle.items[5].kind, ContextKind::Block);

    // Unapproved/changed rules files and pending facts must NEVER leak into the bundle.
    for item in &bundle.items {
        assert!(!item.text.contains("tampered claude"));
        assert!(!item.text.contains("unapproved secret"));
        assert!(!item.text.contains("Pending unapproved"));
        assert!(!item.text.contains("1234567890abcdef"));
        assert!(!item.text.contains("secret_bearer_token"));
    }
    assert!(bundle.items[0].text.contains("[REDACTED:github_token]"));
    assert!(bundle.items[5].text.contains("[REDACTED:auth_header]"));
}

#[test]
fn assemble_respects_exclusions_budget_and_included_toggle_reassembly() {
    let dir = TempDir::new("budget");
    let repo = dir.path().join("client-secret");
    fs::create_dir_all(repo.join(".git")).unwrap();

    let store = Store::open_in_memory().unwrap();
    let key = repo.to_string_lossy().into_owned();
    store
        .create_memory(&NewMemory {
            project_root: key.clone(),
            kind: MemoryKind::Rule,
            text: "A".repeat(120), // 30 tokens
            source: "user".into(),
            approved: true,
        })
        .unwrap();
    store
        .create_memory(&NewMemory {
            project_root: key.clone(),
            kind: MemoryKind::Env,
            text: "Rust toolchain: 1.90.0".into(),
            source: "auto:env".into(),
            approved: true,
        })
        .unwrap();

    let big_block = SelectedBlock {
        command: Some("cargo test".into()),
        exit_code: Some(1),
        output: format!(
            "{}\nFINAL_ERROR_MARKER",
            "noise line 1234567890\n".repeat(25)
        ),
    };

    // 1. Excluded folder -> completely empty bundle.
    let excluded_patterns = vec!["*client-secret*".to_owned()];
    let excluded_bundle = assemble(&AssembleInput {
        cwd: Some(&repo),
        project_root: None,
        request: "help",
        budget_tokens: 1000,
        memory_exclude: &excluded_patterns,
        store: Some(&store),
        selected_blocks: std::slice::from_ref(&big_block),
        included_overrides: &[],
    });
    assert!(excluded_bundle.project_root.is_none());
    assert!(excluded_bundle.items.is_empty());

    // 2. Tight token budget (52 tokens): rule (30) + env (~6) leaves ~16 tokens for block tail.
    let base_input = AssembleInput {
        cwd: Some(&repo),
        project_root: None,
        request: "help",
        budget_tokens: 52,
        memory_exclude: &[],
        store: Some(&store),
        selected_blocks: std::slice::from_ref(&big_block),
        included_overrides: &[],
    };
    let mut bundle = assemble(&base_input);
    assert!(total_included_tokens(&bundle) <= 52);
    let block_before = bundle
        .items
        .iter()
        .find(|i| i.kind == ContextKind::Block)
        .expect("tail-trimmed block fits");
    assert!(block_before.text.contains("FINAL_ERROR_MARKER"));
    let len_before = block_before.text.len();

    // 3. Untick the big rule in the inspector and re-assemble -> frees 30 tokens, giving the
    //    block a longer tail while keeping the unticked rule listed with `included == false`.
    bundle.items[0].included = false;
    let reassembled = reassemble_with_bundle(&base_input, &bundle);
    assert!(!reassembled.items[0].included);
    assert_eq!(included_items(&reassembled).count(), 2);
    assert!(total_included_tokens(&reassembled) <= 52);
    let block_after = reassembled
        .items
        .iter()
        .find(|i| i.kind == ContextKind::Block)
        .expect("block still present");
    assert!(
        block_after.text.len() > len_before,
        "unticking rule should free budget for longer block tail ({} vs {})",
        block_after.text.len(),
        len_before
    );
}
