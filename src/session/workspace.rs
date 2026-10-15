//! Tabs of split panes: which panes exist, how they are arranged, and which one has focus.
//!
//! Window-free: the grid geometry comes in through [`GridGeometry`] and new shells through
//! [`PaneFactory`], so tab and pane lifecycles can be tested with fake panes.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::layout::{dividers, layout, neighbor, Divider, FocusDir, Node, PaneId, Rect, SplitDir};
use crate::pty::Pty;
use crate::session::pane::Pane;

/// A pane is only split when both halves keep at least this many columns / rows.
pub const MIN_SPLIT_COLS: usize = 24;
pub const MIN_SPLIT_ROWS: usize = 10;

/// Where panes go on screen and how many cells fit in a rectangle (the renderer).
pub trait GridGeometry {
    /// Area the panes of the active tab share when there are `tab_count` tabs.
    fn content_rect(&self, tab_count: usize) -> Rect;
    /// Width of the divider between two panes.
    fn gap(&self) -> f32;
    /// Columns and rows that fit in `rect`.
    fn grid_size(&self, rect: Rect) -> (usize, usize);
}

/// Creates the terminal and shell of a new pane.
pub trait PaneFactory {
    /// A pane `id` with a `cols` x `rows` grid, its shell started in `cwd` when given.
    /// `None` when the shell could not be started.
    fn spawn(&mut self, id: PaneId, cols: usize, rows: usize, cwd: Option<PathBuf>)
        -> Option<Pane>;
}

/// A tab: a tree of split panes and the one that has keyboard focus.
#[derive(Clone, Debug)]
pub struct Tab {
    pub root: Node,
    pub focused: PaneId,
}

/// The outcome of closing a pane.
#[derive(Debug, PartialEq, Eq)]
pub struct Closed {
    /// The focused pane before the close, if it still exists.
    pub previous: Option<PaneId>,
    /// The focused pane now; `None` when the last pane was closed.
    pub focused: Option<PaneId>,
}

#[derive(Default)]
pub struct Workspace {
    panes: HashMap<PaneId, Pane>,
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
}

impl Workspace {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- queries ----

    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.get(&id)
    }

    pub fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.panes.get_mut(&id)
    }

    pub fn panes(&self) -> impl Iterator<Item = &Pane> {
        self.panes.values()
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn active(&self) -> usize {
        self.active
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    /// The pane with keyboard focus.
    pub fn focused_id(&self) -> Option<PaneId> {
        self.active_tab().map(|t| t.focused)
    }

    /// The active tab holds a single pane.
    pub fn single_pane(&self) -> bool {
        self.active_tab()
            .is_some_and(|t| matches!(t.root, Node::Leaf(_)))
    }

    /// Pane `id` belongs to the active tab (so it is on screen).
    pub fn is_visible(&self, id: PaneId) -> bool {
        self.active_tab().is_some_and(|t| t.root.contains(id))
    }

    /// The label of tab `index`: its focused pane's title, else "Tab N".
    pub fn tab_title(&self, index: usize) -> String {
        self.tabs
            .get(index)
            .and_then(|t| self.panes.get(&t.focused))
            .and_then(|p| p.title.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| format!("Tab {}", index + 1))
    }

    /// The title of the focused pane, if its program set one.
    pub fn focused_title(&self) -> Option<String> {
        self.focused_id()
            .and_then(|id| self.panes.get(&id))
            .and_then(|p| p.title.clone())
    }

    // ---- geometry ----

    /// Where each pane of the active tab goes.
    pub fn active_layout(&self, geo: &impl GridGeometry) -> Vec<(PaneId, Rect)> {
        match self.active_tab() {
            Some(tab) => layout(&tab.root, geo.content_rect(self.tabs.len()), geo.gap()),
            None => Vec::new(),
        }
    }

    /// The dividers between the panes of the active tab.
    pub fn active_dividers(&self, geo: &impl GridGeometry) -> Vec<Divider> {
        match self.active_tab() {
            Some(tab) => dividers(&tab.root, geo.content_rect(self.tabs.len()), geo.gap()),
            None => Vec::new(),
        }
    }

    pub fn pane_rect(&self, geo: &impl GridGeometry, id: PaneId) -> Option<Rect> {
        self.active_layout(geo)
            .into_iter()
            .find(|(p, _)| *p == id)
            .map(|(_, r)| r)
    }

    pub fn pane_at(&self, geo: &impl GridGeometry, x: f64, y: f64) -> Option<PaneId> {
        self.active_layout(geo)
            .into_iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|(id, _)| id)
    }

    /// Give every pane of every tab the grid its rectangle allows.
    pub fn apply_layout(&mut self, geo: &impl GridGeometry) {
        let content = geo.content_rect(self.tabs.len());
        let gap = geo.gap();
        let mut sizes = Vec::new();
        for tab in &self.tabs {
            for (id, rect) in layout(&tab.root, content, gap) {
                sizes.push((id, geo.grid_size(rect)));
            }
        }
        for (id, (cols, rows)) in sizes {
            if let Some(pane) = self.panes.get_mut(&id) {
                pane.resize(cols, rows);
            }
        }
    }

    /// Set the split ratio of the split at `path` in the active tab. True when it changed.
    pub fn set_ratio(&mut self, path: &[bool], ratio: f32) -> bool {
        match self.tabs.get_mut(self.active) {
            Some(tab) => crate::layout::set_ratio(&mut tab.root, path, ratio),
            None => false,
        }
    }

    // ---- lifecycle ----

    fn alloc_id(&mut self) -> PaneId {
        self.next_id += 1;
        PaneId(self.next_id)
    }

    /// The working directory of the focused pane's shell, where new shells start.
    fn focused_cwd(&self) -> Option<PathBuf> {
        self.focused_id()
            .and_then(|f| self.panes.get(&f))
            .and_then(|p| p.pty.as_ref())
            .and_then(Pty::cwd)
    }

    /// Open a new tab with one pane and make it active. Returns the new pane.
    pub fn new_tab(
        &mut self,
        geo: &impl GridGeometry,
        factory: &mut impl PaneFactory,
    ) -> Option<PaneId> {
        let (cols, rows) = geo.grid_size(geo.content_rect(self.tabs.len() + 1));
        let id = self.alloc_id();
        let pane = factory.spawn(id, cols, rows, self.focused_cwd())?;
        self.panes.insert(id, pane);
        self.tabs.push(Tab {
            root: Node::Leaf(id),
            focused: id,
        });
        self.active = self.tabs.len() - 1;
        Some(id)
    }

    /// Split the focused pane and focus the new one. Returns `(old, new)` focused panes, or
    /// `None` when the pane is too small to split or no shell could be started.
    pub fn split_focused(
        &mut self,
        geo: &impl GridGeometry,
        factory: &mut impl PaneFactory,
        dir: SplitDir,
    ) -> Option<(PaneId, PaneId)> {
        let tab = self.tabs.get(self.active)?;
        let focused = tab.focused;
        let (cols, rows) = self.panes.get(&focused)?.size;
        let fits = match dir {
            SplitDir::Horizontal => cols >= MIN_SPLIT_COLS * 2,
            SplitDir::Vertical => rows >= MIN_SPLIT_ROWS * 2,
        };
        if !fits {
            return None;
        }
        let mut root = tab.root.clone();
        let new_id = self.alloc_id();
        root.split(focused, new_id, dir);
        let content = geo.content_rect(self.tabs.len());
        let rect = layout(&root, content, geo.gap())
            .into_iter()
            .find(|(id, _)| *id == new_id)
            .map(|(_, r)| r)?;
        let (cols, rows) = geo.grid_size(rect);
        let pane = factory.spawn(new_id, cols, rows, self.focused_cwd())?;
        self.panes.insert(new_id, pane);
        self.tabs[self.active].root = root;
        self.tabs[self.active].focused = new_id;
        Some((focused, new_id))
    }

    /// Focus pane `id` of the active tab. Returns the previously focused pane when focus moved.
    pub fn focus_pane(&mut self, id: PaneId) -> Option<PaneId> {
        let tab = self.tabs.get_mut(self.active)?;
        if tab.focused == id || !tab.root.contains(id) {
            return None;
        }
        let old = tab.focused;
        tab.focused = id;
        Some(old)
    }

    /// The pane next to the focused one in direction `dir`.
    pub fn neighbor(&self, geo: &impl GridGeometry, dir: FocusDir) -> Option<PaneId> {
        let from = self.focused_id()?;
        neighbor(&self.active_layout(geo), from, dir)
    }

    /// Make tab `index` active. False when it does not exist or is already active.
    pub fn set_active_tab(&mut self, index: usize) -> bool {
        if index >= self.tabs.len() || index == self.active {
            return false;
        }
        self.active = index;
        true
    }

    /// The tab `step` tabs away from the active one, wrapping around; `None` with one tab.
    pub fn cycled_tab(&self, step: isize) -> Option<usize> {
        let n = self.tabs.len() as isize;
        (n > 1).then(|| (self.active as isize + step).rem_euclid(n) as usize)
    }

    /// Close pane `id`, and its tab when it was the tab's last pane. Dropping the pane ends its
    /// shell. `None` when no tab holds `id`.
    pub fn close_pane(&mut self, id: PaneId) -> Option<Closed> {
        let ti = self.tabs.iter().position(|t| t.root.contains(id))?;
        let previous = self.focused_id();
        let tab = self.tabs.remove(ti);
        let leaves = tab.root.leaves();
        let pos = leaves.iter().position(|l| *l == id).unwrap_or(0);
        match tab.root.remove(id) {
            Some(root) => {
                // Focus the pane that was next to the closed one in reading order.
                let focused = if tab.focused == id {
                    let near = if pos > 0 { pos - 1 } else { pos + 1 };
                    leaves.get(near).copied().unwrap_or(leaves[0])
                } else {
                    tab.focused
                };
                self.tabs.insert(ti, Tab { root, focused });
            }
            None => {
                if ti < self.active {
                    self.active -= 1;
                }
            }
        }
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
        self.panes.remove(&id);
        Some(Closed {
            previous: previous.filter(|p| self.panes.contains_key(p)),
            focused: self.focused_id(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Terminal;

    /// 10x20 px cells, a 20 px tab bar from two tabs on, a 1 px divider.
    struct Grid {
        w: f32,
        h: f32,
    }

    impl GridGeometry for Grid {
        fn content_rect(&self, tab_count: usize) -> Rect {
            let bar = if tab_count > 1 { 20.0 } else { 0.0 };
            Rect::new(0.0, bar, self.w, self.h - bar)
        }
        fn gap(&self) -> f32 {
            1.0
        }
        fn grid_size(&self, rect: Rect) -> (usize, usize) {
            ((rect.w / 10.0) as usize, (rect.h / 20.0) as usize)
        }
    }

    #[derive(Default)]
    struct FakeShells {
        spawned: Vec<(PaneId, usize, usize)>,
        fail: bool,
    }

    impl PaneFactory for FakeShells {
        fn spawn(
            &mut self,
            id: PaneId,
            cols: usize,
            rows: usize,
            _cwd: Option<PathBuf>,
        ) -> Option<Pane> {
            if self.fail {
                return None;
            }
            self.spawned.push((id, cols, rows));
            let terminal = Terminal::new(cols, rows, |_| {});
            Some(Pane::new(id, terminal, None, (cols, rows)))
        }
    }

    const BIG: Grid = Grid {
        w: 1000.0,
        h: 600.0,
    };

    fn ws_with_tab() -> (Workspace, FakeShells) {
        let mut ws = Workspace::new();
        let mut shells = FakeShells::default();
        ws.new_tab(&BIG, &mut shells).unwrap();
        (ws, shells)
    }

    #[test]
    fn new_tabs_become_active_and_get_the_shrunk_grid() {
        let (mut ws, mut shells) = ws_with_tab();
        assert_eq!(ws.focused_id(), Some(PaneId(1)));
        assert_eq!(shells.spawned[0], (PaneId(1), 100, 30));
        let id = ws.new_tab(&BIG, &mut shells).unwrap();
        assert_eq!((id, ws.active(), ws.tabs().len()), (PaneId(2), 1, 2));
        // The second tab already accounts for the tab bar.
        assert_eq!(shells.spawned[1], (PaneId(2), 100, 29));
        assert!(ws.is_visible(PaneId(2)) && !ws.is_visible(PaneId(1)));
    }

    #[test]
    fn failed_spawn_changes_nothing_but_the_id() {
        let mut ws = Workspace::new();
        let mut shells = FakeShells {
            fail: true,
            ..Default::default()
        };
        assert_eq!(ws.new_tab(&BIG, &mut shells), None);
        assert!(ws.is_empty());
    }

    #[test]
    fn split_focuses_the_new_pane_and_refuses_small_panes() {
        let (mut ws, mut shells) = ws_with_tab();
        assert_eq!(
            ws.split_focused(&BIG, &mut shells, SplitDir::Horizontal),
            Some((PaneId(1), PaneId(2)))
        );
        assert_eq!(ws.focused_id(), Some(PaneId(2)));
        assert!(!ws.single_pane());
        ws.apply_layout(&BIG);
        // 49 and 50 columns.
        assert_eq!(
            ws.split_focused(&BIG, &mut shells, SplitDir::Horizontal)
                .map(|s| s.0),
            Some(PaneId(2))
        );
        ws.apply_layout(&BIG);
        // The newest pane is now 24 columns wide: too small to split again.
        assert_eq!(ws.pane(PaneId(3)).unwrap().size.0, 24);
        assert_eq!(
            ws.split_focused(&BIG, &mut shells, SplitDir::Horizontal),
            None
        );
        // 30 rows still split top and bottom.
        assert!(ws
            .split_focused(&BIG, &mut shells, SplitDir::Vertical)
            .is_some());
    }

    #[test]
    fn closing_the_focused_pane_focuses_its_reading_order_neighbour() {
        let (mut ws, mut shells) = ws_with_tab();
        ws.split_focused(&BIG, &mut shells, SplitDir::Horizontal);
        ws.split_focused(&BIG, &mut shells, SplitDir::Vertical);
        // Leaves in reading order: 1, 2, 3; 3 is focused.
        assert_eq!(
            ws.close_pane(PaneId(3)),
            Some(Closed {
                previous: None,
                focused: Some(PaneId(2)),
            })
        );
        ws.focus_pane(PaneId(1));
        assert_eq!(
            ws.close_pane(PaneId(1)),
            Some(Closed {
                previous: None,
                focused: Some(PaneId(2)),
            })
        );
        assert!(ws.single_pane());
        assert_eq!(ws.close_pane(PaneId(9)), None);
    }

    #[test]
    fn closing_an_unfocused_pane_keeps_focus() {
        let (mut ws, mut shells) = ws_with_tab();
        ws.split_focused(&BIG, &mut shells, SplitDir::Horizontal);
        assert_eq!(
            ws.close_pane(PaneId(1)),
            Some(Closed {
                previous: Some(PaneId(2)),
                focused: Some(PaneId(2)),
            })
        );
    }

    #[test]
    fn closing_a_tab_keeps_the_active_one_and_the_last_closes_all() {
        let (mut ws, mut shells) = ws_with_tab();
        ws.new_tab(&BIG, &mut shells);
        ws.new_tab(&BIG, &mut shells);
        assert!(ws.set_active_tab(1));
        // Closing tab 0 (before the active one) keeps tab "2" active.
        ws.close_pane(PaneId(1));
        assert_eq!((ws.active(), ws.focused_id()), (0, Some(PaneId(2))));
        // Closing the active last tab moves to the one before it.
        assert!(ws.set_active_tab(1));
        ws.close_pane(PaneId(3));
        assert_eq!((ws.active(), ws.focused_id()), (0, Some(PaneId(2))));
        assert_eq!(
            ws.close_pane(PaneId(2)),
            Some(Closed {
                previous: None,
                focused: None,
            })
        );
        assert!(ws.is_empty());
    }

    #[test]
    fn tab_switching_and_cycling() {
        let (mut ws, mut shells) = ws_with_tab();
        assert_eq!(ws.cycled_tab(1), None);
        ws.new_tab(&BIG, &mut shells);
        ws.new_tab(&BIG, &mut shells);
        assert!(!ws.set_active_tab(2), "already active");
        assert!(!ws.set_active_tab(3), "no such tab");
        assert_eq!(ws.cycled_tab(1), Some(0));
        assert_eq!(ws.cycled_tab(-1), Some(1));
    }

    #[test]
    fn focus_moves_only_inside_the_active_tab() {
        let (mut ws, mut shells) = ws_with_tab();
        ws.split_focused(&BIG, &mut shells, SplitDir::Horizontal);
        assert_eq!(ws.neighbor(&BIG, FocusDir::Left), Some(PaneId(1)));
        assert_eq!(ws.focus_pane(PaneId(1)), Some(PaneId(2)));
        assert_eq!(ws.focus_pane(PaneId(1)), None, "already focused");
        assert_eq!(ws.focus_pane(PaneId(7)), None, "not in this tab");
    }

    #[test]
    fn tab_titles() {
        let (mut ws, _) = ws_with_tab();
        assert_eq!(ws.tab_title(0), "Tab 1");
        ws.pane_mut(PaneId(1)).unwrap().title = Some(String::new());
        assert_eq!(ws.tab_title(0), "Tab 1");
        ws.pane_mut(PaneId(1)).unwrap().title = Some("vim".into());
        assert_eq!(ws.tab_title(0), "vim");
        assert_eq!(ws.focused_title().as_deref(), Some("vim"));
    }
}
