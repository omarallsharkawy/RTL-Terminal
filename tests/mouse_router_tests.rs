use alacritty_terminal::term::TermMode;
use twitty::mouse::{MouseCommand, MouseRouter};
use winit::event::MouseButton;

#[test]
fn test_mouse_reporting_active_table() {
    // Mouse mode without shift and at bottom -> active
    assert!(MouseRouter::is_mouse_reporting_active(
        TermMode::MOUSE_REPORT_CLICK,
        false,
        0
    ));
    // Shift overrides mouse mode -> inactive
    assert!(!MouseRouter::is_mouse_reporting_active(
        TermMode::MOUSE_REPORT_CLICK,
        true,
        0
    ));
    // Scrollback offset > 0 overrides mouse mode -> inactive
    assert!(!MouseRouter::is_mouse_reporting_active(
        TermMode::MOUSE_REPORT_CLICK,
        false,
        5
    ));
    // No mouse mode enabled -> inactive
    assert!(!MouseRouter::is_mouse_reporting_active(
        TermMode::NONE,
        false,
        0
    ));
}

#[test]
fn test_tui_mouse_drag_does_not_hijack_into_local_selection() {
    let mut router = MouseRouter::new();
    let mode = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;

    // 1. Press inside TUI: should clear old selection and report press to app
    let (cmd1, cmd2) = router.on_press(MouseButton::Left, 10, 5, 1, false, mode, 0);
    assert_eq!(cmd1, MouseCommand::ClearSelection);
    assert_eq!(
        cmd2,
        Some(MouseCommand::ReportPress {
            btn: 0,
            col: 10,
            row: 5
        })
    );

    // 2. Drag inside TUI: must send ReportDrag, NEVER StartSelection
    let move_cmd = router.on_move(12, 5, true, false, mode, 0);
    assert_eq!(
        move_cmd,
        MouseCommand::ReportDrag {
            btn: 0,
            col: 12,
            row: 5
        }
    );

    // 3. Release inside TUI: must send ReportRelease
    let rel_cmd = router.on_release(MouseButton::Left, 12, 5, false, mode, 0);
    assert_eq!(
        rel_cmd,
        MouseCommand::ReportRelease {
            btn: 0,
            col: 12,
            row: 5
        }
    );
}

#[test]
fn test_shift_overrides_tui_mouse_for_local_selection() {
    let mut router = MouseRouter::new();
    let mode = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;

    // 1. Press with Shift: starts local selection, does NOT report press to app
    let (cmd1, cmd2) = router.on_press(MouseButton::Left, 10, 5, 1, true, mode, 0);
    assert_eq!(
        cmd1,
        MouseCommand::StartSelection {
            col: 10,
            row: 5,
            semantic: false,
            lines: false
        }
    );
    assert_eq!(cmd2, None);

    // 2. Move with Shift: extends local selection
    let move_cmd = router.on_move(15, 5, true, true, mode, 0);
    assert_eq!(move_cmd, MouseCommand::ExtendSelection { col: 15, row: 5 });

    // 3. Release: copies selection to clipboard
    let rel_cmd = router.on_release(MouseButton::Left, 15, 5, true, mode, 0);
    assert_eq!(rel_cmd, MouseCommand::CopySelection);
}

#[test]
fn test_tui_double_click_not_hijacked() {
    let mut router = MouseRouter::new();
    let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;

    // Double click in mouse mode without Shift: passes to app, not semantic selection
    let (cmd1, cmd2) = router.on_press(MouseButton::Left, 10, 5, 2, false, mode, 0);
    assert_eq!(cmd1, MouseCommand::ClearSelection);
    assert_eq!(
        cmd2,
        Some(MouseCommand::ReportPress {
            btn: 0,
            col: 10,
            row: 5
        })
    );
}

#[test]
fn test_wheel_dispatch_in_tui_vs_scrollback() {
    let mut router = MouseRouter::new();
    let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;

    // Normal wheel in TUI -> reports to app
    let wheel_cmd = router.on_wheel(1.0, 10, 5, false, mode, 0);
    assert_eq!(
        wheel_cmd,
        MouseCommand::ReportWheel {
            up: true,
            col: 10,
            row: 5
        }
    );

    // Shift+Wheel in TUI -> local scrollback
    let shift_wheel_cmd = router.on_wheel(1.0, 10, 5, true, mode, 0);
    assert_eq!(
        shift_wheel_cmd,
        MouseCommand::ScrollLocal {
            delta: 3,
            extend_selection: false
        }
    );
}

#[test]
fn test_focus_lost_clears_pending_tui_press() {
    let mut router = MouseRouter::new();
    let mode = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;

    router.on_press(MouseButton::Left, 10, 5, 1, false, mode, 0);
    let release_cmd = router.on_focus_lost();
    assert_eq!(
        release_cmd,
        Some(MouseCommand::ReportRelease {
            btn: 0,
            col: 0,
            row: 0
        })
    );
}
