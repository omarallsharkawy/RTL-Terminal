//! The command palette: opening it, the keys and text typed while it is open, and running
//! the chosen command. Its notifications mode lists the notification history through the
//! same state and drawing, and jumps to the pane of the chosen one.
//!
//! While the palette is open it owns all keyboard, IME and pointer input so nothing can reach
//! panes or the PTY behind it. Clicking outside the palette closes it and consumes the click
//! (never starting a selection, pasting, or switching tabs).

use std::time::SystemTime;
use winit::event::{KeyEvent, MouseButton};

use super::App;
use crate::commands;
use crate::input::{key_shortcut, InputAction};
use crate::layout::PaneId;
use crate::render::scene::palette::{PaletteHit, PaletteItem, PaletteView};
use crate::ui::notifications::{jump_target, palette_item};
use crate::ui::palette::{matches, visible_window, PaletteCmd, PaletteMode, PaletteState};

impl App {
    pub(super) fn palette_open(&self) -> bool {
        self.palette.is_some()
    }

    /// Open the palette listing `mode` (a no-op when it already does, so `command.run
    /// palette.open` never closes it).
    pub(super) fn open_palette(&mut self, mode: PaletteMode) {
        if self.palette.as_ref().is_some_and(|p| p.mode == mode) {
            return;
        }
        self.reset_pointer_gesture();
        self.palette = Some(PaletteState {
            mode,
            ..PaletteState::default()
        });
        self.sync_palette();
    }

    /// The palette's own shortcuts: open `mode`, or close the palette when it already lists it.
    fn toggle_palette_mode(&mut self, mode: PaletteMode) {
        if self.palette.as_ref().is_some_and(|p| p.mode == mode) {
            self.close_palette();
        } else {
            self.open_palette(mode);
        }
    }

    pub(super) fn close_palette(&mut self) {
        if self.palette.take().is_some() {
            self.window.set_command_palette(None);
            self.window.request_redraw();
        }
    }

    /// The notification list is on screen (it is refreshed when a notification arrives).
    pub(super) fn notification_list_open(&self) -> bool {
        self.palette
            .as_ref()
            .is_some_and(|p| p.mode == PaletteMode::Notifications)
    }

    /// Hand the current query and results to the renderer.
    pub(super) fn sync_palette(&mut self) {
        let Some(state) = &self.palette else {
            return;
        };
        let (rows, placeholder): (Vec<PaletteItem>, _) = match state.mode {
            PaletteMode::Commands => {
                let all = commands::all();
                let rows = matches(&all, &state.query)
                    .into_iter()
                    .map(|i| PaletteItem {
                        title_en: all[i].title_en.to_string(),
                        title_ar: all[i].title_ar.to_string(),
                        shortcut: all[i].shortcut.map(str::to_string),
                        dimmed: false,
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
        };
        let shown = visible_window(rows.len(), state.selected);
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
        self.window.set_command_palette(Some(PaletteView {
            query: state.query.clone(),
            items,
            selected,
            placeholder,
        }));
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
            _ => {}
        }
        let modifiers = self.modifiers;
        let Some(state) = self.palette.as_mut() else {
            return false;
        };
        let (key, text) = (&event.logical_key, event.text.as_deref());
        let mut target: Option<PaneId> = None;
        let cmd = match state.mode {
            PaletteMode::Commands => state.key(&commands::all(), key, text, modifiers),
            PaletteMode::Notifications => {
                let list = self.notification_history.newest_first();
                let cmd = state.key_in(&list, key, text, modifiers);
                if let PaletteCmd::Choose(i) = cmd {
                    target = list
                        .get(i)
                        .and_then(|e| jump_target(e, |id| self.ws.pane(id).is_some()));
                }
                cmd
            }
        };
        match cmd {
            PaletteCmd::None => self.sync_palette(),
            PaletteCmd::Close => self.close_palette(),
            PaletteCmd::Run(name) => return self.run_palette_command(name),
            // A notification: jump to its pane, unless that pane is gone.
            PaletteCmd::Choose(_) => self.jump_or_stay(target),
        }
        false
    }

    /// Close the palette and run command `name`; choosing "palette.open" just closes it.
    fn run_palette_command(&mut self, name: &str) -> bool {
        self.close_palette();
        name != "palette.open" && self.run_command(name)
    }

    fn jump_or_stay(&mut self, target: Option<PaneId>) {
        match target {
            Some(id) => {
                self.close_palette();
                self.jump_to_pane(id);
            }
            None => self.sync_palette(),
        }
    }

    /// A mouse press while the palette is open. Clicking outside closes the palette and consumes
    /// the click so nothing reaches panes or tabs; left-clicking a row chooses it. Returns true
    /// when the window should close.
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
        let Some(state) = self.palette.as_ref() else {
            return false;
        };
        match state.mode {
            PaletteMode::Commands => {
                let all = commands::all();
                let results = matches(&all, &state.query);
                let shown = visible_window(results.len(), state.selected);
                match results.get(shown.start + row) {
                    Some(&i) => self.run_palette_command(all[i].name),
                    None => false,
                }
            }
            PaletteMode::Notifications => {
                let list = self.notification_history.newest_first();
                let results = matches(&list, &state.query);
                let shown = visible_window(results.len(), state.selected);
                let target = results
                    .get(shown.start + row)
                    .and_then(|&i| jump_target(list[i], |id| self.ws.pane(id).is_some()));
                if results.get(shown.start + row).is_some() {
                    self.jump_or_stay(target);
                }
                false
            }
        }
    }

    /// Text committed by an input method while the palette is open (Arabic typing).
    pub(super) fn palette_text(&mut self, text: &str) {
        if let Some(state) = self.palette.as_mut() {
            state.push_text(text);
            self.sync_palette();
        }
    }

    /// Focus pane `id`, switching to its tab.
    fn jump_to_pane(&mut self, id: PaneId) {
        if self.focus_pane_by_id(id).is_ok() {
            self.window.request_redraw();
        }
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
        if recorded && self.notification_list_open() {
            self.sync_palette();
        }
    }

    /// Run a command by its stable name on the focused pane. Returns true when the window
    /// should close.
    pub(super) fn run_command(&mut self, name: &str) -> bool {
        match (commands::action(name), self.focused_id()) {
            (Some(action), Some(id)) => self.run_action(id, action),
            _ => false,
        }
    }
}
