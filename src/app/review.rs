//! The code review panel's hooks: opening it, loading the focused repository's changes on a
//! worker thread, its keys, clicks and wheel, and carrying out what it asks for (a comment
//! typed into an agent pane, `git commit` typed into the focused pane). The logic is in
//! `crate::review`.

use std::time::Instant;

use alacritty_terminal::term::TermMode;
use winit::event::{KeyEvent, MouseButton};
use winit::keyboard::PhysicalKey;
use winit::window::CursorIcon;

use super::agent::AgentStartError;
use super::event::AppEvent;
use super::App;
use crate::input::route::shortcut_is_idle;
use crate::input::{key_shortcut, InputAction};
use crate::layout::PaneId;
use crate::pty::Pty;
use crate::review::comment::{choose_agent_pane, format_comment, CommentTarget, PaneInfo};
use crate::review::layout::{PanelHit, PanelLayout};
use crate::review::panel::{View, COMMIT_LABEL};
use crate::review::{LoadError, PanelCmd, Snapshot};
use crate::workflows::encode_insert;

impl App {
    /// `review.toggle`: open the panel (focused, loading the changes) or close it.
    pub(super) fn toggle_review(&mut self) {
        if self.review.is_open() {
            self.close_review();
            return;
        }
        self.reset_pointer_gesture();
        self.review.open();
        self.window.set_review_open(true);
        self.apply_layout();
        self.refresh_review();
    }

    fn close_review(&mut self) {
        self.review.close();
        self.window.set_review_open(false);
        self.apply_layout();
        self.refresh_pointer_hover();
    }

    /// Load the focused pane's repository on a worker thread.
    pub(super) fn refresh_review(&mut self) {
        if !self.review.is_open() {
            return;
        }
        let Some(cwd) = self.focused_id().and_then(|id| self.pane_cwd(id)) else {
            self.review.set_no_folder();
            self.window.request_redraw();
            return;
        };
        let Some(generation) = self.review.begin_refresh() else {
            return;
        };
        let proxy = self.proxy.clone();
        let spawned = std::thread::Builder::new()
            .name("twitty-review".into())
            .spawn(move || {
                let result = crate::review::git::load(&cwd);
                let _ = proxy.send_event(AppEvent::Review(generation, result));
            });
        if spawned.is_err() {
            self.review.finish_refresh(
                generation,
                Err(LoadError::Failed("could not start a thread".into())),
            );
        }
        self.window.request_redraw();
    }

    pub(super) fn on_review_loaded(
        &mut self,
        generation: u64,
        result: Result<Snapshot, LoadError>,
    ) {
        if self.review.finish_refresh(generation, result) {
            self.refresh_review();
        }
        self.window.request_redraw();
    }

    /// A command ended in pane `id`: what it did may have changed the files.
    pub(super) fn review_command_ended(&mut self, id: PaneId) {
        if self.review.is_open() && self.focused_id() == Some(id) {
            self.refresh_review();
        }
    }

    fn review_layout(&self) -> Option<PanelLayout> {
        let rect = self.window.review_rect(&self.ws)?;
        let m = self.window.metrics()?;
        Some(PanelLayout::new(
            rect,
            m.line_height,
            m.char_width,
            m.scale,
            self.review.notice().is_some(),
            COMMIT_LABEL.chars().count(),
        ))
    }

    fn review_page(&self) -> usize {
        self.review_layout().map_or(1, |l| l.page_rows())
    }

    /// What the panel draws, while it is open and fits in the window.
    pub(super) fn review_view(&self) -> Option<View> {
        if !self.review.is_open() {
            return None;
        }
        let layout = self.review_layout()?;
        Some(self.review.view(layout.panel, layout.page_rows()))
    }

    /// The panel takes the keyboard: it is open, on screen and was clicked or just opened.
    fn review_has_keys(&self) -> bool {
        self.review.is_focused() && self.window.review_rect(&self.ws).is_some()
    }

    /// A key while the panel has the keyboard. `None` lets the key go on to the pane (Twitty's
    /// own shortcuts); `Some(close)` means it was handled.
    pub(super) fn handle_review_key(&mut self, event: &KeyEvent) -> Option<bool> {
        if !self.review_has_keys() {
            return None;
        }
        match key_shortcut(event, self.modifiers) {
            // Pasting while typing a comment goes into the comment.
            Some(InputAction::Paste) if self.review.is_editing() => {
                if event.state.is_pressed() {
                    if let Some(text) = self.clipboard.text() {
                        let cmd = self.review.text_input(&text);
                        self.run_review_cmd(cmd);
                    }
                }
                return Some(false);
            }
            Some(InputAction::Bytes(_)) | None => {}
            Some(action) => {
                // A shortcut with nothing to act on would send its key to the pane instead.
                let blocks = self
                    .focused_id()
                    .and_then(|id| self.ws.pane(id))
                    .is_some_and(|p| p.blocks.navigable(&p.terminal));
                let tabs = self.ws.tabs().len();
                if shortcut_is_idle(Some(&action), self.ws.single_pane(), tabs, blocks) {
                    return Some(false);
                }
                // Moving to another pane or tab gives the keyboard back to the terminal.
                if matches!(
                    action,
                    InputAction::Focus(_)
                        | InputAction::SelectTab(_)
                        | InputAction::NextTab
                        | InputAction::PrevTab
                        | InputAction::NewTab
                        | InputAction::SplitRight
                        | InputAction::SplitDown
                        | InputAction::ClosePane
                        | InputAction::NextAttention
                ) {
                    self.review.set_focused(false);
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
        let page = self.review_page();
        let cmd = self.review.key(
            &event.logical_key,
            physical,
            event.text.as_deref(),
            self.modifiers,
            page,
        );
        Some(self.run_review_cmd(cmd))
    }

    /// Text from an input method. Returns true when the panel took it.
    pub(super) fn review_text(&mut self, text: &str) -> bool {
        if !self.review_has_keys() {
            return false;
        }
        let cmd = self.review.text_input(text);
        self.run_review_cmd(cmd);
        true
    }

    /// A mouse press: inside the panel it is the panel's (true); outside, the panel gives the
    /// keyboard back and the press goes on (false).
    pub(super) fn review_mouse_press(&mut self, button: MouseButton) -> bool {
        let Some(layout) = self.review_layout() else {
            return false;
        };
        let (x, y) = self.pointer.pos;
        let rows = self.review.rows().len();
        let gutter = self.review_view().map_or(0, |v| v.gutter_cols);
        let top = self.review.visible_top(layout.page_rows());
        let hit = layout.hit(x, y, top, rows, gutter);
        if hit == PanelHit::Outside {
            if self.review.is_focused() {
                self.review.set_focused(false);
                self.window.request_redraw();
            }
            return false;
        }
        if button == MouseButton::Left {
            let cmd = self.review.click(hit, layout.page_rows());
            self.run_review_cmd(cmd);
        } else if !self.review.is_focused() {
            self.review.set_focused(true);
            self.window.request_redraw();
        }
        true
    }

    /// The wheel over the panel scrolls it. Returns true when the pointer is over it.
    pub(super) fn review_wheel(&mut self, lines: f64) -> bool {
        let Some(rect) = self.window.review_rect(&self.ws) else {
            return false;
        };
        let (x, y) = self.pointer.pos;
        if !rect.contains(x, y) {
            return false;
        }
        let page = self.review_page();
        if self.review.wheel(lines, page) {
            self.window.request_redraw();
        }
        true
    }

    /// The pointer icon over the panel (`None` when not over it).
    pub(super) fn review_cursor(&self, x: f64, y: f64) -> Option<CursorIcon> {
        let layout = self.review_layout()?;
        let gutter = self.review_view().map_or(0, |v| v.gutter_cols);
        let top = self.review.visible_top(layout.page_rows());
        let hit = layout.hit(x, y, top, self.review.rows().len(), gutter);
        Some(match hit {
            PanelHit::Outside => return None,
            PanelHit::Close | PanelHit::Refresh | PanelHit::Commit => CursorIcon::Pointer,
            PanelHit::Row { gutter: true, .. } => CursorIcon::Pointer,
            _ => CursorIcon::Default,
        })
    }

    /// Carry out what the panel asked for. Returns true when the window should close (never).
    fn run_review_cmd(&mut self, cmd: PanelCmd) -> bool {
        match cmd {
            PanelCmd::None => return false,
            PanelCmd::Redraw => {}
            PanelCmd::Unfocus => self.review.set_focused(false),
            PanelCmd::Close => {
                self.close_review();
                return false;
            }
            PanelCmd::Refresh => {
                self.review.clear_notice();
                self.refresh_review();
            }
            PanelCmd::Submit(target) => self.send_review_comment(&target),
            PanelCmd::StartAgent(target) => self.start_review_agent(&target),
            PanelCmd::Commit => {
                if let Some(id) = self.focused_id() {
                    self.insert_text(id, "git commit");
                    self.review.set_focused(false);
                }
            }
        }
        self.window.request_redraw();
        false
    }

    /// Type `text` into pane `id` as a paste, without a newline: nothing runs until the user
    /// presses Enter there.
    fn insert_text(&mut self, id: PaneId, text: &str) {
        let bracketed = self.mode_of(id).contains(TermMode::BRACKETED_PASTE);
        let bytes = encode_insert(text, bracketed);
        self.send_input(id, &bytes);
        self.cursor_blink.reset(Instant::now());
    }

    /// The pane of the active tab that runs a coding agent: the focused one if it does.
    fn review_agent_pane(&self) -> Option<PaneId> {
        let tab = self.ws.active_tab()?;
        let focused = self.focused_id();
        let configured: Vec<&str> = self
            .config
            .agent_command
            .as_deref()
            .and_then(|c| c.split_whitespace().next())
            .into_iter()
            .collect();
        let panes: Vec<PaneInfo> = tab
            .root
            .leaves()
            .into_iter()
            .filter_map(|id| {
                let pane = self.ws.pane(id)?;
                Some(PaneInfo {
                    id: Some(id),
                    focused: focused == Some(id),
                    program: pane.pty.as_ref().and_then(Pty::program_name),
                    running_command: pane
                        .meta
                        .last_command
                        .as_deref()
                        .filter(|_| pane.meta.running),
                    agent_status: pane.meta.agent_status.is_some(),
                })
            })
            .collect();
        choose_agent_pane(&panes, &configured)
    }

    fn send_review_comment(&mut self, target: &CommentTarget) {
        let text = format_comment(target);
        if text.is_empty() {
            return;
        }
        match self.review_agent_pane() {
            Some(id) => {
                self.insert_text(id, &text);
                let name = self
                    .ws
                    .pane(id)
                    .and_then(|p| p.pty.as_ref().and_then(Pty::program_name))
                    .filter(|n| crate::review::comment::runs_agent(n, &[]))
                    .map(str::to_owned);
                self.review.comment_sent(name.as_deref());
            }
            None => self.review.offer_agent(),
        }
    }

    fn start_review_agent(&mut self, target: &CommentTarget) {
        let text = format_comment(target);
        if text.is_empty() {
            return;
        }
        let prompt = format!(
            "A review comment on my uncommitted changes (git diff HEAD):\n{}",
            text.trim_end()
        );
        let cwd = self
            .review
            .snapshot()
            .map(|s| s.root.clone())
            .or_else(|| self.focused_id().and_then(|id| self.pane_cwd(id)));
        match self.start_agent(cwd, &prompt) {
            Ok(name) => {
                self.review.agent_started(&name);
                // The new agent pane has the keyboard now.
                self.review.set_focused(false);
            }
            Err(AgentStartError::NotFound) => self.review.warn(
                "No coding agent found (claude, codex, gemini, opencode, grok); set agent_command / مفيش وكيل متسطّب",
            ),
            Err(AgentStartError::Failed) => {
                self.review.warn("Could not start the agent / مقدرناش نشغّل الوكيل")
            }
        }
    }
}
