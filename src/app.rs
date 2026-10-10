use alacritty_terminal::event::{Event as TermEvent, WindowSize};
use alacritty_terminal::index::Side;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, Rgb};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{UserAttentionType, WindowAttributes, WindowId};

use crate::config::TwittyConfig;
use crate::input::{encode_mouse, encode_paste, handle_key, InputAction};
use crate::pty::Pty;
use crate::renderer::Renderer;
use crate::terminal::Terminal;

const WINDOW_TITLE: &str = "Twitty · RTL Terminal";
const DEFAULT_FONT_SIZE: f32 = 14.5;
const LINES_PER_WHEEL_NOTCH: f64 = 3.0;
/// Clicks closer together than this (on the same cell) count as double/triple clicks.
const MULTI_CLICK: Duration = Duration::from_millis(400);
/// How long a copied selection stays highlighted before it is cleared.
const COPIED_HIGHLIGHT: Duration = Duration::from_millis(350);

#[derive(Debug)]
pub enum AppEvent {
    PtyData(Vec<u8>),
    Terminal(TermEvent),
    PtyExit,
    /// Clear the highlight of a copied selection, unless a newer selection was started.
    ClearSelection(u64),
}

pub struct App {
    proxy: EventLoopProxy<AppEvent>,
    terminal: Arc<Mutex<Terminal>>,
    pty: Option<Pty>,
    renderer: Option<Renderer>,
    modifiers: ModifiersState,
    mouse_col: usize,
    mouse_row: usize,
    /// Buttons currently held, for drag reporting (xterm codes 0-2).
    mouse_button_down: Option<u8>,
    mouse_x: f64,
    /// Kept alive for the whole session: on Linux, copied text is served by this object and
    /// would vanish if it were dropped right after copying.
    clipboard: Option<arboard::Clipboard>,
    /// A mouse selection is being dragged.
    selecting: bool,
    /// Bumped on every new selection, so a delayed clear never removes a newer one.
    selection_generation: u64,
    last_click: Option<(Instant, usize, usize)>,
    click_count: u8,
    /// Fractional wheel/trackpad lines not yet applied.
    scroll_accum: f64,
    config: TwittyConfig,
    config_dirty: bool,
    needs_redraw: bool,
}

impl App {
    pub fn new(proxy: EventLoopProxy<AppEvent>) -> Self {
        let proxy_clone = proxy.clone();
        let terminal = Arc::new(Mutex::new(Terminal::with_events(80, 24, move |event| {
            let _ = proxy_clone.send_event(AppEvent::Terminal(event));
        })));

        Self {
            proxy,
            terminal,
            pty: None,
            renderer: None,
            modifiers: ModifiersState::empty(),
            mouse_col: 0,
            mouse_row: 0,
            mouse_button_down: None,
            mouse_x: 0.0,
            clipboard: None,
            selecting: false,
            selection_generation: 0,
            last_click: None,
            click_count: 0,
            scroll_accum: 0.0,
            config: TwittyConfig::load(),
            config_dirty: false,
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

    fn mode(&self) -> TermMode {
        self.terminal
            .lock()
            .map(|t| t.mode())
            .unwrap_or(TermMode::NONE)
    }

    fn write_pty(&self, bytes: &[u8]) {
        if let Some(ref pty) = self.pty {
            let _ = pty.write(bytes);
        }
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
    fn scroll_to_bottom(&mut self) {
        let scrolled = match self.terminal.lock() {
            Ok(mut term) if term.display_offset() > 0 => {
                term.scroll_to_bottom();
                true
            }
            _ => false,
        };
        if scrolled {
            self.request_redraw();
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
        // Written once on exit instead of on every zoom step.
        self.config_dirty = true;
        if let Some(ref mut r) = self.renderer {
            r.set_font_size(clamped);
        }
        self.sync_grid();
    }

    fn paste_clipboard(&mut self) {
        let Some(Ok(text)) = self.clipboard().map(|c| c.get_text()) else {
            return;
        };
        let bracketed = self.mode().contains(TermMode::BRACKETED_PASTE);
        self.write_pty(&encode_paste(&text, bracketed));
        self.scroll_to_bottom();
    }

    fn clipboard(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        self.clipboard.as_mut()
    }

    fn set_clipboard(&mut self, text: String) {
        if let Some(clipboard) = self.clipboard() {
            let _ = clipboard.set_text(text);
        }
    }

    fn clear_selection(&mut self) {
        let had = match self.terminal.lock() {
            Ok(mut term) if term.has_selection() => {
                term.clear_selection();
                true
            }
            _ => false,
        };
        self.selecting = false;
        if had {
            self.request_redraw();
        }
    }

    /// Copy the selection to the clipboard. Returns false when nothing is selected.
    fn copy_selection(&mut self) -> bool {
        let text = self.terminal.lock().ok().and_then(|t| t.selection_text());
        let Some(text) = text else { return false };
        self.set_clipboard(text);
        true
    }

    fn start_selection(&mut self) {
        let now = Instant::now();
        let (col, row) = (self.mouse_col, self.mouse_row);
        self.click_count = match self.last_click {
            Some((t, c, r)) if now - t < MULTI_CLICK && (c, r) == (col, row) => {
                self.click_count % 3 + 1
            }
            _ => 1,
        };
        self.last_click = Some((now, col, row));
        self.selection_generation += 1;
        let side = self
            .renderer
            .as_ref()
            .map_or(Side::Left, |r| r.pixel_side(self.mouse_x));
        if let Ok(mut term) = self.terminal.lock() {
            term.start_selection(col, row, side, self.click_count);
        }
        self.selecting = true;
        self.request_redraw();
    }

    /// Mouse released: copy what was selected, flash it briefly, then clear the highlight.
    fn finish_selection(&mut self) {
        self.selecting = false;
        if !self.copy_selection() {
            self.clear_selection();
            return;
        }
        let generation = self.selection_generation;
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            std::thread::sleep(COPIED_HIGHLIGHT);
            let _ = proxy.send_event(AppEvent::ClearSelection(generation));
        });
    }

    fn send_mouse(&self, button: u8, pressed: bool) {
        let sgr = self.mode().contains(TermMode::SGR_MOUSE);
        let seq = encode_mouse(
            button,
            self.mouse_col,
            self.mouse_row,
            pressed,
            self.modifiers,
            sgr,
        );
        self.write_pty(&seq);
    }

    fn handle_terminal_event(&mut self, event: TermEvent) {
        match event {
            TermEvent::PtyWrite(text) => self.write_pty(text.as_bytes()),
            TermEvent::Title(title) => {
                if let Some(ref r) = self.renderer {
                    r.window.set_title(&title);
                }
            }
            TermEvent::ResetTitle => {
                if let Some(ref r) = self.renderer {
                    r.window.set_title(WINDOW_TITLE);
                }
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
                self.write_pty(format(rgb).as_bytes());
            }
            TermEvent::TextAreaSizeRequest(format) => {
                let Some(ref r) = self.renderer else { return };
                let (cols, rows) = r.compute_grid_size();
                let size = WindowSize {
                    num_lines: rows as u16,
                    num_cols: cols as u16,
                    cell_width: r.char_width as u16,
                    cell_height: r.line_height as u16,
                };
                self.write_pty(format(size).as_bytes());
            }
            TermEvent::ClipboardStore(_, text) => {
                // OSC 52 copy (e.g. from tmux/vim over SSH).
                self.set_clipboard(text);
            }
            // OSC 52 reads are refused: any program could silently exfiltrate the clipboard.
            TermEvent::ClipboardLoad(..) => {}
            _ => {}
        }
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

        let mut renderer =
            match pollster::block_on(Renderer::new(window.clone(), self.config.font_size)) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Failed to initialize renderer: {:?}", e);
                    event_loop.exit();
                    return;
                }
            };

        renderer.set_opacity(self.config.background_opacity);
        renderer.cursor_style = self.config.cursor_style.clone();
        renderer.palette = self.config.palette();

        let (cols, rows) = renderer.compute_grid_size();
        if let Ok(mut term) = self.terminal.lock() {
            term.resize(cols, rows);
        }

        self.pty = self.spawn_pty(cols as u16, rows as u16);
        if self.pty.is_none() {
            event_loop.exit();
            return;
        }
        self.renderer = Some(renderer);
        window.request_redraw();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::PtyData(data) => {
                if let Ok(mut term) = self.terminal.lock() {
                    term.process_bytes(&data);
                }
                self.request_redraw();
            }
            AppEvent::Terminal(event) => self.handle_terminal_event(event),
            // The shell exited (`exit`, crash, or closed by the OS): close the window
            // like Windows Terminal does, instead of respawning forever.
            AppEvent::PtyExit => event_loop.exit(),
            AppEvent::ClearSelection(generation) => {
                if generation == self.selection_generation && !self.selecting {
                    self.clear_selection();
                }
            }
        }
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
                self.sync_grid();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(ref mut r) = self.renderer {
                    r.set_scale_factor(scale_factor as f32);
                }
                self.sync_grid();
            }
            WindowEvent::Focused(focused) => {
                // The release of a drag that ends outside the window may never arrive.
                if !focused && self.selecting {
                    self.mouse_button_down = None;
                    self.finish_selection();
                }
                if let Some(ref mut r) = self.renderer {
                    r.set_focused(focused);
                    r.window.request_redraw();
                }
                if self.mode().contains(TermMode::FOCUS_IN_OUT) {
                    self.write_pty(if focused { b"\x1b[I" } else { b"\x1b[O" });
                }
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let Some(ref r) = self.renderer else { return };
                let (col, row) = r.pixel_to_cell(position.x, position.y);
                let side = r.pixel_side(position.x);
                self.mouse_x = position.x;
                if self.selecting {
                    if let Ok(mut term) = self.terminal.lock() {
                        term.update_selection(col, row, side);
                    }
                    self.request_redraw();
                }
                if (col, row) == (self.mouse_col, self.mouse_row) {
                    return;
                }
                self.mouse_col = col;
                self.mouse_row = row;

                // 1003 reports every motion, 1002 only while a button is held.
                let mode = self.mode();
                let button = match self.mouse_button_down {
                    Some(b) if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) => {
                        Some(b)
                    }
                    None if mode.contains(TermMode::MOUSE_MOTION) => Some(3),
                    _ => None,
                };
                if let Some(b) = button {
                    self.send_mouse(32 + b, true);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let mode = self.mode();
                let code = match button {
                    MouseButton::Left => 0,
                    MouseButton::Middle => 1,
                    MouseButton::Right => 2,
                    _ => return,
                };
                let pressed = state == ElementState::Pressed;
                self.mouse_button_down = if pressed { Some(code) } else { None };

                // Shift bypasses mouse reporting, as in xterm and Windows Terminal.
                if mode.intersects(TermMode::MOUSE_MODE) && !self.modifiers.shift_key() {
                    self.send_mouse(code, pressed);
                } else if button == MouseButton::Left {
                    if pressed {
                        self.start_selection();
                    } else if self.selecting {
                        self.finish_selection();
                    }
                } else if pressed && button == MouseButton::Middle {
                    self.paste_clipboard();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let line_height = self
                    .renderer
                    .as_ref()
                    .map_or(20.0, |r| r.line_height as f64);
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64 * LINES_PER_WHEEL_NOTCH,
                    MouseScrollDelta::PixelDelta(p) => p.y / line_height,
                };

                if self.modifiers.control_key() && self.modifiers.shift_key() {
                    // Ctrl + Shift + wheel: background opacity.
                    let step = if lines > 0.0 { 0.05 } else { -0.05 };
                    let opacity = (self.config.background_opacity + step).clamp(0.2, 1.0);
                    self.config.background_opacity = opacity;
                    self.config_dirty = true;
                    if let Some(ref mut r) = self.renderer {
                        r.set_opacity(opacity);
                    }
                    self.request_redraw();
                    return;
                }
                if self.modifiers.control_key() {
                    // Ctrl + wheel: zoom one step per notch.
                    if lines > 0.0 {
                        let cur = self.config.font_size;
                        self.update_font_size(cur + 1.0);
                    } else if lines < 0.0 {
                        let cur = self.config.font_size;
                        self.update_font_size(cur - 1.0);
                    }
                    return;
                }

                // Accumulate so trackpads scroll smoothly instead of 3 lines per tiny event.
                self.scroll_accum += lines;
                let steps = self.scroll_accum.trunc() as i32;
                if steps == 0 {
                    return;
                }
                self.scroll_accum -= steps as f64;

                let mode = self.mode();
                if mode.intersects(TermMode::MOUSE_MODE) {
                    let btn = if steps > 0 { 64 } else { 65 };
                    for _ in 0..steps.unsigned_abs() {
                        self.send_mouse(btn, true);
                    }
                } else if mode.contains(TermMode::ALT_SCREEN)
                    && mode.contains(TermMode::ALTERNATE_SCROLL)
                {
                    // Alternate screen without mouse mode (less, man): send arrow keys.
                    let key: &[u8] = match (steps > 0, mode.contains(TermMode::APP_CURSOR)) {
                        (true, true) => b"\x1bOA",
                        (true, false) => b"\x1b[A",
                        (false, true) => b"\x1bOB",
                        (false, false) => b"\x1b[B",
                    };
                    self.write_pty(&key.repeat(steps.unsigned_abs() as usize));
                } else if !mode.contains(TermMode::ALT_SCREEN) {
                    if let Ok(mut term) = self.terminal.lock() {
                        term.scroll_display(steps);
                    }
                    self.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let mode = self
                    .terminal
                    .lock()
                    .map(|t| t.mode())
                    .unwrap_or_else(|_| TermMode::empty());
                if let Some(action) = handle_key(&event, self.modifiers, mode) {
                    match action {
                        InputAction::Bytes(bytes) => {
                            self.clear_selection();
                            self.write_pty(&bytes);
                            self.scroll_to_bottom();
                        }
                        InputAction::ZoomIn => {
                            let cur = self.config.font_size;
                            self.update_font_size(cur + 1.0);
                        }
                        InputAction::ZoomOut => {
                            let cur = self.config.font_size;
                            self.update_font_size(cur - 1.0);
                        }
                        InputAction::ZoomReset => self.update_font_size(DEFAULT_FONT_SIZE),
                        InputAction::Paste => self.paste_clipboard(),
                        InputAction::Copy => {
                            self.copy_selection();
                        }
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
