//! Routes keys, mouse presses and pointer motion through the overlays in
//! [`Layer::STACK`] order: the first open layer that takes an event stops it there.

use winit::event::{KeyEvent, MouseButton};
use winit::window::CursorIcon;

use super::App;
use crate::render::scene::palette::PaletteHit;
use crate::ui::layers::Layer;

/// What a layer did with an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Routed {
    /// Not taken: the next layer (and finally the panes) sees it.
    Pass,
    /// Taken.
    Taken,
    /// Taken, and the window should close (e.g. it closed the last pane).
    Exit,
}

impl Routed {
    fn from_exit(exit: bool) -> Self {
        if exit {
            Self::Exit
        } else {
            Self::Taken
        }
    }

    fn taken_if(taken: bool) -> Self {
        if taken {
            Self::Taken
        } else {
            Self::Pass
        }
    }

    fn from_option(close: Option<bool>) -> Self {
        close.map_or(Self::Pass, Self::from_exit)
    }
}

impl App {
    /// Offer `event` to the layers. `Some(close_window)` when one took it.
    pub(super) fn route_key(&mut self, event: &KeyEvent) -> Option<bool> {
        Layer::STACK
            .into_iter()
            .map(|layer| self.layer_key(layer, event))
            .find(|routed| *routed != Routed::Pass)
            .map(|routed| routed == Routed::Exit)
    }

    fn layer_key(&mut self, layer: Layer, event: &KeyEvent) -> Routed {
        match layer {
            Layer::Tooltip | Layer::Toasts => Routed::Pass,
            Layer::ContextMenu => Routed::from_option(self.context_menu_key(event)),
            Layer::AiConsent => Routed::taken_if(self.handle_ai_consent_key(event)),
            Layer::Welcome if self.welcome_open() => {
                Routed::from_exit(self.handle_welcome_key(event))
            }
            Layer::Palette if self.palette_open() => {
                Routed::from_exit(self.handle_palette_key(event))
            }
            Layer::Memory if self.memory_open() => Routed::from_exit(self.handle_memory_key(event)),
            Layer::Settings if self.settings_open() => {
                Routed::from_exit(self.handle_settings_key(event))
            }
            Layer::Welcome | Layer::Palette | Layer::Memory | Layer::Settings => Routed::Pass,
            Layer::BlockAi => Routed::taken_if(self.handle_block_ai_key(event)),
            Layer::Composer => Routed::from_option(self.handle_composer_key(event)),
            Layer::AgentPanel => Routed::from_option(self.handle_agent_panel_key(event)),
            Layer::Review => Routed::from_option(self.handle_review_key(event)),
        }
    }

    /// Offer a mouse button to the layers. `Some(close_window)` when one took it.
    pub(super) fn route_mouse(&mut self, button: MouseButton, pressed: bool) -> Option<bool> {
        Layer::STACK
            .into_iter()
            .map(|layer| self.layer_mouse(layer, button, pressed))
            .find(|routed| *routed != Routed::Pass)
            .map(|routed| routed == Routed::Exit)
    }

    fn layer_mouse(&mut self, layer: Layer, button: MouseButton, pressed: bool) -> Routed {
        match layer {
            Layer::Tooltip | Layer::Toasts => Routed::Pass,
            Layer::ContextMenu if self.context_menu_open() => {
                Routed::from_exit(self.context_menu_mouse(button, pressed))
            }
            Layer::AiConsent if self.ai_consent_open() => {
                if pressed {
                    self.ai_consent_mouse_press(button);
                }
                Routed::Taken
            }
            Layer::Welcome if self.welcome_open() => {
                self.on_welcome_mouse(button, pressed);
                Routed::Taken
            }
            Layer::Palette if self.palette_open() => {
                Routed::from_exit(pressed && self.on_palette_mouse_press(button))
            }
            Layer::Memory if self.memory_open() => {
                self.on_memory_mouse(button, pressed);
                Routed::Taken
            }
            Layer::Settings if self.settings_open() => {
                self.on_settings_mouse(button, pressed);
                Routed::Taken
            }
            Layer::ContextMenu
            | Layer::AiConsent
            | Layer::Welcome
            | Layer::Palette
            | Layer::Memory
            | Layer::Settings => Routed::Pass,
            Layer::BlockAi => Routed::taken_if(pressed && self.block_ai_mouse_press(button)),
            Layer::Composer if pressed && self.composer_mouse_press(button) => {
                Routed::from_exit(self.take_composer_exit_request())
            }
            Layer::Composer => Routed::Pass,
            Layer::AgentPanel => Routed::taken_if(if pressed {
                self.agent_panel_mouse_press(button)
            } else {
                self.agent_panel_mouse_release(button)
            }),
            Layer::Review => Routed::taken_if(pressed && self.review_mouse_press(button)),
        }
    }

    /// Pointer motion for the modal layers, which take all of it while open. True when one did.
    pub(super) fn route_modal_motion(&mut self, x: f64, y: f64) -> bool {
        Layer::STACK
            .into_iter()
            .filter(|layer| layer.is_modal())
            .any(|layer| self.layer_motion(layer, x, y))
    }

    fn layer_motion(&mut self, layer: Layer, x: f64, y: f64) -> bool {
        match layer {
            Layer::ContextMenu if self.context_menu_open() => {
                self.context_menu_motion();
                true
            }
            Layer::AiConsent => match self.ai_consent_cursor(x, y) {
                Some(icon) => {
                    self.window.set_cursor_icon(icon);
                    true
                }
                None => false,
            },
            Layer::Welcome if self.welcome_open() => {
                self.on_welcome_cursor_moved(x, y);
                true
            }
            Layer::Palette if self.palette_open() => {
                let icon = match self.window.palette_hit(x, y) {
                    PaletteHit::Item(_) => CursorIcon::Pointer,
                    _ => CursorIcon::Default,
                };
                self.window.set_cursor_icon(icon);
                true
            }
            Layer::Memory if self.memory_open() => {
                self.on_memory_cursor_moved(x, y);
                true
            }
            Layer::Settings if self.settings_open() => {
                self.on_settings_cursor_moved(x, y);
                true
            }
            _ => false,
        }
    }

    /// The pointer shape the docked layers want at `(x, y)`, top first, and whether the
    /// pointer then stops there (the panes below see no hover). Nothing while a pane holds a
    /// captured press.
    pub(super) fn route_docked_cursor(
        &mut self,
        x: f64,
        y: f64,
        captured: bool,
    ) -> Option<(CursorIcon, bool)> {
        let (icon, stop, over_block) = Layer::STACK
            .into_iter()
            .filter(|layer| !layer.is_modal())
            .find_map(|layer| self.layer_cursor(layer, x, y))?;
        if captured {
            return None;
        }
        if over_block {
            self.window.set_block_hover(None);
        }
        Some((icon, stop))
    }

    /// `(icon, stop, hides block hover)` for a docked layer under the pointer.
    fn layer_cursor(&mut self, layer: Layer, x: f64, y: f64) -> Option<(CursorIcon, bool, bool)> {
        match layer {
            Layer::BlockAi => self.block_ai_cursor(x, y).map(|icon| (icon, true, true)),
            Layer::Composer => self.composer_cursor(x, y).map(|icon| (icon, true, true)),
            Layer::AgentPanel => self
                .agent_panel_mouse_move(x, y)
                .map(|icon| (icon, true, false)),
            Layer::Review => self.review_cursor(x, y).map(|icon| (icon, false, false)),
            Layer::Tooltip
            | Layer::Toasts
            | Layer::ContextMenu
            | Layer::AiConsent
            | Layer::Welcome
            | Layer::Palette
            | Layer::Memory
            | Layer::Settings => None,
        }
    }

    /// The pointer left the window: tell the modal layers that track hover.
    pub(super) fn route_pointer_left(&mut self) {
        for layer in Layer::STACK {
            match layer {
                Layer::Welcome if self.welcome_open() => self.on_welcome_cursor_left(),
                Layer::Memory if self.memory_open() => self.on_memory_cursor_left(),
                Layer::Settings if self.settings_open() => self.on_settings_cursor_left(),
                _ => {}
            }
        }
    }
}
