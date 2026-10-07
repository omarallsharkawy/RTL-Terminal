use alacritty_terminal::term::TermMode;
use winit::event::MouseButton;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseCommand {
    None,
    ReportPress {
        btn: u8,
        col: usize,
        row: usize,
    },
    ReportRelease {
        btn: u8,
        col: usize,
        row: usize,
    },
    ReportDrag {
        btn: u8,
        col: usize,
        row: usize,
    },
    ReportMotion {
        col: usize,
        row: usize,
    },
    ReportWheel {
        up: bool,
        col: usize,
        row: usize,
    },
    AltScroll {
        up: bool,
    },
    StartSelection {
        col: usize,
        row: usize,
        semantic: bool,
        lines: bool,
    },
    ExtendSelection {
        col: usize,
        row: usize,
    },
    CopySelection,
    ClearSelection,
    ScrollLocal {
        delta: i32,
        extend_selection: bool,
    },
}

#[derive(Debug, Default, Clone)]
pub struct MouseRouter {
    pub active_button: Option<MouseButton>,
    pub press_local: bool,
    pub is_dragging: bool,
}

impl MouseRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_mouse_reporting_active(mode: TermMode, shift: bool, display_offset: usize) -> bool {
        mode.intersects(TermMode::MOUSE_MODE) && !shift && display_offset == 0
    }

    #[allow(clippy::too_many_arguments)]
    pub fn on_press(
        &mut self,
        button: MouseButton,
        col: usize,
        row: usize,
        click_count: usize,
        shift: bool,
        mode: TermMode,
        display_offset: usize,
    ) -> (MouseCommand, Option<MouseCommand>) {
        self.active_button = Some(button);
        self.is_dragging = false;

        if Self::is_mouse_reporting_active(mode, shift, display_offset) {
            self.press_local = false;
            let btn = match button {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
                _ => 0,
            };
            // Clear any old local selection when an active TUI mouse interaction begins
            (
                MouseCommand::ClearSelection,
                Some(MouseCommand::ReportPress { btn, col, row }),
            )
        } else {
            self.press_local = true;
            if button == MouseButton::Left {
                let semantic = click_count == 2;
                let lines = click_count >= 3;
                (
                    MouseCommand::StartSelection {
                        col,
                        row,
                        semantic,
                        lines,
                    },
                    None,
                )
            } else {
                (MouseCommand::None, None)
            }
        }
    }

    pub fn on_move(
        &mut self,
        col: usize,
        row: usize,
        has_mouse_down: bool,
        shift: bool,
        mode: TermMode,
        display_offset: usize,
    ) -> MouseCommand {
        if has_mouse_down {
            self.is_dragging = true;
            if self.press_local {
                MouseCommand::ExtendSelection { col, row }
            } else if Self::is_mouse_reporting_active(mode, shift, display_offset) {
                if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) {
                    let btn = match self.active_button {
                        Some(MouseButton::Left) => 0,
                        Some(MouseButton::Middle) => 1,
                        Some(MouseButton::Right) => 2,
                        _ => 0,
                    };
                    MouseCommand::ReportDrag { btn, col, row }
                } else {
                    MouseCommand::None
                }
            } else {
                MouseCommand::None
            }
        } else if Self::is_mouse_reporting_active(mode, shift, display_offset)
            && mode.contains(TermMode::MOUSE_MOTION)
        {
            MouseCommand::ReportMotion { col, row }
        } else {
            MouseCommand::None
        }
    }

    pub fn on_release(
        &mut self,
        button: MouseButton,
        col: usize,
        row: usize,
        shift: bool,
        mode: TermMode,
        display_offset: usize,
    ) -> MouseCommand {
        let was_local = self.press_local;
        let was_active_btn = self.active_button == Some(button);
        if was_active_btn {
            self.active_button = None;
            self.is_dragging = false;
        }

        if was_local {
            if button == MouseButton::Left {
                MouseCommand::CopySelection
            } else {
                MouseCommand::None
            }
        } else if Self::is_mouse_reporting_active(mode, shift, display_offset) {
            let btn = match button {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
                _ => 0,
            };
            MouseCommand::ReportRelease { btn, col, row }
        } else {
            MouseCommand::None
        }
    }

    pub fn on_wheel(
        &mut self,
        delta_y: f32,
        col: usize,
        row: usize,
        shift: bool,
        mode: TermMode,
        display_offset: usize,
    ) -> MouseCommand {
        let up = delta_y > 0.0;
        let prefer_scrollback = shift || display_offset > 0;

        if self.press_local && self.active_button.is_some() {
            // Auto-scroll local scrollback and extend selection while dragging
            let delta = if up { 3 } else { -3 };
            MouseCommand::ScrollLocal {
                delta,
                extend_selection: true,
            }
        } else if mode.intersects(TermMode::MOUSE_MODE) && !prefer_scrollback {
            MouseCommand::ReportWheel { up, col, row }
        } else if mode.contains(TermMode::ALT_SCREEN)
            && mode.contains(TermMode::ALTERNATE_SCROLL)
            && !prefer_scrollback
        {
            MouseCommand::AltScroll { up }
        } else {
            let delta = if up { 3 } else { -3 };
            MouseCommand::ScrollLocal {
                delta,
                extend_selection: self.press_local && self.active_button.is_some(),
            }
        }
    }

    pub fn on_focus_lost(&mut self) -> Option<MouseCommand> {
        let had_press = !self.press_local && self.active_button.is_some();
        let btn = match self.active_button {
            Some(MouseButton::Left) => 0,
            Some(MouseButton::Middle) => 1,
            Some(MouseButton::Right) => 2,
            _ => 0,
        };
        self.active_button = None;
        self.press_local = false;
        self.is_dragging = false;
        if had_press {
            Some(MouseCommand::ReportRelease {
                btn,
                col: 0,
                row: 0,
            })
        } else {
            None
        }
    }
}
