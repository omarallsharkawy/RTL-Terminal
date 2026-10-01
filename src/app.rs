use std::sync::{Arc, Mutex};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{WindowAttributes, WindowId};

use crate::config::TwittyConfig;
use crate::input::{handle_key, InputAction};
use crate::pty::Pty;
use crate::renderer::Renderer;
use crate::terminal::{SelectionType, Terminal};
use alacritty_terminal::term::TermMode;

#[derive(Debug)]
#[allow(clippy::enum_variant_names)]
pub enum AppEvent {
    PtyData(Vec<u8>),
    PtyWriteResponse(String),
    ClipboardStore(String),
    Title(String),
    Bell,
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
    is_selecting: bool,
    mouse_down: bool,
    mouse_down_col: usize,
    mouse_down_row: usize,
    last_click_time: std::time::Instant,
    last_click_pos: (usize, usize),
    click_count: usize,
}

impl App {
    pub fn new(proxy: EventLoopProxy<AppEvent>) -> Self {
        let proxy_clone = proxy.clone();
        let proxy_cb = proxy.clone();
        let proxy_title = proxy.clone();
        let proxy_bell = proxy.clone();
        let terminal = Arc::new(Mutex::new(Terminal::new_full(
            80,
            24,
            move |text| {
                let _ = proxy_clone.send_event(AppEvent::PtyWriteResponse(text));
            },
            move |text| {
                let _ = proxy_cb.send_event(AppEvent::ClipboardStore(text));
            },
            move |title| {
                let _ = proxy_title.send_event(AppEvent::Title(title));
            },
            move || {
                let _ = proxy_bell.send_event(AppEvent::Bell);
            },
        )));

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
            is_selecting: false,
            mouse_down: false,
            mouse_down_col: 0,
            mouse_down_row: 0,
            last_click_time: std::time::Instant::now(),
            last_click_pos: (0, 0),
            click_count: 0,
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
            let cols = cols.max(80);
            let rows = rows.max(24);
            if let Ok(mut term) = self.terminal.lock() {
                term.resize(cols, rows);
            }
            if let Some(ref pty) = self.pty {
                let _ = pty.resize(cols as u16, rows as u16);
            } else {
                self.pty = self.spawn_pty(cols as u16, rows as u16);
            }
            self.needs_redraw = true;
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

fn get_clipboard_text() -> Option<String> {
    if let Ok(mut cb) = arboard::Clipboard::new() {
        if let Ok(text) = cb.get_text() {
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        // 1. Try Omarchy / DMS desktop clipboard
        if let Ok(output) = std::process::Command::new("dms")
            .args(["clipboard", "paste"])
            .output()
        {
            if output.status.success() {
                if let Ok(text) = String::from_utf8(output.stdout) {
                    if !text.is_empty() {
                        return Some(text);
                    }
                }
            }
        }

        // 2. Try wl-paste
        if let Ok(output) = std::process::Command::new("wl-paste")
            .arg("--no-newline")
            .output()
        {
            if output.status.success() {
                if let Ok(text) = String::from_utf8(output.stdout) {
                    if !text.is_empty() {
                        return Some(text);
                    }
                }
            }
        }
        if let Ok(output) = std::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-o"])
            .output()
        {
            if output.status.success() {
                if let Ok(text) = String::from_utf8(output.stdout) {
                    if !text.is_empty() {
                        return Some(text);
                    }
                }
            }
        }
    }
    None
}

fn set_clipboard_text(text: &str) {
    let text = text.to_string();
    std::thread::spawn(move || {
        #[cfg(target_os = "linux")]
        {
            use std::io::Write;
            // 1. Sync to Omarchy / DMS desktop clipboard manager
            if let Ok(mut child) = std::process::Command::new("dms")
                .args(["clipboard", "copy"])
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                let _ = child.wait();
            }

            // 1. Copy to standard Wayland clipboard
            if let Ok(mut child) = std::process::Command::new("wl-copy")
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                let _ = child.wait();
            }
            // 2. Also copy to primary selection
            if let Ok(mut child) = std::process::Command::new("wl-copy")
                .arg("--primary")
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                let _ = child.wait();
            }
            // 3. Fallback for X11
            if let Ok(mut child) = std::process::Command::new("xclip")
                .args(["-selection", "clipboard"])
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
            }
        }

        if let Ok(mut cb) = arboard::Clipboard::new() {
            let _ = cb.set_text(&text);
        }
    });
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

        let renderer = match pollster::block_on(Renderer::new(
            window.clone(),
            self.config.font_size,
            self.config.background_opacity,
            self.config.cursor_style.clone(),
        )) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Failed to initialize renderer: {:?}", e);
                event_loop.exit();
                return;
            }
        };

        let (cols, rows) = renderer.compute_grid_size();
        let cols = cols.max(80);
        let rows = rows.max(24);
        if let Ok(mut term) = self.terminal.lock() {
            term.resize(cols, rows);
        }

        window.set_ime_allowed(true);
        self.renderer = Some(renderer);
        self.needs_redraw = true;
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
            AppEvent::ClipboardStore(text) => {
                set_clipboard_text(&text);
            }
            AppEvent::Title(title) => {
                if let Some(ref r) = self.renderer {
                    r.window.set_title(&title);
                }
            }
            AppEvent::Bell => {
                if let Some(ref r) = self.renderer {
                    r.window.request_user_attention(Some(
                        winit::window::UserAttentionType::Informational,
                    ));
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
                if new_size.width >= 100 && new_size.height >= 100 {
                    if let Some(ref mut r) = self.renderer {
                        r.resize(new_size.width, new_size.height);
                    }
                    self.sync_grid();
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(ref mut r) = self.renderer {
                    let size = r.window.inner_size();
                    r.resize(size.width, size.height);
                }
                self.sync_grid();
            }
            WindowEvent::Ime(ime) => match ime {
                winit::event::Ime::Commit(text) => {
                    if let Some(ref pty) = self.pty {
                        let _ = pty.write(text.as_bytes());
                    }
                }
                _ => {}
            },
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(ref r) = self.renderer {
                    let col = ((position.x - r.padding_left as f64) / r.char_width as f64).max(0.0)
                        as usize;
                    let row = ((position.y - r.padding_top as f64) / r.line_height as f64).max(0.0)
                        as usize;
                    self.mouse_col = col;
                    self.mouse_row = row;

                    if self.mouse_down {
                        let moved = col != self.mouse_down_col || row != self.mouse_down_row;
                        if moved && !self.is_selecting {
                            self.is_selecting = true;
                            let mode = self
                                .terminal
                                .lock()
                                .map(|t| t.mode())
                                .unwrap_or(TermMode::NONE);
                            if mode.intersects(TermMode::MOUSE_MODE) && !self.modifiers.shift_key()
                            {
                                let release_seq = format!(
                                    "[<0;{};{}m",
                                    self.mouse_down_col + 1,
                                    self.mouse_down_row + 1
                                );
                                if let Some(ref pty) = self.pty {
                                    let _ = pty.write(release_seq.as_bytes());
                                }
                            }
                            if let Ok(mut term) = self.terminal.lock() {
                                term.start_selection(self.mouse_down_col, self.mouse_down_row);
                            }
                        }
                    }

                    if self.is_selecting {
                        if let Ok(mut term) = self.terminal.lock() {
                            term.update_selection(col, row);
                        }
                        if let Some(ref r) = self.renderer {
                            r.window.request_redraw();
                        }
                    } else {
                        let mode = self
                            .terminal
                            .lock()
                            .map(|t| t.mode())
                            .unwrap_or(TermMode::NONE);
                        if mode.contains(TermMode::MOUSE_MOTION) {
                            let seq = format!("[<35;{};{}M", col + 1, row + 1);
                            if let Some(ref pty) = self.pty {
                                let _ = pty.write(seq.as_bytes());
                            }
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let mode = self
                    .terminal
                    .lock()
                    .map(|t| t.mode())
                    .unwrap_or(TermMode::NONE);
                let col = self.mouse_col + 1;
                let row = self.mouse_row + 1;
                let in_mouse_mode =
                    mode.intersects(TermMode::MOUSE_MODE) && !self.modifiers.shift_key();

                match state {
                    ElementState::Pressed => match button {
                        MouseButton::Left => {
                            self.mouse_down = true;
                            self.mouse_down_col = self.mouse_col;
                            self.mouse_down_row = self.mouse_row;

                            let now = std::time::Instant::now();
                            let is_multi_click =
                                now.duration_since(self.last_click_time).as_millis() < 400
                                    && self.last_click_pos == (self.mouse_col, self.mouse_row);

                            if is_multi_click {
                                self.click_count += 1;
                            } else {
                                self.click_count = 1;
                            }
                            self.last_click_time = now;
                            self.last_click_pos = (self.mouse_col, self.mouse_row);

                            if self.click_count == 2 {
                                self.is_selecting = true;
                                if let Ok(mut term) = self.terminal.lock() {
                                    term.start_selection_type(
                                        self.mouse_col,
                                        self.mouse_row,
                                        SelectionType::Semantic,
                                    );
                                }
                                if let Some(ref r) = self.renderer {
                                    r.window.request_redraw();
                                }
                            } else if self.click_count >= 3 {
                                self.is_selecting = true;
                                if let Ok(mut term) = self.terminal.lock() {
                                    term.start_selection_type(
                                        self.mouse_col,
                                        self.mouse_row,
                                        SelectionType::Lines,
                                    );
                                }
                                if let Some(ref r) = self.renderer {
                                    r.window.request_redraw();
                                }
                            } else if in_mouse_mode {
                                let seq = format!("[<0;{};{}M", col, row);
                                if let Some(ref pty) = self.pty {
                                    let _ = pty.write(seq.as_bytes());
                                }
                            } else {
                                self.is_selecting = true;
                                if let Ok(mut term) = self.terminal.lock() {
                                    term.start_selection(self.mouse_col, self.mouse_row);
                                }
                                if let Some(ref r) = self.renderer {
                                    r.window.request_redraw();
                                }
                            }
                        }
                        MouseButton::Middle => {
                            if let Some(text) = get_clipboard_text() {
                                if let Some(ref pty) = self.pty {
                                    let bracketed = self
                                        .terminal
                                        .lock()
                                        .map(|t| t.mode().contains(TermMode::BRACKETED_PASTE))
                                        .unwrap_or(false);
                                    if bracketed {
                                        let mut payload = Vec::with_capacity(text.len() + 12);
                                        payload.extend_from_slice(b"[200~");
                                        payload.extend_from_slice(text.as_bytes());
                                        payload.extend_from_slice(b"[201~");
                                        let _ = pty.write(&payload);
                                    } else {
                                        let _ = pty.write(text.as_bytes());
                                    }
                                }
                            }
                        }
                        MouseButton::Right => {
                            let copied = if let Ok(term) = self.terminal.lock() {
                                if let Some(text) = term.selection_text() {
                                    if !text.is_empty() {
                                        set_clipboard_text(&text);
                                        true
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                }
                            } else {
                                false
                            };

                            if copied {
                                if let Ok(mut term) = self.terminal.lock() {
                                    term.clear_selection();
                                }
                                if let Some(ref r) = self.renderer {
                                    r.window.request_redraw();
                                }
                                return;
                            }

                            if in_mouse_mode {
                                let seq = format!("[<2;{};{}M", col, row);
                                if let Some(ref pty) = self.pty {
                                    let _ = pty.write(seq.as_bytes());
                                }
                            } else if let Some(text) = get_clipboard_text() {
                                if let Some(ref pty) = self.pty {
                                    let bracketed = self
                                        .terminal
                                        .lock()
                                        .map(|t| t.mode().contains(TermMode::BRACKETED_PASTE))
                                        .unwrap_or(false);
                                    if bracketed {
                                        let mut payload = Vec::with_capacity(text.len() + 12);
                                        payload.extend_from_slice(b"[200~");
                                        payload.extend_from_slice(text.as_bytes());
                                        payload.extend_from_slice(b"[201~");
                                        let _ = pty.write(&payload);
                                    } else {
                                        let _ = pty.write(text.as_bytes());
                                    }
                                }
                            }
                        }
                        _ => {}
                    },
                    ElementState::Released => match button {
                        MouseButton::Left => {
                            self.mouse_down = false;

                            if self.is_selecting {
                                if let Ok(term) = self.terminal.lock() {
                                    if let Some(text) = term.selection_text() {
                                        if !text.is_empty() {
                                            set_clipboard_text(&text);
                                        }
                                    }
                                }
                                self.is_selecting = false;
                            } else if in_mouse_mode {
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
                    },
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
                    let mode = self
                        .terminal
                        .lock()
                        .map(|t| t.mode())
                        .unwrap_or(TermMode::NONE);
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
                        let key = if delta_y > 0.0 {
                            b"OAOAOA"
                        } else {
                            b"OBOBOB"
                        };
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
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::ControlLeft | KeyCode::ControlRight) => {
                        if event.state.is_pressed() {
                            self.modifiers.insert(ModifiersState::CONTROL);
                        } else {
                            self.modifiers.remove(ModifiersState::CONTROL);
                        }
                    }
                    PhysicalKey::Code(KeyCode::ShiftLeft | KeyCode::ShiftRight) => {
                        if event.state.is_pressed() {
                            self.modifiers.insert(ModifiersState::SHIFT);
                        } else {
                            self.modifiers.remove(ModifiersState::SHIFT);
                        }
                    }
                    PhysicalKey::Code(KeyCode::AltLeft | KeyCode::AltRight) => {
                        if event.state.is_pressed() {
                            self.modifiers.insert(ModifiersState::ALT);
                        } else {
                            self.modifiers.remove(ModifiersState::ALT);
                        }
                    }
                    _ => {}
                }
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
                            if let Some(text) = get_clipboard_text() {
                                if let Some(ref pty) = self.pty {
                                    let bracketed = self
                                        .terminal
                                        .lock()
                                        .map(|t| t.mode().contains(TermMode::BRACKETED_PASTE))
                                        .unwrap_or(false);
                                    if bracketed {
                                        let mut payload = Vec::with_capacity(text.len() + 12);
                                        payload.extend_from_slice(b"[200~");
                                        payload.extend_from_slice(text.as_bytes());
                                        payload.extend_from_slice(b"[201~");
                                        let _ = pty.write(&payload);
                                    } else {
                                        let _ = pty.write(text.as_bytes());
                                    }
                                }
                            }
                        }
                        InputAction::Copy => {
                            if let Ok(term) = self.terminal.lock() {
                                if let Some(text) = term.selection_text() {
                                    if !text.is_empty() {
                                        set_clipboard_text(&text);
                                    }
                                }
                            }
                        }
                        InputAction::CopyOrInterrupt => {
                            let copied = if let Ok(mut term) = self.terminal.lock() {
                                if let Some(text) = term.selection_text() {
                                    if !text.is_empty() {
                                        set_clipboard_text(&text);
                                        term.clear_selection();
                                        if let Some(ref r) = self.renderer {
                                            r.window.request_redraw();
                                        }
                                        true
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                }
                            } else {
                                false
                            };

                            if !copied {
                                if let Some(ref pty) = self.pty {
                                    let _ = pty.write(&[3]); // SIGINT / Ctrl+C
                                }
                            }
                        }
                        InputAction::Cut => {
                            let in_alt_screen = self
                                .terminal
                                .lock()
                                .map(|t| t.mode().contains(TermMode::ALT_SCREEN))
                                .unwrap_or(false);
                            if in_alt_screen {
                                if let Some(ref pty) = self.pty {
                                    let _ = pty.write(&[24]);
                                }
                            } else {
                                let cut = if let Ok(mut term) = self.terminal.lock() {
                                    if let Some(text) = term.selection_text() {
                                        if !text.is_empty() {
                                            set_clipboard_text(&text);
                                            term.clear_selection();
                                            if let Some(ref r) = self.renderer {
                                                r.window.request_redraw();
                                            }
                                            true
                                        } else {
                                            false
                                        }
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                };

                                if !cut {
                                    if let Some(ref pty) = self.pty {
                                        let _ = pty.write(&[24]); // Ctrl+X byte 24
                                    }
                                }
                            }
                        }
                        InputAction::SelectAll => {
                            if let Ok(mut term) = self.terminal.lock() {
                                term.select_all();
                            }
                            if let Some(ref r) = self.renderer {
                                r.window.request_redraw();
                            }
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                self.needs_redraw = false;
                if self.pty.is_none() {
                    self.sync_grid();
                }
                if let Some(ref mut r) = self.renderer {
                    if let Ok(term) = self.terminal.lock() {
                        let (lines, cursor) = term.snapshot();
                        drop(term);
                        if let Err(e) = r.render(&lines, &cursor) {
                            eprintln!("Render error: {:?}", e);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
