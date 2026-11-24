//! The command palette: opening it, the keys, mouse clicks and text typed while it is open,
//! and running the chosen command.
//!
//! While the palette is open it owns all keyboard, IME and pointer input so nothing can reach
//! panes or the PTY behind it. Clicking outside the palette closes it and consumes the click
//! (never starting a selection, pasting, or switching tabs).

use winit::event::{KeyEvent, MouseButton};

use super::App;
use crate::commands;
use crate::input::{key_shortcut, InputAction};
use crate::render::scene::palette::{PaletteHit, PaletteItem, PaletteView};
use crate::ui::palette::{matches, PaletteCmd, PaletteState, MAX_RESULTS};

impl App {
    pub(super) fn palette_open(&self) -> bool {
        self.palette.is_some()
    }

    pub(super) fn open_palette(&mut self) {
        self.reset_pointer_gesture();
        self.palette = Some(PaletteState::default());
        self.sync_palette();
    }

    pub(super) fn close_palette(&mut self) {
        if self.palette.take().is_some() {
            self.window.set_command_palette(None);
            self.window.request_redraw();
        }
    }

    /// Hand the current query and results to the renderer.
    fn sync_palette(&mut self) {
        let Some(state) = &self.palette else {
            return;
        };
        let all = commands::all();
        let items = matches(&all, &state.query)
            .into_iter()
            .take(MAX_RESULTS)
            .map(|i| PaletteItem {
                title_en: all[i].title_en.to_string(),
                title_ar: all[i].title_ar.to_string(),
                shortcut: all[i].shortcut.map(str::to_string),
            })
            .collect::<Vec<_>>();
        let selected = state.selected.min(items.len().saturating_sub(1));
        self.window.set_command_palette(Some(PaletteView {
            query: state.query.clone(),
            items,
            selected,
        }));
        self.window.request_redraw();
    }

    /// A key while the palette is open; nothing reaches the program. Returns true when the
    /// window should close (the command run closed the last pane).
    pub(super) fn handle_palette_key(&mut self, event: &KeyEvent) -> bool {
        if !event.state.is_pressed() {
            return false;
        }
        // The palette shortcut closes it again.
        if key_shortcut(event, self.modifiers) == Some(InputAction::CommandPalette) {
            self.close_palette();
            return false;
        }
        let modifiers = self.modifiers;
        let all = commands::all();
        let Some(state) = self.palette.as_mut() else {
            return false;
        };
        match state.key(&all, &event.logical_key, event.text.as_deref(), modifiers) {
            PaletteCmd::None => self.sync_palette(),
            PaletteCmd::Close => self.close_palette(),
            PaletteCmd::Run(name) => {
                self.close_palette();
                if name == "palette.open" {
                    return false;
                }
                return self.run_command(name);
            }
        }
        false
    }

    /// A mouse press while the palette is open. Clicking outside closes the palette and consumes
    /// the click so nothing reaches panes or tabs; left-clicking an item row runs it. Returns
    /// true when the window should close.
    pub(super) fn on_palette_mouse_press(&mut self, button: MouseButton) -> bool {
        let (x, y) = self.pointer.pos;
        match self.window.palette_hit(x, y) {
            PaletteHit::Outside => {
                self.close_palette();
                false
            }
            PaletteHit::Inside => false,
            PaletteHit::Item(index) if button == MouseButton::Left => {
                let name = self.palette.as_ref().and_then(|state| {
                    let all = commands::all();
                    matches(&all, &state.query)
                        .into_iter()
                        .take(MAX_RESULTS)
                        .nth(index)
                        .map(|i| all[i].name)
                });
                self.close_palette();
                match name {
                    Some("palette.open") | None => false,
                    Some(name) => self.run_command(name),
                }
            }
            PaletteHit::Item(_) => false,
        }
    }

    /// Text committed by an input method while the palette is open (Arabic typing).
    pub(super) fn palette_text(&mut self, text: &str) {
        if let Some(state) = self.palette.as_mut() {
            state.push_text(text);
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
