use std::sync::{Arc, Mutex};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{WindowAttributes, WindowId};

use crate::config::TwittyConfig;
use crate::input::{handle_key, InputAction};
use crate::pty::Pty;
use crate::renderer::Renderer;
use crate::terminal::{SelectionType, Terminal};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::Color as AnsiColor;

#[allow(clippy::enum_variant_names)]
pub enum AppEvent {
    PtyData(Vec<u8>),
    PtyWriteResponse(String),
    ClipboardStore(String),
    ClipboardLoad(Arc<dyn Fn(&str) -> String + Sync + Send + 'static>),
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
    is_selecting: bool,
    mouse_down: bool,
    mouse_down_col: usize,
    mouse_down_row: usize,
    last_click_time: std::time::Instant,
    last_click_pos: (usize, usize),
    click_count: usize,
    preedit: Option<(String, Option<(usize, usize)>)>,
    is_searching: bool,
    search_query: String,
    search_matches: Vec<(i32, usize, usize)>,
    search_match_idx: usize,
    is_dirty: bool,
    last_render_time: std::time::Instant,
    custom_command: Option<(String, Vec<String>)>,
    initial_font_size: f32,
}

impl App {
    pub fn new(
        proxy: EventLoopProxy<AppEvent>,
        custom_command: Option<(String, Vec<String>)>,
    ) -> Self {
        let config = TwittyConfig::load();
        let initial_font_size = config.font_size;
        let scrollback = config.scrollback_lines.unwrap_or(10000);
        let proxy_clone = proxy.clone();
        let proxy_cb = proxy.clone();
        let proxy_cb_load = proxy.clone();
        let proxy_title = proxy.clone();
        let proxy_bell = proxy.clone();
        let terminal = Arc::new(Mutex::new(Terminal::new_full(
            80,
            24,
            scrollback,
            move |text| {
                let _ = proxy_clone.send_event(AppEvent::PtyWriteResponse(text));
            },
            move |text| {
                let _ = proxy_cb.send_event(AppEvent::ClipboardStore(text));
            },
            move |formatter| {
                let _ = proxy_cb_load.send_event(AppEvent::ClipboardLoad(formatter));
            },
            move |title| {
                let _ = proxy_title.send_event(AppEvent::Title(title));
            },
            move || {
                let _ = proxy_bell.send_event(AppEvent::Bell);
            },
        )));

        Self {
            proxy,
            terminal,
            pty: None,
            renderer: None,
            modifiers: ModifiersState::empty(),
            mouse_col: 0,
            mouse_row: 0,
            config,
            is_selecting: false,
            mouse_down: false,
            mouse_down_col: 0,
            mouse_down_row: 0,
            last_click_time: std::time::Instant::now(),
            last_click_pos: (0, 0),
            click_count: 0,
            preedit: None,
            is_searching: false,
            search_query: String::new(),
            search_matches: Vec::new(),
            search_match_idx: 0,
            is_dirty: false,
            last_render_time: std::time::Instant::now(),
            initial_font_size,
            custom_command,
        }
    }

    fn spawn_pty(&self, cols: u16, rows: u16) -> Option<Pty> {
        let proxy = self.proxy.clone();
        let proxy_exit = self.proxy.clone();
        if let Some((ref cmd, ref args)) = self.custom_command {
            match Pty::spawn_command(
                cols,
                rows,
                cmd,
                args,
                move |data| {
                    let _ = proxy.send_event(AppEvent::PtyData(data));
                },
                move || {
                    let _ = proxy_exit.send_event(AppEvent::PtyExit);
                },
            ) {
                Ok(p) => Some(p),
                Err(e) => {
                    eprintln!("Failed to spawn custom command in PTY: {:?}", e);
                    None
                }
            }
        } else {
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
    }

    fn sync_grid(&mut self) {
        if let Some(ref r) = self.renderer {
            let (cols, rows) = r.compute_grid_size();
            if let Ok(mut term) = self.terminal.lock() {
                term.resize(cols, rows);
                term.update_cell_size(r.char_width, r.line_height);
            }
            if let Some(ref pty) = self.pty {
                let _ = pty.resize(cols as u16, rows as u16);
            } else {
                self.pty = self.spawn_pty(cols as u16, rows as u16);
            }
            r.window.request_redraw();
        }
    }

    fn update_font_size(&mut self, new_size: f32) {
        let clamped = new_size.clamp(8.0, 48.0);
        self.config.font_size = clamped;
        TwittyConfig::update_font_size(clamped);
        if let Some(ref mut r) = self.renderer {
            r.set_font_size(clamped);
        }
        self.sync_grid();
    }
}

fn get_clipboard_text() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        // 1. Try native Wayland clipboard
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
    }
    if let Ok(mut cb) = arboard::Clipboard::new() {
        if let Ok(text) = cb.get_text() {
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        // 2. Try Omarchy / DMS desktop clipboard
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
    }
    None
}

fn get_primary_text() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(output) = std::process::Command::new("wl-paste")
            .args(["--primary", "--no-newline"])
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
    get_clipboard_text()
}

fn paste_text(pty: &Pty, text: &str, bracketed: bool) {
    if bracketed {
        let sanitized = text.replace("\x1b[201~", "");
        let mut payload = Vec::with_capacity(sanitized.len() + 16);
        payload.extend_from_slice(b"\x1b[200~");
        payload.extend_from_slice(sanitized.as_bytes());
        payload.extend_from_slice(b"\x1b[201~");
        let _ = pty.write(&payload);
    } else {
        let converted = text.replace('\n', "\r");
        let _ = pty.write(converted.as_bytes());
    }
}

fn set_clipboard_text(text: &str) {
    let text = text.to_string();
    std::thread::spawn(move || {
        #[allow(unused_mut)]
        let mut copied = false;
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
                if child.wait().map(|s| s.success()).unwrap_or(false) {
                    copied = true;
                }
            }

            // 2. Copy to standard Wayland clipboard
            if let Ok(mut child) = std::process::Command::new("wl-copy")
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                if child.wait().map(|s| s.success()).unwrap_or(false) {
                    copied = true;
                }
            }
            // 3. Also copy to primary selection
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
        }

        // 4. Fallback for non-Linux or systems without native Wayland clipboard tools
        if !copied {
            if let Ok(mut cb) = arboard::Clipboard::new() {
                let _ = cb.set_text(&text);
            }
        }
    });
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }

        #[cfg(target_os = "linux")]
        use winit::platform::wayland::WindowAttributesExtWayland;

        #[cfg(target_os = "linux")]
        let window_attrs = WindowAttributes::default()
            .with_title("Twitty · RTL Terminal")
            .with_name("twitty", "twitty")
            .with_transparent(true)
            .with_blur(true)
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 580.0));

        #[cfg(not(target_os = "linux"))]
        let window_attrs = WindowAttributes::default()
            .with_title("Twitty · RTL Terminal")
            .with_transparent(true)
            .with_blur(true)
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
            self.config.font_family.clone(),
        )) {
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

        window.set_ime_allowed(true);
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
                self.is_dirty = true;
                let now = std::time::Instant::now();
                // High-throughput 120 FPS batching threshold (8ms)
                if now.duration_since(self.last_render_time).as_millis() >= 8 {
                    if let Some(ref r) = self.renderer {
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
            AppEvent::ClipboardLoad(formatter) => {
                let proxy = self.proxy.clone();
                std::thread::spawn(move || {
                    let content = get_clipboard_text().unwrap_or_default();
                    let response = formatter(&content);
                    let _ = proxy.send_event(AppEvent::PtyWriteResponse(response));
                });
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
                std::process::exit(0);
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
            WindowEvent::Focused(is_focused) => {
                let mode = self
                    .terminal
                    .lock()
                    .map(|t| t.mode())
                    .unwrap_or(TermMode::NONE);
                if mode.contains(TermMode::FOCUS_IN_OUT) {
                    let seq: &[u8] = if is_focused { b"\x1b[I" } else { b"\x1b[O" };
                    if let Some(ref pty) = self.pty {
                        let _ = pty.write(seq);
                    }
                }
                if let Some(ref r) = self.renderer {
                    r.window.request_redraw();
                }
            }
            WindowEvent::Resized(new_size) => {
                if new_size.width > 0 && new_size.height > 0 {
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
                    self.preedit = None;
                    if let Some(ref pty) = self.pty {
                        let _ = pty.write(text.as_bytes());
                    }
                    if let Some(ref r) = self.renderer {
                        r.window.request_redraw();
                    }
                }
                winit::event::Ime::Preedit(text, cursor) => {
                    self.preedit = if text.is_empty() {
                        None
                    } else {
                        Some((text, cursor))
                    };
                    if let Some(ref r) = self.renderer {
                        r.window.request_redraw();
                    }
                }
                winit::event::Ime::Disabled => {
                    self.preedit = None;
                    if let Some(ref r) = self.renderer {
                        r.window.request_redraw();
                    }
                }
                _ => {}
            },
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(ref r) = self.renderer {
                    let (grid_cols, grid_rows) = r.compute_grid_size();
                    let col = ((position.x - (r.padding_left as f64)) / r.char_width as f64)
                        .max(0.0)
                        .min(grid_cols.saturating_sub(1) as f64)
                        as usize;
                    let row = ((position.y - (r.padding_top as f64)) / r.line_height as f64)
                        .max(0.0)
                        .min(grid_rows.saturating_sub(1) as f64)
                        as usize;
                    self.mouse_col = col;
                    self.mouse_row = row;

                    if self.mouse_down {
                        let moved = col != self.mouse_down_col || row != self.mouse_down_row;
                        if moved && !self.is_selecting {
                            self.is_selecting = true;
                            let display_offset = self
                                .terminal
                                .lock()
                                .map(|t| t.display_offset())
                                .unwrap_or(0);
                            let mode = self
                                .terminal
                                .lock()
                                .map(|t| t.mode())
                                .unwrap_or(TermMode::NONE);
                            if mode.intersects(TermMode::MOUSE_MODE)
                                && !self.modifiers.shift_key()
                                && display_offset == 0
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
                            let seq =
                                crate::input::format_mouse_seq(mode, 35, col + 1, row + 1, false);
                            if let Some(ref pty) = self.pty {
                                let _ = pty.write(&seq);
                            }
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let display_offset = self
                    .terminal
                    .lock()
                    .map(|t| t.display_offset())
                    .unwrap_or(0);
                let mode = self
                    .terminal
                    .lock()
                    .map(|t| t.mode())
                    .unwrap_or(TermMode::NONE);
                let col = self.mouse_col + 1;
                let row = self.mouse_row + 1;
                let in_mouse_mode = mode.intersects(TermMode::MOUSE_MODE)
                    && !self.modifiers.shift_key()
                    && display_offset == 0;

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
                                let seq = crate::input::format_mouse_seq(
                                    mode,
                                    if self.modifiers.shift_key() { 4 } else { 0 }
                                        | if self.modifiers.alt_key() { 8 } else { 0 }
                                        | if self.modifiers.control_key() { 16 } else { 0 },
                                    col,
                                    row,
                                    false,
                                );
                                if let Some(ref pty) = self.pty {
                                    let _ = pty.write(&seq);
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
                            if let Some(text) = get_primary_text() {
                                if let Some(ref pty) = self.pty {
                                    let bracketed = self
                                        .terminal
                                        .lock()
                                        .map(|t| t.mode().contains(TermMode::BRACKETED_PASTE))
                                        .unwrap_or(false);
                                    paste_text(pty, &text, bracketed);
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
                                let seq = crate::input::format_mouse_seq(
                                    mode,
                                    2 | if self.modifiers.shift_key() { 4 } else { 0 }
                                        | if self.modifiers.alt_key() { 8 } else { 0 }
                                        | if self.modifiers.control_key() { 16 } else { 0 },
                                    col,
                                    row,
                                    false,
                                );
                                if let Some(ref pty) = self.pty {
                                    let _ = pty.write(&seq);
                                }
                            } else if let Some(text) = get_clipboard_text() {
                                if let Some(ref pty) = self.pty {
                                    let bracketed = self
                                        .terminal
                                        .lock()
                                        .map(|t| t.mode().contains(TermMode::BRACKETED_PASTE))
                                        .unwrap_or(false);
                                    paste_text(pty, &text, bracketed);
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
                                let seq = crate::input::format_mouse_seq(
                                    mode,
                                    if self.modifiers.shift_key() { 4 } else { 0 }
                                        | if self.modifiers.alt_key() { 8 } else { 0 }
                                        | if self.modifiers.control_key() { 16 } else { 0 },
                                    col,
                                    row,
                                    true,
                                );
                                if let Some(ref pty) = self.pty {
                                    let _ = pty.write(&seq);
                                }
                            }
                        }
                        MouseButton::Right => {
                            if mode.intersects(TermMode::MOUSE_MODE) {
                                let seq = crate::input::format_mouse_seq(
                                    mode,
                                    2 | if self.modifiers.shift_key() { 4 } else { 0 }
                                        | if self.modifiers.alt_key() { 8 } else { 0 }
                                        | if self.modifiers.control_key() { 16 } else { 0 },
                                    col,
                                    row,
                                    true,
                                );
                                if let Some(ref pty) = self.pty {
                                    let _ = pty.write(&seq);
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
                    let display_offset = self
                        .terminal
                        .lock()
                        .map(|t| t.display_offset())
                        .unwrap_or(0);
                    let prefer_scrollback = self.modifiers.shift_key() || display_offset > 0;

                    if mode.intersects(TermMode::MOUSE_MODE) && !prefer_scrollback {
                        // Send SGR mouse wheel reporting (64 = up, 65 = down)
                        let btn = if delta_y > 0.0 { 64 } else { 65 };
                        let seq = crate::input::format_mouse_seq(
                            mode,
                            btn | if self.modifiers.shift_key() { 4 } else { 0 }
                                | if self.modifiers.alt_key() { 8 } else { 0 }
                                | if self.modifiers.control_key() { 16 } else { 0 },
                            col,
                            row,
                            false,
                        );
                        if let Some(ref pty) = self.pty {
                            let _ = pty.write(&seq);
                        }
                    } else if mode.contains(TermMode::ALT_SCREEN)
                        && mode.contains(TermMode::ALTERNATE_SCROLL)
                        && !prefer_scrollback
                    {
                        let app_cursor = self
                            .terminal
                            .lock()
                            .map(|t| t.is_app_cursor())
                            .unwrap_or(false);
                        let key: &[u8] = if delta_y > 0.0 {
                            if app_cursor {
                                b"\x1bOA\x1bOA\x1bOA"
                            } else {
                                b"\x1b[A\x1b[A\x1b[A"
                            }
                        } else {
                            if app_cursor {
                                b"\x1bOB\x1bOB\x1bOB"
                            } else {
                                b"\x1b[B\x1b[B\x1b[B"
                            }
                        };
                        if let Some(ref pty) = self.pty {
                            let _ = pty.write(key);
                        }
                    } else {
                        // Terminal scrollback
                        let lines = if delta_y > 0.0 { 3 } else { -3 };
                        if let Ok(mut term) = self.terminal.lock() {
                            term.scroll_display(lines);
                            if self.is_selecting || self.mouse_down {
                                term.update_selection(self.mouse_col, self.mouse_row);
                            }
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
                if event.state.is_pressed()
                    && event.logical_key == Key::Named(NamedKey::End)
                    && self.modifiers.control_key()
                {
                    if let Ok(mut term) = self.terminal.lock() {
                        if term.display_offset() > 0 {
                            term.scroll_to_bottom();
                            if let Some(ref r) = self.renderer {
                                r.window.request_redraw();
                            }
                        }
                    }
                }

                // Shift + PageUp / PageDown for keyboard scrollback
                if event.state.is_pressed() && self.modifiers.shift_key() {
                    match event.logical_key {
                        Key::Named(NamedKey::PageUp) => {
                            if let Ok(mut term) = self.terminal.lock() {
                                term.scroll_display(20);
                                if let Some(ref r) = self.renderer {
                                    r.window.request_redraw();
                                }
                            }
                            return;
                        }
                        Key::Named(NamedKey::PageDown) => {
                            if let Ok(mut term) = self.terminal.lock() {
                                term.scroll_display(-20);
                                if let Some(ref r) = self.renderer {
                                    r.window.request_redraw();
                                }
                            }
                            return;
                        }
                        _ => {}
                    }
                }

                // Handle interactive scrollback search inputs
                if self.is_searching && event.state.is_pressed() {
                    match event.logical_key {
                        Key::Named(NamedKey::Escape) => {
                            self.is_searching = false;
                            self.search_query.clear();
                            self.search_matches.clear();
                            if let Some(ref r) = self.renderer {
                                r.window.request_redraw();
                            }
                            return;
                        }
                        Key::Named(NamedKey::Enter) => {
                            if !self.search_matches.is_empty() {
                                if self.modifiers.shift_key() {
                                    self.search_match_idx = if self.search_match_idx == 0 {
                                        self.search_matches.len() - 1
                                    } else {
                                        self.search_match_idx - 1
                                    };
                                } else {
                                    self.search_match_idx =
                                        (self.search_match_idx + 1) % self.search_matches.len();
                                }
                                let (line_idx, _, _) = self.search_matches[self.search_match_idx];
                                if let Ok(mut term) = self.terminal.lock() {
                                    term.scroll_to_line(line_idx);
                                }
                                if let Some(ref r) = self.renderer {
                                    r.window.request_redraw();
                                }
                            }
                            return;
                        }
                        Key::Named(NamedKey::Backspace) => {
                            self.search_query.pop();
                            if let Ok(mut term) = self.terminal.lock() {
                                self.search_matches = term.search(&self.search_query);
                                self.search_match_idx = 0;
                                if let Some(&(line_idx, _, _)) = self.search_matches.first() {
                                    term.scroll_to_line(line_idx);
                                }
                            }
                            if let Some(ref r) = self.renderer {
                                r.window.request_redraw();
                            }
                            return;
                        }
                        _ => {
                            if !self.modifiers.control_key() {
                                if let Some(ref txt) = event.text {
                                    if !txt.is_empty() {
                                        self.search_query.push_str(txt);
                                        if let Ok(mut term) = self.terminal.lock() {
                                            self.search_matches = term.search(&self.search_query);
                                            self.search_match_idx = 0;
                                            if let Some(&(line_idx, _, _)) =
                                                self.search_matches.first()
                                            {
                                                term.scroll_to_line(line_idx);
                                            }
                                        }
                                        if let Some(ref r) = self.renderer {
                                            r.window.request_redraw();
                                        }
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }

                let app_cursor = self
                    .terminal
                    .lock()
                    .map(|t| t.is_app_cursor())
                    .unwrap_or(false);
                let kitty_keyboard = self
                    .terminal
                    .lock()
                    .map(|t| t.mode().intersects(TermMode::KITTY_KEYBOARD_PROTOCOL))
                    .unwrap_or(false);
                if let Some(action) = handle_key(&event, self.modifiers, app_cursor, kitty_keyboard)
                {
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
                            self.update_font_size(self.initial_font_size);
                        }
                        InputAction::Paste => {
                            if let Some(text) = get_clipboard_text() {
                                if let Some(ref pty) = self.pty {
                                    let bracketed = self
                                        .terminal
                                        .lock()
                                        .map(|t| t.mode().contains(TermMode::BRACKETED_PASTE))
                                        .unwrap_or(false);
                                    paste_text(pty, &text, bracketed);
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
                        InputAction::Search => {
                            self.is_searching = true;
                            self.search_query.clear();
                            self.search_matches.clear();
                            self.search_match_idx = 0;
                            if let Some(ref r) = self.renderer {
                                r.window.request_redraw();
                            }
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(ref mut r) = self.renderer {
                    if let Ok(term) = self.terminal.lock() {
                        let (lines, cursor) = term.snapshot();
                        drop(term);
                        let mut lines = lines;
                        let mut cursor = cursor;
                        if let Some((ref preedit_text, _)) = self.preedit {
                            if cursor.is_visible && cursor.row < lines.len() {
                                let row = cursor.row;
                                let mut c_idx = cursor.col;
                                for ch in preedit_text.chars() {
                                    if unicode_width::UnicodeWidthChar::width(ch) == Some(0) {
                                        // Combining mark / Tashkeel: attach to previous cell if exists
                                        if c_idx > cursor.col && c_idx - 1 < lines[row].cells.len()
                                        {
                                            lines[row].cells[c_idx - 1].zerowidth.push(ch);
                                        }
                                    } else if c_idx < lines[row].cells.len() {
                                        lines[row].cells[c_idx].c = ch;
                                        lines[row].cells[c_idx].fg = AnsiColor::Named(
                                            alacritty_terminal::vte::ansi::NamedColor::Yellow,
                                        );
                                        lines[row].cells[c_idx].flags.insert(
                                            alacritty_terminal::term::cell::Flags::UNDERLINE,
                                        );
                                        let w =
                                            unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
                                        c_idx += w;
                                    }
                                }
                                cursor.col = c_idx;
                            }
                        }
                        if self.is_searching && !lines.is_empty() {
                            // Highlight visible matches in the viewport
                            let display_offset = self
                                .terminal
                                .lock()
                                .map(|t| t.display_offset())
                                .unwrap_or(0);
                            for (idx, &(m_line, m_col, m_len)) in
                                self.search_matches.iter().enumerate()
                            {
                                let v_row = m_line + display_offset as i32;
                                if v_row >= 0 && (v_row as usize) < lines.len() {
                                    let row = v_row as usize;
                                    let is_current = idx == self.search_match_idx;
                                    for c in m_col..(m_col + m_len).min(lines[row].cells.len()) {
                                        lines[row].cells[c].is_selected = true;
                                        if is_current {
                                            lines[row].cells[c].fg = AnsiColor::Named(
                                                alacritty_terminal::vte::ansi::NamedColor::BrightYellow,
                                            );
                                        }
                                    }
                                }
                            }

                            // Render search overlay on the bottom line
                            let last_r = lines.len() - 1;
                            let match_info = if self.search_matches.is_empty() {
                                if self.search_query.is_empty() {
                                    String::new()
                                } else {
                                    " [0/0]".to_string()
                                }
                            } else {
                                format!(
                                    " [{}/{}]",
                                    self.search_match_idx + 1,
                                    self.search_matches.len()
                                )
                            };
                            let banner =
                                format!(" 🔍 Search: {}_{} ", self.search_query, match_info);
                            let banner_chars: Vec<char> = banner.chars().collect();
                            for (c_idx, cell) in lines[last_r].cells.iter_mut().enumerate() {
                                if c_idx < banner_chars.len() {
                                    cell.c = banner_chars[c_idx];
                                    cell.fg = AnsiColor::Named(
                                        alacritty_terminal::vte::ansi::NamedColor::BrightWhite,
                                    );
                                    cell.bg = AnsiColor::Named(
                                        alacritty_terminal::vte::ansi::NamedColor::Blue,
                                    );
                                    cell.flags
                                        .insert(alacritty_terminal::term::cell::Flags::BOLD);
                                } else {
                                    cell.c = ' ';
                                    cell.bg = AnsiColor::Named(
                                        alacritty_terminal::vte::ansi::NamedColor::Blue,
                                    );
                                }
                            }
                        }
                        match r.render(&lines, &cursor) {
                            Ok(crate::renderer::RenderStatus::Presented) => {
                                self.is_dirty = false;
                                self.last_render_time = std::time::Instant::now();
                            }
                            Ok(crate::renderer::RenderStatus::Retry) => {
                                self.is_dirty = true;
                                r.window.request_redraw();
                            }
                            Ok(crate::renderer::RenderStatus::Skipped) => {}
                            Err(e) => {
                                eprintln!("Render error: {:?}", e);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Ok(mut term) = self.terminal.lock() {
            if let Some(timeout) = term.sync_timeout() {
                if std::time::Instant::now() >= timeout {
                    term.stop_sync();
                    self.is_dirty = true;
                }
            }
        }
        if self.is_dirty {
            if let Some(ref r) = self.renderer {
                r.window.request_redraw();
            }
        }
    }
}
