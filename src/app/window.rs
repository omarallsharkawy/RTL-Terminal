//! The window and its renderer. Every call into the renderer lives in this file.

use alacritty_terminal::index::Side;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, Rgb};
use std::sync::Arc;
use std::time::Instant;
use winit::event_loop::ActiveEventLoop;
use winit::window::{CursorIcon, UserAttentionType, WindowAttributes};

use super::App;
use crate::config::TwittyConfig;
use crate::layout::{PaneId, Rect};
use crate::render::{PaneFrame, Renderer, TabLabel};
use crate::session::workspace::{GridGeometry, Workspace};
use crate::ui::hit::{hit_test, Hit, HitGeometry};
use crate::ui::settings::{clamp_font_size, step_opacity};

pub(super) const WINDOW_TITLE: &str = "Twitty · RTL Terminal";

impl GridGeometry for Renderer {
    fn content_rect(&self, tab_count: usize) -> Rect {
        Renderer::content_rect(self, tab_count)
    }

    fn gap(&self) -> f32 {
        self.divider_gap()
    }

    fn grid_size(&self, rect: Rect) -> (usize, usize) {
        self.compute_grid_size(rect)
    }
}

/// The window, once it is open: its renderer, the pending-redraw flag and the cursor icon.
pub(super) struct Window {
    renderer: Option<Renderer>,
    needs_redraw: bool,
    cursor_icon: CursorIcon,
}

impl Window {
    pub(super) fn new() -> Self {
        Self {
            renderer: None,
            needs_redraw: false,
            cursor_icon: CursorIcon::Default,
        }
    }

    pub(super) fn is_open(&self) -> bool {
        self.renderer.is_some()
    }

    /// Create the window and its renderer. On failure the event loop is told to exit.
    pub(super) fn open(&mut self, event_loop: &ActiveEventLoop, config: &TwittyConfig) -> bool {
        let window_attrs = WindowAttributes::default()
            .with_title(WINDOW_TITLE)
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 580.0))
            .with_transparent(true);

        let window = match event_loop.create_window(window_attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("Failed to create window: {:?}", e);
                event_loop.exit();
                return false;
            }
        };

        window.set_ime_allowed(true);
        window.set_ime_purpose(winit::window::ImePurpose::Terminal);

        let mut renderer = match pollster::block_on(Renderer::new(window, config.font_size)) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Failed to initialize renderer: {:?}", e);
                event_loop.exit();
                return false;
            }
        };

        renderer.set_font_family(config.font_family.as_deref());
        renderer.set_opacity(config.background_opacity);
        renderer.set_cursor_style(config.cursor_style.clone());
        renderer.set_palette(config.palette());
        self.renderer = Some(renderer);
        true
    }

    pub(super) fn close(&mut self) {
        self.renderer = None;
    }

    /// Pane geometry, once the window is open.
    pub(super) fn grid(&self) -> Option<&Renderer> {
        self.renderer.as_ref()
    }

    /// Ask for one redraw; repeated requests before it happens are merged.
    pub(super) fn request_redraw(&mut self) {
        if let Some(ref r) = self.renderer {
            if !self.needs_redraw {
                self.needs_redraw = true;
                r.window().request_redraw();
            }
        }
    }

    /// Ask the system for a redraw directly, bypassing the merge.
    pub(super) fn request_redraw_now(&self) {
        if let Some(ref r) = self.renderer {
            r.window().request_redraw();
        }
    }

    pub(super) fn cursor_icon(&self) -> CursorIcon {
        self.cursor_icon
    }

    pub(super) fn set_cursor_icon(&mut self, icon: CursorIcon) {
        if self.cursor_icon != icon {
            self.cursor_icon = icon;
            if let Some(ref r) = self.renderer {
                r.window().set_cursor(icon);
            }
        }
    }

    pub(super) fn set_title(&self, title: &str) {
        if let Some(ref r) = self.renderer {
            r.window().set_title(title);
        }
    }

    pub(super) fn bell(&self) {
        if let Some(ref r) = self.renderer {
            r.window()
                .request_user_attention(Some(UserAttentionType::Informational));
        }
    }

    pub(super) fn set_preedit(&mut self, preedit: Option<(String, Option<(usize, usize)>)>) {
        if let Some(ref mut r) = self.renderer {
            r.set_preedit(preedit);
        }
    }

    /// The window has keyboard focus.
    pub(super) fn is_focused(&self) -> bool {
        self.renderer.as_ref().is_some_and(Renderer::is_focused)
    }

    pub(super) fn set_focused(&mut self, focused: bool) {
        if let Some(ref mut r) = self.renderer {
            r.set_focused(focused);
            r.window().request_redraw();
        }
    }

    pub(super) fn resize(&mut self, width: u32, height: u32) {
        if let Some(ref mut r) = self.renderer {
            r.resize(width, height);
        }
    }

    pub(super) fn set_scale_factor(&mut self, scale: f32) {
        if let Some(ref mut r) = self.renderer {
            r.set_scale_factor(scale);
        }
    }

    pub(super) fn set_font_size(&mut self, size: f32) {
        if let Some(ref mut r) = self.renderer {
            r.set_font_size(size);
        }
    }

    pub(super) fn set_opacity(&mut self, opacity: f32) {
        if let Some(ref mut r) = self.renderer {
            r.set_opacity(opacity);
        }
    }

    /// Pane `id` was closed: drop what the renderer cached for it.
    pub(super) fn forget_pane(&mut self, id: PaneId) {
        if let Some(r) = self.renderer.as_mut() {
            r.forget_pane(id);
        }
    }

    /// What lies under `(x, y)` in the window.
    pub(super) fn hit_test(&self, ws: &Workspace, x: f64, y: f64) -> Hit {
        let Some(r) = self.renderer.as_ref() else {
            return Hit::Nothing;
        };
        let n = ws.tabs().len();
        let geometry = HitGeometry {
            tab_bar_height: r.tab_bar_height(n),
            tabs: r.tab_rects(n),
            content: r.content_rect(n),
            gap: r.divider_gap(),
            slop: (r.metrics().scale * 3.0).max(2.0),
        };
        hit_test(&geometry, ws.active_tab().map(|t| &t.root), x, y)
    }

    /// The cell of pane `id` (drawn in `rect`) under `(x, y)`, and which side of it.
    pub(super) fn cell_at(
        &self,
        id: PaneId,
        rect: Rect,
        x: f64,
        y: f64,
    ) -> Option<(usize, usize, Side)> {
        let r = self.renderer.as_ref()?;
        Some(r.pixel_to_visual_cell(id, rect, x, y))
    }

    /// Height of a text line in pixels.
    pub(super) fn line_height(&self) -> Option<f64> {
        self.renderer
            .as_ref()
            .map(|r| r.metrics().line_height as f64)
    }

    /// Cell width and height in pixels.
    pub(super) fn cell_size(&self) -> Option<(u16, u16)> {
        let r = self.renderer.as_ref()?;
        let m = r.metrics();
        Some((m.char_width as u16, m.line_height as u16))
    }

    /// Divider width, for dragging dividers.
    pub(super) fn gap(&self) -> f32 {
        self.renderer.as_ref().map_or(1.0, Renderer::divider_gap)
    }

    /// The color OSC 4/10/11/12 asks for: palette entry `index`, or 256 foreground, 257
    /// background, 258 cursor.
    pub(super) fn palette_rgb(&self, index: usize) -> Option<Rgb> {
        let r = self.renderer.as_ref()?;
        let rgba = match index {
            256 => r.palette().foreground,
            257 => r.palette().background,
            258 => r.palette().cursor,
            i if i < 256 => r.palette().resolve(AnsiColor::Indexed(i as u8), false),
            _ => r.palette().foreground,
        };
        let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Some(Rgb {
            r: to8(rgba.r),
            g: to8(rgba.g),
            b: to8(rgba.b),
        })
    }

    /// The drawn cursor blink phase differs from `visible`.
    pub(super) fn blink_differs(&self, visible: bool) -> bool {
        self.renderer
            .as_ref()
            .is_some_and(|r| r.cursor_blink() != visible)
    }

    fn render(&mut self, tabs: &[TabLabel], panes: &[PaneFrame], dividers: &[Rect], blink: bool) {
        if let Some(ref mut r) = self.renderer {
            r.set_cursor_blink(blink);
            if let Err(e) = r.render(tabs, panes, dividers) {
                eprintln!("Render error: {:?}", e);
            }
        }
    }
}

impl App {
    /// Draw the active tab.
    pub(super) fn redraw(&mut self) {
        self.window.needs_redraw = false;
        let (tabs, panes, dividers) = self.frame();
        let visible = self.cursor_blink.visible();
        self.window.render(&tabs, &panes, &dividers, visible);
    }

    /// Snapshot everything the renderer draws for the active tab.
    fn frame(&self) -> (Vec<TabLabel>, Vec<PaneFrame>, Vec<Rect>) {
        let tabs = (0..self.ws.tabs().len())
            .map(|i| TabLabel {
                title: self.ws.tab_title(i),
                active: i == self.ws.active(),
            })
            .collect();
        let Some(r) = self.window.grid() else {
            return (tabs, Vec::new(), Vec::new());
        };
        let focused = self.focused_id();
        let panes = self
            .ws
            .active_layout(r)
            .into_iter()
            .filter_map(|(id, rect)| {
                let pane = self.ws.pane(id)?;
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
        let dividers = self
            .ws
            .active_dividers(r)
            .into_iter()
            .map(|d| d.rect)
            .collect();
        (tabs, panes, dividers)
    }

    /// The window gained or lost keyboard focus.
    pub(super) fn on_window_focus(&mut self, focused: bool) {
        // The release of a drag that ends outside the window may never arrive.
        if !focused {
            let selecting: Vec<PaneId> = self
                .ws
                .panes()
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
        self.window.set_focused(focused);
        if let Some(id) = self.focused_id() {
            self.report_focus(id, focused);
        }
    }

    pub(super) fn update_preedit(&mut self, preedit: Option<(String, Option<(usize, usize)>)>) {
        self.preedit_active = preedit.is_some();
        self.window.set_preedit(preedit);
        self.window.request_redraw();
    }

    /// Zoom by `steps` points.
    pub(super) fn zoom_by(&mut self, steps: i32) {
        let size = self.config.font_size + steps as f32;
        self.update_font_size(size);
    }

    pub(super) fn update_font_size(&mut self, new_size: f32) {
        let clamped = clamp_font_size(new_size);
        self.config.font_size = clamped;
        // Written once on exit instead of on every zoom step.
        self.config_dirty = true;
        self.window.set_font_size(clamped);
        self.apply_layout();
    }

    /// Change the background opacity by `steps` wheel notches.
    pub(super) fn step_opacity_by(&mut self, steps: i32) {
        let opacity = step_opacity(self.config.background_opacity, steps);
        self.config.background_opacity = opacity;
        self.config_dirty = true;
        self.window.set_opacity(opacity);
        self.window.request_redraw();
    }

    /// The window title follows the focused pane.
    pub(super) fn refresh_window_title(&self) {
        let title = self
            .ws
            .focused_title()
            .unwrap_or_else(|| WINDOW_TITLE.to_owned());
        self.window.set_title(&title);
    }
}
