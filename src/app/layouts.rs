//! Named layouts integration: save the active tab's split tree, open a saved layout in a new
//! tab, and delete saved layouts.

use super::App;
use crate::layouts::{capture_active_tab, LayoutStore};
use crate::pty::SpawnOptions;
use crate::ui::chrome::tr;

impl App {
    /// Reload saved layouts from disk when opening a layout palette mode.
    pub(super) fn reload_layouts(&mut self) {
        self.layouts = LayoutStore::load_default();
    }

    /// Save the active tab's split tree, pane cwds, and titles under `name`.
    pub(super) fn save_named_layout(&mut self, name: &str) {
        let Some(layout) = capture_active_tab(name, &self.ws) else {
            return;
        };
        if let Err(err) = self.layouts.upsert(layout) {
            if let Some(id) = self.focused_id() {
                self.show_pane_warning(
                    id,
                    &format!("{}: {err}", tr("Could not save layout", "تعذر حفظ التخطيط")),
                );
            }
            return;
        }
        if let Err(err) = self.layouts.save_default() {
            if let Some(id) = self.focused_id() {
                self.show_pane_warning(
                    id,
                    &format!("{}: {err}", tr("Could not save layout", "تعذر حفظ التخطيط")),
                );
            }
        }
    }

    /// Recreate a saved layout by `name` in a new active tab.
    pub(super) fn open_named_layout(&mut self, name: &str) {
        let Some(prepared) = self.layouts.get(name).map(|l| l.prepare()) else {
            return;
        };
        let Some(grid) = self.window.grid() else {
            return;
        };
        let previous = self.focused_id();
        let socket = self.control_socket_path();
        let mut shells = super::panes::make_shell_factory(
            &self.proxy,
            &self.config,
            socket,
            SpawnOptions::default(),
        );
        let Some(focused) = prepared.open_in(&mut self.ws, grid, &mut shells) else {
            if let Some(id) = previous {
                self.show_pane_warning(
                    id,
                    tr(
                        "Could not open layout: the shells failed to start",
                        "تعذر فتح التخطيط: الـshell ما اشتغلش",
                    ),
                );
            }
            return;
        };
        self.reset_pointer_gesture();
        self.apply_layout();
        self.focus_changed(previous, focused);
    }

    /// Delete a saved layout by `name` and persist the updated store.
    pub(super) fn delete_named_layout(&mut self, name: &str) {
        if !self.layouts.delete(name) {
            return;
        }
        if let Err(err) = self.layouts.save_default() {
            if let Some(id) = self.focused_id() {
                self.show_pane_warning(
                    id,
                    &format!(
                        "{}: {err}",
                        tr("Could not delete layout", "تعذر حذف التخطيط")
                    ),
                );
            }
        }
    }
}
