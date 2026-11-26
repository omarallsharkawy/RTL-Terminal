use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use twitty_store::{
    apply_migrations, db_path_unix, db_path_windows, escape_fts5_query, now_epoch_ms,
    resolve_project_root, DriveIndexEntry, InMemorySecretStore, MemoryKind, MemoryUpdate,
    Migration, NewCommand, NewMemory, SecretError, SecretStore, SecretValue, Store, StoreError,
    StoreHandle, SystemSecretStore, CURRENT_SCHEMA_VERSION, MIGRATIONS, SECRET_SERVICE,
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

        // 3. Opening a v3 DB with only v1 migrations must fail with NewerSchema without downgrading.
        let err = match Store::open_with_migrations(&p, MIGRATIONS) {
            Ok(_) => panic!("expected NewerSchema error"),
            Err(err) => err,
        };
        assert!(
            matches!(
                err,
                StoreError::NewerSchema {
                    found: 3,
                    supported: 1
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

    // 4. Verify apply_migrations on an in-memory connection directly as well.
    let mut mem = rusqlite::Connection::open_in_memory().unwrap();
    assert_eq!(apply_migrations(&mut mem, MIGRATIONS).unwrap(), 1);
    assert_eq!(apply_migrations(&mut mem, MIGRATIONS).unwrap(), 1);
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
