//! Tabs of split panes: which panes exist, how they are arranged, and which one has focus.
//!
//! Window-free: the grid geometry comes in through `GridGeometry` and new shells through
//! `PaneFactory`, so tab and pane lifecycles can be tested with fake panes.

use crate::ui::chrome::t;
use std::collections::HashMap;
use std::path::PathBuf;
use twitty_core::language::fill;

use crate::layout::{dividers, layout, neighbor, Divider, FocusDir, Node, PaneId, Rect, SplitDir};
use crate::pty::Pty;
use crate::remote::Target;
use crate::session::closed::ClosedTab;
use crate::session::pane::Pane;

/// A pane is only split when both halves keep at least this many columns / rows.
pub(crate) const MIN_SPLIT_COLS: usize = 24;
pub(crate) const MIN_SPLIT_ROWS: usize = 10;

/// Why splitting the focused pane failed: a machine-readable code for the control socket and a
/// bilingual (`English / عربي`) message shown both in the UI and over the control socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitFailure {
    pub code: &'static str,
    pub message: String,
}

impl SplitFailure {
    pub fn for_size(size: Option<(usize, usize)>, dir: SplitDir) -> Self {
        let Some((cols, rows)) = size else {
            return Self {
                code: "not_found",
                message: t("No focused pane").into(),
            };
        };
        let min_cols = MIN_SPLIT_COLS * 2;
        let min_rows = MIN_SPLIT_ROWS * 2;
        match dir {
            SplitDir::Horizontal if cols < min_cols => Self {
                code: "pane_too_small",
                message: fill(
                    t("This pane is too narrow to split side by side ({} columns; needs {})"),
                    &[&cols, &min_cols],
                ),
            },
            SplitDir::Vertical if rows < min_rows => Self {
                code: "pane_too_small",
                message: fill(
                    t("This pane is too short to split top and bottom ({} rows; needs {})"),
                    &[&rows, &min_rows],
                ),
            },
            _ => Self {
                code: "internal",
                message: t("Could not split the pane: the shell failed to start").into(),
            },
        }
    }
}

/// Where panes go on screen and how many cells fit in a rectangle (the renderer).
pub(crate) trait GridGeometry {
    /// Area the panes of the active tab share when there are `tab_count` tabs.
    fn content_rect(&self, tab_count: usize) -> Rect;
    /// Width of the divider between two panes.
    fn gap(&self) -> f32;
    /// Columns and rows that fit in `rect`.
    fn grid_size(&self, rect: Rect) -> (usize, usize);
}

/// Creates the terminal and shell of a new pane.
pub(crate) trait PaneFactory {
    /// Inform the factory which stable tab id the next spawned pane belongs to.
    fn set_tab_id(&mut self, _tab_id: u64) {}

    /// A pane `id` with a `cols` x `rows` grid, its shell started in `cwd` when given.
    /// `None` when the shell could not be started.
    fn spawn(&mut self, id: PaneId, cols: usize, rows: usize, cwd: Option<PathBuf>)
        -> Option<Pane>;

    fn spawn_remote(
        &mut self,
        _id: PaneId,
        _cols: usize,
        _rows: usize,
        _target: Target,
    ) -> Option<Pane> {
        None
    }
}

/// A tab: a tree of split panes and the one that has keyboard focus.
#[derive(Clone, Debug)]
pub struct Tab {
    /// Stable monotonic tab id (exposed as `CMUX_WORKSPACE_ID`).
    pub id: u64,
    /// Optional user/control-socket custom title (overrides the focused pane's OSC title).
    pub title: Option<String>,
    pub root: Node,
    pub focused: PaneId,
}

/// The outcome of closing a pane.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Closed {
    /// The focused pane before the close, if it still exists.
    pub previous: Option<PaneId>,
    /// The focused pane now; `None` when the last pane was closed.
    pub focused: Option<PaneId>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CloseOthers {
    keep: PaneId,
    panes: Vec<PaneId>,
    pub confirm: bool,
}

#[derive(Default)]
pub(crate) struct Workspace {
    panes: HashMap<PaneId, Pane>,
    tabs: Vec<Tab>,
    active: usize,
    zoomed: bool,
    next_id: u64,
    next_tab_id: u64,
}

impl Workspace {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- queries ----

    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.get(&id)
    }

    pub(crate) fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
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

    pub(crate) fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    /// The pane with keyboard focus.
    pub(crate) fn focused_id(&self) -> Option<PaneId> {
        self.active_tab().map(|t| t.focused)
    }

    /// The active tab holds a single pane.
    pub(crate) fn single_pane(&self) -> bool {
        self.active_tab()
            .is_some_and(|t| matches!(t.root, Node::Leaf(_)))
    }

    /// Pane `id` is on screen in the active tab.
    pub fn is_visible(&self, id: PaneId) -> bool {
        if let Some(zoomed) = self.zoomed_pane() {
            return id == zoomed;
        }
        self.active_tab().is_some_and(|t| t.root.contains(id))
    }

    pub(crate) fn zoomed_pane(&self) -> Option<PaneId> {
        self.focused_id().filter(|_| self.zoomed)
    }

    pub(crate) fn toggle_pane_zoom(&mut self) -> bool {
        if self.active_tab().is_none() || self.single_pane() {
            return false;
        }
        self.zoomed = !self.zoomed;
        true
    }

    /// The label of tab `index`: its custom title, else its focused pane's title, else "Tab N".
    pub(crate) fn tab_title(&self, index: usize) -> String {
        self.tab_title_for_ui(index, crate::ui::chrome::UiLanguage::English)
    }

    pub(crate) fn tab_title_for_ui(
        &self,
        index: usize,
        language: crate::ui::chrome::UiLanguage,
    ) -> String {
        if let Some(custom) = self
            .tabs
            .get(index)
            .and_then(|t| t.title.as_deref())
            .filter(|t| !t.is_empty())
        {
            return custom.to_owned();
        }
        self.tabs
            .get(index)
            .and_then(|t| self.panes.get(&t.focused))
            .and_then(|p| {
                p.title.clone().or_else(|| {
                    p.remote.as_ref().map(|ssh| {
                        crate::shell_integration::clean_text(ssh.target.destination(), 256)
                    })
                })
            })
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| crate::sidebar::localized_entry_title(None, None, index, language))
    }

    /// Find the current 0-based index of the tab with stable `id`.
    pub(crate) fn tab_index_by_id(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// Stable tab id of the tab containing `pane`.
    pub(crate) fn tab_id_of_pane(&self, pane: PaneId) -> Option<u64> {
        self.tabs
            .iter()
            .find(|t| t.root.contains(pane))
            .map(|t| t.id)
    }

    /// Set or clear the custom title of tab `index`. Returns false when out of range.
    pub(crate) fn rename_tab(&mut self, index: usize, title: Option<String>) -> bool {
        let Some(tab) = self.tabs.get_mut(index) else {
            return false;
        };
        tab.title = title.filter(|t| !t.is_empty());
        true
    }

    /// The title of the focused pane, if its program set one.
    pub(crate) fn focused_title(&self) -> Option<String> {
        self.focused_id()
            .and_then(|id| self.panes.get(&id))
            .and_then(|p| p.title.clone())
    }

    // ---- geometry ----

    /// Where each pane of the active tab goes.
    pub(crate) fn active_layout(&self, geo: &impl GridGeometry) -> Vec<(PaneId, Rect)> {
        let content = geo.content_rect(self.tabs.len());
        if let Some(id) = self.zoomed_pane() {
            return vec![(id, content)];
        }
        match self.active_tab() {
            Some(tab) => layout(&tab.root, content, geo.gap()),
            None => Vec::new(),
        }
    }

    /// The dividers between the panes of the active tab.
    pub(crate) fn active_dividers(&self, geo: &impl GridGeometry) -> Vec<Divider> {
        if self.zoomed {
            return Vec::new();
        }
        match self.active_tab() {
            Some(tab) => dividers(&tab.root, geo.content_rect(self.tabs.len()), geo.gap()),
            None => Vec::new(),
        }
    }

    pub(crate) fn pane_rect(&self, geo: &impl GridGeometry, id: PaneId) -> Option<Rect> {
        self.active_layout(geo)
            .into_iter()
            .find(|(p, _)| *p == id)
            .map(|(_, r)| r)
    }

    pub(crate) fn pane_at(&self, geo: &impl GridGeometry, x: f64, y: f64) -> Option<PaneId> {
        self.active_layout(geo)
            .into_iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|(id, _)| id)
    }

    /// Give every pane of every tab the grid its rectangle allows.
    pub(crate) fn apply_layout(&mut self, geo: &impl GridGeometry) {
        let content = geo.content_rect(self.tabs.len());
        let gap = geo.gap();
        let mut sizes = Vec::new();
        let zoomed = self.zoomed_pane();
        for tab in &self.tabs {
            for (id, rect) in layout(&tab.root, content, gap) {
                let rect = if zoomed == Some(id) { content } else { rect };
                sizes.push((id, geo.grid_size(rect)));
            }
        }
        for (id, (cols, rows)) in sizes {
            if let Some(pane) = self.panes.get_mut(&id) {
                pane.resize(cols, rows);
            }
        }
    }

    /// Resize only the active tab's panes (for active-tab-only geometry changes such as divider
    /// drags, splits, or pane zoom toggles, avoiding scrollback reflow and `TIOCSWINSZ` on
    /// unaffected background tabs).
    pub(crate) fn apply_active_tab_layout(&mut self, geo: &impl GridGeometry) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let content = geo.content_rect(self.tabs.len());
        let gap = geo.gap();
        let zoomed = self.zoomed_pane();
        let sizes: Vec<(PaneId, (usize, usize))> = layout(&tab.root, content, gap)
            .into_iter()
            .map(|(id, rect)| {
                let rect = if zoomed == Some(id) { content } else { rect };
                (id, geo.grid_size(rect))
            })
            .collect();
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

    fn alloc_tab_id(&mut self) -> u64 {
        self.next_tab_id += 1;
        self.next_tab_id
    }

    /// Install one startup tab, with fresh IDs already assigned across the restored tabs.
    /// If a shell cannot start, its sibling takes its space. Reserve failed IDs as their
    /// terminal events may already have been queued.
    pub(super) fn restore_tab(
        &mut self,
        mut tab: Tab,
        tab_count: usize,
        geo: &impl GridGeometry,
        mut spawn: impl FnMut(PaneId, usize, usize) -> Option<Pane>,
    ) -> bool {
        if tab.id == 0 {
            tab.id = self.alloc_tab_id();
        } else {
            self.next_tab_id = self.next_tab_id.max(tab.id);
        }
        let panes = layout(&tab.root, geo.content_rect(tab_count), geo.gap());
        let mut root = Some(tab.root);
        for (id, rect) in panes {
            self.next_id = self.next_id.max(id.0);
            let (cols, rows) = geo.grid_size(rect);
            if let Some(pane) = spawn(id, cols, rows) {
                self.panes.insert(id, pane);
            } else {
                root = root.and_then(|node| node.remove(id));
            }
        }
        let Some(root) = root else {
            return false;
        };
        if !root.contains(tab.focused) {
            tab.focused = root.leaves()[0];
        }
        tab.root = root;
        self.tabs.push(tab);
        true
    }

    /// The working directory of the focused pane's shell, where new shells start.
    fn focused_cwd(&self) -> Option<PathBuf> {
        self.focused_id()
            .and_then(|f| self.panes.get(&f))
            .filter(|pane| pane.remote.is_none())
            .and_then(|p| p.pty.as_ref())
            .and_then(Pty::cwd)
    }

    /// Open a new tab with one pane and make it active. Returns the new pane.
    pub(crate) fn new_tab(
        &mut self,
        geo: &impl GridGeometry,
        factory: &mut impl PaneFactory,
    ) -> Option<PaneId> {
        let (cols, rows) = geo.grid_size(geo.content_rect(self.tabs.len() + 1));
        let id = self.alloc_id();
        let tab_id = self.alloc_tab_id();
        factory.set_tab_id(tab_id);
        let pane = factory.spawn(id, cols, rows, self.focused_cwd())?;
        self.panes.insert(id, pane);
        self.tabs.push(Tab {
            id: tab_id,
            title: None,
            root: Node::Leaf(id),
            focused: id,
        });
        self.active = self.tabs.len() - 1;
        self.zoomed = false;
        Some(id)
    }

    /// Grid size `(cols, rows)` of the focused pane under `geo`'s unzoomed tab layout.
    pub(crate) fn focused_layout_size(&self, geo: &impl GridGeometry) -> Option<(usize, usize)> {
        self.active_tab().and_then(|tab| {
            layout(&tab.root, geo.content_rect(self.tabs.len()), geo.gap())
                .into_iter()
                .find(|(id, _)| *id == tab.focused)
                .map(|(_, rect)| geo.grid_size(rect))
        })
    }

    /// Whether the focused pane is large enough to split in `dir`.
    pub(crate) fn can_split_focused(&self, geo: &impl GridGeometry, dir: SplitDir) -> bool {
        let Some((cols, rows)) = self.focused_layout_size(geo) else {
            return false;
        };
        match dir {
            SplitDir::Horizontal => cols >= MIN_SPLIT_COLS * 2,
            SplitDir::Vertical => rows >= MIN_SPLIT_ROWS * 2,
        }
    }

    /// Explain why splitting the focused pane in `dir` failed under `geo`.
    pub(crate) fn split_failure_reason(
        &self,
        geo: &impl GridGeometry,
        dir: SplitDir,
    ) -> SplitFailure {
        SplitFailure::for_size(self.focused_layout_size(geo), dir)
    }

    /// Split the focused pane and focus the new one. Returns `(old, new)` focused panes, or
    /// `None` when the pane is too small to split or no shell could be started.
    pub(crate) fn split_focused(
        &mut self,
        geo: &impl GridGeometry,
        factory: &mut impl PaneFactory,
        dir: SplitDir,
    ) -> Option<(PaneId, PaneId)> {
        if !self.can_split_focused(geo, dir) {
            return None;
        }
        let tab = self.tabs.get(self.active)?;
        let tab_id = tab.id;
        let focused = tab.focused;
        let mut root = tab.root.clone();
        let new_id = self.alloc_id();
        root.split(focused, new_id, dir);
        let content = geo.content_rect(self.tabs.len());
        let rect = layout(&root, content, geo.gap())
            .into_iter()
            .find(|(id, _)| *id == new_id)
            .map(|(_, r)| r)?;
        let (cols, rows) = geo.grid_size(rect);
        factory.set_tab_id(tab_id);
        let pane = factory.spawn(new_id, cols, rows, self.focused_cwd())?;
        self.panes.insert(new_id, pane);
        self.tabs[self.active].root = root;
        self.tabs[self.active].focused = new_id;
        self.zoomed = false;
        Some((focused, new_id))
    }

    /// Focus pane `id` in whichever tab holds it, switching tabs in one step without firing
    /// intermediate focus events on that tab's previously focused pane.
    /// Returns `(tab_index, previous_focused, tab_changed)`.
    pub(crate) fn focus_any_pane(&mut self, id: PaneId) -> Option<(usize, Option<PaneId>, bool)> {
        let tab_index = self.tabs.iter().position(|t| t.root.contains(id))?;
        let previous = self.focused_id();
        let tab_changed = tab_index != self.active;
        if previous != Some(id) {
            self.zoomed = false;
        }
        self.active = tab_index;
        self.tabs[tab_index].focused = id;
        Some((tab_index, previous, tab_changed))
    }

    /// Focus pane `id` of the active tab. Returns the previously focused pane when focus moved.
    pub(crate) fn focus_pane(&mut self, id: PaneId) -> Option<PaneId> {
        let tab = self.tabs.get_mut(self.active)?;
        if tab.focused == id || !tab.root.contains(id) {
            return None;
        }
        let old = tab.focused;
        tab.focused = id;
        self.zoomed = false;
        Some(old)
    }

    /// Tab index and id of the pane whose unseen notification is the oldest.
    pub(crate) fn oldest_attention(&self) -> Option<(usize, PaneId)> {
        self.attention_targets()
            .min_by_key(|&(at, ..)| at)
            .map(|(_, index, id)| (index, id))
    }

    pub(crate) fn newest_attention(&self) -> Option<PaneId> {
        self.attention_targets()
            .max_by_key(|&(at, ..)| at)
            .map(|(_, _, id)| id)
    }

    fn attention_targets(
        &self,
    ) -> impl Iterator<Item = (std::time::SystemTime, usize, PaneId)> + '_ {
        self.tabs.iter().enumerate().flat_map(move |(index, tab)| {
            tab.root.leaves().into_iter().filter_map(move |id| {
                let at = self.pane(id)?.meta.attention.as_ref()?.at;
                Some((at, index, id))
            })
        })
    }

    /// The pane next to the focused one in direction `dir`.
    pub fn neighbor(&self, geo: &impl GridGeometry, dir: FocusDir) -> Option<PaneId> {
        let tab = self.active_tab()?;
        let panes = layout(&tab.root, geo.content_rect(self.tabs.len()), geo.gap());
        neighbor(&panes, tab.focused, dir)
    }

    /// Make tab `index` active. False when it does not exist or is already active.
    pub(crate) fn set_active_tab(&mut self, index: usize) -> bool {
        if index >= self.tabs.len() || index == self.active {
            return false;
        }
        self.active = index;
        self.zoomed = false;
        true
    }

    /// Move the active tab to another slot, preserving its identity and focused pane.
    pub(crate) fn move_active_tab(&mut self, to: usize) -> bool {
        if to >= self.tabs.len() || to == self.active {
            return false;
        }
        let tab = self.tabs.remove(self.active);
        self.tabs.insert(to, tab);
        self.active = to;
        true
    }

    /// Move the active tab left (`delta < 0`) or right (`delta > 0`), clamping at the ends.
    pub(crate) fn move_active_tab_by(&mut self, delta: isize) -> bool {
        let len = self.tabs.len();
        if len < 2 || delta == 0 {
            return false;
        }
        let target = (self.active as isize + delta).clamp(0, (len - 1) as isize) as usize;
        self.move_active_tab(target)
    }

    /// Snapshot a tab's cwd and titles before closing it so it can be reopened later.
    pub(crate) fn snapshot_closed_tab(&self, index: usize) -> Option<ClosedTab> {
        let tab = self.tabs.get(index)?;
        let pane = self.panes.get(&tab.focused);
        let cwd = pane.and_then(|p| {
            if p.remote.is_some() {
                None
            } else {
                p.pty
                    .as_ref()
                    .and_then(Pty::cwd)
                    .or_else(|| p.meta.cwd.clone())
            }
        });
        let pane_title = pane.and_then(|p| p.title.clone());
        Some(ClosedTab::new(cwd, tab.title.clone(), pane_title))
    }

    /// If `id` is the sole pane of its tab, return `(tab_id, ClosedTab)` before closing it.
    pub(crate) fn sole_pane_closed_tab(&self, id: PaneId) -> Option<(u64, ClosedTab)> {
        let index = self
            .tabs
            .iter()
            .position(|t| matches!(t.root, Node::Leaf(leaf) if leaf == id))?;
        let tab_id = self.tabs[index].id;
        Some((tab_id, self.snapshot_closed_tab(index)?))
    }

    /// Reopen a previously closed tab, restoring its cwd (if it still exists) and titles.
    pub(crate) fn reopen_closed_tab(
        &mut self,
        closed: &ClosedTab,
        geo: &impl GridGeometry,
        factory: &mut impl PaneFactory,
    ) -> Option<PaneId> {
        let (cols, rows) = geo.grid_size(geo.content_rect(self.tabs.len() + 1));
        let id = self.alloc_id();
        let tab_id = self.alloc_tab_id();
        factory.set_tab_id(tab_id);
        let cwd = closed.existing_cwd();
        let mut pane = factory.spawn(id, cols, rows, cwd.clone()).or_else(|| {
            cwd.as_ref()?;
            factory.spawn(id, cols, rows, None)
        })?;
        pane.title.clone_from(&closed.pane_title);
        self.panes.insert(id, pane);
        self.tabs.push(Tab {
            id: tab_id,
            title: closed.tab_title.clone(),
            root: Node::Leaf(id),
            focused: id,
        });
        self.active = self.tabs.len() - 1;
        self.zoomed = false;
        Some(id)
    }

    /// Open a new tab from a prepared split tree whose leaf IDs are local slots, allocating fresh
    /// pane IDs and a fresh tab ID. Returns the focused pane ID when at least one pane started.
    pub(crate) fn open_layout_tab<F: PaneFactory>(
        &mut self,
        tab_title: Option<String>,
        slot_root: Node,
        focused_slot: PaneId,
        mut spawn_slot: impl FnMut(PaneId, PaneId, usize, usize, &mut F) -> Option<Pane>,
        geo: &impl GridGeometry,
        factory: &mut F,
    ) -> Option<PaneId> {
        let tab_id = self.alloc_tab_id();
        factory.set_tab_id(tab_id);
        let mut mapping: Vec<(PaneId, PaneId)> = Vec::new();
        let mapped_root = map_node_ids(&slot_root, &mut |slot| {
            let fresh = self.alloc_id();
            mapping.push((slot, fresh));
            fresh
        });
        let focused = mapping
            .iter()
            .find(|(slot, _)| *slot == focused_slot)
            .map(|(_, fresh)| *fresh)
            .unwrap_or_else(|| mapped_root.leaves()[0]);

        let tab_count = self.tabs.len() + 1;
        let panes = layout(&mapped_root, geo.content_rect(tab_count), geo.gap());
        let mut root = Some(mapped_root);
        for (fresh_id, rect) in panes {
            let slot = mapping
                .iter()
                .find(|(_, fresh)| *fresh == fresh_id)
                .map(|(slot, _)| *slot)
                .expect("every mapped leaf has a slot");
            let (cols, rows) = geo.grid_size(rect);
            if let Some(pane) = spawn_slot(slot, fresh_id, cols, rows, factory) {
                self.panes.insert(fresh_id, pane);
            } else {
                root = root.and_then(|node| node.remove(fresh_id));
            }
        }
        let root = root?;
        let focused = if root.contains(focused) {
            focused
        } else {
            root.leaves()[0]
        };
        self.tabs.push(Tab {
            id: tab_id,
            title: tab_title,
            root,
            focused,
        });
        self.active = self.tabs.len() - 1;
        self.zoomed = false;
        Some(focused)
    }

    /// The tab `step` tabs away from the active one, wrapping around; `None` with one tab.
    pub(crate) fn cycled_tab(&self, step: isize) -> Option<usize> {
        let n = self.tabs.len() as isize;
        (n > 1).then(|| (self.active as isize + step).rem_euclid(n) as usize)
    }

    /// Close pane `id`, and its tab when it was the tab's last pane. Dropping the pane ends its
    /// shell. `None` when no tab holds `id`.
    pub(crate) fn close_pane(&mut self, id: PaneId) -> Option<Closed> {
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
                self.tabs.insert(
                    ti,
                    Tab {
                        id: tab.id,
                        title: tab.title,
                        root,
                        focused,
                    },
                );
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
        self.clear_invalid_zoom(previous);
        Some(Closed {
            previous: previous.filter(|p| self.panes.contains_key(p)),
            focused: self.focused_id(),
        })
    }

    pub(crate) fn other_panes(&self) -> Option<CloseOthers> {
        let tab = self.active_tab()?;
        let panes: Vec<_> = tab
            .root
            .leaves()
            .into_iter()
            .filter(|id| *id != tab.focused)
            .collect();
        if panes.is_empty() {
            return None;
        }
        let confirm = panes
            .iter()
            .any(|id| self.panes.get(id).is_some_and(Pane::has_running_program));
        Some(CloseOthers {
            keep: tab.focused,
            panes,
            confirm,
        })
    }

    /// Close the panes shown in the confirmation, if the focused pane and its siblings still match.
    pub(crate) fn close_others(
        &mut self,
        plan: CloseOthers,
        confirmed: bool,
    ) -> Option<Vec<PaneId>> {
        let current = self.other_panes()?;
        if current.keep != plan.keep
            || current.panes != plan.panes
            || (current.confirm && !confirmed)
        {
            return None;
        }
        for id in &current.panes {
            self.panes.remove(id);
        }
        self.tabs[self.active].root = Node::Leaf(current.keep);
        self.zoomed = false;
        Some(current.panes)
    }

    fn clear_invalid_zoom(&mut self, previous: Option<PaneId>) {
        if previous != self.focused_id() || self.single_pane() {
            self.zoomed = false;
        }
    }

    /// Close every pane in tab `index`. Returns the removed pane ids and the resulting focus
    /// change, or `None` when `index` is out of range.
    pub(crate) fn close_tab(&mut self, index: usize) -> Option<(Vec<PaneId>, Closed)> {
        if index >= self.tabs.len() {
            return None;
        }
        let previous = self.focused_id();
        let tab = self.tabs.remove(index);
        let removed = tab.root.leaves();
        for id in &removed {
            self.panes.remove(id);
        }
        if index < self.active {
            self.active -= 1;
        }
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
        self.clear_invalid_zoom(previous);
        Some((
            removed,
            Closed {
                previous: previous.filter(|p| self.panes.contains_key(p)),
                focused: self.focused_id(),
            },
        ))
    }
}

fn map_node_ids(node: &Node, map: &mut impl FnMut(PaneId) -> PaneId) -> Node {
    match node {
        Node::Leaf(id) => Node::Leaf(map(*id)),
        Node::Split { dir, ratio, a, b } => Node::Split {
            dir: *dir,
            ratio: *ratio,
            a: Box::new(map_node_ids(a, map)),
            b: Box::new(map_node_ids(b, map)),
        },
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

    #[test]
    fn can_split_focused_tracks_narrow_sidebar_area_for_agent_fallback() {
        // Window narrowed by a sidebar: 40 cols x 28 rows (< MIN_SPLIT_COLS*2, >= MIN_SPLIT_ROWS*2).
        let narrow = Grid { w: 400.0, h: 560.0 };
        let mut ws = Workspace::new();
        let mut shells = FakeShells::default();
        ws.new_tab(&narrow, &mut shells).unwrap();
        assert!(!ws.can_split_focused(&narrow, SplitDir::Horizontal));
        assert!(ws.can_split_focused(&narrow, SplitDir::Vertical));
        assert!(ws
            .split_focused(&narrow, &mut shells, SplitDir::Vertical)
            .is_some());
        ws.apply_layout(&narrow);
        // After the vertical split each half is 13-14 rows: too small in both directions.
        assert!(!ws.can_split_focused(&narrow, SplitDir::Horizontal));
        assert!(!ws.can_split_focused(&narrow, SplitDir::Vertical));
    }

    #[test]
    fn focus_any_pane_switches_tab_and_pane_in_one_step_for_hidden_attention() {
        use crate::session::pane::Notification;
        use std::time::{Duration, SystemTime};

        let (mut ws, mut shells) = ws_with_tab();
        // Tab 0: Pane 1. Tab 1: split Pane 2 (focused) + Pane 3 (unfocused).
        ws.new_tab(&BIG, &mut shells).unwrap();
        ws.split_focused(&BIG, &mut shells, SplitDir::Horizontal)
            .unwrap();
        ws.focus_pane(PaneId(2));
        // Switch back to Tab 0 so Tab 1 is hidden.
        assert!(ws.set_active_tab(0));
        assert_eq!(ws.focused_id(), Some(PaneId(1)));

        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        ws.pane_mut(PaneId(3)).unwrap().meta.attention = Some(Notification {
            title: None,
            body: "done".into(),
            at: t0,
        });
        assert_eq!(ws.oldest_attention(), Some((1, PaneId(3))));
        // Jump straight from Pane 1 (Tab 0) to Pane 3 (Tab 1) in one step.
        assert_eq!(
            ws.focus_any_pane(PaneId(3)),
            Some((1, Some(PaneId(1)), true))
        );
        assert_eq!(ws.active(), 1);
        assert_eq!(ws.focused_id(), Some(PaneId(3)));
    }

    #[test]
    fn pane_zoom_maximizes_and_restores_on_toggle_or_focus_move() {
        let g = Grid { w: 800.0, h: 400.0 };
        let mut ws = Workspace::new();
        let mut shells = FakeShells::default();
        let a = ws.new_tab(&g, &mut shells).unwrap();
        assert!(!ws.toggle_pane_zoom(), "single pane cannot zoom");
        assert_eq!(ws.zoomed_pane(), None);

        let (_, b) = ws
            .split_focused(&g, &mut shells, SplitDir::Horizontal)
            .unwrap();
        ws.apply_layout(&g);
        assert_eq!(ws.active_layout(&g).len(), 2);
        let b_split_size = ws.pane(b).unwrap().size;

        assert!(ws.toggle_pane_zoom());
        assert_eq!(ws.zoomed_pane(), Some(b));
        assert!(ws.is_visible(b));
        assert!(!ws.is_visible(a));
        assert!(ws.active_dividers(&g).is_empty());
        assert_eq!(ws.active_layout(&g), vec![(b, g.content_rect(1))]);
        ws.apply_layout(&g);
        assert!(ws.pane(b).unwrap().size.0 > b_split_size.0);

        // Toggle restores split layout and pane size.
        assert!(ws.toggle_pane_zoom());
        assert_eq!(ws.zoomed_pane(), None);
        assert!(ws.is_visible(a));
        assert_eq!(ws.active_layout(&g).len(), 2);
        assert_eq!(ws.active_dividers(&g).len(), 1);
        ws.apply_layout(&g);
        assert_eq!(ws.pane(b).unwrap().size, b_split_size);

        // Moving focus while zoomed restores unzoomed layout.
        assert!(ws.toggle_pane_zoom());
        assert_eq!(ws.neighbor(&g, FocusDir::Left), Some(a));
        assert_eq!(ws.focus_pane(a), Some(b));
        assert_eq!(ws.zoomed_pane(), None);
        assert_eq!(ws.active_layout(&g).len(), 2);

        // Opening or switching tabs while zoomed also restores.
        assert!(ws.toggle_pane_zoom());
        let c = ws.new_tab(&g, &mut shells).unwrap();
        assert_eq!(ws.zoomed_pane(), None);
        assert_eq!(ws.focused_id(), Some(c));
    }

    #[test]
    fn jump_to_newest_unread_across_tabs() {
        use crate::session::pane::Notification;
        use std::time::{Duration, SystemTime};

        let g = Grid { w: 800.0, h: 400.0 };
        let mut ws = Workspace::new();
        let mut shells = FakeShells::default();
        let p1 = ws.new_tab(&g, &mut shells).unwrap();
        let (_, p2) = ws
            .split_focused(&g, &mut shells, SplitDir::Horizontal)
            .unwrap();
        let p3 = ws.new_tab(&g, &mut shells).unwrap();
        let p4 = ws.new_tab(&g, &mut shells).unwrap();

        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        ws.pane_mut(p1).unwrap().meta.attention = Some(Notification {
            title: None,
            body: "oldest".into(),
            at: base + Duration::from_secs(10),
        });
        ws.pane_mut(p3).unwrap().meta.attention = Some(Notification {
            title: Some("Agent".into()),
            body: "newest".into(),
            at: base + Duration::from_secs(30),
        });
        ws.pane_mut(p2).unwrap().meta.attention = Some(Notification {
            title: None,
            body: "middle".into(),
            at: base + Duration::from_secs(20),
        });

        assert_eq!(ws.active(), 2);
        assert_eq!(ws.focused_id(), Some(p4));
        assert_eq!(ws.oldest_attention(), Some((0, p1)));
        assert_eq!(ws.newest_attention(), Some(p3));

        let (tab_idx, prev, changed) = ws.focus_any_pane(p3).unwrap();
        assert_eq!((tab_idx, prev, changed), (1, Some(p4), true));
        ws.pane_mut(p3).unwrap().meta.attention = None;
        assert_eq!(ws.newest_attention(), Some(p2));

        let (tab_idx, _, changed) = ws.focus_any_pane(p2).unwrap();
        assert_eq!((tab_idx, changed), (0, true));
        ws.pane_mut(p2).unwrap().meta.attention = None;
        assert_eq!(ws.newest_attention(), Some(p1));
    }

    #[test]
    fn close_others_confirmation_rule() {
        let g = Grid { w: 800.0, h: 400.0 };
        let mut ws = Workspace::new();
        let mut shells = FakeShells::default();
        let a = ws.new_tab(&g, &mut shells).unwrap();
        assert_eq!(ws.other_panes(), None, "single pane has no others");

        let (_, b) = ws
            .split_focused(&g, &mut shells, SplitDir::Horizontal)
            .unwrap();
        let (_, c) = ws
            .split_focused(&g, &mut shells, SplitDir::Vertical)
            .unwrap();
        assert_eq!(ws.focused_id(), Some(c));

        // Idle siblings require no confirmation.
        let plan = ws.other_panes().unwrap();
        assert!(!plan.confirm);
        assert_eq!(plan.panes, vec![a, b]);

        // Running command in another pane requires confirmation.
        ws.pane_mut(a).unwrap().meta.running = true;
        let plan = ws.other_panes().unwrap();
        assert!(plan.confirm);
        assert_eq!(
            ws.close_others(
                CloseOthers {
                    keep: plan.keep,
                    panes: plan.panes.clone(),
                    confirm: plan.confirm,
                },
                false
            ),
            None,
            "unconfirmed close refused while another pane runs a program"
        );
        let removed = ws.close_others(plan, true).unwrap();
        assert_eq!(removed, vec![a, b]);
        assert!(ws.single_pane());
        assert_eq!(ws.focused_id(), Some(c));
        assert!(ws.pane(a).is_none() && ws.pane(b).is_none() && ws.pane(c).is_some());

        // Alternate screen program (e.g. vim) in another pane also requires confirmation.
        let (_, d) = ws
            .split_focused(&g, &mut shells, SplitDir::Horizontal)
            .unwrap();
        ws.pane_mut(c)
            .unwrap()
            .terminal
            .process_bytes(b"\x1b[?1049h");
        let plan = ws.other_panes().unwrap();
        assert!(plan.confirm);
        assert_eq!(ws.focused_id(), Some(d));
        let removed = ws.close_others(plan, true).unwrap();
        assert_eq!(removed, vec![c]);
        assert!(ws.single_pane());
    }

    #[test]
    fn move_active_tab_left_right_and_reopen_closed_tab() {
        use crate::session::closed::ClosedTabs;

        #[derive(Default)]
        struct CwdShells {
            cwds: Vec<(PaneId, Option<PathBuf>)>,
        }
        impl PaneFactory for CwdShells {
            fn spawn(
                &mut self,
                id: PaneId,
                cols: usize,
                rows: usize,
                cwd: Option<PathBuf>,
            ) -> Option<Pane> {
                self.cwds.push((id, cwd));
                Some(Pane::new(
                    id,
                    Terminal::new(cols, rows, |_| {}),
                    None,
                    (cols, rows),
                ))
            }
        }

        let tmp = std::env::temp_dir().canonicalize().unwrap();
        let mut ws = Workspace::new();
        let mut shells = CwdShells::default();
        let p1 = ws.new_tab(&BIG, &mut shells).unwrap();
        ws.rename_tab(0, Some("First".into()));
        let p2 = ws.new_tab(&BIG, &mut shells).unwrap();
        ws.rename_tab(1, Some("Second · الثاني".into()));
        ws.pane_mut(p2).unwrap().meta.cwd = Some(tmp.clone());
        ws.pane_mut(p2).unwrap().title = Some("shell-2".into());
        let p3 = ws.new_tab(&BIG, &mut shells).unwrap();
        ws.rename_tab(2, Some("Third".into()));
        let _ = (p1, p3);

        // Active is tab 2 ("Third"). Moving right at the right edge is a no-op.
        assert!(!ws.move_active_tab_by(1));
        assert!(ws.move_active_tab_by(-1));
        assert_eq!(ws.active(), 1);
        assert_eq!(ws.tabs()[1].title.as_deref(), Some("Third"));
        assert!(ws.move_active_tab_by(-1));
        assert_eq!(ws.active(), 0);
        assert_eq!(ws.tabs()[0].title.as_deref(), Some("Third"));
        // Moving left at the left edge is a no-op.
        assert!(!ws.move_active_tab_by(-1));
        assert!(ws.move_active_tab_by(1));
        assert_eq!(ws.active(), 1);
        assert_eq!(ws.tabs()[1].title.as_deref(), Some("Third"));

        // Close tab 2 ("Second · الثاني") via sole-pane close and reopen it.
        let mut closed_tabs = ClosedTabs::new();
        let (_, snap) = ws.sole_pane_closed_tab(p2).unwrap();
        ws.close_pane(p2).unwrap();
        closed_tabs.push(snap);
        assert_eq!(ws.tabs().len(), 2);

        let reopened_snap = closed_tabs.pop().unwrap();
        let reopened_id = ws
            .reopen_closed_tab(&reopened_snap, &BIG, &mut shells)
            .unwrap();
        assert_eq!(ws.tabs().len(), 3);
        assert_eq!(ws.active(), 2);
        assert_eq!(ws.tabs()[2].title.as_deref(), Some("Second · الثاني"));
        assert_eq!(
            ws.pane(reopened_id).unwrap().title.as_deref(),
            Some("shell-2")
        );
        assert_eq!(shells.cwds.last().unwrap().1.as_ref(), Some(&tmp));
    }
}
