use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use twitty_store::{
    apply_migrations, compute_frequent_commands, db_path_unix, db_path_windows, detect_project_env,
    escape_fts5_query, is_path_excluded_with_home, now_epoch_ms, resolve_project_root, sha256_hex,
    DriveIndexEntry, HistoryEntry, InMemorySecretStore, MemoryKind, MemoryUpdate, Migration,
    NewCommand, NewMemory, RulesFileState, SecretError, SecretStore, SecretValue, Store,
    StoreError, StoreHandle, SystemSecretStore, AUTO_COMMAND_SOURCE, AUTO_ENV_SOURCE,
    CURRENT_SCHEMA_VERSION, MIGRATIONS, SECRET_SERVICE,
};

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
            "twitty-store-test-{tag}-{}-{id}-{nanos}",
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
fn db_paths_follow_xdg_and_windows_conventions() {
    use std::ffi::OsStr;

    // Unix-style absolute paths are only absolute on Unix.
    if cfg!(unix) {
        assert_eq!(
            db_path_unix(
                Some(OsStr::new("/srv/data")),
                Some(OsStr::new("/home/omar"))
            ),
            Some(PathBuf::from("/srv/data/twitty/twitty.db"))
        );
        assert_eq!(
            db_path_unix(None, Some(OsStr::new("/home/omar"))),
            Some(PathBuf::from("/home/omar/.local/share/twitty/twitty.db"))
        );
        // Relative XDG_DATA_HOME is ignored and falls back to HOME.
        assert_eq!(
            db_path_unix(
                Some(OsStr::new("relative/data")),
                Some(OsStr::new("/home/omar"))
            ),
            Some(PathBuf::from("/home/omar/.local/share/twitty/twitty.db"))
        );
    }
    assert_eq!(db_path_unix(None, None), None);

    assert_eq!(
        db_path_windows(
            Some(OsStr::new("C:\\Users\\Omar\\AppData\\Local")),
            Some(OsStr::new("C:\\Users\\Omar\\AppData\\Roaming")),
        ),
        Some(PathBuf::from(
            "C:\\Users\\Omar\\AppData\\Local/twitty/twitty.db"
        ))
    );
    assert_eq!(
        db_path_windows(None, Some(OsStr::new("C:\\Users\\Omar\\AppData\\Roaming"))),
        Some(PathBuf::from(
            "C:\\Users\\Omar\\AppData\\Roaming/twitty/twitty.db"
        ))
    );
}

#[test]
fn file_db_pragmas_and_unix_permissions() {
    let dir = TempDir::new("perms");
    let db_path = dir.path().join("twitty").join("twitty.db");

    let store = Store::open(&db_path).expect("open fresh file db");
    assert_eq!(store.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
    assert_eq!(store.journal_mode().unwrap().to_lowercase(), "wal");
    assert!(store.foreign_keys_enabled().unwrap());
    assert!(!store.is_read_only());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir_mode = fs::metadata(db_path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let file_mode = fs::metadata(&db_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "db parent dir must be 0700");
        assert_eq!(file_mode, 0o600, "db file must be 0600");
    }

    // Re-opening is a no-op migration.
    drop(store);
    let reopened = Store::open(&db_path).expect("reopen existing db");
    assert_eq!(reopened.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
}

#[test]
fn migrations_fresh_intermediate_rerun_and_newer_schema() {
    // 1. Fresh DB -> latest real schema, then re-run is a no-op.
    let dir = TempDir::new("migrate");
    let db_path = dir.path().join("v1.db");
    let store = Store::open(&db_path).unwrap();
    assert_eq!(store.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
    drop(store);
    let store2 = Store::open(&db_path).unwrap();
    assert_eq!(store2.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
    drop(store2);

    // 2. Multi-step migration chain: fresh (0 -> 3) and every intermediate version (1 -> 3, 2 -> 3).
    const CHAIN: &[Migration] = &[
        Migration {
            version: 1,
            sql: MIGRATIONS[0].sql,
        },
        Migration {
            version: 2,
            sql: "ALTER TABLE drive_index ADD COLUMN notes TEXT NOT NULL DEFAULT '';",
        },
        Migration {
            version: 3,
            sql: "CREATE TABLE extra_meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);",
        },
    ];

    // Fresh 0 -> 3.
    let p_fresh = dir.path().join("chain_fresh.db");
    let s = Store::open_with_migrations(&p_fresh, CHAIN).unwrap();
    assert_eq!(s.schema_version().unwrap(), 3);
    drop(s);
    // Re-run at 3 -> no-op.
    let s = Store::open_with_migrations(&p_fresh, CHAIN).unwrap();
    assert_eq!(s.schema_version().unwrap(), 3);
    drop(s);

    // Every intermediate version (1 and 2) -> latest (3).
    for start_ver in 1..CHAIN.len() {
        let p = dir.path().join(format!("chain_from_{start_ver}.db"));
        let s_init = Store::open_with_migrations(&p, &CHAIN[..start_ver]).unwrap();
        assert_eq!(s_init.schema_version().unwrap(), start_ver as u32);
        // Seed a history row at intermediate version and verify it survives upgrade.
        s_init
            .record_command(&NewCommand {
                command: "cargo check".into(),
                exit_code: Some(0),
                duration_ms: Some(120),
                cwd: "/tmp/proj".into(),
                project_root: Some("/tmp/proj".into()),
                shell: Some("zsh".into()),
                started_at: 1000,
                finished_at: Some(1120),
            })
            .unwrap();
        drop(s_init);

        let s_upgraded = Store::open_with_migrations(&p, CHAIN).unwrap();
        assert_eq!(s_upgraded.schema_version().unwrap(), 3);
        let rows = s_upgraded.recent_history(None, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].command, "cargo check");
        drop(s_upgraded);

        // 3. Opening a v3 DB with only supported migrations must fail with NewerSchema without downgrading.
        let err = match Store::open_with_migrations(&p, MIGRATIONS) {
            Ok(_) => panic!("expected NewerSchema error"),
            Err(err) => err,
        };
        assert!(
            matches!(
                err,
                StoreError::NewerSchema {
                    found: 3,
                    supported: CURRENT_SCHEMA_VERSION,
                }
            ),
            "unexpected error: {err:?}"
        );
        // Verify version was NOT downgraded and the DB is still readable read-only.
        let ro = Store::open_read_only(&p).unwrap();
        assert!(ro.is_read_only());
        assert_eq!(ro.schema_version().unwrap(), 3);
        let rows = ro.recent_history(None, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].command, "cargo check");
    }

    // 4. Verify real v1 -> v2 upgrade preserves existing rows and creates rules_file_approvals.
    let v1_path = dir.path().join("upgrade_v1_to_v2.db");
    {
        let s_v1 = Store::open_with_migrations(&v1_path, &MIGRATIONS[..1]).unwrap();
        assert_eq!(s_v1.schema_version().unwrap(), 1);
        s_v1.create_memory(&NewMemory {
            project_root: "/repo/v1".into(),
            kind: MemoryKind::Rule,
            text: "Keep tests deterministic".into(),
            source: "user".into(),
            approved: true,
        })
        .unwrap();
    }
    {
        let s_v2 = Store::open(&v1_path).unwrap();
        assert_eq!(s_v2.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
        assert_eq!(s_v2.list_memory("/repo/v1", true, 10).unwrap().len(), 1);
        s_v2.approve_rules_file("/repo/v1", "AGENTS.md", "deadbeef")
            .unwrap();
        assert_eq!(s_v2.list_rules_approvals("/repo/v1").unwrap().len(), 1);
    }

    // 5. Verify apply_migrations on an in-memory connection directly as well.
    let mut mem = rusqlite::Connection::open_in_memory().unwrap();
    assert_eq!(
        apply_migrations(&mut mem, MIGRATIONS).unwrap(),
        CURRENT_SCHEMA_VERSION
    );
    assert_eq!(
        apply_migrations(&mut mem, MIGRATIONS).unwrap(),
        CURRENT_SCHEMA_VERSION
    );
}

#[test]
fn history_record_finish_redaction_and_fts_search() {
    let store = Store::open_in_memory().unwrap();
    let t0 = now_epoch_ms();

    let id1 = store
        .record_command(&NewCommand {
            command: "curl -H 'Authorization: Bearer topsecret999' https://api.example.com".into(),
            exit_code: None,
            duration_ms: None,
            cwd: "/home/omar/proj".into(),
            project_root: Some("/home/omar/proj".into()),
            shell: Some("zsh".into()),
            started_at: t0,
            finished_at: None,
        })
        .unwrap();
    assert!(store.finish_command(id1, Some(0), Some(345)).unwrap());
    assert!(!store.finish_command(999_999, Some(1), Some(10)).unwrap());

    let _id2 = store
        .record_command(&NewCommand {
            command: "echo مرحبا بالعالم && cargo test --workspace".into(),
            exit_code: Some(0),
            duration_ms: Some(900),
            cwd: "/home/omar/مشروع".into(),
            project_root: Some("/home/omar/مشروع".into()),
            shell: Some("bash".into()),
            started_at: t0 + 10,
            finished_at: Some(t0 + 910),
        })
        .unwrap();

    // Recent history: newest first, redacted on write.
    let recent = store.recent_history(None, 10).unwrap();
    assert_eq!(recent.len(), 2);
    assert_eq!(
        recent[0].command,
        "echo مرحبا بالعالم && cargo test --workspace"
    );
    assert_eq!(
        recent[1].command,
        "curl -H 'Authorization: Bearer [REDACTED:auth_header]' https://api.example.com"
    );
    assert!(!recent[1].command.contains("topsecret999"));
    assert_eq!(recent[1].exit_code, Some(0));
    assert_eq!(recent[1].duration_ms, Some(345));
    assert!(recent[1].finished_at.is_some());

    // Project scoping works.
    let scoped = store.recent_history(Some("/home/omar/proj"), 10).unwrap();
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].id, id1);

    // FTS5 search: English prefix, Arabic words, and redacted secret cannot be found.
    let hits = store.search_history("carg", None, 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].command.contains("cargo test"));

    let arabic_hits = store.search_history("مرحبا", None, 10).unwrap();
    assert_eq!(arabic_hits.len(), 1);
    assert_eq!(arabic_hits[0].cwd, "/home/omar/مشروع");

    let secret_hits = store.search_history("topsecret999", None, 10).unwrap();
    assert!(secret_hits.is_empty(), "raw secret must never be indexed");

    // Hostile FTS5 queries must never error or inject syntax.
    for hostile in [
        "\"",
        "\"\"\"",
        "*",
        "***",
        "NEAR(",
        "NEAR(a b, 5)",
        "-",
        "-cargo",
        "cargo OR NOT AND",
        "command:cargo",
        "(foo) ^bar +baz",
        "\0\x07\x1b",
        "مرحبا \"NEAR(*\" - اختبار",
    ] {
        let _ = escape_fts5_query(hostile);
        let res = store.search_history(hostile, None, 10);
        assert!(res.is_ok(), "hostile query {hostile:?} errored: {res:?}");
    }
}

#[test]
fn memory_crud_redaction_and_fts_sync() {
    let store = Store::open_in_memory().unwrap();

    let id = store
        .create_memory(&NewMemory {
            project_root: "/repo/app".into(),
            kind: MemoryKind::Fact,
            text: "Run tests with OPENAI_API_KEY=sk-proj-abcdefghijklmnopqrstuvwxyz0123456789 and cargo test".into(),
            source: "agent".into(),
            approved: false,
        })
        .unwrap();

    let row = store.get_memory(id).unwrap().expect("memory exists");
    assert_eq!(row.kind, MemoryKind::Fact);
    assert!(!row.approved);
    assert!(
        !row.text.contains("sk-proj-"),
        "secret must be redacted before storage: {}",
        row.text
    );
    assert!(row.text.contains("[REDACTED:env_secret]"));

    // Approval filter.
    assert!(store.list_memory("/repo/app", true, 10).unwrap().is_empty());
    assert_eq!(store.list_memory("/repo/app", false, 10).unwrap().len(), 1);
    assert!(store.set_memory_approved(id, true).unwrap());
    assert_eq!(store.list_memory("/repo/app", true, 10).unwrap().len(), 1);

    // Update text and verify FTS5 update trigger keeps index in sync.
    assert!(store
        .update_memory(
            id,
            &MemoryUpdate {
                kind: Some(MemoryKind::Rule),
                text: Some("دائماً شغّل الاختبارات باستخدام cargo test --workspace".into()),
                source: Some("user".into()),
                approved: Some(true),
            },
        )
        .unwrap());
    let updated = store.get_memory(id).unwrap().unwrap();
    assert_eq!(updated.kind, MemoryKind::Rule);
    assert_eq!(updated.source, "user");

    let hits = store
        .search_memory(Some("/repo/app"), "الاختبارات", 10)
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, id);

    // Old token removed from FTS5 after update.
    let stale = store
        .search_memory(Some("/repo/app"), "OPENAI", 10)
        .unwrap();
    assert!(stale.is_empty());

    // Delete removes from both table and FTS5.
    assert!(store.delete_memory(id).unwrap());
    assert!(store.get_memory(id).unwrap().is_none());
    assert!(store
        .search_memory(Some("/repo/app"), "الاختبارات", 10)
        .unwrap()
        .is_empty());
}

#[test]
fn drive_index_upsert_list_remove_and_redaction() {
    let store = Store::open_in_memory().unwrap();

    store
        .upsert_drive_index(&DriveIndexEntry {
            path: "workflows/deploy.workflow.toml".into(),
            kind: "workflow".into(),
            title: "Deploy with ghp_1234567890abcdef1234567890abcdef1234".into(),
            updated_at: 100,
            hash: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
        })
        .unwrap();
    store
        .upsert_drive_index(&DriveIndexEntry {
            path: "prompts/review.prompt.md".into(),
            kind: "prompt".into(),
            title: "مراجعة الكود".into(),
            updated_at: 200,
            hash: "0123456789abcdef".into(),
        })
        .unwrap();

    let all = store.list_drive_index(None).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].path, "prompts/review.prompt.md");
    assert_eq!(all[1].title, "Deploy with [REDACTED:github_token]");
    // SHA-256 hex hash stays intact.
    assert_eq!(
        all[1].hash,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );

    // Upsert updates existing row.
    store
        .upsert_drive_index(&DriveIndexEntry {
            path: "workflows/deploy.workflow.toml".into(),
            kind: "workflow".into(),
            title: "Deploy production".into(),
            updated_at: 300,
            hash: "abcdef0123456789".into(),
        })
        .unwrap();
    let workflows = store.list_drive_index(Some("workflow")).unwrap();
    assert_eq!(workflows.len(), 1);
    assert_eq!(workflows[0].title, "Deploy production");

    assert!(store
        .remove_drive_index("workflows/deploy.workflow.toml")
        .unwrap());
    assert!(!store
        .remove_drive_index("workflows/deploy.workflow.toml")
        .unwrap());
    assert_eq!(store.list_drive_index(None).unwrap().len(), 1);
}

#[test]
fn store_handle_background_worker_and_graceful_degradation() {
    let dir = TempDir::new("worker");
    let repo = dir.path().join("my-repo");
    let sub = repo.join("src").join("nested");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(&sub).unwrap();

    let handle = StoreHandle::open_in_memory();
    let clone = handle.clone();
    clone.record_finished_block_async(
        "cargo test --workspace".into(),
        Some(0),
        Some(420),
        sub.clone(),
        true,
        Some("bash".into()),
        1000,
        Some(1420),
    );
    handle.flush();

    let rows = handle.recent_history(None, 5).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].command, "cargo test --workspace");
    assert_eq!(
        rows[0].project_root.as_deref(),
        Some(repo.to_string_lossy().as_ref())
    );
    assert_eq!(rows[0].cwd, sub.to_string_lossy());

    // Outside any git repo: project_root falls back to cwd.
    // Use an absolute path outside `/tmp` in case the host has a stray `/tmp/.git`.
    let outside = PathBuf::from("/nonexistent-twitty-root/plain-dir");
    assert_eq!(resolve_project_root(&outside), outside);
    assert_eq!(resolve_project_root(&sub), repo);

    // Graceful degradation when DB path cannot be opened (e.g. directory path used as DB file).
    let broken_path = dir.path().join("is-a-dir");
    fs::create_dir_all(&broken_path).unwrap();
    let degraded = StoreHandle::open(broken_path);
    degraded.record_finished_block_async(
        "ls".into(),
        Some(0),
        Some(5),
        outside,
        true,
        None,
        10,
        Some(15),
    );
    degraded.flush();
    assert!(matches!(
        degraded.recent_history(None, 5),
        Err(StoreError::Unavailable)
    ));
}

#[test]
fn secret_store_crud_and_debug_never_leaks_secrets() {
    let store = InMemorySecretStore::new();
    assert_eq!(store.service(), SECRET_SERVICE);

    let secret_str = "sk-ant-api03-super-secret-value-never-log-me";
    assert_eq!(store.get("anthropic").unwrap(), None);
    assert!(!store.delete("anthropic").unwrap());

    store.set("anthropic", secret_str).unwrap();
    let got = store.get("anthropic").unwrap().expect("secret stored");
    assert_eq!(got.expose(), secret_str);

    // Debug output on SecretValue, InMemorySecretStore, and SystemSecretStore must never contain
    // the secret material.
    let val_dbg = format!("{got:?}");
    let store_dbg = format!("{store:?}");
    let sys_dbg = format!("{:?}", SystemSecretStore::new());
    assert!(
        !val_dbg.contains(secret_str),
        "SecretValue leaked: {val_dbg}"
    );
    assert!(
        !store_dbg.contains(secret_str),
        "InMemorySecretStore leaked: {store_dbg}"
    );
    assert!(val_dbg.contains("REDACTED"), "val_dbg={val_dbg}");
    assert!(sys_dbg.contains(SECRET_SERVICE), "sys_dbg={sys_dbg}");

    // Empty account rejected.
    assert_eq!(store.get(""), Err(SecretError::InvalidAccount));
    assert_eq!(store.set("   ", "x"), Err(SecretError::InvalidAccount));
    assert_eq!(store.delete(""), Err(SecretError::InvalidAccount));

    // Delete removes the secret cleanly.
    assert!(store.delete("anthropic").unwrap());
    assert_eq!(store.get("anthropic").unwrap(), None);

    // Trait object ergonomics.
    let dyn_store: Box<dyn SecretStore> = Box::new(store);
    dyn_store.set("openai", "sk-test-val").unwrap();
    assert_eq!(
        dyn_store.get("openai").unwrap(),
        Some(SecretValue::new("sk-test-val"))
    );
}

#[test]
fn project_exclusion_matches_paths_tilde_and_globs() {
    let home = Path::new("/home/omar");
    let patterns: Vec<String> = vec![
        "~/secrets".into(),
        "/work/clients/*".into(),
        "**/private/**".into(),
        "*-confidential".into(),
    ];

    assert!(is_path_excluded_with_home(
        Path::new("/home/omar/secrets"),
        &patterns,
        Some(home)
    ));
    assert!(is_path_excluded_with_home(
        Path::new("/home/omar/secrets/subrepo"),
        &patterns,
        Some(home)
    ));
    assert!(!is_path_excluded_with_home(
        Path::new("/home/omar/secrets2"),
        &patterns,
        Some(home)
    ));
    assert!(is_path_excluded_with_home(
        Path::new("/work/clients/acme/service"),
        &patterns,
        Some(home)
    ));
    assert!(is_path_excluded_with_home(
        Path::new("/a/b/private/c/d"),
        &patterns,
        Some(home)
    ));
    assert!(is_path_excluded_with_home(
        Path::new("/srv/app-confidential/repo"),
        &patterns,
        Some(home)
    ));
    assert!(!is_path_excluded_with_home(
        Path::new("/home/omar/Projects/twitty"),
        &patterns,
        Some(home)
    ));
}

#[test]
fn rules_files_require_sha256_approval_and_silently_lapse_on_change() {
    let dir = TempDir::new("rules");
    let root = dir.path();
    fs::create_dir_all(root.join(".twitty")).unwrap();

    let agents_v1 = format!(
        "# Rules\nNever leak {}\nAlways run tests.",
        concat!("gh", "p_1234567890abcdef1234567890abcdef1234")
    );
    fs::write(root.join("AGENTS.md"), &agents_v1).unwrap();
    fs::write(root.join("CLAUDE.md"), "Use cargo fmt").unwrap();
    fs::write(root.join(".twitty/rules.md"), "قواعد عربية للمشروع").unwrap();

    let store = Store::open_in_memory().unwrap();
    let key = root.to_string_lossy().into_owned();

    // Initially nothing is approved; content must NOT be read.
    let initial = store.inspect_rules_files(root).unwrap();
    assert_eq!(initial.len(), 3);
    assert_eq!(initial[0].rel_path, "AGENTS.md");
    assert_eq!(initial[1].rel_path, "CLAUDE.md");
    assert_eq!(initial[2].rel_path, ".twitty/rules.md");
    for f in &initial {
        assert_eq!(f.state, RulesFileState::Unapproved);
        assert!(f.approved_content.is_none());
    }

    // Approve AGENTS.md with its current SHA-256 -> content is returned redacted.
    let sha_v1 = sha256_hex(agents_v1.as_bytes());
    assert_eq!(initial[0].current_sha256, sha_v1);
    store
        .approve_rules_file(&key, "AGENTS.md", &sha_v1)
        .unwrap();

    let after_approve = store.inspect_rules_files(root).unwrap();
    assert_eq!(after_approve[0].state, RulesFileState::Approved);
    let content = after_approve[0]
        .approved_content
        .as_deref()
        .expect("approved content returned");
    assert!(content.contains("[REDACTED:github_token]"));
    assert!(!content.contains("1234567890abcdef"));
    assert_eq!(after_approve[1].state, RulesFileState::Unapproved);
    assert!(after_approve[1].approved_content.is_none());

    // Modify AGENTS.md on disk -> approval silently lapses to Changed and content is withheld!
    let agents_v2 = "# Rules v2\nChanged content";
    fs::write(root.join("AGENTS.md"), agents_v2).unwrap();
    let after_change = store.inspect_rules_files(root).unwrap();
    assert_eq!(after_change[0].state, RulesFileState::Changed);
    assert!(after_change[0].approved_content.is_none());
    assert_eq!(
        after_change[0].approved_sha256.as_deref(),
        Some(sha_v1.as_str())
    );
    assert_ne!(after_change[0].current_sha256, sha_v1);

    // Re-approve with new SHA-256 -> Approved again.
    let sha_v2 = sha256_hex(agents_v2.as_bytes());
    store
        .approve_rules_file(&key, "AGENTS.md", &sha_v2)
        .unwrap();
    let reapproved = store.inspect_rules_files(root).unwrap();
    assert_eq!(reapproved[0].state, RulesFileState::Approved);
    assert_eq!(
        reapproved[0].approved_content.as_deref(),
        Some("# Rules v2\nChanged content")
    );

    // Revoke -> Unapproved.
    assert!(store.revoke_rules_file(&key, "AGENTS.md").unwrap());
    let revoked = store.inspect_rules_files(root).unwrap();
    assert_eq!(revoked[0].state, RulesFileState::Unapproved);
    assert!(revoked[0].approved_content.is_none());

    // Symlink escaping project root must be ignored.
    #[cfg(unix)]
    {
        let outside = TempDir::new("outside-rules");
        let outside_file = outside.path().join("secret.md");
        fs::write(&outside_file, "outside secret").unwrap();
        std::os::unix::fs::symlink(&outside_file, root.join("TWITTY.md")).unwrap();
        let inspected = store.inspect_rules_files(root).unwrap();
        assert!(
            !inspected.iter().any(|f| f.rel_path == "TWITTY.md"),
            "symlink escaping project root must be skipped"
        );
    }
}

#[test]
fn env_notes_detection_and_refresh_preserve_user_notes() {
    let dir = TempDir::new("env");
    let root = dir.path();

    fs::write(
        root.join("rust-toolchain.toml"),
        "[toolchain]\nchannel = \"1.90.0\"\ncomponents = [\"clippy\", \"rustfmt\"]\n",
    )
    .unwrap();
    fs::write(
        root.join("mise.toml"),
        "[tools]\nnode = \"22\"\npython = \"3.12\"\n",
    )
    .unwrap();
    fs::write(root.join(".tool-versions"), "golang 1.23.1\n").unwrap();
    fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
    fs::write(root.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();

    let detected = detect_project_env(root);
    assert_eq!(detected.len(), 4);
    assert!(detected[0].text.contains("Rust toolchain: 1.90.0"));
    assert!(detected[0].text.contains("clippy, rustfmt"));
    assert!(detected[1]
        .text
        .contains("mise tools: node 22, python 3.12"));
    assert!(detected[2].text.contains(".tool-versions: golang 1.23.1"));
    assert!(detected[3].text.contains("Package managers: cargo, pnpm"));

    let store = Store::open_in_memory().unwrap();
    let key = root.to_string_lossy().into_owned();
    // Add a user-authored env note; refresh must preserve it.
    store
        .create_memory(&NewMemory {
            project_root: key.clone(),
            kind: MemoryKind::Env,
            text: "Uses Mesa llvmpipe for golden tests".into(),
            source: "user".into(),
            approved: true,
        })
        .unwrap();

    let refreshed = store.refresh_env_notes(root).unwrap();
    assert_eq!(refreshed.len(), 5);
    assert!(refreshed
        .iter()
        .any(|e| e.source == "user" && e.text.contains("llvmpipe")));
    assert_eq!(
        refreshed
            .iter()
            .filter(|e| e.source == AUTO_ENV_SOURCE)
            .count(),
        4
    );

    // Update rust-toolchain.toml -> refresh replaces only auto:env entries cleanly.
    fs::write(root.join("rust-toolchain.toml"), "channel = \"nightly\"\n").unwrap();
    let refreshed2 = store.refresh_env_notes(root).unwrap();
    assert!(refreshed2
        .iter()
        .any(|e| e.text.contains("Rust toolchain: nightly")));
    assert!(!refreshed2.iter().any(|e| e.text.contains("1.90.0")));
    assert!(refreshed2
        .iter()
        .any(|e| e.source == "user" && e.text.contains("llvmpipe")));
}

#[test]
fn frequent_commands_compute_success_rates_filter_noise_and_forget_project() {
    let store = Store::open_in_memory().unwrap();
    let proj = "/repo/alpha";
    let other = "/repo/beta";

    // Trivial commands should be filtered out even when run frequently.
    for i in 0..5 {
        store
            .record_command(&NewCommand {
                command: "ls -la".into(),
                exit_code: Some(0),
                duration_ms: Some(5),
                cwd: proj.into(),
                project_root: Some(proj.into()),
                shell: Some("bash".into()),
                started_at: 100 + i,
                finished_at: Some(105 + i),
            })
            .unwrap();
    }
    // `cargo test --workspace`: 4 runs, 3 succeeded (75%).
    for (idx, code) in [0, 0, 1, 0].into_iter().enumerate() {
        store
            .record_command(&NewCommand {
                command: "cargo   test  --workspace".into(),
                exit_code: Some(code),
                duration_ms: Some(200),
                cwd: proj.into(),
                project_root: Some(proj.into()),
                shell: Some("bash".into()),
                started_at: 200 + idx as i64,
                finished_at: Some(400 + idx as i64),
            })
            .unwrap();
    }
    // `cargo clippy`: 2 runs, 2 succeeded (100%).
    for idx in 0..2 {
        store
            .record_command(&NewCommand {
                command: "cargo clippy".into(),
                exit_code: Some(0),
                duration_ms: Some(100),
                cwd: proj.into(),
                project_root: Some(proj.into()),
                shell: Some("bash".into()),
                started_at: 300 + idx,
                finished_at: Some(400 + idx),
            })
            .unwrap();
    }
    // Single run command -> below min_runs=2.
    store
        .record_command(&NewCommand {
            command: "cargo doc".into(),
            exit_code: Some(0),
            duration_ms: Some(50),
            cwd: proj.into(),
            project_root: Some(proj.into()),
            shell: Some("bash".into()),
            started_at: 500,
            finished_at: Some(550),
        })
        .unwrap();
    // Unrelated project must stay untouched by forget_project.
    store
        .create_memory(&NewMemory {
            project_root: other.into(),
            kind: MemoryKind::Fact,
            text: "Beta fact".into(),
            source: "user".into(),
            approved: true,
        })
        .unwrap();

    let freq = store.frequent_commands(proj, 2, 10).unwrap();
    assert_eq!(freq.len(), 2);
    assert_eq!(freq[0].command, "cargo test --workspace");
    assert_eq!(freq[0].runs, 4);
    assert_eq!(freq[0].success_rate_pct, Some(75));
    assert_eq!(freq[1].command, "cargo clippy");
    assert_eq!(freq[1].runs, 2);
    assert_eq!(freq[1].success_rate_pct, Some(100));

    let synced = store.sync_frequent_commands(proj, 2, 10).unwrap();
    assert_eq!(synced.len(), 2);
    assert!(synced.iter().all(|e| e.source == AUTO_COMMAND_SOURCE));

    store
        .approve_rules_file(proj, "AGENTS.md", "abc123")
        .unwrap();
    let report = store.forget_project(proj).unwrap();
    assert_eq!(report.memories_deleted, 2);
    assert_eq!(report.rules_approvals_deleted, 1);
    assert!(report.history_deleted >= 12);

    assert!(store.list_memory(proj, false, 10).unwrap().is_empty());
    assert!(store.list_rules_approvals(proj).unwrap().is_empty());
    assert!(store.recent_history(Some(proj), 10).unwrap().is_empty());
    // Other project completely untouched!
    assert_eq!(store.list_memory(other, true, 10).unwrap().len(), 1);

    // Pure helper sanity check with empty input.
    let empty: Vec<HistoryEntry> = Vec::new();
    assert!(compute_frequent_commands(&empty, 2, 5).is_empty());
}

#[test]
fn worker_respects_exclusions_on_history_and_env_refresh() {
    let dir = TempDir::new("worker-excl");
    let repo = dir.path().join("secret-repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::write(repo.join("Cargo.toml"), "[package]\nname=\"s\"\n").unwrap();

    let handle = StoreHandle::open_in_memory();
    let excludes = vec![repo.to_string_lossy().into_owned()];

    handle.record_finished_block_with_excludes_async(
        "cargo test".into(),
        Some(0),
        Some(10),
        repo.clone(),
        true,
        excludes.clone(),
        Some("bash".into()),
        100,
        Some(110),
    );
    handle.refresh_env_notes_async(repo.clone(), excludes.clone());
    handle.flush();

    assert!(handle.recent_history(None, 10).unwrap().is_empty());
    assert!(handle
        .list_memory(repo.to_string_lossy(), false, 10)
        .unwrap()
        .is_empty());

    let snap = handle.load_project_snapshot(repo, excludes).unwrap();
    assert!(snap.excluded);
    assert!(snap.entries.is_empty());
    assert!(snap.rules_files.is_empty());
}

#[test]
fn sha256_matches_fips_180_test_vectors() {
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
    assert_eq!(
        sha256_hex(&vec![b'a'; 1_000_000]),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
    // Lengths around the 55/56/64-byte padding boundaries.
    for len in [55usize, 56, 63, 64, 65, 119, 120] {
        assert_eq!(sha256_hex(&vec![0u8; len]).len(), 64);
    }
}

#[cfg(unix)]
#[test]
fn open_rejects_symlinks_and_preserves_existing_non_twitty_parent_mode() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let dir = TempDir::new("symlink-guard");
    let shared = dir.path().join("shared");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).unwrap();

    // Opening directly in an existing non-"twitty" directory must not silently chmod 0700 it.
    let db = shared.join("custom.db");
    let _store = Store::open(&db).expect("open in existing dir");
    let mode = fs::metadata(&shared).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o755,
        "existing non-twitty parent mode must be preserved"
    );

    // Symlink DB path must be refused.
    let real_target = dir.path().join("real.db");
    fs::write(&real_target, b"").unwrap();
    let link_db = dir.path().join("link.db");
    symlink(&real_target, &link_db).unwrap();
    assert!(matches!(
        Store::open(&link_db),
        Err(StoreError::InvalidInput(_))
    ));

    // Symlink parent directory must be refused.
    let link_parent = dir.path().join("link_dir");
    symlink(&shared, &link_parent).unwrap();
    assert!(matches!(
        Store::open(&link_parent.join("twitty.db")),
        Err(StoreError::InvalidInput(_))
    ));
}
