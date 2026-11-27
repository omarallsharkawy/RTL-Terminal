//! Named tab layouts: save the active tab's split tree, pane cwds and titles under a name,
//! recreate it in a new tab, and delete saved layouts.
//!
//! Stored in the state directory next to `session.json` (`layouts.json`), versioned, bounded,
//! written atomically with `0600` permissions. Window-free; `app::layouts` hooks it into the UI.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::warn;
use crate::layout::{Node, PaneId, SplitDir, MIN_RATIO};
use crate::projects::mru::{atomic_write_state_file, open_state_file};
use crate::pty::Pty;
use crate::render::scene::palette::PaletteItem;
use crate::session::workspace::{GridGeometry, PaneFactory, Workspace};
use crate::shell_integration::clean_text;
use crate::ui::palette::Searchable;

/// Current on-disk schema version for `layouts.json`.
pub const LAYOUTS_VERSION: u32 = 1;

/// Maximum number of saved named layouts.
pub const MAX_LAYOUTS: usize = 64;

/// Maximum panes restored from a single named layout.
pub const MAX_LAYOUT_PANES: usize = 32;

/// Maximum split nesting depth in a single named layout.
pub const MAX_SPLIT_DEPTH: usize = 16;

/// Maximum characters in a layout name.
pub const MAX_LAYOUT_NAME_CHARS: usize = 64;

/// Maximum characters in a saved tab or pane title.
pub const MAX_TITLE_CHARS: usize = 256;

/// Maximum `layouts.json` size read from or written to disk (2 MiB).
pub const MAX_LAYOUTS_FILE_BYTES: usize = 2 * 1024 * 1024;

pub fn save_layout_placeholder() -> &'static str {
    crate::ui::chrome::t("Name this layout…")
}
pub fn open_layout_placeholder() -> &'static str {
    crate::ui::chrome::t("Open layout…")
}
pub fn delete_layout_placeholder() -> &'static str {
    crate::ui::chrome::t("Delete layout…")
}

/// Clean and validate a layout name (strips control and bidi-override characters, trims, bounds).
pub fn clean_layout_name(raw: &str) -> Option<String> {
    let cleaned = clean_text(raw, MAX_LAYOUT_NAME_CHARS);
    (!cleaned.is_empty()).then_some(cleaned)
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutSplitDir {
    Horizontal,
    Vertical,
}

impl From<SplitDir> for LayoutSplitDir {
    fn from(dir: SplitDir) -> Self {
        match dir {
            SplitDir::Horizontal => Self::Horizontal,
            SplitDir::Vertical => Self::Vertical,
        }
    }
}

impl From<LayoutSplitDir> for SplitDir {
    fn from(dir: LayoutSplitDir) -> Self {
        match dir {
            LayoutSplitDir::Horizontal => Self::Horizontal,
            LayoutSplitDir::Vertical => Self::Vertical,
        }
    }
}

/// One node in a flat postorder split tree (children precede parents; last node is the root).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LayoutNode {
    Pane {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    Split {
        direction: LayoutSplitDir,
        ratio: f32,
        a: usize,
        b: usize,
    },
}

/// One saved named tab layout.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct NamedLayout {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_title: Option<String>,
    pub nodes: Vec<LayoutNode>,
    pub focused: usize,
}

impl NamedLayout {
    /// Number of panes in this layout.
    pub fn pane_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|n| matches!(n, LayoutNode::Pane { .. }))
            .count()
    }

    /// Validate tree structure (postorder DAG, single parent per child, finite ratios, valid focus).
    pub fn validate(&self) -> io::Result<()> {
        if clean_layout_name(&self.name).as_deref() != Some(self.name.as_str()) {
            return Err(invalid("layout has an invalid name"));
        }
        if self.nodes.is_empty() || self.nodes.len() > MAX_LAYOUT_PANES * 2 {
            return Err(invalid("layout node count is out of bounds"));
        }
        if !matches!(self.nodes.get(self.focused), Some(LayoutNode::Pane { .. })) {
            return Err(invalid("layout has no valid focused pane"));
        }
        let mut parents = vec![false; self.nodes.len()];
        let mut panes = 0usize;
        for (index, node) in self.nodes.iter().enumerate() {
            match *node {
                LayoutNode::Pane { .. } => panes += 1,
                LayoutNode::Split { ratio, a, b, .. } => {
                    if !ratio.is_finite() {
                        return Err(invalid("layout split has an invalid ratio"));
                    }
                    for child in [a, b] {
                        if child >= index || std::mem::replace(&mut parents[child], true) {
                            return Err(invalid("layout split has an invalid child"));
                        }
                    }
                }
            }
        }
        if panes == 0 || panes > MAX_LAYOUT_PANES {
            return Err(invalid("layout pane count is out of bounds"));
        }
        if parents[..parents.len() - 1].contains(&false) {
            return Err(invalid("layout contains disconnected nodes"));
        }
        Ok(())
    }

    /// Format this layout as a command-palette row (English + Arabic pane count).
    pub fn to_palette_item(&self) -> PaletteItem {
        let count = self.pane_count();
        let title_ar = if count == 1 {
            "1 pane · لوحة واحدة".into()
        } else {
            format!("{count} panes · {count} لوحات")
        };
        PaletteItem {
            title_en: self.name.clone(),
            title_ar,
            shortcut: None,
            dimmed: false,
        }
    }

    /// Prepare this layout for opening in a new tab: bounds split depth/count, clamps ratios,
    /// and validates directories (missing directories fall back to `None`).
    pub fn prepare(&self) -> PreparedLayout {
        let mut splits_left = MAX_LAYOUT_PANES.saturating_sub(1);
        let mut panes = Vec::new();
        let mut focused = None;
        let root = self.prepare_node(
            self.nodes.len() - 1,
            0,
            &mut splits_left,
            &mut panes,
            &mut focused,
        );
        let tab_title = self
            .tab_title
            .as_deref()
            .map(|s| clean_text(s, MAX_TITLE_CHARS))
            .filter(|s| !s.is_empty());
        let focused = focused.unwrap_or_else(|| root.leaves()[0]);
        PreparedLayout {
            tab_title,
            root,
            focused,
            panes,
        }
    }

    fn prepare_node(
        &self,
        mut index: usize,
        depth: usize,
        splits_left: &mut usize,
        panes: &mut Vec<PreparedPane>,
        focused: &mut Option<PaneId>,
    ) -> Node {
        loop {
            match &self.nodes[index] {
                LayoutNode::Pane { cwd, title } => {
                    let resolved_cwd = cwd
                        .as_deref()
                        .map(Path::new)
                        .filter(|p| p.is_absolute() && p.is_dir())
                        .map(Path::to_path_buf);
                    let cleaned_title = title
                        .as_deref()
                        .map(|s| clean_text(s, MAX_TITLE_CHARS))
                        .filter(|s| !s.is_empty());
                    panes.push(PreparedPane {
                        cwd: resolved_cwd,
                        title: cleaned_title,
                    });
                    // Local 1-based slot index; remapped to fresh PaneIds when opening.
                    let slot = PaneId(panes.len() as u64);
                    if index == self.focused {
                        *focused = Some(slot);
                    }
                    return Node::Leaf(slot);
                }
                LayoutNode::Split {
                    direction,
                    ratio,
                    a,
                    b,
                } if depth < MAX_SPLIT_DEPTH && *splits_left > 0 => {
                    *splits_left -= 1;
                    return Node::Split {
                        dir: (*direction).into(),
                        ratio: ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO),
                        a: Box::new(self.prepare_node(*a, depth + 1, splits_left, panes, focused)),
                        b: Box::new(self.prepare_node(*b, depth + 1, splits_left, panes, focused)),
                    };
                }
                LayoutNode::Split { a, .. } => index = *a,
            }
        }
    }
}

impl Searchable for NamedLayout {
    fn search_fields(&self) -> Vec<&str> {
        let mut fields = vec![self.name.as_str()];
        if let Some(tab_title) = self.tab_title.as_deref() {
            fields.push(tab_title);
        }
        fields
    }
}

/// One pane ready to spawn when opening a named layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedPane {
    pub cwd: Option<PathBuf>,
    pub title: Option<String>,
}

/// Validated, bounded tab layout ready to recreate in a workspace.
#[derive(Clone, Debug)]
pub struct PreparedLayout {
    pub tab_title: Option<String>,
    pub root: Node,
    pub focused: PaneId,
    pub panes: Vec<PreparedPane>,
}

impl PreparedLayout {
    /// Recreate this layout as a new active tab in `workspace`. Returns the focused pane id.
    pub(crate) fn open_in(
        self,
        workspace: &mut Workspace,
        geo: &impl GridGeometry,
        factory: &mut impl PaneFactory,
    ) -> Option<PaneId> {
        workspace.open_layout_tab(
            self.tab_title,
            self.root,
            self.focused,
            |slot, id, cols, rows, factory| {
                let spec = &self.panes[slot.0 as usize - 1];
                let mut pane = factory
                    .spawn(id, cols, rows, spec.cwd.clone())
                    .or_else(|| {
                        spec.cwd.as_ref()?;
                        factory.spawn(id, cols, rows, None)
                    })?;
                pane.title.clone_from(&spec.title);
                Some(pane)
            },
            geo,
            factory,
        )
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct LayoutsFile {
    version: u32,
    layouts: Vec<NamedLayout>,
}

/// Persistent bounded store of named tab layouts (`layouts.json`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayoutStore {
    layouts: Vec<NamedLayout>,
}

impl LayoutStore {
    /// Compute the layouts file path from `XDG_STATE_HOME` and `HOME` (relative paths ignored).
    pub fn path_from(xdg_state_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
        let absolute = |v: Option<&OsStr>| {
            v.filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        };
        let base = if let Some(xdg) = absolute(xdg_state_home) {
            xdg
        } else {
            absolute(home)?.join(".local").join("state")
        };
        Some(base.join("twitty").join("layouts.json"))
    }

    /// Default path next to `session.json`.
    pub fn default_path() -> Option<PathBuf> {
        #[cfg(windows)]
        {
            let base = std::env::var_os("LOCALAPPDATA")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())?;
            Some(base.join("twitty").join("layouts.json"))
        }
        #[cfg(not(windows))]
        {
            let xdg = std::env::var_os("XDG_STATE_HOME");
            let home = std::env::var_os("HOME");
            Self::path_from(xdg.as_deref(), home.as_deref())
        }
    }

    /// Load from the default path; missing or invalid files yield an empty store.
    pub fn load_default() -> Self {
        Self::default_path().map_or_else(Self::default, |path| Self::load_from(&path))
    }

    /// Load from `path`, refusing symlinks, non-regular files, oversized files, and unsupported
    /// schema versions. Invalid individual layouts are dropped with a warning.
    pub fn load_from(path: &Path) -> Self {
        use std::io::Read;

        let file = match open_state_file(path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Self::default(),
            Err(err) => {
                warn(format!(
                    "twitty: ignoring layouts {} ({err})",
                    path.display()
                ));
                return Self::default();
            }
        };
        let mut bytes = Vec::new();
        if let Err(err) = file
            .take(MAX_LAYOUTS_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
        {
            warn(format!(
                "twitty: could not read layouts {} ({err})",
                path.display()
            ));
            return Self::default();
        }
        if bytes.len() > MAX_LAYOUTS_FILE_BYTES {
            warn(format!(
                "twitty: ignoring oversized layouts {}",
                path.display()
            ));
            return Self::default();
        }
        let parsed: LayoutsFile = match serde_json::from_slice(&bytes) {
            Ok(parsed) => parsed,
            Err(err) => {
                warn(format!(
                    "twitty: ignoring corrupt layouts {} ({err})",
                    path.display()
                ));
                return Self::default();
            }
        };
        if parsed.version != LAYOUTS_VERSION {
            warn(format!(
                "twitty: ignoring unsupported layouts version {} in {}",
                parsed.version,
                path.display()
            ));
            return Self::default();
        }
        let mut layouts: Vec<NamedLayout> = Vec::new();
        for mut layout in parsed.layouts {
            let Some(clean_name) = clean_layout_name(&layout.name) else {
                continue;
            };
            layout.name = clean_name;
            if layout.validate().is_err() {
                continue;
            }
            if layouts.iter().any(|existing| existing.name == layout.name) {
                continue;
            }
            layouts.push(layout);
            if layouts.len() >= MAX_LAYOUTS {
                break;
            }
        }
        Self { layouts }
    }

    pub fn layouts(&self) -> &[NamedLayout] {
        &self.layouts
    }

    pub fn len(&self) -> usize {
        self.layouts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.layouts.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&NamedLayout> {
        let name = clean_layout_name(name)?;
        self.layouts.iter().find(|l| l.name == name)
    }

    /// Insert or replace a named layout (most recently saved first, bounded to [`MAX_LAYOUTS`]).
    pub fn upsert(&mut self, mut layout: NamedLayout) -> io::Result<()> {
        let name = clean_layout_name(&layout.name)
            .ok_or_else(|| invalid("layout name cannot be empty"))?;
        layout.name = name;
        layout.validate()?;
        self.layouts.retain(|existing| existing.name != layout.name);
        self.layouts.insert(0, layout);
        if self.layouts.len() > MAX_LAYOUTS {
            self.layouts.truncate(MAX_LAYOUTS);
        }
        Ok(())
    }

    /// Delete the layout with `name`. Returns `true` when one was removed.
    pub fn delete(&mut self, name: &str) -> bool {
        let before = self.layouts.len();
        self.layouts.retain(|l| l.name != name);
        self.layouts.len() != before
    }

    /// Atomically save to the default state path.
    pub fn save_default(&self) -> io::Result<()> {
        let path = Self::default_path().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "could not determine state directory for layouts",
            )
        })?;
        self.save_to(&path)
    }

    /// Atomically save to `path` (`0600` in a `0700` dir, refusing symlinks).
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        let payload = LayoutsFile {
            version: LAYOUTS_VERSION,
            layouts: self.layouts.iter().take(MAX_LAYOUTS).cloned().collect(),
        };
        let bytes = serde_json::to_vec_pretty(&payload).map_err(io::Error::other)?;
        if bytes.len() > MAX_LAYOUTS_FILE_BYTES {
            return Err(invalid("layouts file exceeds size bound"));
        }
        atomic_write_state_file(path, ".layouts", &bytes)
    }
}

/// Capture the active tab of `workspace` as a [`NamedLayout`].
pub(crate) fn capture_active_tab(name: &str, workspace: &Workspace) -> Option<NamedLayout> {
    let name = clean_layout_name(name)?;
    let tab = workspace.active_tab()?;
    let tab_title = tab
        .title
        .as_deref()
        .map(|s| clean_text(s, MAX_TITLE_CHARS))
        .filter(|s| !s.is_empty());
    let mut nodes = Vec::new();
    let mut focused = 0usize;
    capture_node(workspace, &tab.root, tab.focused, &mut nodes, &mut focused);
    let layout = NamedLayout {
        name,
        tab_title,
        nodes,
        focused,
    };
    layout.validate().ok()?;
    Some(layout)
}

/// Capture an arbitrary split tree with caller-supplied `(cwd, title)` per pane.
/// Useful for window-free tests and programmatic construction.
pub fn capture_tree(
    name: &str,
    tab_title: Option<&str>,
    root: &Node,
    focused_pane: PaneId,
    mut pane_info: impl FnMut(PaneId) -> (Option<PathBuf>, Option<String>),
) -> Option<NamedLayout> {
    let name = clean_layout_name(name)?;
    let tab_title = tab_title
        .map(|s| clean_text(s, MAX_TITLE_CHARS))
        .filter(|s| !s.is_empty());
    let mut nodes = Vec::new();
    let mut focused = 0usize;
    capture_tree_node(root, focused_pane, &mut pane_info, &mut nodes, &mut focused);
    let layout = NamedLayout {
        name,
        tab_title,
        nodes,
        focused,
    };
    layout.validate().ok()?;
    Some(layout)
}

fn capture_node(
    workspace: &Workspace,
    node: &Node,
    focused_pane: PaneId,
    nodes: &mut Vec<LayoutNode>,
    focused_out: &mut usize,
) -> usize {
    capture_tree_node(
        node,
        focused_pane,
        &mut |id| {
            let Some(pane) = workspace.pane(id) else {
                return (None, None);
            };
            let cwd = if pane.remote.is_some() {
                None
            } else {
                pane.pty
                    .as_ref()
                    .and_then(Pty::cwd)
                    .or_else(|| pane.meta.cwd.clone())
            };
            (cwd, pane.title.clone())
        },
        nodes,
        focused_out,
    )
}

fn capture_tree_node(
    node: &Node,
    focused_pane: PaneId,
    pane_info: &mut impl FnMut(PaneId) -> (Option<PathBuf>, Option<String>),
    nodes: &mut Vec<LayoutNode>,
    focused_out: &mut usize,
) -> usize {
    let saved = match node {
        Node::Leaf(id) => {
            if *id == focused_pane {
                *focused_out = nodes.len();
            }
            let (cwd, title) = pane_info(*id);
            LayoutNode::Pane {
                cwd: cwd
                    .as_deref()
                    .filter(|p| p.is_absolute())
                    .and_then(Path::to_str)
                    .map(str::to_owned),
                title: title
                    .as_deref()
                    .map(|s| clean_text(s, MAX_TITLE_CHARS))
                    .filter(|s| !s.is_empty()),
            }
        }
        Node::Split { dir, ratio, a, b } => LayoutNode::Split {
            direction: (*dir).into(),
            ratio: *ratio,
            a: capture_tree_node(a, focused_pane, pane_info, nodes, focused_out),
            b: capture_tree_node(b, focused_pane, pane_info, nodes, focused_out),
        },
    };
    let index = nodes.len();
    nodes.push(saved);
    index
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Rect;
    use crate::session::pane::Pane;
    use crate::terminal::Terminal;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let base = std::env::temp_dir().canonicalize().unwrap();
            loop {
                let n = SEQ.fetch_add(1, Ordering::Relaxed);
                let dir = base.join(format!("twitty-layouts-unit-{}-{n}", std::process::id()));
                match fs::create_dir(&dir) {
                    Ok(()) => return Self(dir),
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(e) => panic!("temp dir: {e}"),
                }
            }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct Grid;
    impl GridGeometry for Grid {
        fn content_rect(&self, _tabs: usize) -> Rect {
            Rect::new(0.0, 0.0, 1200.0, 800.0)
        }
        fn gap(&self) -> f32 {
            1.0
        }
        fn grid_size(&self, rect: Rect) -> (usize, usize) {
            (
                ((rect.w / 10.0) as usize).max(2),
                ((rect.h / 20.0) as usize).max(1),
            )
        }
    }

    #[derive(Default)]
    struct FakeShells {
        spawned: Vec<(PaneId, Option<PathBuf>)>,
    }

    impl PaneFactory for FakeShells {
        fn spawn(
            &mut self,
            id: PaneId,
            cols: usize,
            rows: usize,
            cwd: Option<PathBuf>,
        ) -> Option<Pane> {
            self.spawned.push((id, cwd));
            Some(Pane::new(
                id,
                Terminal::new(cols, rows, |_| {}),
                None,
                (cols, rows),
            ))
        }
    }

    #[test]
    fn workspace_save_load_and_open_named_layout_round_trip() {
        let tmp = TempDir::new();
        let dir_a = tmp.0.join("alpha");
        let dir_b = tmp.0.join("مشروع");
        fs::create_dir(&dir_a).unwrap();
        fs::create_dir(&dir_b).unwrap();

        let mut ws = Workspace::new();
        let mut shells = FakeShells::default();
        let p1 = ws.new_tab(&Grid, &mut shells).unwrap();
        let (_, p2) = ws
            .split_focused(&Grid, &mut shells, SplitDir::Horizontal)
            .unwrap();
        let (_, p3) = ws
            .split_focused(&Grid, &mut shells, SplitDir::Vertical)
            .unwrap();
        assert!(ws.set_ratio(&[], 0.35));
        assert!(ws.set_ratio(&[true], 0.7));
        ws.rename_tab(0, Some("Dev Stack · العمل".into()));
        ws.focus_pane(p2);

        ws.pane_mut(p1).unwrap().meta.cwd = Some(dir_a.clone());
        ws.pane_mut(p1).unwrap().title = Some("editor".into());
        ws.pane_mut(p2).unwrap().meta.cwd = Some(dir_b.clone());
        ws.pane_mut(p2).unwrap().title = Some("بناء".into());
        ws.pane_mut(p3).unwrap().meta.cwd = Some(tmp.0.join("deleted-later"));
        ws.pane_mut(p3).unwrap().title = Some("logs".into());

        let captured = capture_active_tab("backend · الخلفية", &ws).unwrap();
        assert_eq!(captured.pane_count(), 3);

        let store_path = tmp.0.join("state").join("layouts.json");
        let mut store = LayoutStore::default();
        store.upsert(captured.clone()).unwrap();
        store.save_to(&store_path).unwrap();

        let loaded = LayoutStore::load_from(&store_path);
        let saved = loaded.get("backend · الخلفية").unwrap();
        assert_eq!(saved, &captured);

        // Open the saved layout as a brand-new tab in the workspace.
        let before_tabs = ws.tabs().len();
        let mut reopen_shells = FakeShells::default();
        let focused = saved
            .prepare()
            .open_in(&mut ws, &Grid, &mut reopen_shells)
            .unwrap();
        assert_eq!(ws.tabs().len(), before_tabs + 1);
        assert_eq!(ws.active(), before_tabs);
        assert_eq!(
            ws.tabs()[ws.active()].title.as_deref(),
            Some("Dev Stack · العمل")
        );
        assert_eq!(ws.focused_id(), Some(focused));
        assert_eq!(ws.pane(focused).unwrap().title.as_deref(), Some("بناء"));

        // Verify spawned cwds: existing dirs restored, missing dir falls back to None.
        let cwds: Vec<_> = reopen_shells
            .spawned
            .iter()
            .map(|(_, c)| c.clone())
            .collect();
        assert_eq!(cwds, vec![Some(dir_a), Some(dir_b), None]);

        // Re-capturing the opened tab reproduces the same split tree and ratios.
        let leaves = ws.tabs()[ws.active()].root.leaves();
        assert_eq!(leaves.len(), 3);
        match &ws.tabs()[ws.active()].root {
            Node::Split { dir, ratio, b, .. } => {
                assert_eq!(*dir, SplitDir::Horizontal);
                assert!((*ratio - 0.35).abs() < 0.001);
                match b.as_ref() {
                    Node::Split { dir, ratio, .. } => {
                        assert_eq!(*dir, SplitDir::Vertical);
                        assert!((*ratio - 0.7).abs() < 0.001);
                    }
                    _ => panic!("expected nested vertical split"),
                }
            }
            _ => panic!("expected horizontal split at root"),
        }
    }
}
