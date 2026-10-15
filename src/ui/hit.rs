//! What lies under the pointer: a tab, the tab bar, a divider or a pane.

use crate::layout::{dividers, layout, Divider, Node, PaneId, Rect};

#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    Tab(usize),
    TabBar,
    Divider(Divider),
    Pane(PaneId),
    Nothing,
}

/// The window geometry hit testing needs, taken from the renderer.
#[derive(Clone, Debug)]
pub struct HitGeometry {
    pub tab_bar_height: f32,
    /// Rectangles of the tab labels in the tab bar.
    pub tabs: Vec<Rect>,
    /// Area below the tab bar the panes of the active tab share.
    pub content: Rect,
    /// Width of a divider between panes.
    pub gap: f32,
    /// Extra pixels either side of a divider that still grab it.
    pub slop: f32,
}

/// Hit test `(x, y)` against the tab bar and the active tab's split tree `root`.
pub fn hit_test(g: &HitGeometry, root: Option<&Node>, x: f64, y: f64) -> Hit {
    if y < g.tab_bar_height as f64 {
        return match g.tabs.iter().position(|t| t.contains(x, y)) {
            Some(i) => Hit::Tab(i),
            None => Hit::TabBar,
        };
    }
    let Some(root) = root else {
        return Hit::Nothing;
    };
    // The line is thin, so grab it from a few pixels either side.
    let slop = g.slop;
    for d in dividers(root, g.content, g.gap) {
        let grab = Rect::new(
            d.rect.x - slop,
            d.rect.y - slop,
            d.rect.w + slop * 2.0,
            d.rect.h + slop * 2.0,
        );
        if grab.contains(x, y) {
            return Hit::Divider(d);
        }
    }
    match layout(root, g.content, g.gap)
        .into_iter()
        .find(|(_, r)| r.contains(x, y))
    {
        Some((id, _)) => Hit::Pane(id),
        None => Hit::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Rect, SplitDir};

    fn geometry() -> HitGeometry {
        HitGeometry {
            tab_bar_height: 20.0,
            tabs: vec![
                Rect::new(0.0, 0.0, 50.0, 20.0),
                Rect::new(50.0, 0.0, 50.0, 20.0),
            ],
            content: Rect::new(0.0, 20.0, 201.0, 100.0),
            gap: 1.0,
            slop: 2.0,
        }
    }

    fn split() -> Node {
        let mut root = Node::Leaf(PaneId(1));
        root.split(PaneId(1), PaneId(2), SplitDir::Horizontal);
        root
    }

    #[test]
    fn tab_bar() {
        let g = geometry();
        assert_eq!(hit_test(&g, None, 60.0, 5.0), Hit::Tab(1));
        assert_eq!(hit_test(&g, None, 150.0, 5.0), Hit::TabBar);
    }

    #[test]
    fn panes_and_divider_with_slop() {
        let g = geometry();
        let root = split();
        assert_eq!(hit_test(&g, Some(&root), 10.0, 50.0), Hit::Pane(PaneId(1)));
        assert_eq!(hit_test(&g, Some(&root), 190.0, 50.0), Hit::Pane(PaneId(2)));
        let mid = match hit_test(&g, Some(&root), 100.0, 50.0) {
            Hit::Divider(d) => d.rect.x as f64,
            other => panic!("expected the divider, got {other:?}"),
        };
        assert!(matches!(
            hit_test(&g, Some(&root), mid - 1.5, 50.0),
            Hit::Divider(_)
        ));
        assert_eq!(hit_test(&g, None, 10.0, 50.0), Hit::Nothing);
    }
}
