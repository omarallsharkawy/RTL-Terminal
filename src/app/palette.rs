//! The command palette: opening it, the keys, mouse clicks and text typed while it is open, and
//! acting on the chosen row.
//!
//! It lists one thing at a time ([`PaletteMode`]):
//! - **Commands:** the built-in ones and the custom ones (`workflows`: `config.json` and the
//!   project's `.twitty/commands.json`). Choosing an untrusted project command first shows the
//!   trust confirmation with the file's full command list.
//! - **Notifications:** the notification history, newest first; choosing one jumps to its pane.
//! - **Search:** the lines of every pane matching the query (`ui::search_all`, scanned a slice
//!   at a time once typing pauses); choosing one jumps to its pane and line, highlighted by the
//!   scrollback search.
//!
//! While the palette is open it owns all keyboard, IME and pointer input so nothing can reach
//! panes or the PTY behind it. Clicking outside the palette closes it and consumes the click
//! (never starting a selection, pasting, or switching tabs).

use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use alacritty_terminal::term::TermMode;
use winit::event::{KeyEvent, MouseButton};
use winit::keyboard::{Key, ModifiersState, NamedKey};

use super::App;
use crate::commands;
use crate::input::{key_shortcut, InputAction};
use crate::layout::{PaneId, SplitDir};
use crate::pty::{Pty, SpawnOptions};
use crate::render::scene::palette::{PaletteHit, PaletteItem, PaletteView};
use crate::session::workspace::CloseOthers;
use crate::ui::notifications::{jump_target, palette_item};
use crate::ui::palette::{
    matches, visible_window_in, PaletteCmd, PaletteEntry, PaletteId, PaletteMode, PaletteState,
    RenameCmd, RENAME_TAB_PLACEHOLDER,
};
use crate::ui::search_all::{
    jump_target as search_target, PaneInfo, Query, SearchAll, SearchHit, PLACEHOLDER, SLICE,
};
use crate::workflows::{
    encode_insert, resolve_run_cwd, run_shell_argv, trust_confirm_view, CommandMode, ConfirmCmd,
    TrustConfirmState, TrustStore, Workflow, WorkflowSet,
};

pub(super) struct OpenPalette {
    state: PaletteState,
    /// Custom commands found when the palette opened (commands mode).
    workflows: WorkflowSet,
    /// Built-in and custom commands, in palette order.
    entries: Vec<PaletteEntry>,
    pending_trust: Option<PendingTrust>,
    close_others: Option<CloseOthers>,
    rename_tab: Option<u64>,
    /// Results of searching every pane (search mode).
    search: SearchAll,
}

struct PendingTrust {
    workflow_index: usize,
    confirm: TrustConfirmState,
}

impl App {
    pub(super) fn palette_open(&self) -> bool {
        self.palette.is_some()
    }

    /// Working directory of pane `id`: the shell's own first, else OSC 7.
    pub(super) fn pane_cwd(&self, id: PaneId) -> Option<PathBuf> {
        let pane = self.ws.pane(id)?;
        if pane.remote.is_some() {
            return None;
        }
        pane.pty
            .as_ref()
            .and_then(Pty::cwd)
            .or_else(|| pane.meta.cwd.clone())
    }

    /// Open the palette listing `mode` (a no-op when it already does, so `command.run
    /// palette.open` never closes it).
    pub(super) fn open_palette(&mut self, mode: PaletteMode) {
        if self.palette.as_ref().is_some_and(|p| p.state.mode == mode) {
            return;
        }
        self.close_settings();
        self.reset_pointer_gesture();
        let (workflows, entries) = if mode == PaletteMode::Commands {
            let cwd = self.focused_id().and_then(|id| self.pane_cwd(id));
            let workflows = WorkflowSet::load(&self.config.commands, cwd.as_deref());
            let entries = workflows.palette_entries();
            (workflows, entries)
        } else {
            (WorkflowSet::default(), Vec::new())
        };
        self.palette = Some(OpenPalette {
            state: PaletteState {
                mode,
                ..PaletteState::default()
            },
            workflows,
            entries,
            pending_trust: None,
            close_others: None,
            rename_tab: self
                .ws
                .active_tab()
                .map(|tab| tab.id)
                .filter(|_| mode == PaletteMode::RenameTab),
            search: SearchAll::default(),
        });
        self.sync_palette();
    }

    pub(super) fn open_close_others_confirmation(&mut self, plan: CloseOthers) {
        self.open_palette(PaletteMode::CloseOtherPanes);
        if let Some(open) = self.palette.as_mut() {
            open.close_others = Some(plan);
        }
        self.sync_palette();
    }

    /// The palette's own shortcuts: open `mode`, or close the palette when it already lists it.
    fn toggle_palette_mode(&mut self, mode: PaletteMode) {
        if self.palette.as_ref().is_some_and(|p| p.state.mode == mode) {
            self.close_palette();
        } else {
            self.open_palette(mode);
        }
    }

    pub(super) fn close_palette(&mut self) {
        if self.palette.take().is_some() {
            self.window.set_command_palette(None);
            self.refresh_pointer_hover();
            self.window.request_redraw();
        }
    }

    /// The notification list is on screen (it is refreshed when a notification arrives).
    pub(super) fn notification_list_open(&self) -> bool {
        self.palette
            .as_ref()
            .is_some_and(|p| p.state.mode == PaletteMode::Notifications)
    }

    fn palette_capacity(&self) -> usize {
        self.window.palette_rows(crate::ui::palette::MAX_RESULTS)
    }

    /// Hand the current query and results (or the trust confirmation) to the renderer.
    pub(super) fn sync_palette(&mut self) {
        if let Some(open) = self.palette.as_mut() {
            let ws = &self.ws;
            open.search.retain_open(|id| ws.pane(id).is_some());
        }
        let Some(open) = &self.palette else {
            return;
        };
        if let Some(pending) = &open.pending_trust {
            let view = match open.workflows.project() {
                Some(project) => trust_confirm_view(project, pending.confirm.scroll),
                None => PaletteView::default(),
            };
            self.window.set_command_palette(Some(view));
            self.window.request_redraw();
            return;
        }
        let state = &open.state;
        let (rows, placeholder): (Vec<PaletteItem>, _) = match state.mode {
            PaletteMode::Commands => {
                let rows = matches(&open.entries, &state.query)
                    .into_iter()
                    .map(|i| {
                        let entry = &open.entries[i];
                        PaletteItem {
                            title_en: entry.title_en.clone(),
                            title_ar: entry.title_ar.clone(),
                            shortcut: entry.shortcut.clone(),
                            dimmed: false,
                        }
                    })
                    .collect();
                (rows, None)
            }
            PaletteMode::Notifications => {
                let list = self.notification_history.newest_first();
                let now = SystemTime::now();
                let rows = matches(&list, &state.query)
                    .into_iter()
                    .map(|i| palette_item(list[i], self.ws.pane(list[i].pane).is_some(), now))
                    .collect();
                (
                    rows,
                    Some("Filter notifications… / دوّر في الإشعارات…".to_string()),
                )
            }
            PaletteMode::RenameTab => (
                vec![PaletteItem {
                    title_en: "Save tab name (blank clears it)".into(),
                    title_ar: "احفظ اسم التاب (الاسم الفاضي بيمسحه)".into(),
                    shortcut: None,
                    dimmed: false,
                }],
                Some(RENAME_TAB_PLACEHOLDER.into()),
            ),
            PaletteMode::CloseOtherPanes => (
                vec![
                    PaletteItem {
                        title_en: "Close other panes".into(),
                        title_ar: "اقفل باقي اللوحات".into(),
                        shortcut: None,
                        dimmed: false,
                    },
                    PaletteItem {
                        title_en: "Cancel".into(),
                        title_ar: "إلغاء".into(),
                        shortcut: None,
                        dimmed: false,
                    },
                ],
                Some("Close panes with running programs? / تقفل لوحات فيها برامج شغّالة؟".into()),
            ),
            PaletteMode::Search => (
                open.search.rows(&state.query),
                Some(PLACEHOLDER.to_string()),
            ),
        };
        let shown = visible_window_in(rows.len(), state.selected, self.palette_capacity());
        let selected = state.selected.min(rows.len().saturating_sub(1)) - shown.start;
        let mut items: Vec<PaletteItem> = rows[shown].to_vec();
        if items.is_empty() && state.mode == PaletteMode::Notifications && state.query.is_empty() {
            items.push(PaletteItem {
                title_en: "No notifications yet".into(),
                title_ar: "مفيش إشعارات لسه".into(),
                shortcut: None,
                dimmed: true,
            });
        }
        let view = PaletteView {
            query: state.query.clone(),
            items,
            selected,
            placeholder,
        };
        self.window.set_command_palette(Some(view));
        self.window.request_redraw();
    }

    /// A key while the palette is open; nothing reaches the program. Returns true when the
    /// window should close (the command run closed the last pane).
    pub(super) fn handle_palette_key(&mut self, event: &KeyEvent) -> bool {
        if !event.state.is_pressed() {
            return false;
        }
        // The palette shortcuts switch between its modes, or close it from its own.
        match key_shortcut(event, self.modifiers) {
            Some(InputAction::CommandPalette) => {
                self.toggle_palette_mode(PaletteMode::Commands);
                return false;
            }
            Some(InputAction::NotificationList) => {
                self.toggle_palette_mode(PaletteMode::Notifications);
                return false;
            }
            Some(InputAction::SearchAll) => {
                self.toggle_palette_mode(PaletteMode::Search);
                return false;
            }
            Some(InputAction::RenameTab) => {
                self.toggle_palette_mode(PaletteMode::RenameTab);
                return false;
            }
            Some(InputAction::Settings) => {
                self.close_palette();
                self.open_settings();
                return false;
            }
            _ => {}
        }
        let modifiers = self.modifiers;
        let Some(open) = self.palette.as_mut() else {
            return false;
        };

        if let Some(pending) = open.pending_trust.as_mut() {
            let cmd_count = open.workflows.project().map_or(0, |p| p.commands.len());
            match pending.confirm.key(&event.logical_key, cmd_count) {
                ConfirmCmd::None => self.sync_palette(),
                ConfirmCmd::Cancel => {
                    open.pending_trust = None;
                    self.sync_palette();
                }
                ConfirmCmd::Trust => self.confirm_pending_trust(),
            }
            return false;
        }

        let (key, text) = (&event.logical_key, event.text.as_deref());
        if open.state.mode == PaletteMode::Search {
            self.search_all_key(key, text, modifiers);
            return false;
        }
        match open.state.mode {
            PaletteMode::Commands => match open.state.key(&open.entries, key, text, modifiers) {
                PaletteCmd::Run(id) => return self.activate_palette_id(id),
                PaletteCmd::Close => self.close_palette(),
                PaletteCmd::None | PaletteCmd::Choose(_) => self.sync_palette(),
            },
            PaletteMode::Notifications => {
                let list = self.notification_history.newest_first();
                match open.state.key_in(&list, key, text, modifiers) {
                    // A notification: jump to its pane, unless that pane is gone.
                    PaletteCmd::Choose(i) => {
                        let target = list
                            .get(i)
                            .and_then(|e| jump_target(e, |id| self.ws.pane(id).is_some()));
                        self.jump_or_stay(target);
                    }
                    PaletteCmd::Close => self.close_palette(),
                    PaletteCmd::None | PaletteCmd::Run(_) => self.sync_palette(),
                }
            }
            PaletteMode::RenameTab => match open.state.rename_key(key, text, modifiers) {
                RenameCmd::Edit => self.sync_palette(),
                RenameCmd::Cancel => self.close_palette(),
                RenameCmd::Save(title) => self.save_tab_title(title),
            },
            PaletteMode::CloseOtherPanes => {
                // Confirmation is a fixed choice. Only navigation, Enter and Escape affect it.
                if matches!(
                    key,
                    Key::Named(
                        NamedKey::ArrowUp
                            | NamedKey::ArrowDown
                            | NamedKey::Enter
                            | NamedKey::Escape
                    )
                ) {
                    match open.state.key_list(2, key, None, modifiers) {
                        PaletteCmd::Choose(0) => self.confirm_close_others(),
                        PaletteCmd::Choose(_) | PaletteCmd::Close => self.close_palette(),
                        _ => self.sync_palette(),
                    }
                }
            }
            PaletteMode::Search => {}
        }
        false
    }

    /// A mouse press while the palette is open. Clicking outside closes the palette and consumes
    /// the click so nothing reaches panes or tabs; left-clicking a row chooses it (in the trust
    /// confirmation, the first row trusts). Returns true when the window should close.
    pub(super) fn on_palette_mouse_press(&mut self, button: MouseButton) -> bool {
        let (x, y) = self.pointer.pos;
        let row = match self.window.palette_hit(x, y) {
            PaletteHit::Outside => {
                self.close_palette();
                return false;
            }
            PaletteHit::Item(row) if button == MouseButton::Left => row,
            PaletteHit::Inside | PaletteHit::Item(_) => return false,
        };
        let Some(open) = self.palette.as_ref() else {
            return false;
        };
        if open.pending_trust.is_some() {
            if row == 0 {
                self.confirm_pending_trust();
            }
            return false;
        }
        let state = &open.state;
        match state.mode {
            PaletteMode::Commands => {
                let results = matches(&open.entries, &state.query);
                let shown =
                    visible_window_in(results.len(), state.selected, self.palette_capacity());
                match results.get(shown.start + row) {
                    Some(&i) => {
                        let id = open.entries[i].id.clone();
                        self.activate_palette_id(id)
                    }
                    None => false,
                }
            }
            PaletteMode::Notifications => {
                let list = self.notification_history.newest_first();
                let results = matches(&list, &state.query);
                let shown =
                    visible_window_in(results.len(), state.selected, self.palette_capacity());
                if let Some(&i) = results.get(shown.start + row) {
                    let target = jump_target(list[i], |id| self.ws.pane(id).is_some());
                    self.jump_or_stay(target);
                }
                false
            }
            PaletteMode::RenameTab => {
                let title = open.state.tab_title();
                self.save_tab_title(title);
                false
            }
            PaletteMode::CloseOtherPanes => {
                let shown = visible_window_in(2, open.state.selected, self.palette_capacity());
                if shown.start + row == 0 {
                    self.confirm_close_others();
                } else {
                    self.close_palette();
                }
                false
            }
            PaletteMode::Search => {
                let hits = open.search.hits();
                let shown = visible_window_in(hits.len(), state.selected, self.palette_capacity());
                let hit = hits.get(shown.start + row).cloned();
                self.jump_to_hit(hit);
                false
            }
        }
    }

    fn save_tab_title(&mut self, title: Option<String>) {
        let tab = self.palette.as_ref().and_then(|open| open.rename_tab);
        self.close_palette();
        if let Some(index) = tab.and_then(|id| self.ws.tab_index_by_id(id)) {
            self.ws.rename_tab(index, title);
            self.refresh_window_title();
        }
    }

    fn confirm_close_others(&mut self) {
        let plan = self
            .palette
            .as_mut()
            .and_then(|open| open.close_others.take());
        self.close_palette();
        if let Some(plan) = plan {
            self.close_other_panes(plan, true);
        }
    }

    /// Run the chosen command palette entry. Returns true when the window should close.
    fn activate_palette_id(&mut self, id: PaletteId) -> bool {
        match id {
            PaletteId::Builtin(name) => {
                self.close_palette();
                // Choosing "palette.open" from the palette just closes it.
                name != "palette.open" && self.run_command(name)
            }
            PaletteId::Custom(index) => {
                let Some(open) = self.palette.as_mut() else {
                    return false;
                };
                let Some(workflow) = open.workflows.get(index).cloned() else {
                    self.close_palette();
                    return false;
                };
                if !workflow.trusted {
                    open.pending_trust = Some(PendingTrust {
                        workflow_index: index,
                        confirm: TrustConfirmState::default(),
                    });
                    self.sync_palette();
                    return false;
                }
                self.close_palette();
                if let Some(pane_id) = self.focused_id() {
                    if let Err(err) = self.run_workflow(pane_id, &workflow) {
                        self.show_pane_warning(pane_id, &err);
                    }
                }
                false
            }
        }
    }

    /// Trust the project file shown in the confirmation (keyed by directory and the hash of the
    /// content that was read and shown) and run the chosen command.
    fn confirm_pending_trust(&mut self) {
        let Some((project, mut workflow)) = self.palette.as_ref().and_then(|open| {
            let pending = open.pending_trust.as_ref()?;
            let project = open.workflows.project()?.clone();
            let workflow = open.workflows.get(pending.workflow_index)?.clone();
            Some((project, workflow))
        }) else {
            self.close_palette();
            return;
        };

        let mut store = TrustStore::load_default();
        store.trust(&project.root, &project.sha256);
        if let Err(err) = store.save_default() {
            self.close_palette();
            if let Some(pane_id) = self.focused_id() {
                self.show_pane_warning(
                    pane_id,
                    &format!("could not save trust store: {err} / تعذر حفظ سجل الثقة"),
                );
            }
            return;
        }
        workflow.trusted = true;
        self.close_palette();
        if let Some(pane_id) = self.focused_id() {
            if let Err(err) = self.run_workflow(pane_id, &workflow) {
                self.show_pane_warning(pane_id, &err);
            }
        }
    }

    fn jump_or_stay(&mut self, target: Option<PaneId>) {
        match target {
            Some(id) => {
                self.close_palette();
                self.jump_to_notification(id);
            }
            // A closed pane: nothing to jump to, the list stays open.
            None => self.sync_palette(),
        }
    }

    /// Text committed by an input method while the palette is open (Arabic typing).
    pub(super) fn palette_text(&mut self, text: &str) {
        if let Some(open) = self.palette.as_mut() {
            if open.pending_trust.is_some() || open.state.mode == PaletteMode::CloseOtherPanes {
                return;
            }
            open.state.push_text(text);
            if open.state.mode == PaletteMode::Search {
                open.search.query_changed(&open.state.query, Instant::now());
            }
            self.sync_palette();
        }
    }

    // ---- search all terminals ----

    /// A key in search mode: arrows choose, Enter jumps (running a waiting scan first so it
    /// answers the query on screen), typing schedules a new scan.
    fn search_all_key(&mut self, key: &Key, text: Option<&str>, modifiers: ModifiersState) {
        let pending = self.palette.as_ref().is_some_and(|p| p.search.is_pending());
        if pending && *key == Key::Named(NamedKey::Enter) {
            self.finish_search_all();
        }
        let Some(open) = self.palette.as_mut() else {
            return;
        };
        let before = open.state.query.clone();
        match open
            .state
            .key_list(open.search.hits().len(), key, text, modifiers)
        {
            PaletteCmd::Choose(i) => {
                let hit = open.search.hits().get(i).cloned();
                self.jump_to_hit(hit);
            }
            PaletteCmd::Close => self.close_palette(),
            PaletteCmd::None | PaletteCmd::Run(_) => {
                if open.state.query != before {
                    open.search.query_changed(&open.state.query, Instant::now());
                }
                self.sync_palette();
            }
        }
    }

    /// Start the waiting scan once it is due, then run it a slice at a time (input and
    /// drawing go on in between). Returns when it wants to run again.
    pub(super) fn poll_search_all(&mut self, now: Instant) -> Option<Instant> {
        let search = &self.palette.as_ref()?.search;
        if let Some(due) = search.due() {
            if due > now {
                return Some(due);
            }
            self.start_search_all();
        }
        self.step_search_all(Some(now + SLICE));
        let pending = self.palette.as_ref()?.search.is_pending();
        pending.then(Instant::now)
    }

    /// Start or finish the scan now, in one go (Enter while results are still coming).
    fn finish_search_all(&mut self) {
        if self
            .palette
            .as_ref()
            .is_some_and(|p| p.search.due().is_some())
        {
            self.start_search_all();
        }
        self.step_search_all(None);
    }

    /// Start a scan of every pane of every tab for the palette's query: tabs in order, each
    /// pane's newest lines first.
    fn start_search_all(&mut self) {
        let Some(open) = self.palette.as_ref() else {
            return;
        };
        let Some(query) = Query::new(&open.state.query) else {
            return;
        };
        let mut panes = Vec::new();
        for (tab, t) in self.ws.tabs().iter().enumerate() {
            let tab_title = super::window::sidebar_title(&self.ws, tab);
            let leaves = t.root.leaves();
            let split = leaves.len() > 1;
            for id in leaves {
                let pane_title = self.ws.pane(id).filter(|_| split).and_then(|pane| {
                    let program = pane.pty.as_ref().and_then(Pty::program_name);
                    pane.title.as_deref().or(program).map(str::to_owned)
                });
                panes.push(PaneInfo {
                    pane: id,
                    tab,
                    tab_title: tab_title.clone(),
                    pane_title,
                });
            }
        }
        if let Some(open) = self.palette.as_mut() {
            open.search.start(query, panes);
        }
    }

    /// Run the scan in progress until `deadline` (or to its end) and list what it found.
    fn step_search_all(&mut self, deadline: Option<Instant>) {
        let Some(open) = self.palette.as_mut() else {
            return;
        };
        let Some(job) = open.search.job_mut() else {
            return;
        };
        let ws = &mut self.ws;
        job.run_until(
            |id, step| step(ws.pane_mut(id).map(|p| &mut p.terminal)),
            deadline,
        );
        open.search.update();
        self.sync_palette();
    }

    /// Jump to a search result: its tab and pane, scrolled to its line, which the pane's
    /// scrollback search highlights (its bar stays open: Enter goes on to older matches, Esc
    /// back to the shell). A result whose pane was closed is dropped and the list stays.
    fn jump_to_hit(&mut self, hit: Option<SearchHit>) {
        let ws = &self.ws;
        let Some(hit) = hit
            .as_ref()
            .and_then(|h| search_target(h, |id| ws.pane(id).is_some()))
            .cloned()
        else {
            self.sync_palette();
            return;
        };
        self.close_palette();
        if self.focus_pane_by_id(hit.pane).is_err() {
            return;
        }
        self.clear_selection(hit.pane);
        if let Some(pane) = self.ws.pane_mut(hit.pane) {
            let line = &hit.line;
            pane.ui.search_query = Some(line.needle.clone());
            pane.terminal
                .focus_search_at(&line.needle, line.pos, line.row, line.column);
        }
        self.window.request_redraw();
    }

    /// Record a notification from pane `id` in the history; it counts as seen when the user
    /// is looking at that pane already.
    pub(super) fn record_notification(&mut self, id: PaneId, title: Option<&str>, body: &str) {
        let Some(tab) = self.ws.tabs().iter().position(|t| t.root.contains(id)) else {
            return;
        };
        let seen = self.window_focused && self.focused_id() == Some(id);
        let tab_title = super::window::sidebar_title(&self.ws, tab);
        let recorded =
            self.notification_history
                .push(id, title, body, &tab_title, SystemTime::now(), seen);
        if recorded {
            if self.notification_list_open() {
                self.sync_palette();
            }
            self.window.request_redraw();
        }
    }

    /// Run a built-in command by its stable name on the focused pane. Returns true when the
    /// window should close.
    pub(super) fn run_command(&mut self, name: &str) -> bool {
        match (commands::action(name), self.focused_id()) {
            (Some(action), Some(id)) => self.run_action(id, action),
            _ => false,
        }
    }

    /// Run a trusted custom workflow command on pane `id`.
    pub(super) fn run_workflow(&mut self, id: PaneId, workflow: &Workflow) -> Result<(), String> {
        if !workflow.trusted {
            return Err(format!(
                "refusing to run untrusted project command {:?} / رفض تشغيل أمر مشروع غير موثوق",
                workflow.spec.name
            ));
        }
        match workflow.spec.mode {
            CommandMode::Insert => {
                let bracketed = self.mode_of(id).contains(TermMode::BRACKETED_PASTE);
                let bytes = encode_insert(&workflow.spec.command, bracketed);
                self.send_input(id, &bytes);
                self.cursor_blink.reset(Instant::now());
                Ok(())
            }
            CommandMode::Run => {
                let pane_cwd = self.pane_cwd(id);
                let cwd = resolve_run_cwd(workflow, pane_cwd.as_deref())?;
                let options = SpawnOptions {
                    cwd,
                    command: run_shell_argv(&workflow.spec.command),
                    env: Vec::new(),
                };
                let spawned = if self.can_split_focused(SplitDir::Horizontal) {
                    self.split_focused_with(SplitDir::Horizontal, options).ok()
                } else if self.can_split_focused(SplitDir::Vertical) {
                    self.split_focused_with(SplitDir::Vertical, options).ok()
                } else {
                    self.new_tab_with(options)
                };
                if spawned.is_none() {
                    return Err("failed to start custom command / تعذر تشغيل الأمر المخصص".into());
                }
                Ok(())
            }
        }
    }

    pub(super) fn show_pane_warning(&mut self, id: PaneId, message: &str) {
        let Some(note) = format_pane_warning(message) else {
            return;
        };
        self.term(id, |t| t.process_bytes(note.as_bytes()));
        self.window.request_redraw();
    }
}

/// Sanitize a user-facing pane warning (which may embed paths or config values) so it cannot
/// inject terminal control sequences.
pub fn format_pane_warning(message: &str) -> Option<String> {
    let clean = crate::shell_integration::clean_text(message, 512);
    (!clean.is_empty()).then(|| format!("\r\n\x1b[33mTwitty: {clean}\x1b[0m\r\n"))
}
