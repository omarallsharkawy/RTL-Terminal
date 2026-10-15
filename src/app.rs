use alacritty_terminal::event::{Event as TermEvent, WindowSize};
use alacritty_terminal::index::{Direction, Side};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, Rgb};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, Ime, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, UserAttentionType, WindowAttributes, WindowId};

use crate::blink::{BlinkState, BLINK_INTERVAL};
use crate::config::TwittyConfig;
use crate::input::mouse::{
    button_code, button_report, motion_report, wheel_action, wheel_lines, StepAccumulator,
    WheelAction, LINES_PER_WHEEL_NOTCH,
};
use crate::input::pointer::{cursor_icon_for, PointerState, PressTarget};
use crate::input::route::{route_key, shortcut_is_idle, KeyRoute};
use crate::input::{
    encode_mouse, encode_paste, handle_key, key_shortcut, terminal_key, InputAction,
};
use crate::layout::{
    dividers, layout, neighbor, ratio_at, set_ratio, tab_rects, Divider, FocusDir, Node, PaneId,
    Rect, SplitDir,
};
use crate::pty::Pty;
use crate::renderer::{PaneFrame, Renderer, TabLabel};
use crate::session::pane::Pane;
use crate::terminal::Terminal;
use crate::ui::clipboard::Clipboard;
use crate::ui::hit::{hit_test, Hit, HitGeometry};
use crate::ui::search::{search_bar_text, search_key, SearchCmd};
use crate::ui::selection::COPIED_HIGHLIGHT;
use crate::ui::settings::{clamp_font_size, step_opacity, DEFAULT_FONT_SIZE};

const WINDOW_TITLE: &str = "Twitty · RTL Terminal";
/// A pane is only split when both halves keep at least this many columns / rows.
const MIN_SPLIT_COLS: usize = 24;
const MIN_SPLIT_ROWS: usize = 10;

#[derive(Debug)]
pub enum AppEvent {
    PtyData(PaneId, Vec<u8>),
    Terminal(PaneId, TermEvent),
    PtyExit(PaneId),
    /// Clear the highlight of a copied selection, unless a newer selection was started.
    ClearSelection(PaneId, u64),
}

/// A tab: a tree of split panes and the one that has keyboard focus.
struct Tab {
    root: Node,
    focused: PaneId,
}

pub struct App {
    proxy: EventLoopProxy<AppEvent>,
    panes: HashMap<PaneId, Pane>,
    tabs: Vec<Tab>,
    active: usize,
    next_pane_id: u64,
    renderer: Option<Renderer>,
    modifiers: ModifiersState,
    pointer: PointerState,
    cursor_icon: CursorIcon,
    clipboard: Clipboard,
    config: TwittyConfig,
    config_dirty: bool,
    needs_redraw: bool,
    /// An IME composition (preedit) is on screen.
    preedit_active: bool,
    cursor_blink: BlinkState,
    /// Fractional Ctrl+wheel notches not yet turned into zoom or opacity steps.
    zoom_steps: StepAccumulator,
}

impl App {
    pub fn new(proxy: EventLoopProxy<AppEvent>) -> Self {
        Self {
            proxy,
            panes: HashMap::new(),
            tabs: Vec::new(),
            active: 0,
            next_pane_id: 0,
            renderer: None,
            modifiers: ModifiersState::empty(),
            pointer: PointerState::default(),
            cursor_icon: CursorIcon::Default,
            clipboard: Clipboard::default(),
            config: TwittyConfig::load(),
            config_dirty: false,
            needs_redraw: false,
            preedit_active: false,
            cursor_blink: BlinkState::new(BLINK_INTERVAL),
            zoom_steps: StepAccumulator::default(),
        }
    }

    // ---- panes, tabs and layout ----

    fn alloc_pane_id(&mut self) -> PaneId {
        self.next_pane_id += 1;
        PaneId(self.next_pane_id)
    }

    /// A terminal and shell for pane `id` with the given grid size.
    fn spawn_pane(&self, id: PaneId, cols: usize, rows: usize) -> Option<Pane> {
        let proxy = self.proxy.clone();
        let mut terminal = Terminal::with_events(cols, rows, move |event| {
            let _ = proxy.send_event(AppEvent::Terminal(id, event));
        });
        terminal.set_default_cursor_blink(self.config.cursor_blink);
        let proxy = self.proxy.clone();
        let proxy_exit = self.proxy.clone();
        // New tabs and splits open in the directory of the pane that has focus.
        let cwd = self
            .focused_id()
            .and_then(|f| self.panes.get(&f))
            .and_then(|p| p.pty.as_ref())
            .and_then(Pty::cwd);
        let pty = match Pty::spawn_in(
            cwd.as_deref(),
            cols as u16,
            rows as u16,
            move |data| {
                let _ = proxy.send_event(AppEvent::PtyData(id, data));
            },
            move || {
                let _ = proxy_exit.send_event(AppEvent::PtyExit(id));
            },
        ) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Failed to spawn PTY: {:?}", e);
                return None;
            }
        };
        Some(Pane::new(id, terminal, Some(pty), (cols, rows)))
    }

    fn focused_id(&self) -> Option<PaneId> {
        self.tabs.get(self.active).map(|t| t.focused)
    }

    /// Run `f` on the terminal of pane `id`.
    fn term<R>(&mut self, id: PaneId, f: impl FnOnce(&mut Terminal) -> R) -> Option<R> {
        self.panes.get_mut(&id).map(|p| f(&mut p.terminal))
    }

    fn mode_of(&self, id: PaneId) -> TermMode {
        self.panes.get(&id).map_or(TermMode::NONE, Pane::mode)
    }

    fn write_to(&self, id: PaneId, bytes: &[u8]) {
        if let Some(pane) = self.panes.get(&id) {
            pane.write(bytes);
        }
    }

    fn content_rect(&self, tab_count: usize) -> Option<Rect> {
        self.renderer.as_ref().map(|r| r.content_rect(tab_count))
    }

    fn gap(&self) -> f32 {
        self.renderer.as_ref().map_or(1.0, Renderer::divider_gap)
    }

    /// Where each pane of the active tab goes.
    fn active_layout(&self) -> Vec<(PaneId, Rect)> {
        let (Some(tab), Some(rect)) = (
            self.tabs.get(self.active),
            self.content_rect(self.tabs.len()),
        ) else {
            return Vec::new();
        };
        layout(&tab.root, rect, self.gap())
    }

    fn pane_rect(&self, id: PaneId) -> Option<Rect> {
        self.active_layout()
            .into_iter()
            .find(|(p, _)| *p == id)
            .map(|(_, r)| r)
    }

    fn pane_at(&self, x: f64, y: f64) -> Option<PaneId> {
        self.active_layout()
            .into_iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|(id, _)| id)
    }

    fn hit_test(&self, x: f64, y: f64) -> Hit {
        let Some(r) = self.renderer.as_ref() else {
            return Hit::Nothing;
        };
        let n = self.tabs.len();
        let geometry = HitGeometry {
            tab_bar_height: r.tab_bar_height(n),
            tabs: tab_rects(n, r.config.width as f32, r.line_height, r.char_width * 28.0),
            content: r.content_rect(n),
            gap: r.divider_gap(),
            slop: (r.scale * 3.0).max(2.0),
        };
        hit_test(&geometry, self.tabs.get(self.active).map(|t| &t.root), x, y)
    }

    /// Give every pane of every tab the grid its rectangle allows.
    fn apply_layout(&mut self) {
        let Some(content) = self.content_rect(self.tabs.len()) else {
            return;
        };
        let gap = self.gap();
        let mut sizes = Vec::new();
        if let Some(r) = self.renderer.as_ref() {
            for tab in &self.tabs {
                for (id, rect) in layout(&tab.root, content, gap) {
                    sizes.push((id, r.compute_grid_size(rect)));
                }
            }
        }
        for (id, (cols, rows)) in sizes {
            if let Some(pane) = self.panes.get_mut(&id) {
                pane.resize(cols, rows);
            }
        }
        self.request_redraw();
    }

    fn new_tab(&mut self) {
        let Some(content) = self.content_rect(self.tabs.len() + 1) else {
            return;
        };
        let Some((cols, rows)) = self.renderer.as_ref().map(|r| r.compute_grid_size(content))
        else {
            return;
        };
        let id = self.alloc_pane_id();
        let Some(pane) = self.spawn_pane(id, cols, rows) else {
            return;
        };
        let previous = self.focused_id();
        self.panes.insert(id, pane);
        self.tabs.push(Tab {
            root: Node::Leaf(id),
            focused: id,
        });
        self.active = self.tabs.len() - 1;
        self.release_held_buttons();
        self.pointer.cancel();
        // A second tab shows the tab bar, which shrinks every pane.
        self.apply_layout();
        self.focus_changed(previous, id);
    }

    fn split_focused(&mut self, dir: SplitDir) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let focused = tab.focused;
        let Some((cols, rows)) = self.panes.get(&focused).map(|p| p.size) else {
            return;
        };
        let fits = match dir {
            SplitDir::Horizontal => cols >= MIN_SPLIT_COLS * 2,
            SplitDir::Vertical => rows >= MIN_SPLIT_ROWS * 2,
        };
        if !fits {
            return;
        }
        let mut root = tab.root.clone();
        let new_id = self.alloc_pane_id();
        root.split(focused, new_id, dir);
        let Some(content) = self.content_rect(self.tabs.len()) else {
            return;
        };
        let rect = layout(&root, content, self.gap())
            .into_iter()
            .find(|(id, _)| *id == new_id)
            .map(|(_, r)| r);
        let Some((cols, rows)) = rect
            .zip(self.renderer.as_ref())
            .map(|(rect, r)| r.compute_grid_size(rect))
        else {
            return;
        };
        let Some(pane) = self.spawn_pane(new_id, cols, rows) else {
            return;
        };
        self.panes.insert(new_id, pane);
        self.tabs[self.active].root = root;
        self.tabs[self.active].focused = new_id;
        self.apply_layout();
        self.focus_changed(Some(focused), new_id);
    }

    /// Keyboard focus moved from `old` to `new`: tell the programs, restart the cursor blink and
    /// show the new pane's title.
    fn focus_changed(&mut self, old: Option<PaneId>, new: PaneId) {
        if old == Some(new) {
            return;
        }
        if self.renderer.as_ref().is_some_and(|r| r.focused) {
            if let Some(old) = old {
                self.report_focus(old, false);
            }
            self.report_focus(new, true);
        }
        self.cursor_blink.reset(Instant::now());
        self.refresh_window_title();
        self.request_redraw();
    }

    /// Tell the program in pane `id` it gained or lost focus, if it asked (mode 1004).
    fn report_focus(&self, id: PaneId, focused: bool) {
        if self.mode_of(id).contains(TermMode::FOCUS_IN_OUT) {
            self.write_to(id, if focused { b"\x1b[I" } else { b"\x1b[O" });
        }
    }

    fn focus_pane(&mut self, id: PaneId) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        if tab.focused == id || !tab.root.contains(id) {
            return;
        }
        let old = tab.focused;
        tab.focused = id;
        self.focus_changed(Some(old), id);
    }

    fn set_active_tab(&mut self, index: usize) {
        if index >= self.tabs.len() || index == self.active {
            return;
        }
        let old = self.focused_id();
        self.active = index;
        self.release_held_buttons();
        self.pointer.cancel();
        if let Some(new) = self.focused_id() {
            self.focus_changed(old, new);
        }
    }

    fn cycle_tab(&mut self, step: isize) {
        let n = self.tabs.len() as isize;
        if n > 1 {
            self.set_active_tab((self.active as isize + step).rem_euclid(n) as usize);
        }
    }

    fn focus_direction(&mut self, dir: FocusDir) {
        let Some(from) = self.focused_id() else {
            return;
        };
        if let Some(to) = neighbor(&self.active_layout(), from, dir) {
            self.focus_pane(to);
        }
    }

    /// Close one pane. Returns true when it was the last one and the window should close.
    fn close_pane(&mut self, id: PaneId) -> bool {
        let Some(ti) = self.tabs.iter().position(|t| t.root.contains(id)) else {
            return false;
        };
        let previous = self.focused_id();
        let tab = self.tabs.remove(ti);
        let leaves = tab.root.leaves();
        let pos = leaves.iter().position(|l| *l == id).unwrap_or(0);
        match tab.root.remove(id) {
            Some(root) => {
                // Focus the pane that was next to the closed one in reading order.
                let focused = if tab.focused == id {
                    let near = if pos > 0 { pos - 1 } else { pos + 1 };
                    leaves.get(near).copied().unwrap_or(leaves[0])
                } else {
                    tab.focused
                };
                self.tabs.insert(ti, Tab { root, focused });
            }
            None => {
                if ti < self.active {
                    self.active -= 1;
                }
            }
        }
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
        // Dropping the pane ends its shell.
        self.panes.remove(&id);
        if let Some(r) = self.renderer.as_mut() {
            r.forget_pane(id);
        }
        self.pointer.forget(id);
        let Some(now) = self.focused_id() else {
            return true;
        };
        self.apply_layout();
        self.focus_changed(previous.filter(|p| self.panes.contains_key(p)), now);
        self.refresh_window_title();
        false
    }

    fn tab_title(&self, index: usize) -> String {
        self.tabs
            .get(index)
            .and_then(|t| self.panes.get(&t.focused))
            .and_then(|p| p.title.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| format!("Tab {}", index + 1))
    }

    /// The window title follows the focused pane.
    fn refresh_window_title(&self) {
        let title = self
            .focused_id()
            .and_then(|id| self.panes.get(&id))
            .and_then(|p| p.title.clone())
            .unwrap_or_else(|| WINDOW_TITLE.to_owned());
        if let Some(ref r) = self.renderer {
            r.window.set_title(&title);
        }
    }

    // ---- window-level helpers ----

    fn update_preedit(&mut self, preedit: Option<(String, Option<(usize, usize)>)>) {
        self.preedit_active = preedit.is_some();
        if let Some(ref mut r) = self.renderer {
            r.set_preedit(preedit);
        }
        self.request_redraw();
    }

    fn request_redraw(&mut self) {
        if let Some(ref r) = self.renderer {
            if !self.needs_redraw {
                self.needs_redraw = true;
                r.window.request_redraw();
            }
        }
    }

    /// Any input sent to the shell snaps the view back to the live screen.
    fn scroll_to_bottom(&mut self, id: PaneId) {
        let scrolled = self
            .term(id, |term| {
                let scrolled = term.display_offset() > 0;
                if scrolled {
                    term.scroll_to_bottom();
                }
                scrolled
            })
            .unwrap_or(false);
        if scrolled {
            self.request_redraw();
        }
    }

    /// Zoom by `steps` points.
    fn zoom_by(&mut self, steps: i32) {
        let size = self.config.font_size + steps as f32;
        self.update_font_size(size);
    }

    fn update_font_size(&mut self, new_size: f32) {
        let clamped = clamp_font_size(new_size);
        self.config.font_size = clamped;
        // Written once on exit instead of on every zoom step.
        self.config_dirty = true;
        if let Some(ref mut r) = self.renderer {
            r.set_font_size(clamped);
        }
        self.apply_layout();
    }

    fn paste_clipboard(&mut self, id: PaneId) {
        let Some(text) = self.clipboard.text() else {
            return;
        };
        let bracketed = self.mode_of(id).contains(TermMode::BRACKETED_PASTE);
        self.write_to(id, &encode_paste(&text, bracketed));
        self.cursor_blink.reset(Instant::now());
        self.scroll_to_bottom(id);
    }

    fn set_cursor_icon(&mut self, icon: CursorIcon) {
        if self.cursor_icon != icon {
            self.cursor_icon = icon;
            if let Some(ref r) = self.renderer {
                r.window.set_cursor(icon);
            }
        }
    }

    // ---- selection, links, mouse ----

    fn clear_selection(&mut self, id: PaneId) {
        let had = self
            .term(id, |term| {
                let had = term.has_selection();
                if had {
                    term.clear_selection();
                }
                had
            })
            .unwrap_or(false);
        if let Some(pane) = self.panes.get_mut(&id) {
            pane.ui.selection.end();
        }
        if had {
            self.request_redraw();
        }
    }

    /// Copy the selection to the clipboard. Returns false when nothing is selected.
    fn copy_selection(&mut self, id: PaneId) -> bool {
        let text = self.term(id, |t| t.selection_text()).flatten();
        let Some(text) = text else { return false };
        self.clipboard.set_text(text);
        true
    }

    fn start_selection(&mut self, id: PaneId) {
        let now = Instant::now();
        let side = self.panes.get(&id).map_or(Side::Left, |p| p.ui.mouse_side);
        let Some(pane) = self.panes.get_mut(&id) else {
            return;
        };
        let (col, row) = (pane.ui.mouse_col, pane.ui.mouse_row);
        let clicks = pane.ui.clicks.click(now, col, row);
        pane.ui.selection.begin();
        pane.terminal.start_selection(col, row, side, clicks);
        self.request_redraw();
    }

    /// Mouse released: copy what was selected, flash it briefly, then clear the highlight.
    fn finish_selection(&mut self, id: PaneId) {
        if let Some(pane) = self.panes.get_mut(&id) {
            pane.ui.selection.end();
        }
        if !self.copy_selection(id) {
            self.clear_selection(id);
            return;
        }
        let Some(generation) = self.panes.get(&id).map(|p| p.ui.selection.generation()) else {
            return;
        };
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            std::thread::sleep(COPIED_HIGHLIGHT);
            let _ = proxy.send_event(AppEvent::ClearSelection(id, generation));
        });
    }

    /// Underline the link under the pointer while Ctrl is held, and show a pointer cursor.
    fn update_link_hover(&mut self, id: PaneId) {
        let Some((col, row)) = self
            .panes
            .get(&id)
            .map(|p| (p.ui.mouse_col, p.ui.mouse_row))
        else {
            return;
        };
        let ctrl = self.modifiers.control_key();
        let result = self.term(id, |t| {
            let link = if ctrl { t.link_at(col, row) } else { None };
            let hover = link.as_ref().map(|l| (l.row, l.cols.clone()));
            (t.set_link_hover(hover), link.is_some())
        });
        if let Some((true, over_link)) = result {
            self.set_cursor_icon(if over_link {
                CursorIcon::Pointer
            } else {
                CursorIcon::Text
            });
            self.request_redraw();
        }
    }

    /// The pointer left a pane: its link underline goes away.
    fn clear_link_hover(&mut self, id: PaneId) {
        if self.term(id, |t| t.set_link_hover(None)) == Some(true) {
            self.request_redraw();
        }
    }

    fn send_mouse(&self, id: PaneId, button: u8, pressed: bool) {
        let Some(pane) = self.panes.get(&id) else {
            return;
        };
        let sgr = pane.mode().contains(TermMode::SGR_MOUSE);
        let seq = encode_mouse(
            button,
            pane.ui.mouse_col,
            pane.ui.mouse_row,
            pressed,
            self.modifiers,
            sgr,
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
                .panes
                .get(&id)
                .map(|p| (p.ui.mouse_col, p.ui.mouse_row, p.ui.link_press))
            else {
                return;
            };
            if pressed && self.modifiers.control_key() {
                let link = self.term(id, |t| t.link_at(col, row)).flatten();
                if let Some(link) = link {
                    crate::links::open_url(&link.uri);
                    if let Some(p) = self.panes.get_mut(&id) {
                        p.ui.link_press = true;
                    }
                    return;
                }
            } else if !pressed && link_press {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.ui.link_press = false;
                }
                return;
            }
        }
        let held = self.panes.get(&id).and_then(|p| p.ui.mouse_button_down);
        let (report, held) = button_report(pressed, mode, self.modifiers.shift_key(), held, code);
        if let Some(p) = self.panes.get_mut(&id) {
            p.ui.mouse_button_down = held;
        }

        if report {
            if mode.intersects(TermMode::MOUSE_MODE) {
                self.send_mouse(id, code, pressed);
            }
        } else if button == MouseButton::Left {
            if pressed {
                self.start_selection(id);
            } else if self
                .panes
                .get(&id)
                .is_some_and(|p| p.ui.selection.selecting)
            {
                self.finish_selection(id);
            }
        } else if pressed && button == MouseButton::Middle {
            self.paste_clipboard(id);
        }
    }

    /// Release every button a program saw pressed, e.g. when the window loses focus mid-drag and
    /// the real release may never arrive. Otherwise later motion is reported as a drag.
    fn release_held_buttons(&mut self) {
        let held: Vec<(PaneId, u8)> = self
            .panes
            .values()
            .filter_map(|p| p.ui.mouse_button_down.map(|b| (p.id, b)))
            .collect();
        for (id, button) in held {
            if self.mode_of(id).intersects(TermMode::MOUSE_MODE) {
                self.send_mouse(id, button, false);
            }
            if let Some(p) = self.panes.get_mut(&id) {
                p.ui.mouse_button_down = None;
            }
        }
    }

    fn drag_divider_to(&mut self, divider: &Divider) {
        let pos = match divider.dir {
            SplitDir::Horizontal => self.pointer.pos.0,
            SplitDir::Vertical => self.pointer.pos.1,
        };
        let ratio = ratio_at(divider, pos, self.gap());
        if let Some(tab) = self.tabs.get_mut(self.active) {
            if set_ratio(&mut tab.root, &divider.path, ratio) {
                self.apply_layout();
            }
        }
    }

    // ---- terminal events ----

    fn handle_terminal_event(&mut self, id: PaneId, event: TermEvent) {
        match event {
            TermEvent::PtyWrite(text) => self.write_to(id, text.as_bytes()),
            TermEvent::Title(title) => {
                if let Some(pane) = self.panes.get_mut(&id) {
                    pane.title = Some(title);
                }
                self.refresh_window_title();
                self.request_redraw();
            }
            TermEvent::ResetTitle => {
                if let Some(pane) = self.panes.get_mut(&id) {
                    pane.title = None;
                }
                self.refresh_window_title();
                self.request_redraw();
            }
            TermEvent::Bell => {
                if let Some(ref r) = self.renderer {
                    r.window
                        .request_user_attention(Some(UserAttentionType::Informational));
                }
            }
            TermEvent::ColorRequest(index, format) => {
                // OSC 4/10/11/12 queries (used by vim, fzf, delta to detect dark themes).
                let Some(ref r) = self.renderer else { return };
                let rgba = match index {
                    256 => r.palette.foreground,
                    257 => r.palette.background,
                    258 => r.palette.cursor,
                    i if i < 256 => r.palette.resolve(AnsiColor::Indexed(i as u8), false),
                    _ => r.palette.foreground,
                };
                let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                let rgb = Rgb {
                    r: to8(rgba.r),
                    g: to8(rgba.g),
                    b: to8(rgba.b),
                };
                self.write_to(id, format(rgb).as_bytes());
            }
            TermEvent::TextAreaSizeRequest(format) => {
                let Some(ref r) = self.renderer else { return };
                let Some((cols, rows)) = self.panes.get(&id).map(|p| p.size) else {
                    return;
                };
                let size = WindowSize {
                    num_lines: rows as u16,
                    num_cols: cols as u16,
                    cell_width: r.char_width as u16,
                    cell_height: r.line_height as u16,
                };
                self.write_to(id, format(size).as_bytes());
            }
            TermEvent::ClipboardStore(_, text) => {
                // OSC 52 copy (e.g. from tmux/vim over SSH).
                self.clipboard.set_text(text);
            }
            // OSC 52 reads are refused: any program could silently exfiltrate the clipboard.
            TermEvent::ClipboardLoad(..) => {}
            _ => {}
        }
    }
}

impl App {
    /// Text of the search bar for the current query and match position.
    fn search_bar_text(&self, id: PaneId, query: &str) -> String {
        match self.panes.get(&id).map(|p| &p.terminal) {
            Some(t) => search_bar_text(query, t.has_search_match(), || t.search_position()),
            None => format!("Find: {query}"),
        }
    }

    fn open_search(&mut self, id: PaneId) {
        self.clear_selection(id);
        if let Some(pane) = self.panes.get_mut(&id) {
            pane.ui.search_query = Some(String::new());
        }
        self.request_redraw();
    }

    fn close_search(&mut self, id: PaneId) {
        if let Some(pane) = self.panes.get_mut(&id) {
            pane.ui.search_query = None;
        }
        self.term(id, |t| t.clear_search());
        self.request_redraw();
    }

    /// Run the search for the current query in `direction` and update the bar.
    fn run_search(&mut self, id: PaneId, direction: Direction) {
        let Some(query) = self.panes.get(&id).and_then(|p| p.ui.search_query.clone()) else {
            return;
        };
        self.term(id, |t| t.search(&query, direction));
        self.request_redraw();
    }

    /// Key press while the search bar is open. Nothing reaches the PTY.
    fn handle_search_key(&mut self, id: PaneId, event: &KeyEvent) {
        if !event.state.is_pressed() {
            return;
        }
        let modifiers = self.modifiers;
        let Some(query) = self
            .panes
            .get_mut(&id)
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

    /// Run a shortcut action on the focused pane. Returns true when the window should close.
    fn run_action(&mut self, id: PaneId, action: InputAction) -> bool {
        match action {
            InputAction::Bytes(bytes) => {
                self.clear_selection(id);
                self.write_to(id, &bytes);
                self.cursor_blink.reset(Instant::now());
                self.scroll_to_bottom(id);
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
            InputAction::NextTab => self.cycle_tab(1),
            InputAction::PrevTab => self.cycle_tab(-1),
            InputAction::SplitRight => self.split_focused(SplitDir::Horizontal),
            InputAction::SplitDown => self.split_focused(SplitDir::Vertical),
            InputAction::Focus(dir) => self.focus_direction(dir),
        }
        false
    }

    /// Snapshot everything the renderer draws for the active tab.
    fn frame(&self) -> (Vec<TabLabel>, Vec<PaneFrame>, Vec<Rect>) {
        let tabs = (0..self.tabs.len())
            .map(|i| TabLabel {
                title: self.tab_title(i),
                active: i == self.active,
            })
            .collect();
        let focused = self.focused_id();
        let panes = self
            .active_layout()
            .into_iter()
            .filter_map(|(id, rect)| {
                let pane = self.panes.get(&id)?;
                let (lines, cursor) = pane.terminal.snapshot();
                let status = pane
                    .ui
                    .search_query
                    .as_deref()
                    .map(|q| self.search_bar_text(id, q));
                Some(PaneFrame {
                    id,
                    rect,
                    lines,
                    cursor,
                    focused: focused == Some(id),
                    status,
                })
            })
            .collect();
        let dividers = match (
            self.tabs.get(self.active),
            self.content_rect(self.tabs.len()),
        ) {
            (Some(tab), Some(content)) => dividers(&tab.root, content, self.gap())
                .into_iter()
                .map(|d| d.rect)
                .collect(),
            _ => Vec::new(),
        };
        (tabs, panes, dividers)
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }

        let window_attrs = WindowAttributes::default()
            .with_title(WINDOW_TITLE)
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 580.0))
            .with_transparent(true);

        let window = match event_loop.create_window(window_attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("Failed to create window: {:?}", e);
                event_loop.exit();
                return;
            }
        };

        window.set_ime_allowed(true);
        window.set_ime_purpose(winit::window::ImePurpose::Terminal);

        let mut renderer =
            match pollster::block_on(Renderer::new(window.clone(), self.config.font_size)) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Failed to initialize renderer: {:?}", e);
                    event_loop.exit();
                    return;
                }
            };

        renderer.set_font_family(self.config.font_family.as_deref());
        renderer.set_opacity(self.config.background_opacity);
        renderer.cursor_style = self.config.cursor_style.clone();
        renderer.palette = self.config.palette();
        self.renderer = Some(renderer);

        self.new_tab();
        if self.tabs.is_empty() {
            self.renderer = None;
            event_loop.exit();
            return;
        }
        window.request_redraw();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::PtyData(id, data) => {
                let Some(pane) = self.panes.get_mut(&id) else {
                    return;
                };
                let term = &mut pane.terminal;
                let before = term.cursor_position();
                term.process_bytes(&data);
                let moved = term.cursor_position() != before;
                // Output that moves the cursor keeps it solid while text is appearing.
                if moved && self.focused_id() == Some(id) {
                    self.cursor_blink.reset(Instant::now());
                }
                if self
                    .tabs
                    .get(self.active)
                    .is_some_and(|t| t.root.contains(id))
                {
                    self.request_redraw();
                }
            }
            AppEvent::Terminal(id, event) => self.handle_terminal_event(id, event),
            // The shell of a pane exited (`exit`, crash, or closed by the OS): close that pane,
            // and the window once nothing is left.
            AppEvent::PtyExit(id) => {
                if self.close_pane(id) {
                    event_loop.exit();
                }
            }
            AppEvent::ClearSelection(id, generation) => {
                let current = self
                    .panes
                    .get(&id)
                    .is_some_and(|p| p.ui.selection.may_clear(generation));
                if current {
                    self.clear_selection(id);
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let (blinking, shown) = self
            .focused_id()
            .and_then(|id| {
                self.term(id, |t| {
                    (
                        t.cursor_blinking(),
                        t.mode().contains(TermMode::SHOW_CURSOR),
                    )
                })
            })
            .unwrap_or((false, false));
        self.cursor_blink.set_enabled(now, blinking && shown);
        self.cursor_blink.tick(now);

        let visible = self.cursor_blink.visible();
        if self
            .renderer
            .as_ref()
            .is_some_and(|r| r.cursor_blink_on != visible)
        {
            self.request_redraw();
        }
        event_loop.set_control_flow(match self.cursor_blink.next_deadline() {
            Some(deadline) => ControlFlow::WaitUntil(deadline),
            None => ControlFlow::Wait,
        });
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if self.config_dirty {
            self.config.save();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(new_size) => {
                if let Some(ref mut r) = self.renderer {
                    r.resize(new_size.width, new_size.height);
                }
                self.apply_layout();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(ref mut r) = self.renderer {
                    r.set_scale_factor(scale_factor as f32);
                }
                self.apply_layout();
            }
            WindowEvent::Focused(focused) => {
                // The release of a drag that ends outside the window may never arrive.
                if !focused {
                    let selecting: Vec<PaneId> = self
                        .panes
                        .values()
                        .filter(|p| p.ui.selection.selecting)
                        .map(|p| p.id)
                        .collect();
                    for id in selecting {
                        self.finish_selection(id);
                    }
                    self.release_held_buttons();
                    self.pointer.cancel();
                }
                self.cursor_blink.set_focused(Instant::now(), focused);
                if let Some(ref mut r) = self.renderer {
                    r.set_focused(focused);
                    r.window.request_redraw();
                }
                if let Some(id) = self.focused_id() {
                    self.report_focus(id, focused);
                }
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
                if let Some(id) = self.pointer.hover.or(self.focused_id()) {
                    self.update_link_hover(id);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer.pos = (position.x, position.y);
                if let Some(divider) = self.pointer.drag.clone() {
                    self.drag_divider_to(&divider);
                    return;
                }
                let hit = self.hit_test(position.x, position.y);
                let captured = self.pointer.capture.is_some();
                if let Some(icon) = cursor_icon_for(&hit, captured, self.cursor_icon) {
                    self.set_cursor_icon(icon);
                }
                let (target, left) = self.pointer.moved(&hit);
                if let Some(old) = left {
                    self.clear_link_hover(old);
                }
                let Some(id) = target else { return };
                let Some(rect) = self.pane_rect(id) else {
                    return;
                };
                let Some(ref r) = self.renderer else { return };
                let (col, row, side) = r.pixel_to_visual_cell(id, rect, position.x, position.y);
                let Some(pane) = self.panes.get_mut(&id) else {
                    return;
                };
                pane.ui.mouse_side = side;
                if pane.ui.selection.selecting {
                    pane.terminal.update_selection(col, row, side);
                    self.request_redraw();
                }
                let Some(pane) = self.panes.get_mut(&id) else {
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
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                let target = if pressed {
                    let (x, y) = self.pointer.pos;
                    let hit = self.hit_test(x, y);
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
            WindowEvent::MouseWheel { delta, .. } => {
                let line_height = self
                    .renderer
                    .as_ref()
                    .map_or(20.0, |r| r.line_height as f64);
                let lines = wheel_lines(delta, line_height);

                if self.modifiers.control_key() {
                    // Ctrl (+ Shift) + wheel: one zoom (opacity) step per notch. Trackpads send
                    // many tiny deltas, so they are summed into notches first.
                    let steps = self.zoom_steps.add(lines / LINES_PER_WHEEL_NOTCH);
                    if steps == 0 {
                        return;
                    }
                    if self.modifiers.shift_key() {
                        let opacity = step_opacity(self.config.background_opacity, steps);
                        self.config.background_opacity = opacity;
                        self.config_dirty = true;
                        if let Some(ref mut r) = self.renderer {
                            r.set_opacity(opacity);
                        }
                        self.request_redraw();
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
                let Some(pane) = self.panes.get_mut(&id) else {
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
                        self.request_redraw();
                    }
                    WheelAction::None => {}
                }
            }
            WindowEvent::Ime(ime) => match ime {
                Ime::Enabled => {}
                Ime::Disabled => self.update_preedit(None),
                Ime::Preedit(text, cursor) => {
                    self.update_preedit(crate::ime::normalize_preedit(text, cursor))
                }
                Ime::Commit(text) => {
                    self.update_preedit(None);
                    let Some(id) = self.focused_id() else { return };
                    // While searching, composed text belongs to the query, not the shell.
                    if let Some(query) = self
                        .panes
                        .get_mut(&id)
                        .and_then(|p| p.ui.search_query.as_mut())
                    {
                        query.push_str(&text);
                        self.run_search(id, Direction::Left);
                        return;
                    }
                    self.clear_selection(id);
                    self.write_to(id, text.as_bytes());
                    self.scroll_to_bottom(id);
                }
            },
            WindowEvent::KeyboardInput { .. }
                if !crate::ime::should_forward_key(self.preedit_active) => {}
            WindowEvent::KeyboardInput { event, .. } => {
                let Some(id) = self.focused_id() else { return };
                let mode = self.mode_of(id);
                let mut action = handle_key(&event, self.modifiers, mode);
                let single_pane = self
                    .tabs
                    .get(self.active)
                    .is_some_and(|t| matches!(t.root, Node::Leaf(_)));
                let shortcut = key_shortcut(&event, self.modifiers);
                if shortcut_is_idle(shortcut.as_ref(), single_pane, self.tabs.len()) {
                    action = terminal_key(&event, self.modifiers, mode).map(InputAction::Bytes);
                }
                let search_open = self
                    .panes
                    .get(&id)
                    .is_some_and(|p| p.ui.search_query.is_some());
                match route_key(action, search_open) {
                    KeyRoute::SearchBar => self.handle_search_key(id, &event),
                    KeyRoute::Action(action) => {
                        if self.run_action(id, action) {
                            event_loop.exit();
                        }
                    }
                    KeyRoute::Ignore => {}
                }
            }
            WindowEvent::RedrawRequested => {
                self.needs_redraw = false;
                let (tabs, panes, dividers) = self.frame();
                let visible = self.cursor_blink.visible();
                if let Some(ref mut r) = self.renderer {
                    r.cursor_blink_on = visible;
                    if let Err(e) = r.render(&tabs, &panes, &dividers) {
                        eprintln!("Render error: {:?}", e);
                    }
                }
            }
            _ => {}
        }
    }
}
