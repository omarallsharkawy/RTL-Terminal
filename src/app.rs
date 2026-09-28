use std::sync::{Arc, Mutex};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{WindowAttributes, WindowId};

use crate::input::handle_key;
use crate::pty::Pty;
use crate::renderer::Renderer;
use crate::terminal::Terminal;

#[derive(Debug)]
pub enum AppEvent {
    PtyData(Vec<u8>),
}

pub struct App {
    proxy: EventLoopProxy<AppEvent>,
    terminal: Arc<Mutex<Terminal>>,
    pty: Option<Pty>,
    renderer: Option<Renderer>,
    modifiers: ModifiersState,
}

impl App {
    pub fn new(proxy: EventLoopProxy<AppEvent>) -> Self {
        Self {
            proxy,
            terminal: Arc::new(Mutex::new(Terminal::new(80, 24))),
            pty: None,
            renderer: None,
            modifiers: ModifiersState::empty(),
        }
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

        let renderer = match pollster::block_on(Renderer::new(window.clone())) {
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

        let proxy = self.proxy.clone();
        let pty = match Pty::spawn(cols as u16, rows as u16, move |data| {
            let _ = proxy.send_event(AppEvent::PtyData(data));
        }) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Failed to spawn PTY: {:?}", e);
                event_loop.exit();
                return;
            }
        };

        self.pty = Some(pty);
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
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some(bytes) = handle_key(&event, self.modifiers) {
                    if let Some(ref pty) = self.pty {
                        let _ = pty.write(&bytes);
                    }
                }
            }
            WindowEvent::RedrawRequested => {
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
