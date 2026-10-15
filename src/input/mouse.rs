//! Mouse decisions that need no window: click counting, wheel accumulation, and which
//! buttons and motions are reported to the program.

use alacritty_terminal::term::TermMode;
use std::time::{Duration, Instant};
use winit::event::{MouseButton, MouseScrollDelta};

/// Clicks closer together than this (on the same cell) count as double/triple clicks.
pub const MULTI_CLICK: Duration = Duration::from_millis(400);
/// Lines scrolled by one notch of a mouse wheel.
pub const LINES_PER_WHEEL_NOTCH: f64 = 3.0;

/// xterm button code of a mouse button (0 left, 1 middle, 2 right).
pub fn button_code(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        _ => None,
    }
}

/// Counts single, double and triple clicks on one cell.
#[derive(Debug, Default)]
pub struct ClickCounter {
    last: Option<(Instant, usize, usize)>,
    count: u8,
}

impl ClickCounter {
    /// A press at `(col, row)`: returns 1, 2 or 3, cycling back to 1 after a triple click.
    pub fn click(&mut self, now: Instant, col: usize, row: usize) -> u8 {
        self.count = match self.last {
            Some((t, c, r)) if now - t < MULTI_CLICK && (c, r) == (col, row) => self.count % 3 + 1,
            _ => 1,
        };
        self.last = Some((now, col, row));
        self.count
    }
}

/// Sums fractional wheel or trackpad deltas into whole steps.
#[derive(Debug, Default)]
pub struct StepAccumulator {
    accum: f64,
}

impl StepAccumulator {
    /// Add `delta` and return the whole steps it completes (keeping the remainder).
    pub fn add(&mut self, delta: f64) -> i32 {
        self.accum += delta;
        let steps = self.accum.trunc();
        if steps != 0.0 {
            self.accum -= steps;
        }
        steps as i32
    }
}

/// Lines a wheel event scrolls (positive is up).
pub fn wheel_lines(delta: MouseScrollDelta, line_height: f64) -> f64 {
    match delta {
        MouseScrollDelta::LineDelta(_, y) => y as f64 * LINES_PER_WHEEL_NOTCH,
        MouseScrollDelta::PixelDelta(p) => p.y / line_height,
    }
}

/// What `steps` whole wheel lines do in a pane with terminal mode `mode`.
#[derive(Debug, PartialEq, Eq)]
pub enum WheelAction {
    /// Report wheel button `button` (64 up, 65 down) `count` times.
    Report {
        button: u8,
        count: u32,
    },
    /// Alternate screen without mouse mode (less, man): send these arrow keys.
    Keys(Vec<u8>),
    /// Scroll the scrollback by this many lines.
    Scrollback(i32),
    None,
}

pub fn wheel_action(mode: TermMode, steps: i32) -> WheelAction {
    if steps == 0 {
        return WheelAction::None;
    }
    if mode.intersects(TermMode::MOUSE_MODE) {
        let button = if steps > 0 { 64 } else { 65 };
        WheelAction::Report {
            button,
            count: steps.unsigned_abs(),
        }
    } else if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
        let key: &[u8] = match (steps > 0, mode.contains(TermMode::APP_CURSOR)) {
            (true, true) => b"\x1bOA",
            (true, false) => b"\x1b[A",
            (false, true) => b"\x1bOB",
            (false, false) => b"\x1b[B",
        };
        WheelAction::Keys(key.repeat(steps.unsigned_abs() as usize))
    } else if !mode.contains(TermMode::ALT_SCREEN) {
        WheelAction::Scrollback(steps)
    } else {
        WheelAction::None
    }
}

/// Whether a button press or release is reported to the program, and the button the
/// program sees held afterwards.
///
/// Shift bypasses mouse reporting, as in xterm and Windows Terminal. A release goes where
/// its press went, so a Shift+drag selection never leaks drag/release reports.
pub fn button_report(
    pressed: bool,
    mode: TermMode,
    shift: bool,
    held: Option<u8>,
    code: u8,
) -> (bool, Option<u8>) {
    let report = if pressed {
        mode.intersects(TermMode::MOUSE_MODE) && !shift
    } else {
        held == Some(code)
    };
    let held = if pressed && report {
        Some(code)
    } else if !pressed && held == Some(code) {
        None
    } else {
        held
    };
    (report, held)
}

/// The motion report (button code + 32) for a pointer move onto a new cell, if any:
/// 1003 reports every motion, 1002 only while a button is held, and a Shift+drag selection
/// is not reported.
pub fn motion_report(mode: TermMode, held: Option<u8>, selecting: bool) -> Option<u8> {
    let button = match held {
        Some(b) if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) => Some(b),
        None if mode.contains(TermMode::MOUSE_MOTION) && !selecting => Some(3),
        _ => None,
    };
    button.map(|b| 32 + b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_count_up_to_three_on_one_cell() {
        let mut c = ClickCounter::default();
        let t = Instant::now();
        assert_eq!(c.click(t, 1, 1), 1);
        assert_eq!(c.click(t + Duration::from_millis(100), 1, 1), 2);
        assert_eq!(c.click(t + Duration::from_millis(200), 1, 1), 3);
        assert_eq!(c.click(t + Duration::from_millis(300), 1, 1), 1);
    }

    #[test]
    fn slow_or_moved_clicks_start_over() {
        let mut c = ClickCounter::default();
        let t = Instant::now();
        c.click(t, 1, 1);
        assert_eq!(c.click(t + MULTI_CLICK, 1, 1), 1);
        assert_eq!(c.click(t + MULTI_CLICK, 2, 1), 1);
    }

    #[test]
    fn steps_keep_the_remainder() {
        let mut a = StepAccumulator::default();
        assert_eq!(a.add(0.4), 0);
        assert_eq!(a.add(0.4), 0);
        assert_eq!(a.add(0.4), 1);
        assert_eq!(a.add(-1.3), -1);
        assert_eq!(a.add(2.0), 1);
    }

    #[test]
    fn wheel_in_each_mode() {
        assert_eq!(
            wheel_action(TermMode::MOUSE_REPORT_CLICK, -2),
            WheelAction::Report {
                button: 65,
                count: 2
            }
        );
        let alt = TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL;
        assert_eq!(
            wheel_action(alt, 2),
            WheelAction::Keys(b"\x1b[A\x1b[A".to_vec())
        );
        assert_eq!(
            wheel_action(alt | TermMode::APP_CURSOR, -1),
            WheelAction::Keys(b"\x1bOB".to_vec())
        );
        assert_eq!(wheel_action(TermMode::ALT_SCREEN, 1), WheelAction::None);
        assert_eq!(
            wheel_action(TermMode::NONE, -3),
            WheelAction::Scrollback(-3)
        );
    }

    #[test]
    fn shift_drag_is_never_reported() {
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG;
        assert_eq!(button_report(true, mode, true, None, 0), (false, None));
        assert_eq!(button_report(false, mode, false, None, 0), (false, None));
        assert_eq!(button_report(true, mode, false, None, 0), (true, Some(0)));
        assert_eq!(button_report(false, mode, true, Some(0), 0), (true, None));
        assert_eq!(
            button_report(false, mode, false, Some(0), 2),
            (false, Some(0))
        );
    }

    #[test]
    fn motion_reports() {
        assert_eq!(
            motion_report(TermMode::MOUSE_DRAG, Some(0), false),
            Some(32)
        );
        assert_eq!(motion_report(TermMode::MOUSE_DRAG, None, false), None);
        assert_eq!(motion_report(TermMode::MOUSE_MOTION, None, false), Some(35));
        assert_eq!(motion_report(TermMode::MOUSE_MOTION, None, true), None);
        assert_eq!(motion_report(TermMode::NONE, Some(1), false), None);
    }
}
