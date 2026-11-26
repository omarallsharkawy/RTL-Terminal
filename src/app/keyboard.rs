//! Keys, input-method text, the search bar, and the shortcut actions they trigger.

use alacritty_terminal::index::Direction;
use std::time::Instant;
use winit::event::{Ime, KeyEvent};
use winit::keyboard::{Key, ModifiersState, NamedKey};

use super::App;
use crate::blocks::{BlockList, BlockNav};
use crate::input::route::{route_key, shortcut_is_idle, KeyRoute};
use crate::input::{handle_key, key_shortcut, terminal_key, InputAction};
use crate::layout::{PaneId, SplitDir};
use crate::ui::palette::PaletteMode;
use crate::ui::search::{search_bar_text, search_key, SearchCmd};
use crate::ui::settings::DEFAULT_FONT_SIZE;

/// Esc pressed without modifiers.
fn is_plain_escape(event: &KeyEvent, modifiers: ModifiersState) -> bool {
    event.state.is_pressed()
        && event.logical_key == Key::Named(NamedKey::Escape)
        && modifiers.is_empty()
}

impl App {
    /// A key press or release. Returns true when the window should close.
    pub(super) fn on_key(&mut self, event: &KeyEvent) -> bool {
        if self.palette_open() {
            return self.handle_palette_key(event);
        }
        if let Some(close) = self.handle_composer_key(event) {
            return close;
        }
        if let Some(close) = self.handle_review_key(event) {
            return close;
        }
        let Some(id) = self.focused_id() else {
            return false;
        };
        let mode = self.mode_of(id);
        let mut action = handle_key(event, self.modifiers, mode);
        let single_pane = self.ws.single_pane();
        let shortcut = key_shortcut(event, self.modifiers);
        let blocks = self
            .ws
            .pane(id)
            .is_some_and(|p| p.blocks.navigable(&p.terminal));
        if shortcut_is_idle(shortcut.as_ref(), single_pane, self.ws.tabs().len(), blocks) {
            action = terminal_key(event, self.modifiers, mode).map(InputAction::Bytes);
        }
        let search_open = self
            .ws
            .pane(id)
            .is_some_and(|p| p.ui.search_query.is_some());
        if !search_open
            && event.state.is_pressed()
            && self.modifiers.is_empty()
            && event.logical_key == Key::Named(NamedKey::Enter)
            && self.manual_ssh_retry(id)
        {
            return false;
        }
        // Double-Esc turns broadcast input off cleanly without sending the second Esc.
        if !search_open && is_plain_escape(event, self.modifiers) && !event.repeat {
            if let Some(tab) = self.ws.active_tab() {
                if self.broadcast.on_plain_escape(tab.id, Instant::now()) {
                    self.window.request_redraw();
                    return false;
                }
            }
        } else if event.state.is_pressed() {
            self.broadcast.cancel_escape();
        }
        // Esc clears a selected block instead of reaching the shell.
        if !search_open && is_plain_escape(event, self.modifiers) && self.clear_block_selection(id)
        {
            return false;
        }
        // Holding a pane command should not toggle zoom repeatedly or jump past unread panes.
        if event.repeat
            && matches!(
                action,
                Some(
                    InputAction::TogglePaneZoom
                        | InputAction::CloseOtherPanes
                        | InputAction::JumpUnread
                        | InputAction::ToggleBroadcast
                )
            )
        {
            return false;
        }
        match route_key(action, search_open) {
            KeyRoute::SearchBar => {
                self.handle_search_key(id, event);
                false
            }
            KeyRoute::Action(InputAction::Bytes(bytes)) if self.active_tab_broadcast() => {
                let targets = self.broadcast_targets(id);
                for target in targets {
                    let out = if target == id {
                        bytes.clone()
                    } else {
                        terminal_key(event, self.modifiers, self.mode_of(target))
                            .unwrap_or_else(|| bytes.clone())
                    };
                    self.send_input(target, &out);
                }
                self.cursor_blink.reset(Instant::now());
                false
            }
            KeyRoute::Action(action) => self.run_action(id, action),
            KeyRoute::Ignore => false,
        }
    }

    pub(super) fn on_ime(&mut self, ime: Ime) {
        match ime {
            Ime::Enabled => {}
            Ime::Disabled => {
                if !self.composer_ime_preedit(None) {
                    self.update_preedit(None);
                }
            }
            Ime::Preedit(text, cursor) => {
                let norm = crate::ime::normalize_preedit(text, cursor);
                if !self.composer_ime_preedit(norm.clone()) {
                    self.update_preedit(norm);
                }
            }
            Ime::Commit(text) => {
                if !self.composer_ime_preedit(None) {
                    self.update_preedit(None);
                }
                if self.palette_open() {
                    self.palette_text(&text);
                    return;
                }
                if self.composer_ime_commit(&text) {
                    return;
                }
                if self.review_text(&text) {
                    return;
                }
                let Some(id) = self.focused_id() else { return };
                // While searching, composed text belongs to the query, not the shell.
                if let Some(query) = self
                    .ws
                    .pane_mut(id)
                    .and_then(|p| p.ui.search_query.as_mut())
                {
                    if crate::ui::search::push_search_text(query, &text) {
                        self.run_search(id, Direction::Left);
                    }
                    return;
                }
                self.broadcast.cancel_escape();
                self.send_user_input(id, text.as_bytes());
                self.cursor_blink.reset(Instant::now());
            }
        }
    }

    /// Whether broadcast input is currently enabled for the active tab.
    pub(super) fn active_tab_broadcast(&self) -> bool {
        self.ws
            .active_tab()
            .is_some_and(|tab| self.broadcast.is_active(tab.id))
    }

    /// Panes that should receive user keystrokes/pastes aimed at `id`.
    pub(super) fn broadcast_targets(&self, id: PaneId) -> Vec<PaneId> {
        let Some(tab) = self.ws.tabs().iter().find(|t| t.root.contains(id)) else {
            return vec![id];
        };
        let leaves = tab.root.leaves();
        self.broadcast.target_panes(tab.id, id, &leaves)
    }

    /// Toggle broadcast input for the active tab.
    fn toggle_broadcast(&mut self) {
        if let Some(tab) = self.ws.active_tab() {
            let tab_id = tab.id;
            self.broadcast.toggle(tab_id);
            self.window.request_redraw();
        }
    }

    /// Interactive user input: routes to every pane of the tab when broadcast is enabled.
    fn send_user_input(&mut self, id: PaneId, bytes: &[u8]) {
        for target in self.broadcast_targets(id) {
            self.send_input(target, bytes);
        }
    }

    /// Typed input for one shell: drops the selection and snaps the view to the live screen.
    pub(super) fn send_input(&mut self, id: PaneId, bytes: &[u8]) {
        self.clear_selection(id);
        self.clear_block_selection(id);
        self.write_to(id, bytes);
        self.scroll_to_bottom(id);
    }

    /// Run a shortcut action on the focused pane. Returns true when the window should close.
    pub(super) fn run_action(&mut self, id: PaneId, action: InputAction) -> bool {
        match action {
            InputAction::Bytes(bytes) => {
                self.send_user_input(id, &bytes);
                self.cursor_blink.reset(Instant::now());
            }
            InputAction::ZoomIn => self.zoom_by(1),
            InputAction::ZoomOut => self.zoom_by(-1),
            InputAction::ZoomReset => self.update_font_size(DEFAULT_FONT_SIZE),
            InputAction::Paste => self.paste_clipboard(id),
            InputAction::Copy => {
                self.copy_selection(id);
            }
            InputAction::Search => self.open_search(id),
            InputAction::NewTab => self.new_tab(),
            InputAction::ClosePane => return self.close_pane(id),
            InputAction::CloseOtherPanes => self.request_close_others(),
            InputAction::TogglePaneZoom => self.toggle_pane_zoom(),
            InputAction::FlashPane => self.window.flash_pane(id),
            InputAction::RenameTab => self.open_palette(PaletteMode::RenameTab),
            InputAction::JumpUnread => self.focus_newest_attention(),
            InputAction::NextTab => self.cycle_tab(1),
            InputAction::PrevTab => self.cycle_tab(-1),
            InputAction::MoveTabLeft => self.move_active_tab_by(-1),
            InputAction::MoveTabRight => self.move_active_tab_by(1),
            InputAction::ReopenClosedTab => self.reopen_closed_tab(),
            InputAction::OpenProject => self.open_palette(PaletteMode::Projects),
            InputAction::SaveLayout => self.open_palette(PaletteMode::SaveLayout),
            InputAction::OpenLayout => self.open_palette(PaletteMode::OpenLayout),
            InputAction::DeleteLayout => self.open_palette(PaletteMode::DeleteLayout),
            InputAction::ToggleBroadcast => self.toggle_broadcast(),
            InputAction::SplitRight => {
                let _ = self.split_focused(SplitDir::Horizontal);
            }
            InputAction::SplitDown => {
                let _ = self.split_focused(SplitDir::Vertical);
            }
            InputAction::Focus(dir) => self.focus_direction(dir),
            InputAction::SelectTab(index) => self.set_active_tab(index),
            InputAction::NextAttention => self.focus_next_attention(),
            InputAction::ToggleSidebar => self.toggle_sidebar(),
            InputAction::CommandPalette => self.open_palette(PaletteMode::Commands),
            InputAction::NotificationList => self.open_palette(PaletteMode::Notifications),
            InputAction::SearchAll => self.open_palette(PaletteMode::Search),
            InputAction::AskAgent => self.ask_agent(id),
            InputAction::ToggleReview => self.toggle_review(),
            InputAction::ToggleComposer => self.toggle_composer(),
            InputAction::PrevBlock => self.navigate_blocks(id, BlockList::select_prev),
            InputAction::NextBlock => self.navigate_blocks(id, BlockList::select_next),
            InputAction::CopyBlockOutput => self.copy_block_output(id),
        }
        false
    }

    // ---- command blocks ----

    /// Select another block and scroll its prompt to the top (or back to the live screen).
    fn navigate_blocks(&mut self, id: PaneId, step: fn(&mut BlockList) -> BlockNav) {
        let Some(pane) = self.ws.pane_mut(id) else {
            return;
        };
        pane.blocks.refresh(&pane.terminal);
        if !pane.blocks.navigable(&pane.terminal) {
            return;
        }
        match step(&mut pane.blocks) {
            BlockNav::ScrollTo(line) => pane.terminal.scroll_to_line(line),
            BlockNav::Bottom => pane.terminal.scroll_to_bottom(),
            BlockNav::None => {}
        }
        self.window.request_redraw();
    }

    /// Copy the output of the selected block (else the newest finished one) as plain text.
    fn copy_block_output(&mut self, id: PaneId) {
        let text = self
            .ws
            .pane(id)
            .and_then(|p| p.blocks.target_text(&p.terminal));
        if let Some(text) = text {
            self.clipboard.set_text(text.output);
        }
    }

    /// Clear the selected block of pane `id`. Returns true when one was selected.
    pub(super) fn clear_block_selection(&mut self, id: PaneId) -> bool {
        let cleared = self
            .ws
            .pane_mut(id)
            .is_some_and(|p| p.blocks.clear_selection());
        if cleared {
            self.window.request_redraw();
        }
        cleared
    }

    // ---- search bar ----

    /// Text of the search bar for the current query and match position.
    pub(super) fn search_bar_text(&self, id: PaneId, query: &str) -> String {
        match self.ws.pane(id).map(|p| &p.terminal) {
            Some(t) => search_bar_text(query, t.has_search_match(), || t.search_position()),
            None => format!("Find / بحث: {query}"),
        }
    }

    fn open_search(&mut self, id: PaneId) {
        self.clear_selection(id);
        if let Some(pane) = self.ws.pane_mut(id) {
            pane.ui.search_query = Some(String::new());
        }
        self.window.request_redraw();
    }

    fn close_search(&mut self, id: PaneId) {
        if let Some(pane) = self.ws.pane_mut(id) {
            pane.ui.search_query = None;
        }
        self.term(id, |t| t.clear_search());
        self.window.request_redraw();
    }

    /// Run the search for the current query in `direction` and update the bar.
    fn run_search(&mut self, id: PaneId, direction: Direction) {
        let Some(query) = self.ws.pane(id).and_then(|p| p.ui.search_query.clone()) else {
            return;
        };
        self.term(id, |t| t.search(&query, direction));
        self.window.request_redraw();
    }

    /// Key press while the search bar is open. Nothing reaches the PTY.
    fn handle_search_key(&mut self, id: PaneId, event: &KeyEvent) {
        if !event.state.is_pressed() {
            return;
        }
        let modifiers = self.modifiers;
        let Some(query) = self
            .ws
            .pane_mut(id)
            .and_then(|p| p.ui.search_query.as_mut())
        else {
            return;
        };
        match search_key(query, &event.logical_key, event.text.as_deref(), modifiers) {
            SearchCmd::Close => self.close_search(id),
            SearchCmd::Search(direction) => self.run_search(id, direction),
            SearchCmd::None => {}
        }
    }
}
