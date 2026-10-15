//! The winit application: owns the window, the workspace of tabs and panes, and the input
//! state, and routes each event to the module that handles it.
//!
//! - `event`: shell output and terminal requests arriving through the event proxy.
//! - `window`: the window and renderer (every renderer call lives there).
//! - `panes`: opening, splitting, focusing and closing tabs and panes.
//! - `keyboard`: keys, input-method text, the search bar and shortcut actions.
//! - `mouse`: pointer, mouse reporting, divider dragging and the wheel.
//! - `selection`: mouse selection and the clipboard.

mod event;
mod keyboard;
mod mouse;
mod panes;
mod selection;
mod window;

pub use event::AppEvent;

use alacritty_terminal::term::TermMode;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::WindowId;

use crate::blink::{BlinkState, BLINK_INTERVAL};
use crate::config::TwittyConfig;
use crate::input::mouse::StepAccumulator;
use crate::input::pointer::PointerState;
use crate::layout::PaneId;
use crate::session::pane::Pane;
use crate::session::workspace::Workspace;
use crate::terminal::Terminal;
use crate::ui::clipboard::Clipboard;
use window::Window;

pub struct App {
    proxy: EventLoopProxy<AppEvent>,
    ws: Workspace,
    window: Window,
    modifiers: ModifiersState,
    pointer: PointerState,
    clipboard: Clipboard,
    config: TwittyConfig,
    config_dirty: bool,
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
            ws: Workspace::new(),
            window: Window::new(),
            modifiers: ModifiersState::empty(),
            pointer: PointerState::default(),
            clipboard: Clipboard::default(),
            config: TwittyConfig::load(),
            config_dirty: false,
            preedit_active: false,
            cursor_blink: BlinkState::new(BLINK_INTERVAL),
            zoom_steps: StepAccumulator::default(),
        }
    }

    fn focused_id(&self) -> Option<PaneId> {
        self.ws.focused_id()
    }

    /// Run `f` on the terminal of pane `id`.
    fn term<R>(&mut self, id: PaneId, f: impl FnOnce(&mut Terminal) -> R) -> Option<R> {
        self.ws.pane_mut(id).map(|p| f(&mut p.terminal))
    }

    fn mode_of(&self, id: PaneId) -> TermMode {
        self.ws.pane(id).map_or(TermMode::NONE, Pane::mode)
    }

    fn write_to(&self, id: PaneId, bytes: &[u8]) {
        if let Some(pane) = self.ws.pane(id) {
            pane.write(bytes);
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
            self.window.request_redraw();
        }
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_open() {
            return;
        }
        if !self.window.open(event_loop, &self.config) {
            return;
        }
        self.new_tab();
        if self.ws.is_empty() {
            self.window.close();
            event_loop.exit();
            return;
        }
        self.window.request_redraw_now();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::PtyData(id, data) => self.on_pty_data(id, &data),
            AppEvent::Terminal(id, event) => self.on_terminal_event(id, event),
            // The shell of a pane exited (`exit`, crash, or closed by the OS): close that pane,
            // and the window once nothing is left.
            AppEvent::PtyExit(id) => {
                if self.close_pane(id) {
                    event_loop.exit();
                }
            }
            AppEvent::ClearSelection(id, generation) => self.on_clear_selection(id, generation),
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

        if self.window.blink_differs(self.cursor_blink.visible()) {
            self.window.request_redraw();
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
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                self.window.resize(size.width, size.height);
                self.apply_layout();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.window.set_scale_factor(scale_factor as f32);
                self.apply_layout();
            }
            WindowEvent::Focused(focused) => self.on_window_focus(focused),
            WindowEvent::ModifiersChanged(mods) => self.on_modifiers(mods.state()),
            WindowEvent::CursorMoved { position, .. } => {
                self.on_cursor_moved(position.x, position.y)
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.on_mouse_input(button, state == ElementState::Pressed)
            }
            WindowEvent::MouseWheel { delta, .. } => self.on_wheel(delta),
            WindowEvent::Ime(ime) => self.on_ime(ime),
            // While an IME composes, keys belong to the IME.
            WindowEvent::KeyboardInput { .. }
                if !crate::ime::should_forward_key(self.preedit_active) => {}
            WindowEvent::KeyboardInput { event, .. } => {
                if self.on_key(&event) {
                    event_loop.exit();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }
}
