//! Pointer events: hover and link underlines, presses on tabs, dividers and panes, mouse
//! reporting to programs, divider dragging, and the wheel.

use alacritty_terminal::term::TermMode;
use winit::event::{MouseButton, MouseScrollDelta};
use winit::keyboard::ModifiersState;
use winit::window::CursorIcon;

use super::App;
use crate::input::encode_mouse_for_mode;
use crate::input::mouse::{
    button_code, button_report, motion_report, wheel_action, wheel_lines, WheelAction,
    LINES_PER_WHEEL_NOTCH,
};
use crate::input::pointer::{cursor_icon_for, PressTarget};
use crate::layout::{ratio_at, Divider, PaneId, SplitDir};

impl App {
    pub(super) fn on_modifiers(&mut self, modifiers: ModifiersState) {
        self.modifiers = modifiers;
        if let Some(id) = self.pointer.hover.or(self.focused_id()) {
            self.update_link_hover(id);
        }
    }

    pub(super) fn on_cursor_moved(&mut self, x: f64, y: f64) {
        self.pointer.pos = (x, y);
        if let Some(divider) = self.pointer.drag.clone() {
            self.drag_divider_to(&divider);
            return;
        }
        let hit = self.window.hit_test(&self.ws, x, y);
        let captured = self.pointer.capture.is_some();
        if let Some(icon) = cursor_icon_for(&hit, captured, self.window.cursor_icon()) {
            self.window.set_cursor_icon(icon);
        }
        let (target, left) = self.pointer.moved(&hit);
        if let Some(old) = left {
            self.clear_link_hover(old);
        }
        let Some(id) = target else { return };
        let Some(rect) = self.pane_rect(id) else {
            return;
        };
        let Some((col, row, side)) = self.window.cell_at(id, rect, x, y) else {
            return;
        };
        let Some(pane) = self.ws.pane_mut(id) else {
            return;
        };
        pane.ui.mouse_side = side;
        if pane.ui.selection.selecting {
            pane.terminal.update_selection(col, row, side);
            self.window.request_redraw();
        }
        let Some(pane) = self.ws.pane_mut(id) else {
            return;
        };
        if (col, row) == (pane.ui.mouse_col, pane.ui.mouse_row) {
            return;
        }
        pane.ui.mouse_col = col;
        pane.ui.mouse_row = row;
        let (down, selecting) = (pane.ui.mouse_button_down, pane.ui.selection.selecting);
        self.update_link_hover(id);

        if let Some(button) = motion_report(self.mode_of(id), down, selecting) {
            self.send_mouse(id, button, true);
        }
    }

    pub(super) fn on_mouse_input(&mut self, button: MouseButton, pressed: bool) {
        let target = if pressed {
            let (x, y) = self.pointer.pos;
            let hit = self.window.hit_test(&self.ws, x, y);
            match self.pointer.press(hit, button == MouseButton::Left) {
                PressTarget::Tab(i) => {
                    self.set_active_tab(i);
                    None
                }
                PressTarget::Pane(id) => {
                    self.focus_pane(id);
                    Some(id)
                }
                PressTarget::None => None,
            }
        } else {
            self.pointer.release()
        };
        if let Some(id) = target {
            self.handle_mouse_button(id, button, pressed);
        }
    }

    pub(super) fn on_wheel(&mut self, delta: MouseScrollDelta) {
        let line_height = self.window.line_height().unwrap_or(20.0);
        let lines = wheel_lines(delta, line_height);

        if self.modifiers.control_key() {
            // Ctrl (+ Shift) + wheel: one zoom (opacity) step per notch. Trackpads send
            // many tiny deltas, so they are summed into notches first.
            let steps = self.zoom_steps.add(lines / LINES_PER_WHEEL_NOTCH);
            if steps == 0 {
                return;
            }
            if self.modifiers.shift_key() {
                self.step_opacity_by(steps);
            } else {
                self.zoom_by(steps);
            }
            return;
        }

        // The wheel scrolls the pane under the pointer.
        let Some(id) = self.pane_at(self.pointer.pos.0, self.pointer.pos.1) else {
            return;
        };
        // Accumulate so trackpads scroll smoothly instead of 3 lines per tiny event.
        let Some(pane) = self.ws.pane_mut(id) else {
            return;
        };
        let steps = pane.ui.scroll_steps.add(lines);
        if steps == 0 {
            return;
        }
        match wheel_action(pane.mode(), steps) {
            WheelAction::Report { button, count } => {
                for _ in 0..count {
                    self.send_mouse(id, button, true);
                }
            }
            WheelAction::Keys(keys) => self.write_to(id, &keys),
            WheelAction::Scrollback(steps) => {
                self.term(id, |t| t.scroll_display(steps));
                self.window.request_redraw();
            }
            WheelAction::None => {}
        }
    }

    /// Underline the link under the pointer while Ctrl is held, and show a pointer cursor.
    fn update_link_hover(&mut self, id: PaneId) {
        let Some((col, row)) = self.ws.pane(id).map(|p| (p.ui.mouse_col, p.ui.mouse_row)) else {
            return;
        };
        let ctrl = self.modifiers.control_key();
        let result = self.term(id, |t| {
            let link = if ctrl { t.link_at(col, row) } else { None };
            let hover = link.as_ref().map(|l| (l.row, l.cols.clone()));
            (t.set_link_hover(hover), link.is_some())
        });
        if let Some((true, over_link)) = result {
            self.window.set_cursor_icon(if over_link {
                CursorIcon::Pointer
            } else {
                CursorIcon::Text
            });
            self.window.request_redraw();
        }
    }

    /// The pointer left a pane: its link underline goes away.
    fn clear_link_hover(&mut self, id: PaneId) {
        if self.term(id, |t| t.set_link_hover(None)) == Some(true) {
            self.window.request_redraw();
        }
    }

    fn send_mouse(&self, id: PaneId, button: u8, pressed: bool) {
        let Some(pane) = self.ws.pane(id) else {
            return;
        };
        let seq = encode_mouse_for_mode(
            button,
            pane.ui.mouse_col,
            pane.ui.mouse_row,
            pressed,
            self.modifiers,
            pane.mode(),
        );
        pane.write(&seq);
    }

    /// A mouse button was pressed or released inside pane `id`.
    fn handle_mouse_button(&mut self, id: PaneId, button: MouseButton, pressed: bool) {
        let mode = self.mode_of(id);
        let Some(code) = button_code(button) else {
            return;
        };
        // Ctrl+Click opens a link and takes priority over mouse reporting.
        if button == MouseButton::Left {
            let Some((col, row, link_press)) = self
                .ws
                .pane(id)
                .map(|p| (p.ui.mouse_col, p.ui.mouse_row, p.ui.link_press))
            else {
                return;
            };
            if pressed && self.modifiers.control_key() {
                let link = self.term(id, |t| t.link_at(col, row)).flatten();
                if let Some(link) = link {
                    crate::links::open_url(&link.uri);
                    if let Some(p) = self.ws.pane_mut(id) {
                        p.ui.link_press = true;
                    }
                    return;
                }
            } else if !pressed && link_press {
                if let Some(p) = self.ws.pane_mut(id) {
                    p.ui.link_press = false;
                }
                return;
            }
        }
        let held = self.ws.pane(id).and_then(|p| p.ui.mouse_button_down);
        let (report, held) = button_report(pressed, mode, self.modifiers.shift_key(), held, code);
        if let Some(p) = self.ws.pane_mut(id) {
            p.ui.mouse_button_down = held;
        }

        if report {
            if mode.intersects(TermMode::MOUSE_MODE) {
                self.send_mouse(id, code, pressed);
            }
        } else if button == MouseButton::Left {
            if pressed {
                self.start_selection(id);
            } else if self.ws.pane(id).is_some_and(|p| p.ui.selection.selecting) {
                self.finish_selection(id);
            }
        } else if pressed && button == MouseButton::Middle {
            self.paste_clipboard(id);
        }
    }

    /// Release every button a program saw pressed, e.g. when the window loses focus mid-drag and
    /// the real release may never arrive. Otherwise later motion is reported as a drag.
    pub(super) fn release_held_buttons(&mut self) {
        let held: Vec<(PaneId, u8)> = self
            .ws
            .panes()
            .filter_map(|p| p.ui.mouse_button_down.map(|b| (p.id, b)))
            .collect();
        for (id, button) in held {
            if self.mode_of(id).intersects(TermMode::MOUSE_MODE) {
                self.send_mouse(id, button, false);
            }
            if let Some(p) = self.ws.pane_mut(id) {
                p.ui.mouse_button_down = None;
            }
        }
    }

    fn drag_divider_to(&mut self, divider: &Divider) {
        let pos = match divider.dir {
            SplitDir::Horizontal => self.pointer.pos.0,
            SplitDir::Vertical => self.pointer.pos.1,
        };
        let ratio = ratio_at(divider, pos, self.window.gap());
        if self.ws.set_ratio(&divider.path, ratio) {
            self.apply_layout();
        }
    }
}
