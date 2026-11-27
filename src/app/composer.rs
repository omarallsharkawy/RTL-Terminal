//! App-level hooks for the command composer: visibility lifecycle, off-thread shell history,
//! directory listing, `$PATH` / path-stat, and dynamic generator loads, keyboard/IME/mouse
//! routing, and building the per-frame `ComposerView`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

use alacritty_terminal::term::TermMode;
use winit::event::{KeyEvent, MouseButton};
use winit::keyboard::PhysicalKey;
use winit::window::CursorIcon;

use super::{App, AppEvent};
use crate::composer::complete::{read_dir_entries, DirEntryInfo, MAX_DIR_ENTRIES};
use crate::composer::generators::{
    run_generator, scan_path_executables, GeneratedItem, GeneratorCache, GeneratorKey,
};
use crate::composer::highlight::{
    extract_candidate_paths, highlight_command, resolve_shell_path, HighlightContext,
};
use crate::composer::history::{load_shell_history_default, HistoryStore};
use crate::composer::lint::lint_command;
use crate::composer::slash::{self, Foreground};
use crate::composer::{
    encode_submit, should_show_composer, ComposerAction, ComposerMode, ComposerState,
};
use crate::input::route::shortcut_is_idle;
use crate::input::{key_shortcut_with, InputAction};
use crate::layout::PaneId;
use crate::pty::Pty;
use crate::render::scene::composer::{
    ComposerHit, ComposerLayout, ComposerPopupItem, ComposerPopupView, ComposerView,
};
use crate::shell_integration::MarkKind;
use crate::workflows::WorkflowSet;

const MAX_PATH_STAT_CACHE: usize = 256;
/// Skills and MCP configs change rarely; rescan at most this often per project.
const SLASH_CATALOG_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// Window-wide composer state owned by `App`.
#[derive(Default)]
pub(super) struct ComposerHost {
    pub state: ComposerState,
    pub history: HistoryStore,
    history_requested: bool,
    manual_open: bool,
    dismissed_for_prompt: bool,
    focused: bool,
    workflows: WorkflowSet,
    workflows_cwd: Option<PathBuf>,
    dir_cache: Option<(PathBuf, Vec<DirEntryInfo>)>,
    dir_pending: Option<PathBuf>,
    generator_cache: GeneratorCache,
    generators_pending: HashSet<GeneratorKey>,
    path_commands: Option<HashSet<String>>,
    path_commands_requested: bool,
    /// Agent slash catalog: project root it was built for, when, and whether a scan runs.
    slash_root: Option<Option<PathBuf>>,
    slash_loaded_at: Option<Instant>,
    slash_pending: bool,
    /// A slash-menu command asked to close the window (e.g. it closed the last pane).
    exit_requested: bool,
    path_exists_cache: HashMap<PathBuf, bool>,
    path_exists_pending: HashSet<PathBuf>,
    last_has_lints: std::cell::Cell<bool>,
    /// Persistent fast-filesystem worker lane (dir listings, path stats, history, PATH, slash).
    fs_worker: Option<mpsc::Sender<FsJob>>,
    /// Persistent subprocess generator lane (git/docker/systemctl/etc.) so slow commands never
    /// stall fast filesystem completions.
    gen_worker: Option<mpsc::Sender<GeneratorKey>>,
}

enum FsJob {
    Slash {
        home: PathBuf,
        root: Option<PathBuf>,
    },
    History,
    PathCommands,
    Dir(PathBuf),
    PathStats(Vec<PathBuf>),
}

impl ComposerHost {
    fn send_fs(&mut self, proxy: &winit::event_loop::EventLoopProxy<AppEvent>, job: FsJob) -> bool {
        if let Some(tx) = &self.fs_worker {
            if tx.send(job).is_ok() {
                return true;
            }
            self.fs_worker = None;
            return false;
        }
        let (tx, rx) = mpsc::channel::<FsJob>();
        let proxy = proxy.clone();
        let spawned = std::thread::Builder::new()
            .name("twitty-composer-fs".into())
            .spawn(move || {
                while let Ok(first) = rx.recv() {
                    // Drain any queued duplicate/stale Dir jobs to avoid redundant listings.
                    let mut pending = vec![first];
                    while let Ok(more) = rx.try_recv() {
                        pending.push(more);
                    }
                    let last_dir = pending.iter().rposition(|j| matches!(j, FsJob::Dir(_)));
                    for (idx, job) in pending.into_iter().enumerate() {
                        match job {
                            FsJob::Slash { home, root } => {
                                let catalog = slash::discover(
                                    &home,
                                    root.as_deref(),
                                    &slash::installed_agents(),
                                );
                                let _ =
                                    proxy.send_event(AppEvent::ComposerSlash(Box::new(catalog)));
                            }
                            FsJob::History => {
                                let entries = load_shell_history_default();
                                let _ = proxy.send_event(AppEvent::ComposerHistory(entries));
                            }
                            FsJob::PathCommands => {
                                let cmds = scan_path_executables();
                                let _ = proxy.send_event(AppEvent::ComposerPathInfo {
                                    commands: Some(cmds),
                                    existing: Vec::new(),
                                });
                            }
                            FsJob::Dir(dir) => {
                                if Some(idx) == last_dir {
                                    let entries = read_dir_entries(&dir, MAX_DIR_ENTRIES);
                                    let _ = proxy.send_event(AppEvent::ComposerDir(dir, entries));
                                }
                            }
                            FsJob::PathStats(paths) => {
                                let existing = paths
                                    .into_iter()
                                    .map(|p| {
                                        let ok = std::fs::symlink_metadata(&p).is_ok();
                                        (p, ok)
                                    })
                                    .collect();
                                let _ = proxy.send_event(AppEvent::ComposerPathInfo {
                                    commands: None,
                                    existing,
                                });
                            }
                        }
                    }
                }
            });
        if spawned.is_err() {
            return false;
        }
        let ok = tx.send(job).is_ok();
        if ok {
            self.fs_worker = Some(tx);
        }
        ok
    }

    fn send_gen(
        &mut self,
        proxy: &winit::event_loop::EventLoopProxy<AppEvent>,
        key: GeneratorKey,
    ) -> bool {
        if let Some(tx) = &self.gen_worker {
            if tx.send(key).is_ok() {
                return true;
            }
            self.gen_worker = None;
            return false;
        }
        let (tx, rx) = mpsc::channel::<GeneratorKey>();
        let proxy = proxy.clone();
        let spawned = std::thread::Builder::new()
            .name("twitty-composer-gen".into())
            .spawn(move || {
                while let Ok(key) = rx.recv() {
                    let items = run_generator(key.kind, key.cwd.as_deref());
                    let _ = proxy.send_event(AppEvent::ComposerGenerator(key, items));
                }
            });
        if spawned.is_err() {
            return false;
        }
        let ok = tx.send(key).is_ok();
        if ok {
            self.gen_worker = Some(tx);
        }
        ok
    }
}

impl App {
    /// Whether the composer should currently be drawn on the focused pane.
    pub(super) fn composer_visible(&self) -> bool {
        let mode = self.config.composer_mode();
        if mode == ComposerMode::Off {
            return false;
        }
        let Some(id) = self.focused_id() else {
            return false;
        };
        let Some(pane) = self.ws.pane(id) else {
            return false;
        };
        let alt_screen = pane.mode().contains(TermMode::ALT_SCREEN);
        let running =
            pane.meta.running || pane.pty.as_ref().is_some_and(Pty::has_foreground_program);
        should_show_composer(
            mode,
            self.composer.manual_open,
            self.composer.dismissed_for_prompt,
            pane.meta.prompt_state,
            running,
            alt_screen,
        )
    }

    /// Whether keyboard and IME events currently belong to the composer.
    pub(super) fn composer_has_keys(&self) -> bool {
        self.composer_visible() && self.composer.focused && !self.palette_open()
    }

    /// `composer.open` / `Ctrl+Shift+Space`: open and focus the composer, or close it if it is
    /// already open and focused.
    pub(super) fn toggle_composer(&mut self) {
        let mode = self.config.composer_mode();
        if mode == ComposerMode::Off {
            return;
        }
        if self.composer_visible() && self.composer.focused {
            self.close_composer();
            return;
        }
        self.reset_pointer_gesture();
        self.composer.manual_open = true;
        self.composer.dismissed_for_prompt = false;
        self.composer.focused = true;
        self.composer.state.ai_enabled = self.config.ai;
        self.prepare_composer_sources();
        self.cursor_blink.reset(Instant::now());
        self.window.request_redraw();
    }

    /// Open the composer populated with a suggested AI fix command for review (NEVER auto-runs).
    pub(super) fn open_composer_with_ai_fix(&mut self, command: &str) {
        if self.config.composer_mode() == ComposerMode::Off {
            return;
        }
        self.reset_pointer_gesture();
        self.composer.manual_open = true;
        self.composer.dismissed_for_prompt = false;
        self.composer.focused = true;
        self.composer.state.ai_enabled = self.config.ai;
        self.composer.state.apply_ai_command(command);
        let explanation = self
            .window
            .ui_language()
            .t("Suggested fix — review before running")
            .to_owned();
        self.ai.composer_ai = crate::ai::ComposerAiState::Ready {
            explanation,
            original_prompt: String::new(),
        };
        self.prepare_composer_sources();
        self.cursor_blink.reset(Instant::now());
        self.window.request_redraw();
    }

    /// Open the composer pre-filled with `command` (e.g. from a parameterized workflow via
    /// `Alt+Enter`) so the user can review/edit before executing.
    pub(super) fn open_composer_with_command(&mut self, command: &str) {
        if self.config.composer_mode() == ComposerMode::Off {
            if let Some(id) = self.focused_id() {
                let bracketed = self.mode_of(id).contains(TermMode::BRACKETED_PASTE);
                let bytes = crate::workflows::encode_insert(command, bracketed);
                self.send_input(id, &bytes);
                self.cursor_blink.reset(Instant::now());
            }
            return;
        }
        self.reset_pointer_gesture();
        self.clear_composer_ai_state();
        self.composer.manual_open = true;
        self.composer.dismissed_for_prompt = false;
        self.composer.focused = true;
        self.composer.state.ai_enabled = self.config.ai;
        self.composer.state.apply_ai_command(command);
        self.prepare_composer_sources();
        self.cursor_blink.reset(Instant::now());
        self.window.request_redraw();
    }

    pub(super) fn close_composer(&mut self) {
        self.clear_composer_ai_state();
        let mode = self.config.composer_mode();
        self.composer.manual_open = false;
        if mode == ComposerMode::Always {
            self.composer.dismissed_for_prompt = true;
        }
        self.composer.focused = false;
        self.composer.state.popup = None;
        self.composer.state.editor.set_preedit(None);
        if self.preedit_active {
            self.preedit_active = false;
        }
        self.window.request_redraw();
    }

    /// Apply off-thread loaded workflows to the composer and refresh ghost suggestions.
    pub(super) fn on_composer_workflows_loaded(
        &mut self,
        cwd: Option<PathBuf>,
        workflows: WorkflowSet,
    ) {
        self.composer.workflows = workflows;
        self.composer.workflows_cwd = cwd;
        self.composer.state.refresh_ghost(
            self.composer.history.entries(),
            self.composer.workflows.workflows(),
        );
    }

    /// Reload saved commands on the next composer refresh (after Settings → Commands edits).
    pub(super) fn invalidate_composer_workflows(&mut self) {
        self.workflows_cache = None;
        self.composer.workflows_cwd = None;
        self.composer.workflows = WorkflowSet::default();
    }

    /// Called when focus moves to another pane or tab.
    pub(super) fn on_composer_focus_changed(&mut self) {
        self.composer.manual_open = false;
        self.composer.dismissed_for_prompt = false;
        self.composer.state.popup = None;
        self.composer.state.editor.set_preedit(None);
        self.composer.dir_cache = None;
        self.composer.dir_pending = None;
        self.composer.workflows_cwd = None;
        self.composer.path_exists_cache.clear();
        self.composer.path_exists_pending.clear();
        if self.composer_visible() {
            self.composer.focused = true;
            self.prepare_composer_sources();
        } else {
            self.composer.focused = false;
        }
    }

    /// Called after an OSC 133 mark or command line arrives on pane `id`.
    pub(super) fn on_composer_shell_mark(&mut self, id: PaneId, kind: MarkKind) {
        if matches!(kind, MarkKind::CommandEnd { .. }) {
            if let Some(cmd) = self
                .ws
                .pane(id)
                .and_then(|p| p.blocks.blocks().next_back())
                .and_then(|b| b.command.clone())
            {
                self.composer.history.push_session(&cmd);
            }
            if let Some(cwd) = self.pane_cwd(id) {
                self.composer.generator_cache.invalidate_cwd(&cwd);
            }
            self.composer.path_exists_cache.clear();
        }
        if self.focused_id() != Some(id) {
            return;
        }
        match kind {
            MarkKind::PromptStart => {
                self.composer.dismissed_for_prompt = false;
                if self.config.composer_mode() == ComposerMode::Always && self.composer_visible() {
                    self.composer.focused = true;
                    self.prepare_composer_sources();
                }
            }
            MarkKind::OutputStart => {
                if self.config.composer_mode() == ComposerMode::Always {
                    self.composer.manual_open = false;
                    self.composer.state.popup = None;
                    self.composer.state.editor.set_preedit(None);
                }
            }
            MarkKind::CommandStart | MarkKind::CommandEnd { .. } => {}
        }
    }

    /// Record an explicit OSC 633;E command line in session history.
    pub(super) fn on_composer_command_line(&mut self, line: &str) {
        self.composer.history.push_session(line);
    }

    fn prepare_composer_sources(&mut self) {
        self.harvest_block_history();
        self.ensure_shell_history_loaded();
        self.ensure_path_commands_loaded();
        let cwd = self.focused_id().and_then(|id| self.pane_cwd(id));
        if self.composer.workflows_cwd != cwd || self.composer.workflows.workflows().is_empty() {
            self.composer.workflows = self.cached_workflows(cwd.as_deref());
            self.composer.workflows_cwd = cwd.clone();
            self.request_workflows_load(cwd.clone());
        }
        self.composer.state.refresh_ghost(
            self.composer.history.entries(),
            self.composer.workflows.workflows(),
        );
        self.maybe_request_composer_dir(cwd.as_deref());
        self.maybe_request_composer_generators(cwd.as_deref());
        self.maybe_request_composer_path_stats(cwd.as_deref());
        let argv = self
            .focused_id()
            .and_then(|id| self.ws.pane(id))
            .and_then(|pane| pane.pty.as_ref())
            .and_then(Pty::foreground_argv);
        self.composer.state.slash_foreground = Foreground::from_argv(argv.as_deref());
        self.maybe_request_slash_catalog(cwd.as_deref());
    }

    /// Rescan skills, agent commands and MCP servers for the pane's project when the project
    /// changed or the last scan is older than [`SLASH_CATALOG_TTL`].
    fn maybe_request_slash_catalog(&mut self, cwd: Option<&Path>) {
        let root = crate::memory::resolve_active_project_root(cwd);
        let fresh = self.composer.slash_root.as_ref() == Some(&root)
            && self
                .composer
                .slash_loaded_at
                .is_some_and(|at| at.elapsed() < SLASH_CATALOG_TTL);
        if fresh || self.composer.slash_pending {
            return;
        }
        let Some(home) = crate::projects::home_dir() else {
            return;
        };
        self.composer.slash_pending = true;
        if !self
            .composer
            .send_fs(&self.proxy, FsJob::Slash { home, root })
        {
            self.composer.slash_pending = false;
        }
    }

    pub(super) fn on_composer_slash(&mut self, catalog: slash::SlashCatalog) {
        self.composer.slash_pending = false;
        self.composer.slash_root = Some(catalog.project_root.clone());
        self.composer.slash_loaded_at = Some(Instant::now());
        self.composer.state.slash = std::sync::Arc::new(catalog);
        // Refresh an open slash menu with the new items.
        if self.composer.state.popup.is_none()
            || slash::slash_query(
                self.composer.state.editor.text(),
                self.composer.state.editor.caret(),
            )
            .is_some()
        {
            self.composer
                .state
                .open_slash_menu(self.composer.workflows.workflows());
        }
        if self.composer_visible() {
            self.window.request_redraw();
        }
    }

    /// Collect commands from all panes' command blocks (newest first).
    fn harvest_block_history(&mut self) {
        let mut timed: Vec<(Option<Instant>, usize, String)> = Vec::new();
        let mut seq = 0usize;
        for pane in self.ws.panes() {
            for block in pane.blocks.blocks() {
                if let Some(cmd) = &block.command {
                    timed.push((block.started, seq, cmd.clone()));
                    seq += 1;
                }
            }
        }
        timed.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
        self.composer
            .history
            .merge_session_commands(timed.into_iter().map(|(_, _, c)| c));
    }

    fn ensure_shell_history_loaded(&mut self) {
        if self.composer.history_requested {
            return;
        }
        self.composer.history_requested = true;
        if !self.composer.send_fs(&self.proxy, FsJob::History) {
            self.composer.history_requested = false;
        }
    }

    fn ensure_path_commands_loaded(&mut self) {
        if self.composer.path_commands_requested {
            return;
        }
        self.composer.path_commands_requested = true;
        if !self.composer.send_fs(&self.proxy, FsJob::PathCommands) {
            self.composer.path_commands_requested = false;
        }
    }

    pub(super) fn on_composer_history(&mut self, entries: Vec<String>) {
        self.composer.history.set_shell_history(entries);
        self.composer.state.refresh_ghost(
            self.composer.history.entries(),
            self.composer.workflows.workflows(),
        );
        if self.composer_visible() {
            self.window.request_redraw();
        }
    }

    fn maybe_request_composer_dir(&mut self, cwd: Option<&Path>) {
        let Some(dir) = self.composer.state.needed_dir(cwd) else {
            return;
        };
        if self
            .composer
            .dir_cache
            .as_ref()
            .is_some_and(|(cached, _)| cached == &dir)
            || self.composer.dir_pending.as_ref() == Some(&dir)
        {
            return;
        }
        self.request_composer_dir(dir);
    }

    fn request_composer_dir(&mut self, dir: PathBuf) {
        self.composer.dir_pending = Some(dir.clone());
        if !self.composer.send_fs(&self.proxy, FsJob::Dir(dir)) {
            self.composer.dir_pending = None;
        }
    }

    pub(super) fn on_composer_dir(&mut self, dir: PathBuf, entries: Vec<DirEntryInfo>) {
        if self.composer.dir_pending.as_ref() == Some(&dir) {
            self.composer.dir_pending = None;
        }
        self.composer.dir_cache = Some((dir, entries));
        self.refresh_composer_popup_after_async();
    }

    fn maybe_request_composer_generators(&mut self, cwd: Option<&Path>) {
        let keys = self.composer.state.needed_generators(cwd);
        self.request_composer_generators(keys);
    }

    fn request_composer_generators(&mut self, keys: Vec<GeneratorKey>) {
        let now = Instant::now();
        for key in keys {
            if self.composer.generator_cache.get(&key, now).is_some()
                || self.composer.generators_pending.contains(&key)
            {
                continue;
            }
            self.composer.generators_pending.insert(key.clone());
            let key_on_err = key.clone();
            if !self.composer.send_gen(&self.proxy, key) {
                self.composer.generators_pending.remove(&key_on_err);
            }
        }
    }

    pub(super) fn on_composer_generator(&mut self, key: GeneratorKey, items: Vec<GeneratedItem>) {
        self.composer.generators_pending.remove(&key);
        self.composer
            .generator_cache
            .insert(key, items, Instant::now());
        self.refresh_composer_popup_after_async();
    }

    fn maybe_request_composer_path_stats(&mut self, cwd: Option<&Path>) {
        // Only stat local filesystem paths (never for remote SSH panes).
        let is_remote = self
            .focused_id()
            .and_then(|id| self.ws.pane(id))
            .is_some_and(|p| p.remote.is_some());
        if is_remote {
            return;
        }
        let candidates = extract_candidate_paths(self.composer.state.editor.text(), cwd);
        let to_check: Vec<PathBuf> = candidates
            .into_iter()
            .filter(|p| {
                !self.composer.path_exists_cache.contains_key(p)
                    && !self.composer.path_exists_pending.contains(p)
            })
            .collect();
        if to_check.is_empty() {
            return;
        }
        for p in &to_check {
            self.composer.path_exists_pending.insert(p.clone());
        }
        let rollback = to_check.clone();
        if !self
            .composer
            .send_fs(&self.proxy, FsJob::PathStats(to_check))
        {
            for p in &rollback {
                self.composer.path_exists_pending.remove(p);
            }
        }
    }

    pub(super) fn on_composer_path_info(
        &mut self,
        commands: Option<HashSet<String>>,
        existing: Vec<(PathBuf, bool)>,
    ) {
        let mut changed = false;
        if let Some(cmds) = commands {
            self.composer.path_commands = Some(cmds);
            changed = true;
        }
        for (path, ok) in existing {
            self.composer.path_exists_pending.remove(&path);
            if self.composer.path_exists_cache.len() >= MAX_PATH_STAT_CACHE {
                self.composer.path_exists_cache.clear();
            }
            if self.composer.path_exists_cache.insert(path, ok) != Some(ok) {
                changed = true;
            }
        }
        if changed && self.composer_visible() {
            self.refresh_composer_popup_after_async();
            self.window.request_redraw();
        }
    }

    fn refresh_composer_popup_after_async(&mut self) {
        let cwd = self.focused_id().and_then(|id| self.pane_cwd(id));
        let dir_cache = self
            .composer
            .dir_cache
            .as_ref()
            .map(|(d, e)| (d.as_path(), e.as_slice()));
        let gen_snap = self
            .composer
            .generator_cache
            .snapshot_for(cwd.as_deref(), Instant::now());
        if self.composer.state.refresh_popup_if_open_with(
            self.composer.history.entries(),
            self.composer.workflows.workflows(),
            dir_cache,
            cwd.as_deref(),
            &gen_snap,
            self.composer.path_commands.as_ref(),
        ) {
            self.window.request_redraw();
        }
    }

    /// Handle a key event while the composer owns keyboard focus.
    /// Returns `None` to let a global Twitty shortcut pass through; `Some(close)` when handled.
    pub(super) fn handle_composer_key(&mut self, event: &KeyEvent) -> Option<bool> {
        if !self.composer_has_keys() {
            return None;
        }
        match key_shortcut_with(event, self.modifiers, &self.keybindings) {
            Some(InputAction::ToggleComposer) => {
                if event.state.is_pressed() {
                    self.close_composer();
                }
                return Some(false);
            }
            // Inside the composer, Ctrl+Shift+Z is Redo and Ctrl+Shift+C/V are Copy/Paste.
            Some(InputAction::TogglePaneZoom | InputAction::Copy | InputAction::Paste)
            | Some(InputAction::Bytes(_))
            | None => {}
            Some(action) => {
                let blocks = self
                    .focused_id()
                    .and_then(|id| self.ws.pane(id))
                    .is_some_and(|p| p.blocks.navigable(&p.terminal));
                let tabs = self.ws.tabs().len();
                if shortcut_is_idle(Some(&action), self.ws.single_pane(), tabs, blocks) {
                    return Some(false);
                }
                return None;
            }
        }
        if !event.state.is_pressed() {
            return Some(false);
        }

        let physical = match event.physical_key {
            PhysicalKey::Code(code) => Some(code),
            _ => None,
        };
        let cwd = self.focused_id().and_then(|id| self.pane_cwd(id));
        let dir_cache = self
            .composer
            .dir_cache
            .as_ref()
            .map(|(d, e)| (d.as_path(), e.as_slice()));
        self.composer.state.ai_enabled = self.config.ai;
        let gen_snap = self
            .composer
            .generator_cache
            .snapshot_for(cwd.as_deref(), Instant::now());
        let action = self.composer.state.key_with_context(
            &event.logical_key,
            physical,
            event.text.as_deref(),
            self.modifiers,
            self.composer.history.entries(),
            self.composer.workflows.workflows(),
            dir_cache,
            cwd.as_deref(),
            &gen_snap,
            self.composer.path_commands.as_ref(),
        );
        self.apply_composer_action(action, cwd.as_deref());
        Some(std::mem::take(&mut self.composer.exit_requested))
    }

    /// Handle IME preedit when the composer has focus. Returns true if consumed.
    pub(super) fn composer_ime_preedit(
        &mut self,
        preedit: Option<(String, Option<(usize, usize)>)>,
    ) -> bool {
        if !self.composer_has_keys() {
            return false;
        }
        self.preedit_active = preedit.is_some();
        self.composer.state.editor.set_preedit(preedit);
        self.composer.state.refresh_ghost(
            self.composer.history.entries(),
            self.composer.workflows.workflows(),
        );
        self.window.set_preedit(None);
        self.window.request_redraw();
        true
    }

    /// Handle committed IME text when the composer has focus. Returns true if consumed.
    pub(super) fn composer_ime_commit(&mut self, text: &str) -> bool {
        if !self.composer_has_keys() {
            return false;
        }
        let cwd = self.focused_id().and_then(|id| self.pane_cwd(id));
        let action = self.composer.state.insert_text(
            text,
            self.composer.history.entries(),
            self.composer.workflows.workflows(),
        );
        self.apply_composer_action(action, cwd.as_deref());
        true
    }

    fn apply_composer_action(&mut self, action: ComposerAction, cwd: Option<&Path>) {
        match action {
            ComposerAction::None => {}
            ComposerAction::Redraw => {
                self.cursor_blink.reset(Instant::now());
                self.maybe_request_composer_dir(cwd);
                self.maybe_request_composer_generators(cwd);
                self.maybe_request_composer_path_stats(cwd);
                self.window.request_redraw();
            }
            ComposerAction::Close => self.close_composer(),
            ComposerAction::Copy(text) => {
                self.clipboard.set_text(text);
                self.window.request_redraw();
            }
            ComposerAction::RequestPaste => {
                if let Some(text) = self.clipboard.text() {
                    let act = self.composer.state.insert_text(
                        &text,
                        self.composer.history.entries(),
                        self.composer.workflows.workflows(),
                    );
                    self.apply_composer_action(act, cwd);
                }
            }
            ComposerAction::RequestDir(dir) => {
                self.request_composer_dir(dir);
                self.window.request_redraw();
            }
            ComposerAction::RequestGenerators(keys) => {
                self.request_composer_generators(keys);
                self.window.request_redraw();
            }
            ComposerAction::GenerateAi(request) => {
                self.trigger_composer_ai(request);
            }
            ComposerAction::CancelAi => {
                self.cancel_ai_request();
            }
            ComposerAction::DiscardAi => {
                self.clear_composer_ai_state();
                self.window.request_redraw();
            }
            ComposerAction::RunCommand(name) => {
                self.composer.manual_open = false;
                if self.config.composer_mode() == ComposerMode::OnDemand {
                    self.composer.focused = false;
                }
                match <Self as crate::control::cmux::ControlHost>::command_run(self, &name, None) {
                    Ok((_, exit)) => self.composer.exit_requested |= exit,
                    Err(err) => tracing::warn!("slash command {name:?} failed: {err}"),
                }
                self.window.request_redraw();
            }
            ComposerAction::Submit(cmd) => {
                self.clear_composer_ai_state();
                if !cmd.trim().is_empty() {
                    self.composer.history.push_session(&cmd);
                }
                let mode = self.config.composer_mode();
                self.composer.manual_open = false;
                if mode == ComposerMode::OnDemand {
                    self.composer.focused = false;
                }
                if let Some(id) = self.focused_id() {
                    // An agent reads `/command` as a command only when it is typed: pasted
                    // text is plain prompt text to Claude Code, Antigravity and others.
                    let typed_command =
                        matches!(self.composer.state.slash_foreground, Foreground::Agent(_))
                            && crate::composer::is_typed_slash_command(&cmd);
                    let bracketed =
                        self.mode_of(id).contains(TermMode::BRACKETED_PASTE) && !typed_command;
                    let bytes = encode_submit(&cmd, bracketed);
                    self.send_input(id, &bytes);
                    self.cursor_blink.reset(Instant::now());
                }
                self.window.request_redraw();
            }
        }
    }

    fn composer_highlight_and_lint(
        &self,
        text: &str,
        cwd: Option<&Path>,
    ) -> (
        Vec<crate::composer::highlight::HighlightSpan>,
        Vec<crate::composer::lint::LintMark>,
    ) {
        let path_exists = |token: &str| -> bool {
            let Some(path) = resolve_shell_path(token, cwd) else {
                return false;
            };
            if let Some(&ok) = self.composer.path_exists_cache.get(&path) {
                return ok;
            }
            // Fast fallback against already-cached directory listing for simple filenames.
            if let Some((cached_dir, entries)) = &self.composer.dir_cache {
                if path.parent() == Some(cached_dir.as_path()) {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        return entries.iter().any(|e| e.name == name);
                    }
                }
            }
            false
        };
        let hctx =
            HighlightContext::with_defaults(self.composer.path_commands.as_ref(), &path_exists);
        let highlights = highlight_command(text, &hctx);
        // A `/query` picking from the slash menu is not a shell command yet.
        let lints = if self.composer.state.slash_menu_open() {
            Vec::new()
        } else {
            lint_command(text, &hctx)
        };
        (highlights, lints)
    }

    fn composer_layout(&self) -> Option<ComposerLayout> {
        if !self.composer_visible() {
            return None;
        }
        let id = self.focused_id()?;
        let pane_rect = self.pane_rect(id)?;
        let metrics = self.window.metrics()?;
        let line_count = self.composer.state.editor.line_count();
        let (caret_line, _) = self.composer.state.editor.caret_line_col();
        let popup_items = self
            .composer
            .state
            .popup
            .as_ref()
            .map_or(0, |p| p.visible_range().len());
        let ai_view = self.composer_ai_view();
        let has_lints =
            !self.composer.state.slash_menu_open() && self.composer.last_has_lints.get();
        Some(ComposerLayout::with_ai_and_lint(
            pane_rect,
            metrics,
            metrics.scale,
            line_count,
            caret_line,
            popup_items,
            ai_view.enabled,
            ai_view.has_banner(),
            has_lints,
        ))
    }

    /// Pointer cursor icon when hovering over the composer.
    pub(super) fn composer_cursor(&self, x: f64, y: f64) -> Option<CursorIcon> {
        let layout = self.composer_layout()?;
        match layout.hit(x, y) {
            ComposerHit::Outside => None,
            ComposerHit::PopupItem(_) | ComposerHit::GenerateButton => Some(CursorIcon::Pointer),
            ComposerHit::Input => Some(CursorIcon::Text),
            ComposerHit::Card | ComposerHit::Popup => Some(CursorIcon::Default),
        }
    }

    /// Handle a mouse press when the composer is visible. Returns true if consumed.
    /// True once after a slash-menu command asked to close the window.
    pub(super) fn take_composer_exit_request(&mut self) -> bool {
        std::mem::take(&mut self.composer.exit_requested)
    }

    pub(super) fn composer_mouse_press(&mut self, button: MouseButton) -> bool {
        let Some(layout) = self.composer_layout() else {
            return false;
        };
        let (x, y) = self.pointer.pos;
        match layout.hit(x, y) {
            ComposerHit::Outside => {
                if self.composer.focused {
                    self.composer.focused = false;
                    self.composer.state.popup = None;
                    self.window.request_redraw();
                }
                false
            }
            ComposerHit::GenerateButton => {
                self.composer.focused = true;
                if button == MouseButton::Left && self.config.ai && !self.composer.state.ai_loading
                {
                    if let Some(req) =
                        crate::composer::extract_ai_request(self.composer.state.editor.text())
                    {
                        self.trigger_composer_ai(req);
                    } else {
                        self.composer.state.editor.set_text("# ".into());
                        self.cursor_blink.reset(Instant::now());
                        self.window.request_redraw();
                    }
                }
                true
            }
            ComposerHit::PopupItem(vis_row) => {
                self.composer.focused = true;
                if button == MouseButton::Left {
                    if let Some(range) = self
                        .composer
                        .state
                        .popup
                        .as_ref()
                        .map(|p| p.visible_range())
                    {
                        let idx = range.start + vis_row;
                        let action = self.composer.state.accept_popup_index(
                            idx,
                            self.composer.history.entries(),
                            self.composer.workflows.workflows(),
                        );
                        let cwd = self.focused_id().and_then(|id| self.pane_cwd(id));
                        self.apply_composer_action(action, cwd.as_deref());
                    }
                }
                self.cursor_blink.reset(Instant::now());
                self.window.request_redraw();
                true
            }
            ComposerHit::Input | ComposerHit::Card | ComposerHit::Popup => {
                self.composer.focused = true;
                if button == MouseButton::Middle {
                    let cwd = self.focused_id().and_then(|id| self.pane_cwd(id));
                    self.apply_composer_action(ComposerAction::RequestPaste, cwd.as_deref());
                } else {
                    self.cursor_blink.reset(Instant::now());
                    self.window.request_redraw();
                }
                true
            }
        }
    }

    /// Build the `ComposerView` for the current frame, or `None` when hidden.
    pub(super) fn composer_view(&self) -> Option<ComposerView> {
        if !self.composer_visible() {
            return None;
        }
        let id = self.focused_id()?;
        let pane_rect = self.pane_rect(id)?;
        let cwd = self.pane_cwd(id);
        let ed = &self.composer.state.editor;
        let lines = ed.lines().into_iter().map(str::to_owned).collect();
        let caret = ed.caret_line_col();
        let selection = ed.selection_line_cols();
        let preedit = ed.preedit().map(|p| (p.text.clone(), p.cursor));
        let language = self.window.ui_language();
        let (highlights, lints) = self.composer_highlight_and_lint(ed.text(), cwd.as_deref());
        self.composer.last_has_lints.set(!lints.is_empty());
        let popup = self.composer.state.popup.as_ref().map(|p| {
            let range = p.visible_range();
            let items = p.items[range.clone()]
                .iter()
                .map(|item| ComposerPopupItem {
                    kind: item.kind,
                    label: item.label.clone(),
                    detail: item.localized_detail(language).map(str::to_owned),
                })
                .collect();
            ComposerPopupView {
                items,
                selected: p.selected.saturating_sub(range.start),
            }
        });
        Some(ComposerView {
            pane_rect,
            lines,
            caret,
            selection,
            preedit,
            ghost: self.composer.state.ghost.clone(),
            highlights,
            lints,
            focused: self.composer.focused && self.window_focused,
            cursor_blink_on: self.cursor_blink.visible(),
            popup,
            language,
            ai: self.composer_ai_view(),
            context: cwd.as_deref().map(|dir| {
                let home = std::env::var_os("HOME").map(PathBuf::from);
                let path = crate::projects::abbreviate_home(dir, home.as_deref());
                match self.window.branch_for(dir) {
                    Some(branch) => format!("{path} · {branch}"),
                    None => path,
                }
            }),
        })
    }
}
