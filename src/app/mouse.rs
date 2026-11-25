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
use crate::ui::chrome::ChromeAction;
use crate::ui::hit::Hit;

impl App {
    pub(super) fn on_modifiers(&mut self, modifiers: ModifiersState) {
        self.modifiers = modifiers;
        if self.palette_open() {
            return;
        }
        let (x, y) = self.pointer.pos;
        if let Some(id) = self.pane_at(x, y).or(self.pointer.hover) {
            self.update_link_hover(id);
        }
    }

    pub(super) fn on_cursor_moved(&mut self, x: f64, y: f64) {
        self.pointer.pos = (x, y);
        self.window.update_sidebar_drag();
        if self.palette_open() {
            let hit = self.window.palette_hit(x, y);
            let icon = if matches!(hit, crate::render::scene::palette::PaletteHit::Item(_)) {
                CursorIcon::Pointer
            } else {
                CursorIcon::Default
            };
            self.window.set_cursor_icon(icon);
            return;
        }
        if let Some(divider) = self.pointer.drag.clone() {
            self.drag_divider_to(&divider);
            return;
        }
        let hit = self.window.hit_test(&self.ws, x, y);
        let over_sidebar = self.window.sidebar_contains(&self.ws, x, y);
        let captured = self.pointer.capture.is_some();
        self.update_chrome_hover(&hit, captured);
        if let Some(icon) = self.review_cursor(x, y).filter(|_| !captured) {
            self.window.set_cursor_icon(icon);
        } else if let Some(icon) = cursor_icon_for(&hit, captured, self.window.cursor_icon()) {
            self.window.set_cursor_icon(icon);
        }
        let previous_hover = self.pointer.hover;
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
        let selecting = pane.ui.selection.selecting;
        if selecting {
            pane.terminal.update_selection(col, row, side);
            self.window.request_redraw();
        }
        if over_sidebar || matches!(hit, Hit::Chrome(_) | Hit::ChromeBackground) {
            // A selection may extend to its pane's edge, but programs only receive the
            // release of their captured press, using the last reported pane cell.
            self.clear_link_hover(id);
            return;
        }
        pane.ui.mouse_side = side;
        let changed = (col, row) != (pane.ui.mouse_col, pane.ui.mouse_row);
        (pane.ui.mouse_col, pane.ui.mouse_row) = (col, row);
        let down = pane.ui.mouse_button_down;
        if changed || previous_hover != Some(id) || self.modifiers.control_key() {
            let over_pane = hit == Hit::Pane(id);
            let cell = (over_pane && self.modifiers.control_key()).then_some((col, row));
            self.set_link_hover(id, over_pane, cell);
        }

        if changed {
            if let Some(button) = motion_report(self.mode_of(id), down, selecting) {
                self.send_mouse(id, button, true);
            }
        }
    }

    pub(super) fn on_cursor_left(&mut self) {
        self.window.set_chrome_hover(None, None);
        self.window.cancel_sidebar_drag();
        if self.pointer.capture.is_none() && self.pointer.drag.is_none() {
            self.pointer.pos = (f64::NEG_INFINITY, f64::NEG_INFINITY);
            if let Some(id) = self.pointer.hover.take() {
                self.clear_link_hover(id);
            }
            self.window.set_cursor_icon(CursorIcon::Default);
        }
    }

    /// Returns true when the press closed the last pane and the window should exit.
    pub(super) fn on_mouse_input(&mut self, button: MouseButton, pressed: bool) -> bool {
        if self.palette_open() {
            return pressed && self.on_palette_mouse_press(button);
        }
        if pressed && self.review_mouse_press(button) {
            return false;
        }
        let Some(code) = button_code(button) else {
            return false;
        };
        let target = if pressed {
            let (x, y) = self.pointer.pos;
            let hit = self.window.hit_test(&self.ws, x, y);
            if let Hit::Chrome(action) = hit {
                return button == MouseButton::Left
                    && self.pointer.capture.is_none()
                    && self.pointer.drag.is_none()
                    && self.click_chrome(action);
            }
            match self.pointer.press(hit, button) {
                PressTarget::Tab(i) => {
                    let sidebar = self.window.sidebar_contains(&self.ws, x, y);
                    self.set_active_tab(i);
                    if sidebar {
                        if let Some(tab) = self.ws.tabs().get(i) {
                            self.window.start_sidebar_drag(tab.id, y);
                        }
                    }
                    self.refresh_pointer_hover();
                    None
                }
                PressTarget::Pane(id) => {
                    self.focus_pane(id);
                    // Geometry can change under a stationary pointer (e.g. sidebar toggle).
                    self.update_mouse_cell(id);
                    let gutter = button == MouseButton::Left && self.click_block_gutter(id);
                    (!gutter).then_some(id)
                }
                PressTarget::None => None,
            }
        } else {
            if button == MouseButton::Left {
                if let Some(index) = self.window.sidebar_drop(&self.ws, self.pointer.pos) {
                    if self.ws.move_active_tab(index) {
                        self.window.request_redraw();
                    }
                }
            }
            self.pointer.release(button)
        };
        if let Some(id) = target {
            self.handle_mouse_button(id, button, code, pressed);
        }
        false
    }

    fn click_chrome(&mut self, action: ChromeAction) -> bool {
        match action {
            ChromeAction::Sidebar => self.run_command("sidebar.toggle"),
            ChromeAction::Search => self.run_command("palette.open"),
            ChromeAction::Diff => self.run_command("review.toggle"),
            ChromeAction::Notifications => self.run_command("notifications.list"),
            ChromeAction::NewTab => self.run_command("tab.new"),
            ChromeAction::CloseTab(index) => self.close_tab(index),
            ChromeAction::FocusPane(id) => {
                self.focus_pane(id);
                false
            }
            ChromeAction::SplitRight(id) | ChromeAction::SplitDown(id) => {
                self.focus_pane(id);
                let dir = if matches!(action, ChromeAction::SplitRight(_)) {
                    SplitDir::Horizontal
                } else {
                    SplitDir::Vertical
                };
                self.split_focused(dir);
                self.refresh_pointer_hover();
                false
            }
            ChromeAction::Close(id) => {
                let exit = self.close_pane(id);
                self.refresh_pointer_hover();
                exit
            }
        }
    }

    fn update_chrome_hover(&mut self, hit: &Hit, captured: bool) {
        let hover = match hit {
            Hit::Chrome(action) if !captured => Some(*action),
            _ => None,
        };
        let tab = match hit {
            Hit::Tab(i) if !captured => Some(*i),
            _ => None,
        };
        self.window.set_chrome_hover(hover, tab);
    }

    pub(super) fn on_wheel(&mut self, delta: MouseScrollDelta) {
        if self.palette_open() {
            return;
        }
        let line_height = self.window.line_height().unwrap_or(20.0);
        let lines = wheel_lines(delta, line_height);
        if self.review_wheel(lines) {
            return;
        }
        if self
            .window
            .scroll_sidebar(&self.ws, self.pointer.pos.0, self.pointer.pos.1, lines)
        {
            return;
        }

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
                self.update_mouse_cell(id);
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

    /// Underline the link under the pointer while Ctrl is held, show a pointer cursor and the
    /// link's target. Also called after output, which can change the link under a still pointer.
    pub(super) fn update_link_hover(&mut self, id: PaneId) {
        let (x, y) = self.pointer.pos;
        let over_pane = self.pane_at(x, y) == Some(id)
            && matches!(self.window.hit_test(&self.ws, x, y), Hit::Pane(hit_id) if hit_id == id);
        let cell = (over_pane && self.modifiers.control_key())
            .then(|| {
                self.pane_rect(id)
                    .and_then(|r| self.window.cell_at(id, r, x, y))
                    .map(|(col, row, _)| (col, row))
            })
            .flatten();
        self.set_link_hover(id, over_pane, cell);
    }

    fn set_link_hover(&mut self, id: PaneId, over_pane: bool, cell: Option<(usize, usize)>) {
        let result = self.term(id, |t| {
            let link = cell.and_then(|(col, row)| t.link_at(col, row));
            let hover = link.as_ref().map(|l| (l.row, l.cols.clone()));
            (t.set_link_hover(hover), link)
        });
        let Some((cells_changed, link)) = result else {
            return;
        };
        let over_link = link.is_some();
        let target = link.map(|l| crate::links::display_target(&l.uri));
        let target_changed = self
            .ws
            .pane_mut(id)
            .is_some_and(|p| std::mem::replace(&mut p.ui.hover_target, target.clone()) != target);
        if over_pane {
            self.window.set_cursor_icon(if over_link {
                CursorIcon::Pointer
            } else {
                CursorIcon::Text
            });
        }
        if cells_changed || target_changed {
            self.window.request_redraw();
        }
    }

    /// The pointer left a pane: its link underline goes away.
    fn clear_link_hover(&mut self, id: PaneId) {
        let target_changed = self
            .ws
            .pane_mut(id)
            .is_some_and(|p| p.ui.hover_target.take().is_some());
        if self.term(id, |t| t.set_link_hover(None)) == Some(true) || target_changed {
            self.window.request_redraw();
        }
    }

    fn update_mouse_cell(&mut self, id: PaneId) {
        let (x, y) = self.pointer.pos;
        let cell = self
            .pane_rect(id)
            .and_then(|r| self.window.cell_at(id, r, x, y));
        if let (Some(pane), Some((col, row, side))) = (self.ws.pane_mut(id), cell) {
            (pane.ui.mouse_col, pane.ui.mouse_row, pane.ui.mouse_side) = (col, row, side);
        }
    }

    pub(super) fn reset_pointer_gesture(&mut self) {
        if let Some(id) = self.pointer.capture {
            if let Some(pane) = self.ws.pane_mut(id) {
                pane.ui.link_press = false;
                if pane.ui.selection.selecting {
                    self.finish_selection(id);
                }
            }
        }
        if let Some(id) = self.pointer.hover.take() {
            self.clear_link_hover(id);
        }
        self.window.set_cursor_icon(CursorIcon::Default);
        self.window.set_chrome_hover(None, None);
        self.window.cancel_sidebar_drag();
        self.release_held_buttons();
        self.pointer.cancel();
    }

    pub(super) fn refresh_pointer_hover(&mut self) {
        if self.palette_open() || self.pointer.capture.is_some() || self.pointer.drag.is_some() {
            return;
        }
        let (x, y) = self.pointer.pos;
        let hit = self.window.hit_test(&self.ws, x, y);
        self.update_chrome_hover(&hit, false);
        let (target, left) = self.pointer.moved(&hit);
        if let Some(id) = left {
            self.clear_link_hover(id);
            self.window.set_cursor_icon(CursorIcon::Default);
        }
        if let Some(icon) = cursor_icon_for(&hit, false, self.window.cursor_icon()) {
            self.window.set_cursor_icon(icon);
        }
        if let Some(id) = target {
            self.update_link_hover(id);
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
    fn handle_mouse_button(&mut self, id: PaneId, button: MouseButton, code: u8, pressed: bool) {
        let mode = self.mode_of(id);
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
                    // Open only the target the status line shows; if output changed the link
                    // since, show the new one and wait for another click.
                    let shown = self.ws.pane(id).and_then(|p| p.ui.hover_target.clone());
                    if shown.as_deref() == Some(&*crate::links::display_target(&link.uri)) {
                        crate::links::open_url(&link.uri);
                    } else {
                        self.update_link_hover(id);
                    }
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

    /// A left press in the left padding of pane `id`, where block gutters are drawn, selects
    /// the block on that row. Returns true when it did (the press is then not a selection).
    fn click_block_gutter(&mut self, id: PaneId) -> bool {
        let (x, y) = self.pointer.pos;
        let Some(rect) = self.pane_rect(id) else {
            return false;
        };
        let Some(pad) = self.window.metrics().map(|m| m.padding_left) else {
            return false;
        };
        if x >= f64::from(rect.x + pad) || self.mode_of(id).intersects(TermMode::MOUSE_MODE) {
            return false;
        }
        let Some((_, row, _)) = self.window.cell_at(id, rect, x, y) else {
            return false;
        };
        let selected = self
            .ws
            .pane_mut(id)
            .is_some_and(|p| p.blocks.select_at_row(&p.terminal, row));
        if selected {
            self.window.request_redraw();
        }
        selected
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
