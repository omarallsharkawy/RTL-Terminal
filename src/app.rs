use std::sync::{Arc, Mutex};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{WindowAttributes, WindowId};

use crate::config::TwittyConfig;
use crate::input::{handle_key, InputAction};
use crate::pty::Pty;
use crate::renderer::Renderer;
use crate::terminal::Terminal;
use alacritty_terminal::term::TermMode;

#[derive(Debug)]
pub enum AppEvent {
    PtyData(Vec<u8>),
    PtyWriteResponse(String),
    PtyExit,
}

pub struct App {
    proxy: EventLoopProxy<AppEvent>,
    terminal: Arc<Mutex<Terminal>>,
    pty: Option<Pty>,
    renderer: Option<Renderer>,
    modifiers: ModifiersState,
    mouse_col: usize,
    mouse_row: usize,
    config: TwittyConfig,
    needs_redraw: bool,
}

impl App {
    pub fn new(proxy: EventLoopProxy<AppEvent>) -> Self {
        let proxy_clone = proxy.clone();
        let terminal = Arc::new(Mutex::new(Terminal::new(80, 24, move |text| {
            let _ = proxy_clone.send_event(AppEvent::PtyWriteResponse(text));
        })));

        let config = TwittyConfig::load();

        Self {
            proxy,
            terminal,
            pty: None,
            renderer: None,
            modifiers: ModifiersState::empty(),
            mouse_col: 0,
            mouse_row: 0,
            config,
            needs_redraw: false,
        }
    }

    fn spawn_pty(&self, cols: u16, rows: u16) -> Option<Pty> {
        let proxy = self.proxy.clone();
        let proxy_exit = self.proxy.clone();
        match Pty::spawn(
            cols,
            rows,
            move |data| {
                let _ = proxy.send_event(AppEvent::PtyData(data));
            },
            move || {
                let _ = proxy_exit.send_event(AppEvent::PtyExit);
            },
        ) {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!("Failed to spawn PTY: {:?}", e);
                None
            }
        }
    }

    fn sync_grid(&mut self) {
        if let Some(ref r) = self.renderer {
            let (cols, rows) = r.compute_grid_size();
            if let Ok(mut term) = self.terminal.lock() {
                term.resize(cols, rows);
            }
            if let Some(ref pty) = self.pty {
                let _ = pty.resize(cols as u16, rows as u16);
            }
            r.window.request_redraw();
        }
    }

    fn update_font_size(&mut self, new_size: f32) {
        let clamped = new_size.clamp(8.0, 48.0);
        self.config.font_size = clamped;
        self.config.save();
        if let Some(ref mut r) = self.renderer {
            r.set_font_size(clamped);
        }
        self.sync_grid();
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }

        let window_attrs = WindowAttributes::default()
            .with_title("Twitty · RTL Terminal")
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 580.0));

        let window = match event_loop.create_window(window_attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("Failed to create window: {:?}", e);
                event_loop.exit();
                return;
            }
        };

        let renderer = match pollster::block_on(Renderer::new(window.clone(), self.config.font_size)) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Failed to initialize renderer: {:?}", e);
                event_loop.exit();
                return;
            }
        };

        let (cols, rows) = renderer.compute_grid_size();
        if let Ok(mut term) = self.terminal.lock() {
            term.resize(cols, rows);
        }

        self.pty = self.spawn_pty(cols as u16, rows as u16);
        self.renderer = Some(renderer);
        window.request_redraw();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::PtyData(data) => {
                if let Ok(mut term) = self.terminal.lock() {
                    term.process_bytes(&data);
                }
                if let Some(ref r) = self.renderer {
                    if !self.needs_redraw {
                        self.needs_redraw = true;
                        r.window.request_redraw();
                    }
                }
            }
            AppEvent::PtyWriteResponse(text) => {
                if let Some(ref pty) = self.pty {
                    let _ = pty.write(text.as_bytes());
                }
            }
            AppEvent::PtyExit => {
                let (cols, rows) = match self.renderer {
                    Some(ref r) => r.compute_grid_size(),
                    None => (80, 24),
                };
                self.pty = self.spawn_pty(cols as u16, rows as u16);
                if let Some(ref r) = self.renderer {
                    r.window.request_redraw();
                }
            }
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
                self.sync_grid();
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(ref r) = self.renderer {
                    let col = ((position.x - r.padding_left as f64) / r.char_width as f64).max(0.0) as usize;
                    let row = ((position.y - r.padding_top as f64) / r.line_height as f64).max(0.0) as usize;
                    self.mouse_col = col;
                    self.mouse_row = row;

                    let mode = self.terminal.lock().map(|t| t.mode()).unwrap_or(TermMode::NONE);
                    if mode.contains(TermMode::MOUSE_MOTION) {
                        let seq = format!("[<35;{};{}M", col + 1, row + 1);
                        if let Some(ref pty) = self.pty {
                            let _ = pty.write(seq.as_bytes());
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let mode = self.terminal.lock().map(|t| t.mode()).unwrap_or(TermMode::NONE);
                let col = self.mouse_col + 1;
                let row = self.mouse_row + 1;

                match state {
                    ElementState::Pressed => {
                        match button {
                            MouseButton::Left => {
                                if mode.intersects(TermMode::MOUSE_MODE) {
                                    let seq = format!("[<0;{};{}M", col, row);
                                    if let Some(ref pty) = self.pty {
                                        let _ = pty.write(seq.as_bytes());
                                    }
                                }
                            }
                            MouseButton::Middle => {
                                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                    if let Ok(text) = clipboard.get_text() {
                                        if let Some(ref pty) = self.pty {
                                            let _ = pty.write(text.as_bytes());
                                        }
                                    }
                                }
                            }
                            MouseButton::Right => {
                                if mode.intersects(TermMode::MOUSE_MODE) {
                                    let seq = format!("[<2;{};{}M", col, row);
                                    if let Some(ref pty) = self.pty {
                                        let _ = pty.write(seq.as_bytes());
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    ElementState::Released => {
                        match button {
                            MouseButton::Left => {
                                if mode.intersects(TermMode::MOUSE_MODE) {
                                    let seq = format!("[<0;{};{}m", col, row);
                                    if let Some(ref pty) = self.pty {
                                        let _ = pty.write(seq.as_bytes());
                                    }
                                }
                            }
                            MouseButton::Right => {
                                if mode.intersects(TermMode::MOUSE_MODE) {
                                    let seq = format!("[<2;{};{}m", col, row);
                                    if let Some(ref pty) = self.pty {
                                        let _ = pty.write(seq.as_bytes());
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let delta_y = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y / 20.0) as f32,
                };

                if self.modifiers.control_key() {
                    // Ctrl + Wheel: Zoom in / Zoom out & save config
                    if delta_y > 0.1 {
                        let cur = self.config.font_size;
                        self.update_font_size(cur + 1.0);
                    } else if delta_y < -0.1 {
                        let cur = self.config.font_size;
                        self.update_font_size(cur - 1.0);
                    }
                } else {
                    let mode = self.terminal.lock().map(|t| t.mode()).unwrap_or(TermMode::NONE);
                    let col = self.mouse_col + 1;
                    let row = self.mouse_row + 1;

                    if mode.intersects(TermMode::MOUSE_MODE) {
                        // Send SGR mouse wheel reporting (64 = up, 65 = down)
                        let btn = if delta_y > 0.0 { 64 } else { 65 };
                        let seq = format!("[<{};{};{}M", btn, col, row);
                        if let Some(ref pty) = self.pty {
                            let _ = pty.write(seq.as_bytes());
                        }
                    } else if mode.contains(TermMode::ALT_SCREEN) {
                        // Alternate screen without mouse mode (vim, less, opencode): send arrow keys
                        let key = if delta_y > 0.0 { b"OAOAOA" } else { b"OBOBOB" };
                        if let Some(ref pty) = self.pty {
                            let _ = pty.write(key);
                        }
                    } else {
                        // Terminal scrollback
                        let lines = if delta_y > 0.0 { 3 } else { -3 };
                        if let Ok(mut term) = self.terminal.lock() {
                            term.scroll_display(lines);
                        }
                        if let Some(ref r) = self.renderer {
                            r.window.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let app_cursor = self
                    .terminal
                    .lock()
                    .map(|t| t.is_app_cursor())
                    .unwrap_or(false);
                if let Some(action) = handle_key(&event, self.modifiers, app_cursor) {
                    match action {
                        InputAction::Bytes(bytes) => {
                            if let Some(ref pty) = self.pty {
                                let _ = pty.write(&bytes);
                            }
                        }
                        InputAction::ZoomIn => {
                            let cur = self.config.font_size;
                            self.update_font_size(cur + 1.0);
                        }
                        InputAction::ZoomOut => {
                            let cur = self.config.font_size;
                            self.update_font_size(cur - 1.0);
                        }
                        InputAction::ZoomReset => {
                            self.update_font_size(14.5);
                        }
                        InputAction::Paste => {
                            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                if let Ok(text) = clipboard.get_text() {
                                    if let Some(ref pty) = self.pty {
                                        let _ = pty.write(text.as_bytes());
                                    }
                                }
                            }
                        }
                        InputAction::Copy => {}
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                self.needs_redraw = false;
                if let Some(ref mut r) = self.renderer {
                    let (lines, cursor) = self.terminal.lock().unwrap().snapshot();
                    if let Err(e) = r.render(&lines, &cursor) {
                        eprintln!("Render error: {:?}", e);
                    }
                }
            }
            _ => {}
        }
    }
}
