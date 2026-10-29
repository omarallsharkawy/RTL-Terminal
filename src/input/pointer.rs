//! The pointer: where it is, which pane a held button belongs to, the pane it hovers, and a
//! divider being dragged.

use winit::window::CursorIcon;

use crate::layout::{Divider, PaneId, SplitDir};
use crate::ui::hit::Hit;

#[derive(Debug, Default)]
pub(crate) struct PointerState {
    /// Last pointer position in physical pixels.
    pub pos: (f64, f64),
    /// The pane a pressed mouse button belongs to until it is released.
    pub capture: Option<PaneId>,
    /// The pane the pointer last moved over.
    pub hover: Option<PaneId>,
    /// A divider is being dragged.
    pub drag: Option<Divider>,
}

/// What a button press lands on.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PressTarget {
    /// Activate this tab.
    Tab(usize),
    /// The press belongs to this pane, which now captures the pointer.
    Pane(PaneId),
    /// Nothing more to do (tab bar, divider grab, empty space, non-left click on a tab).
    None,
}

impl PointerState {
    /// A button was pressed on `hit`. Left presses on a divider start dragging it.
    pub fn press(&mut self, hit: Hit, left: bool) -> PressTarget {
        match hit {
            Hit::Tab(i) if left => PressTarget::Tab(i),
            Hit::Divider(d) => {
                if left {
                    self.drag = Some(d);
                }
                PressTarget::None
            }
            Hit::Pane(id) => {
                self.capture = Some(id);
                PressTarget::Pane(id)
            }
            Hit::Tab(_) | Hit::TabBar | Hit::Nothing => PressTarget::None,
        }
    }

    /// A button was released: the pane that receives the release, if any. The release that
    /// ends a divider drag is swallowed.
    pub fn release(&mut self) -> Option<PaneId> {
        if self.drag.take().is_some() {
            return None;
        }
        self.capture.take()
    }

    /// Drop the capture and any divider drag (tab switch, focus loss...).
    pub(crate) fn cancel(&mut self) {
        self.capture = None;
        self.drag = None;
    }

    /// Pane `id` was closed.
    pub(crate) fn forget(&mut self, id: PaneId) {
        if self.capture == Some(id) {
            self.capture = None;
        }
        if self.hover == Some(id) {
            self.hover = None;
        }
        self.drag = None;
    }

    /// The pointer moved over `hit`. Returns the pane that gets the motion (the captured one,
    /// else the one under the pointer) and the pane it just left, if it left one.
    pub(crate) fn moved(&mut self, hit: &Hit) -> (Option<PaneId>, Option<PaneId>) {
        let target = self.capture.or(match hit {
            Hit::Pane(id) => Some(*id),
            _ => None,
        });
        let mut left = None;
        if self.hover != target {
            left = self.hover;
            self.hover = target;
        }
        (target, left)
    }
}

/// The cursor icon to switch to over `hit`, if it changes: a resize arrow over a divider (not
/// while a button is captured), back to the default arrow when leaving one.
pub(crate) fn cursor_icon_for(
    hit: &Hit,
    captured: bool,
    current: CursorIcon,
) -> Option<CursorIcon> {
    match hit {
        Hit::Divider(d) if !captured => Some(match d.dir {
            SplitDir::Horizontal => CursorIcon::ColResize,
            SplitDir::Vertical => CursorIcon::RowResize,
        }),
        _ if matches!(current, CursorIcon::ColResize | CursorIcon::RowResize) => {
            Some(CursorIcon::Default)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Rect;

    fn divider() -> Divider {
        let r = Rect::new(0.0, 0.0, 1.0, 10.0);
        Divider {
            rect: r,
            dir: SplitDir::Horizontal,
            path: vec![],
            area: r,
        }
    }

    #[test]
    fn press_and_release_in_a_pane() {
        let mut p = PointerState::default();
        assert_eq!(
            p.press(Hit::Pane(PaneId(3)), true),
            PressTarget::Pane(PaneId(3))
        );
        assert_eq!(p.capture, Some(PaneId(3)));
        assert_eq!(p.release(), Some(PaneId(3)));
        assert_eq!(p.capture, None);
        assert_eq!(p.release(), None);
    }

    #[test]
    fn divider_drag_swallows_its_release() {
        let mut p = PointerState::default();
        assert_eq!(p.press(Hit::Divider(divider()), true), PressTarget::None);
        assert!(p.drag.is_some());
        assert_eq!(p.release(), None);
        assert!(p.drag.is_none());
        // Right click on a divider does not drag.
        p.press(Hit::Divider(divider()), false);
        assert!(p.drag.is_none());
    }

    #[test]
    fn tabs_activate_only_on_left_click() {
        let mut p = PointerState::default();
        assert_eq!(p.press(Hit::Tab(2), true), PressTarget::Tab(2));
        assert_eq!(p.press(Hit::Tab(2), false), PressTarget::None);
        assert_eq!(p.press(Hit::TabBar, true), PressTarget::None);
    }

    #[test]
    fn capture_wins_over_hover_and_leaving_is_reported() {
        let mut p = PointerState::default();
        assert_eq!(p.moved(&Hit::Pane(PaneId(1))), (Some(PaneId(1)), None));
        assert_eq!(p.moved(&Hit::Pane(PaneId(1))), (Some(PaneId(1)), None));
        p.press(Hit::Pane(PaneId(1)), true);
        assert_eq!(p.moved(&Hit::Pane(PaneId(2))), (Some(PaneId(1)), None));
        p.release();
        assert_eq!(
            p.moved(&Hit::Pane(PaneId(2))),
            (Some(PaneId(2)), Some(PaneId(1)))
        );
        assert_eq!(p.moved(&Hit::Nothing), (None, Some(PaneId(2))));
    }

    #[test]
    fn cancel_and_forget() {
        let mut p = PointerState::default();
        p.press(Hit::Pane(PaneId(1)), true);
        p.moved(&Hit::Pane(PaneId(1)));
        p.forget(PaneId(1));
        assert_eq!((p.capture, p.hover), (None, None));
        p.press(Hit::Divider(divider()), true);
        p.press(Hit::Pane(PaneId(2)), true);
        p.cancel();
        assert!(p.capture.is_none() && p.drag.is_none());
    }

    #[test]
    fn resize_cursor_over_dividers_only() {
        let d = Hit::Divider(divider());
        assert_eq!(
            cursor_icon_for(&d, false, CursorIcon::Default),
            Some(CursorIcon::ColResize)
        );
        assert_eq!(cursor_icon_for(&d, true, CursorIcon::Text), None);
        assert_eq!(
            cursor_icon_for(&Hit::Nothing, false, CursorIcon::ColResize),
            Some(CursorIcon::Default)
        );
        assert_eq!(
            cursor_icon_for(&Hit::Nothing, false, CursorIcon::Pointer),
            None
        );
    }
}
