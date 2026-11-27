//! Window-free integration tests for command history recording from finished shell-integration
//! blocks into `twitty-store` off the UI thread.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use twitty::blocks::{Block, BlockList};
use twitty::config::TwittyConfig;
use twitty::history::{record_finished_block, should_record_command};
use twitty::shell_integration::Signal;
use twitty::terminal::Terminal;
use twitty::ui::settings_panel::{read_bool, write_bool, SettingId, SettingsSection};
use twitty_store::StoreHandle;

const A: &str = "\x1b]133;A\x07";
const B: &str = "\x1b]133;B\x07";
const C: &str = "\x1b]133;C\x07";

fn d(code: i32) -> String {
    format!("\x1b]133;D;{code}\x07")
}

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
            "twitty-history-wiring-{tag}-{}-{id}-{nanos}",
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

struct Harness {
    term: Terminal,
    blocks: BlockList,
    cwd: Option<PathBuf>,
    clock: Instant,
    store: StoreHandle,
    record_history: bool,
}

impl Harness {
    fn new() -> Self {
        Self {
            term: Terminal::new(80, 24, |_| {}),
            blocks: BlockList::new(),
            cwd: None,
            clock: Instant::now(),
            store: StoreHandle::open_in_memory(),
            record_history: true,
        }
    }

    fn feed(&mut self, bytes: &str) {
        self.term.process_bytes(bytes.as_bytes());
        for event in self.term.take_shell_events() {
            let running_id = self
                .blocks
                .blocks()
                .next_back()
                .filter(|b| b.is_running())
                .map(Block::id);
            self.blocks.on_event(&event, &self.term, self.clock);
            if let Signal::Cwd(path) = &event.signal {
                self.cwd = Some(path.clone());
            }
            if let Some(block) = running_id
                .and_then(|id| self.blocks.block_by_id(id))
                .filter(|b| !b.is_running())
            {
                record_finished_block(
                    &self.store,
                    self.record_history,
                    block,
                    self.cwd.as_deref(),
                    true,
                    Some("zsh"),
                );
            }
        }
    }
}

#[test]
fn finished_blocks_record_with_git_root_duration_exit_and_redaction() {
    let dir = TempDir::new("git");
    let repo = dir.path().join("workspace");
    let nested = repo.join("crates").join("app");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(&nested).unwrap();

    let mut h = Harness::new();
    h.cwd = Some(nested.clone());

    // Normal command via OSC 133 B..C with duration and exit code 2.
    h.feed(&format!("{A}$ {B}cargo test --workspace\r\n{C}failed\r\n"));
    h.clock += Duration::from_millis(350);
    h.feed(&d(2));

    // Command containing a secret via OSC 633;E -> redacted before SQLite storage.
    h.feed(&format!(
        "{A}$ {B}\x1b]633;E;curl -H 'Authorization: Bearer super_secret_tok' https://api.example.com\x07{C}ok\r\n"
    ));
    h.clock += Duration::from_millis(45);
    h.feed(&d(0));

    // Arabic command via OSC 633;E.
    h.feed(&format!(
        "{A}$ {B}\x1b]633;E;echo مرحبا بالعالم\x07{C}مرحبا بالعالم\r\n"
    ));
    h.clock += Duration::from_millis(12);
    h.feed(&d(0));

    // Duplicate D mark must not double-record.
    h.feed(&d(0));

    h.store.flush();
    let rows = h.store.recent_history(None, 10).unwrap();
    assert_eq!(rows.len(), 3, "expected 3 unique finished commands");

    // Newest first (all share close timestamps, tie-broken by id DESC).
    assert_eq!(rows[0].command, "echo مرحبا بالعالم");
    assert_eq!(rows[0].exit_code, Some(0));
    assert_eq!(rows[0].duration_ms, Some(12));
    assert_eq!(rows[0].shell.as_deref(), Some("zsh"));

    assert_eq!(
        rows[1].command,
        "curl -H 'Authorization: Bearer [REDACTED:auth_header]' https://api.example.com"
    );
    assert!(!rows[1].command.contains("super_secret_tok"));
    assert_eq!(rows[1].exit_code, Some(0));
    assert_eq!(rows[1].duration_ms, Some(45));

    assert_eq!(rows[2].command, "cargo test --workspace");
    assert_eq!(rows[2].exit_code, Some(2));
    assert_eq!(rows[2].duration_ms, Some(350));
    assert_eq!(rows[2].cwd, nested.to_string_lossy());
    assert_eq!(
        rows[2].project_root.as_deref(),
        Some(repo.to_string_lossy().as_ref())
    );

    // Search finds Arabic and English, never raw secret.
    assert_eq!(h.store.search_history("مرحبا", None, 10).unwrap().len(), 1);
    assert!(h
        .store
        .search_history("super_secret_tok", None, 10)
        .unwrap()
        .is_empty());
}

#[test]
fn leading_space_and_disabled_config_never_record() {
    assert!(!should_record_command(false, "ls -la"));
    assert!(!should_record_command(true, " secret-command"));
    assert!(!should_record_command(true, "   "));
    assert!(should_record_command(true, "ls -la"));

    let mut h = Harness::new();
    h.cwd = Some(PathBuf::from("/nonexistent-twitty-cwd/work"));

    // 1. Leading space via OSC 633;E (zsh/fish path) -> never recorded.
    h.feed(&format!(
        "{A}$ {B}\x1b]633;E; echo private_633\x07{C}private_633\r\n{}",
        d(0)
    ));
    // 2. Leading space via OSC 133 B..C grid read (bash path) -> never recorded.
    h.feed(&format!(
        "{A}$ {B} echo private_grid\r\n{C}private_grid\r\n{}",
        d(0)
    ));
    // 3. Config disabled -> never recorded.
    h.record_history = false;
    h.feed(&format!("{A}$ {B}ls -la\r\n{C}files\r\n{}", d(0)));

    // 4. Re-enabled normal command -> recorded with cwd as project_root (outside any git repo).
    h.record_history = true;
    h.feed(&format!("{A}$ {B}git status\r\n{C}clean\r\n{}", d(0)));

    h.store.flush();
    let rows = h.store.recent_history(None, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].command, "git status");
    assert_eq!(
        rows[0].project_root.as_deref(),
        Some("/nonexistent-twitty-cwd/work")
    );
}

#[test]
fn record_history_config_default_and_settings_behaviour_toggle() {
    let mut cfg = TwittyConfig::default();
    assert!(cfg.record_history, "record_history must default to true");

    assert_eq!(
        SettingId::RecordHistory.section(),
        SettingsSection::Behaviour
    );
    assert_eq!(
        SettingId::RecordHistory.config_key(),
        Some("record_history")
    );
    assert!(SettingId::in_section(SettingsSection::Behaviour).contains(&SettingId::RecordHistory));
    assert!(SettingId::config_settings().any(|id| id == SettingId::RecordHistory));

    let desc = SettingId::RecordHistory.descriptor();
    assert!(!desc
        .label(twitty::ui::chrome::UiLanguage::English)
        .is_empty());
    assert_ne!(
        desc.label(twitty::ui::chrome::UiLanguage::Arabic),
        desc.label(twitty::ui::chrome::UiLanguage::English)
    );

    assert!(read_bool(SettingId::RecordHistory, &cfg));
    assert!(write_bool(SettingId::RecordHistory, false, &mut cfg));
    assert!(!cfg.record_history);
    assert!(!read_bool(SettingId::RecordHistory, &cfg));
}
