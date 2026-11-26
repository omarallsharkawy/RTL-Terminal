//! App-level hooks for the command composer: visibility lifecycle, off-thread shell history and
//! directory listing loads, keyboard/IME/mouse routing, and building the per-frame `ComposerView`.

use std::path::PathBuf;
use std::time::Instant;

use alacritty_terminal::term::TermMode;
use winit::event::{KeyEvent, MouseButton};
use winit::keyboard::PhysicalKey;
use winit::window::CursorIcon;

use super::{App, AppEvent};
use crate::composer::complete::{read_dir_entries, DirEntryInfo, MAX_DIR_ENTRIES};
use crate::composer::history::{load_shell_history_default, HistoryStore};
use crate::composer::{
    encode_submit, should_show_composer, ComposerAction, ComposerMode, ComposerState,
};
use crate::input::route::shortcut_is_idle;
use crate::input::{key_shortcut, InputAction};
use crate::layout::PaneId;
use crate::pty::Pty;
use crate::render::scene::composer::{
    ComposerHit, ComposerLayout, ComposerPopupItem, ComposerPopupView, ComposerView,
};
use crate::shell_integration::MarkKind;
use crate::workflows::WorkflowSet;

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
        self.prepare_composer_sources();
        self.cursor_blink.reset(Instant::now());
        self.window.request_redraw();
    }

    pub(super) fn close_composer(&mut self) {
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

    /// Called when focus moves to another pane or tab.
    pub(super) fn on_composer_focus_changed(&mut self) {
        self.composer.manual_open = false;
        self.composer.dismissed_for_prompt = false;
        self.composer.state.popup = None;
        self.composer.state.editor.set_preedit(None);
        self.composer.dir_cache = None;
        self.composer.dir_pending = None;
        self.composer.workflows_cwd = None;
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
        let cwd = self.focused_id().and_then(|id| self.pane_cwd(id));
        if self.composer.workflows_cwd != cwd || self.composer.workflows.workflows().is_empty() {
            self.composer.workflows = WorkflowSet::load(&self.config.commands, cwd.as_deref());
            self.composer.workflows_cwd = cwd.clone();
        }
        self.composer.state.refresh_ghost(
            self.composer.history.entries(),
            self.composer.workflows.workflows(),
        );
        self.maybe_request_composer_dir(cwd.as_deref());
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
        // Newest first: sort by timestamp descending, breaking ties by reverse insertion order.
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
        let proxy = self.proxy.clone();
        let _ = std::thread::Builder::new()
            .name("twitty-composer-history".into())
            .spawn(move || {
                let entries = load_shell_history_default();
                let _ = proxy.send_event(AppEvent::ComposerHistory(entries));
            });
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

    fn maybe_request_composer_dir(&mut self, cwd: Option<&std::path::Path>) {
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
        let proxy = self.proxy.clone();
        let _ = std::thread::Builder::new()
            .name("twitty-composer-dir".into())
            .spawn(move || {
                let entries = read_dir_entries(&dir, MAX_DIR_ENTRIES);
                let _ = proxy.send_event(AppEvent::ComposerDir(dir, entries));
            });
    }

    pub(super) fn on_composer_dir(&mut self, dir: PathBuf, entries: Vec<DirEntryInfo>) {
        if self.composer.dir_pending.as_ref() == Some(&dir) {
            self.composer.dir_pending = None;
        }
        self.composer.dir_cache = Some((dir, entries));
        let cwd = self.focused_id().and_then(|id| self.pane_cwd(id));
        let dir_cache = self
            .composer
            .dir_cache
            .as_ref()
            .map(|(d, e)| (d.as_path(), e.as_slice()));
        if self.composer.state.refresh_popup_if_open(
            self.composer.history.entries(),
            self.composer.workflows.workflows(),
            dir_cache,
            cwd.as_deref(),
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
        match key_shortcut(event, self.modifiers) {
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
        let action = self.composer.state.key(
            &event.logical_key,
            physical,
            event.text.as_deref(),
            self.modifiers,
            self.composer.history.entries(),
            self.composer.workflows.workflows(),
            dir_cache,
            cwd.as_deref(),
        );
        self.apply_composer_action(action, cwd.as_deref());
        Some(false)
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

    fn apply_composer_action(&mut self, action: ComposerAction, cwd: Option<&std::path::Path>) {
        match action {
            ComposerAction::None => {}
            ComposerAction::Redraw => {
                self.cursor_blink.reset(Instant::now());
                self.maybe_request_composer_dir(cwd);
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
            ComposerAction::Submit(cmd) => {
                if !cmd.trim().is_empty() {
                    self.composer.history.push_session(&cmd);
                }
                let mode = self.config.composer_mode();
                self.composer.manual_open = false;
                if mode == ComposerMode::OnDemand {
                    self.composer.focused = false;
                }
                if let Some(id) = self.focused_id() {
                    let bracketed = self.mode_of(id).contains(TermMode::BRACKETED_PASTE);
                    let bytes = encode_submit(&cmd, bracketed);
                    self.send_input(id, &bytes);
                    self.cursor_blink.reset(Instant::now());
                }
                self.window.request_redraw();
            }
        }
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
        Some(ComposerLayout::new(
            pane_rect,
            metrics,
            metrics.scale,
            line_count,
            caret_line,
            popup_items,
        ))
    }

    /// Pointer cursor icon when hovering over the composer.
    pub(super) fn composer_cursor(&self, x: f64, y: f64) -> Option<CursorIcon> {
        let layout = self.composer_layout()?;
        match layout.hit(x, y) {
            ComposerHit::Outside => None,
            ComposerHit::PopupItem(_) => Some(CursorIcon::Pointer),
            ComposerHit::Input => Some(CursorIcon::Text),
            ComposerHit::Card | ComposerHit::Popup => Some(CursorIcon::Default),
        }
    }

    /// Handle a mouse press when the composer is visible. Returns true if consumed.
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
                        self.composer.state.accept_popup_index(
                            idx,
                            self.composer.history.entries(),
                            self.composer.workflows.workflows(),
                        );
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
        let ed = &self.composer.state.editor;
        let lines = ed.lines().into_iter().map(str::to_owned).collect();
        let caret = ed.caret_line_col();
        let selection = ed.selection_line_cols();
        let preedit = ed.preedit().map(|p| (p.text.clone(), p.cursor));
        let popup = self.composer.state.popup.as_ref().map(|p| {
            let range = p.visible_range();
            let items = p.items[range.clone()]
                .iter()
                .map(|item| ComposerPopupItem {
                    kind: item.kind,
                    label: item.label.clone(),
                    detail: item.detail.clone(),
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
            focused: self.composer.focused && self.window_focused,
            cursor_blink_on: self.cursor_blink.visible(),
            popup,
            language: self.window.ui_language(),
        })
    }
}
