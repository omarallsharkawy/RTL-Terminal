//! Save the workspace layout and restart local shells or reattach to remote sessions.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::layout::{Node, PaneId, SplitDir, MIN_RATIO};
use crate::pty::Pty;
use crate::remote::Target;
use crate::session::workspace::{GridGeometry, PaneFactory, Tab, Workspace};
use crate::shell_integration::clean_text;

const VERSION: u32 = 1;
const MAX_TABS: usize = 32;
const MAX_PANES: usize = 64;
const MAX_SPLIT_DEPTH: usize = 16;
const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
const MAX_TITLE_CHARS: usize = 256;

#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct Session {
    version: u32,
    active_tab: usize,
    sidebar_visible: bool,
    tabs: Vec<SavedTab>,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct SavedTab {
    // A postorder list: children precede their parent and the last node is the root.
    // Flat JSON lets us prune deep trees without recursing while deserializing them.
    nodes: Vec<SavedNode>,
    focused: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SavedNode {
    Pane {
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    Ssh {
        #[serde(flatten)]
        target: Target,
    },
    Split {
        direction: Direction,
        ratio: f32,
        a: usize,
        b: usize,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Direction {
    Horizontal,
    Vertical,
}

impl Session {
    fn capture(workspace: &Workspace, sidebar_visible: bool) -> Self {
        Self {
            version: VERSION,
            active_tab: workspace.active(),
            sidebar_visible,
            tabs: workspace
                .tabs()
                .iter()
                .map(|tab| {
                    let mut saved = SavedTab {
                        nodes: Vec::new(),
                        focused: 0,
                        title: tab.title.clone(),
                    };
                    saved.capture_node(workspace, &tab.root, tab.focused);
                    saved
                })
                .collect(),
        }
    }

    fn read_from(path: &Path) -> io::Result<Self> {
        let mut bytes = Vec::new();
        open_session(path)?
            .take(MAX_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(invalid("session file is too large"));
        }
        let session: Self = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if session.version != VERSION {
            return Err(invalid("unsupported session version"));
        }
        if session.active_tab >= session.tabs.len() {
            return Err(invalid("session has no valid active tab"));
        }
        for tab in &session.tabs {
            tab.validate()?;
        }
        Ok(session)
    }

    fn prepare(self) -> PreparedSession {
        let tab_count = self.tabs.len().min(MAX_TABS);
        // Each retained tab gets a leaf before we spend any budget on splits.
        let mut splits_left = MAX_PANES - tab_count;
        let mut panes = Vec::new();
        let tabs = self
            .tabs
            .iter()
            .take(tab_count)
            .enumerate()
            .map(|(tab_index, tab)| {
                let mut focused = None;
                let root = tab.prepare_node(
                    tab.nodes.len() - 1,
                    0,
                    &mut splits_left,
                    &mut panes,
                    &mut focused,
                );
                Tab {
                    id: (tab_index + 1) as u64,
                    title: tab
                        .title
                        .as_deref()
                        .map(|s| clean_text(s, MAX_TITLE_CHARS))
                        .filter(|s| !s.is_empty()),
                    focused: focused.unwrap_or_else(|| root.leaves()[0]),
                    root,
                }
            })
            .collect();
        PreparedSession {
            tabs,
            panes,
            active_tab: self.active_tab,
            sidebar_visible: self.sidebar_visible,
        }
    }
}

impl SavedTab {
    fn capture_node(&mut self, workspace: &Workspace, node: &Node, focused: PaneId) -> usize {
        let saved = match node {
            Node::Leaf(id) => {
                if *id == focused {
                    self.focused = self.nodes.len();
                }
                let pane = workspace.pane(*id).expect("every leaf has a pane");
                if let Some(remote) = &pane.remote {
                    let index = self.nodes.len();
                    self.nodes.push(SavedNode::Ssh {
                        target: remote.target.clone(),
                    });
                    return index;
                }
                // The shell's own directory first, like new tabs: OSC 7 is only as fresh as the
                // last prompt (`cd /tmp && server` never reports /tmp) and may name a remote host.
                let cwd = pane
                    .pty
                    .as_ref()
                    .and_then(Pty::cwd)
                    .or_else(|| pane.meta.cwd.clone());
                SavedNode::Pane {
                    cwd: cwd.as_deref().and_then(Path::to_str).map(str::to_owned),
                    title: pane.title.as_deref().map(session_title),
                }
            }
            Node::Split { dir, ratio, a, b } => SavedNode::Split {
                direction: match dir {
                    SplitDir::Horizontal => Direction::Horizontal,
                    SplitDir::Vertical => Direction::Vertical,
                },
                ratio: *ratio,
                a: self.capture_node(workspace, a, focused),
                b: self.capture_node(workspace, b, focused),
            },
        };
        let index = self.nodes.len();
        self.nodes.push(saved);
        index
    }

    fn validate(&self) -> io::Result<()> {
        if !matches!(
            self.nodes.get(self.focused),
            Some(SavedNode::Pane { .. } | SavedNode::Ssh { .. })
        ) {
            return Err(invalid("tab has no valid focused pane"));
        }
        let mut parents = vec![false; self.nodes.len()];
        for (index, node) in self.nodes.iter().enumerate() {
            if let SavedNode::Split { ratio, a, b, .. } = *node {
                if !ratio.is_finite() {
                    return Err(invalid("split has an invalid ratio"));
                }
                for child in [a, b] {
                    if child >= index || std::mem::replace(&mut parents[child], true) {
                        return Err(invalid("split has an invalid child"));
                    }
                }
            }
        }
        // With forward references excluded and one parent per child, this proves
        // connectivity to the final node, with no cycles or duplicate leaves.
        if parents[..parents.len() - 1].contains(&false) {
            return Err(invalid("tab contains disconnected nodes"));
        }
        Ok(())
    }

    fn prepare_node(
        &self,
        mut index: usize,
        depth: usize,
        splits_left: &mut usize,
        panes: &mut Vec<PaneSettings>,
        focused: &mut Option<PaneId>,
    ) -> Node {
        loop {
            match &self.nodes[index] {
                SavedNode::Pane { .. } | SavedNode::Ssh { .. } => {
                    panes.push(PaneSettings::from_node(&self.nodes[index]));
                    let id = PaneId(panes.len() as u64);
                    if index == self.focused {
                        *focused = Some(id);
                    }
                    return Node::Leaf(id);
                }
                SavedNode::Split {
                    direction,
                    ratio,
                    a,
                    b,
                } if depth < MAX_SPLIT_DEPTH && *splits_left > 0 => {
                    *splits_left -= 1;
                    return Node::Split {
                        dir: match direction {
                            Direction::Horizontal => SplitDir::Horizontal,
                            Direction::Vertical => SplitDir::Vertical,
                        },
                        ratio: ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO),
                        a: Box::new(self.prepare_node(*a, depth + 1, splits_left, panes, focused)),
                        b: Box::new(self.prepare_node(*b, depth + 1, splits_left, panes, focused)),
                    };
                }
                // Collapse a clipped subtree to its leftmost/topmost leaf.
                SavedNode::Split { a, .. } => index = *a,
            }
        }
    }
}

enum PaneSettings {
    Local {
        cwd: Option<PathBuf>,
        title: Option<String>,
    },
    Ssh(Target),
}

impl PaneSettings {
    fn from_node(node: &SavedNode) -> Self {
        match node {
            SavedNode::Pane { cwd, title } => {
                let cwd = cwd
                    .as_deref()
                    .map(Path::new)
                    .filter(|p| p.is_absolute() && p.is_dir());
                Self::Local {
                    cwd: cwd.map(Path::to_path_buf),
                    title: title.as_deref().map(session_title),
                }
            }
            SavedNode::Ssh { target } => Self::Ssh(target.clone()),
            SavedNode::Split { .. } => unreachable!("only leaf nodes have pane settings"),
        }
    }
}

pub(crate) struct PreparedSession {
    tabs: Vec<Tab>,
    panes: Vec<PaneSettings>,
    active_tab: usize,
    pub sidebar_visible: bool,
}

impl PreparedSession {
    pub fn rebuild(self, geo: &impl GridGeometry, factory: &mut impl PaneFactory) -> Workspace {
        let mut workspace = Workspace::new();
        let mut active = 0;
        let tab_count = self.tabs.len();
        for (index, tab) in self.tabs.into_iter().enumerate() {
            factory.set_tab_id(tab.id);
            let restored = workspace.restore_tab(tab, tab_count, geo, |id, cols, rows| {
                match &self.panes[id.0 as usize - 1] {
                    PaneSettings::Ssh(target) => {
                        factory.spawn_remote(id, cols, rows, target.clone())
                    }
                    PaneSettings::Local { cwd, title } => {
                        let mut pane =
                            factory.spawn(id, cols, rows, cwd.clone()).or_else(|| {
                                cwd.as_ref()?;
                                factory.spawn(id, cols, rows, None)
                            })?;
                        // Leave meta.cwd to OSC 7 so later directory changes stay visible.
                        pane.title.clone_from(title);
                        Some(pane)
                    }
                }
            });
            if restored && index == self.active_tab {
                active = workspace.tabs().len() - 1;
            }
        }
        workspace.set_active_tab(active);
        workspace.apply_layout(geo);
        workspace
    }
}

/// A single window's session file and the reason it is shutting down.
pub(crate) struct SessionStore {
    path: Option<PathBuf>,
    last_pane_closed: bool,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            path: session_path(),
            last_pane_closed: false,
        }
    }

    pub fn load(&self) -> Option<PreparedSession> {
        let Some(path) = &self.path else {
            crate::config::warn("twitty: no session state directory is configured".into());
            return None;
        };
        match Session::read_from(path) {
            Ok(session) => Some(session.prepare()),
            // No saved session (first start, or the last shell was closed): start fresh quietly.
            Err(err) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => {
                warn("could not restore", path, &err);
                None
            }
        }
    }

    pub fn last_pane_closed(&mut self) {
        self.last_pane_closed = true;
    }

    pub fn finish(&self, workspace: &Workspace, sidebar_visible: bool) {
        let Some(path) = &self.path else {
            return;
        };
        let (action, result) = if self.last_pane_closed {
            ("could not remove", remove_session(path))
        } else if !workspace.is_empty() {
            let session = Session::capture(workspace, sidebar_visible);
            ("could not save", write_session(path, &session))
        } else {
            return;
        };
        if let Err(err) = result {
            warn(action, path, &err);
        }
    }
}

fn session_title(title: &str) -> String {
    clean_text(title, MAX_TITLE_CHARS)
}

fn warn(action: &str, path: &Path, err: &io::Error) {
    crate::config::warn(clean_text(
        &format!("twitty: {action} session {} ({err})", path.display()),
        512,
    ));
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn session_path() -> Option<PathBuf> {
    let absolute_dir = |key| {
        std::env::var_os(key)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let base = if cfg!(windows) {
        absolute_dir("LOCALAPPDATA")?
    } else {
        absolute_dir("XDG_STATE_HOME")
            .or_else(|| Some(absolute_dir("HOME")?.join(".local/state")))?
    };
    Some(base.join("twitty").join("session.json"))
}

fn session_dir(path: &Path) -> &Path {
    path.parent()
        .expect("session file lives in a state directory")
}

fn require_directory(path: &Path) -> io::Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        Ok(metadata)
    } else {
        Err(invalid("session directory must be a real directory"))
    }
}

fn require_file(metadata: fs::Metadata) -> io::Result<()> {
    if metadata.is_file() {
        Ok(())
    } else {
        Err(invalid("session file must be a regular file"))
    }
}

fn open_session(path: &Path) -> io::Result<File> {
    require_directory(session_dir(path))?;
    require_file(fs::symlink_metadata(path)?)?;
    let mut options = OpenOptions::new();
    options.read(true);
    // Enforce the no-follow rule at open too, in case the entry changed since the check.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    require_file(file.metadata()?)?;
    Ok(file)
}

fn remove_session(path: &Path) -> io::Result<()> {
    let remove = || {
        require_directory(session_dir(path))?;
        require_file(fs::symlink_metadata(path)?)?;
        fs::remove_file(path)
    };
    match remove() {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn write_session(path: &Path, session: &Session) -> io::Result<()> {
    let bytes = serde_json::to_vec(session).map_err(io::Error::other)?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(invalid("session file is too large"));
    }
    let dir = session_dir(path);
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    let metadata = require_directory(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o7777 != 0o700 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
    }
    #[cfg(not(unix))]
    let _ = metadata;

    let (temp_path, file) = create_temp_file(dir)?;
    let result = (|| {
        write_temp_file(file, &bytes)?;
        match fs::symlink_metadata(path) {
            Ok(metadata) => require_file(metadata)?,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        fs::rename(&temp_path, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp_path);
    }
    result
}

fn create_temp_file(dir: &Path) -> io::Result<(PathBuf, File)> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    for _ in 0..64 {
        let suffix = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(".session-{}-{suffix}.tmp", std::process::id()));
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::other("could not create temporary session file"))
}

fn write_temp_file(mut file: File, bytes: &[u8]) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    mod ssh {
        include!("../../tests/support/ssh_session_restore.rs");
    }

    use super::*;
    use crate::layout::Rect;
    use crate::session::pane::Pane;
    use crate::terminal::Terminal;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
            let base = std::env::temp_dir().canonicalize().unwrap();
            loop {
                let serial = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
                let dir = base.join(format!("twitty-restore-{}-{serial}", std::process::id()));
                match fs::create_dir(&dir) {
                    Ok(()) => return Self(dir),
                    Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(err) => panic!("create test directory: {err}"),
                }
            }
        }

        fn store(&self) -> SessionStore {
            SessionStore {
                path: Some(self.0.join("twitty").join("session.json")),
                last_pane_closed: false,
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
        fn content_rect(&self, tab_count: usize) -> Rect {
            let top = if tab_count > 1 { 20.0 } else { 0.0 };
            Rect::new(0.0, top, 1400.0, 900.0 - top)
        }

        fn gap(&self) -> f32 {
            1.0
        }

        fn grid_size(&self, rect: Rect) -> (usize, usize) {
            let cols = (rect.w / 10.0) as usize;
            let rows = (rect.h / 20.0) as usize;
            (cols.max(2), rows.max(1))
        }
    }

    #[derive(Default)]
    struct FakeShells {
        calls: Vec<(PaneId, Option<PathBuf>)>,
        refuse_cwd: bool,
        refuse_id: Option<PaneId>,
    }

    impl PaneFactory for FakeShells {
        fn spawn(
            &mut self,
            id: PaneId,
            cols: usize,
            rows: usize,
            cwd: Option<PathBuf>,
        ) -> Option<Pane> {
            let has_cwd = cwd.is_some();
            self.calls.push((id, cwd));
            if self.refuse_id == Some(id) || (has_cwd && self.refuse_cwd) {
                return None;
            }
            Some(Pane::new(
                id,
                Terminal::new(cols, rows, |_| {}),
                None,
                (cols, rows),
            ))
        }
    }

    fn leaf(cwd: Option<String>) -> SavedNode {
        SavedNode::Pane { cwd, title: None }
    }

    fn split(a: usize, b: usize) -> SavedNode {
        SavedNode::Split {
            direction: Direction::Horizontal,
            ratio: 0.25,
            a,
            b,
        }
    }

    fn one_tab(nodes: Vec<SavedNode>, focused: usize) -> Session {
        Session {
            version: VERSION,
            active_tab: 0,
            sidebar_visible: true,
            tabs: vec![SavedTab {
                nodes,
                focused,
                title: None,
            }],
        }
    }

    #[test]
    fn round_trips_tab_order_layout_ratios_directories_titles_and_focus() {
        let temp = TempDir::new();
        let store = temp.store();
        let mut source = Workspace::new();
        let mut shells = FakeShells::default();
        let discarded = source.new_tab(&Grid, &mut shells).unwrap();
        source.close_pane(discarded).unwrap();
        let first = source.new_tab(&Grid, &mut shells).unwrap();
        let second = source.new_tab(&Grid, &mut shells).unwrap();
        let (_, right) = source
            .split_focused(&Grid, &mut shells, SplitDir::Horizontal)
            .unwrap();
        let (_, bottom) = source
            .split_focused(&Grid, &mut shells, SplitDir::Vertical)
            .unwrap();
        assert!(source.set_ratio(&[], 0.35));
        assert!(source.set_ratio(&[true], 0.8));
        assert_eq!(source.focus_pane(right), Some(bottom));
        let third = source.new_tab(&Grid, &mut shells).unwrap();
        assert!(source.set_active_tab(1));
        source.apply_layout(&Grid);
        for (id, name) in [
            (first, "أول"),
            (second, "ثان"),
            (right, "يمين"),
            (bottom, "أسفل"),
            (third, "ثالث"),
        ] {
            let cwd = temp.0.join(name);
            fs::create_dir(&cwd).unwrap();
            let pane = source.pane_mut(id).unwrap();
            pane.meta.cwd = Some(cwd);
            pane.title = Some(name.to_owned());
            pane.meta.last_command = Some("secret-command".into());
            pane.terminal.process_bytes(b"secret-output");
        }

        store.finish(&source, false);
        let path = store.path.as_deref().unwrap();
        let json = fs::read_to_string(path).unwrap();
        assert!(!json.contains("secret"));
        let loaded = Session::read_from(path).unwrap();
        assert_eq!(loaded, Session::capture(&source, false));
        let prepared = store.load().unwrap();
        assert!(!prepared.sidebar_visible);
        let mut restarted = FakeShells::default();
        let mut restored = prepared.rebuild(&Grid, &mut restarted);
        assert_eq!(restored.active(), 1);
        let expected = [first, second, right, bottom, third];
        assert_eq!(restarted.calls.len(), expected.len());
        for ((id, cwd), old_id) in restarted.calls.into_iter().zip(expected) {
            assert_ne!(id, old_id);
            let original = source.pane(old_id).unwrap();
            let fresh = restored.pane_mut(id).unwrap();
            assert_eq!(cwd, original.meta.cwd);
            assert_eq!(fresh.title, original.title);
            assert_eq!(fresh.size, original.size);
            assert!(fresh.meta.last_command.is_none());
            assert!(fresh.meta.cwd.is_none());
            // Emulate the fresh shell reporting its cwd after startup.
            fresh.meta.cwd = cwd;
        }
        // File-local indexes compare the complete layout and focus independent of runtime IDs.
        assert_eq!(Session::capture(&restored, false), loaded);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn prefers_the_linux_shell_directory_over_the_last_osc_7() {
        use crate::pty::SpawnOptions;

        let temp = TempDir::new();
        let mut workspace = Workspace::new();
        let id = workspace
            .new_tab(&Grid, &mut FakeShells::default())
            .unwrap();
        let options = SpawnOptions {
            cwd: Some(temp.0.clone()),
            command: vec!["sleep".into(), "10".into()],
            ..SpawnOptions::default()
        };
        workspace.pane_mut(id).unwrap().pty =
            Some(Pty::spawn_with(&options, 80, 24, || {}, || {}).unwrap());
        assert_eq!(
            Session::capture(&workspace, true).tabs[0].nodes,
            [leaf(temp.0.to_str().map(str::to_owned))]
        );

        // A stale OSC 7 (`cd x && server` never reaches a prompt) does not win over the shell.
        let reported = temp.0.join("مشروع");
        workspace.pane_mut(id).unwrap().meta.cwd = Some(reported.clone());
        assert_eq!(
            Session::capture(&workspace, true).tabs[0].nodes,
            [leaf(temp.0.to_str().map(str::to_owned))]
        );

        // Without a shell directory to read, OSC 7 is used.
        workspace.pane_mut(id).unwrap().pty = None;
        assert_eq!(
            Session::capture(&workspace, true).tabs[0].nodes,
            [leaf(reported.to_str().map(str::to_owned))]
        );
    }

    #[test]
    fn missing_empty_corrupt_and_future_files_are_rejected() {
        let temp = TempDir::new();
        let store = temp.store();
        let path = store.path.as_deref().unwrap();
        assert!(store.load().is_none());
        fs::create_dir(session_dir(path)).unwrap();
        for contents in ["", "{broken", "{\"version\":2,\"active_tab\":0,\"sidebar_visible\":true,\"tabs\":[{\"focused\":0,\"nodes\":[{\"kind\":\"pane\"}]}]}"] {
            fs::write(path, contents).unwrap();
            assert!(store.load().is_none());
        }
        fs::remove_file(path).unwrap();
        fs::create_dir(path).unwrap();
        assert!(store.load().is_none());
    }

    #[test]
    fn malformed_trees_are_rejected_before_any_shells_start() {
        let temp = TempDir::new();
        let store = temp.store();
        let path = store.path.as_deref().unwrap();
        for session in [
            one_tab(vec![leaf(None), split(0, 1)], 0), // a cycle
            one_tab(vec![leaf(None), split(0, 0)], 0), // shared child
            one_tab(vec![leaf(None), leaf(None)], 0),  // disconnected leaf
            one_tab(vec![leaf(None), leaf(None), split(0, 1)], 2), // focus is a split
        ] {
            write_session(path, &session).unwrap();
            assert!(Session::read_from(path).is_err());
        }
    }

    #[test]
    fn missing_directory_falls_back_to_default() {
        let temp = TempDir::new();
        let missing = temp.0.join("missing").to_str().unwrap().to_owned();
        let mut restarted = FakeShells::default();
        let restored = one_tab(vec![leaf(Some(missing))], 0)
            .prepare()
            .rebuild(&Grid, &mut restarted);
        assert!(!restored.is_empty());
        assert_eq!(restarted.calls, [(PaneId(1), None)]);
    }

    #[test]
    fn failed_directory_spawn_retries_and_failed_pane_leaves_a_valid_workspace() {
        let temp = TempDir::new();
        let mut restarted = FakeShells {
            refuse_cwd: true,
            refuse_id: Some(PaneId(2)),
            ..Default::default()
        };
        let session = one_tab(
            vec![
                leaf(temp.0.to_str().map(str::to_owned)),
                leaf(None),
                split(0, 1),
            ],
            1,
        );
        let mut restored = session.prepare().rebuild(&Grid, &mut restarted);
        assert_eq!(
            restarted.calls,
            [
                (PaneId(1), Some(temp.0.clone())),
                (PaneId(1), None),
                (PaneId(2), None)
            ]
        );
        assert_eq!(restored.tabs()[0].root, Node::Leaf(PaneId(1)));
        assert_eq!(restored.focused_id(), Some(PaneId(1)));
        assert_eq!(
            restored.pane(PaneId(1)).unwrap().size,
            Grid.grid_size(Grid.content_rect(1))
        );
        assert_eq!(restored.new_tab(&Grid, &mut restarted), Some(PaneId(3)));
    }

    #[test]
    fn prunes_deep_and_large_trees_and_keeps_a_pane_in_each_retained_tab() {
        let temp = TempDir::new();
        let store = temp.store();
        let path = store.path.as_deref().unwrap();
        let mut nodes = vec![leaf(None)];
        for _ in 0..500 {
            let child = nodes.len();
            nodes.push(leaf(None));
            nodes.push(split(child - 1, child));
        }
        let mut session = one_tab(nodes, 0);
        for _ in 0..MAX_TABS + 2 {
            session.tabs.push(SavedTab {
                nodes: vec![leaf(None), leaf(None), split(0, 1)],
                focused: 0,
                title: None,
            });
        }
        session.active_tab = MAX_TABS + 1;
        write_session(path, &session).unwrap();
        assert_eq!(Session::read_from(path).unwrap(), session);
        let workspace = store
            .load()
            .unwrap()
            .rebuild(&Grid, &mut FakeShells::default());
        assert_eq!(workspace.tabs().len(), MAX_TABS);
        assert_eq!(workspace.panes().count(), MAX_PANES);
        assert_eq!(workspace.tabs()[0].root.leaves().len(), MAX_SPLIT_DEPTH + 1);
        assert_eq!(workspace.tabs()[0].focused, PaneId(1));
        assert_eq!(workspace.active(), 0);
    }

    #[test]
    fn oversized_files_are_rejected_before_deserialization() {
        let temp = TempDir::new();
        let store = temp.store();
        let path = store.path.as_deref().unwrap();
        fs::create_dir(session_dir(path)).unwrap();
        File::create(path)
            .unwrap()
            .set_len(MAX_FILE_BYTES as u64 + 1)
            .unwrap();
        assert_eq!(
            Session::read_from(path).unwrap_err().to_string(),
            "session file is too large"
        );
    }

    #[test]
    fn leaves_previous_snapshot_until_normal_window_close_or_last_pane_exit() {
        let temp = TempDir::new();
        let mut store = temp.store();
        let mut workspace = Workspace::new();
        let mut shells = FakeShells::default();
        let first = workspace.new_tab(&Grid, &mut shells).unwrap();
        store.finish(&workspace, true);
        let path = store.path.clone().unwrap();
        let second = workspace.new_tab(&Grid, &mut shells).unwrap();
        assert_eq!(Session::read_from(&path).unwrap().tabs.len(), 1);
        store.finish(&workspace, false);
        assert_eq!(Session::read_from(&path).unwrap().tabs.len(), 2);
        assert!(workspace.close_pane(first).unwrap().focused.is_some());
        assert!(path.exists());
        assert!(workspace.close_pane(second).unwrap().focused.is_none());
        store.last_pane_closed();
        store.finish(&workspace, false);
        assert!(!path.exists());
        store.finish(&workspace, false); // already absent
    }

    #[cfg(unix)]
    #[test]
    fn creates_private_directory_and_file_and_refuses_symlinked_file() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let temp = TempDir::new();
        let store = temp.store();
        let path = store.path.as_deref().unwrap();
        let session = one_tab(vec![leaf(None)], 0);
        write_session(path, &session).unwrap();
        let mode = |p: &Path| fs::symlink_metadata(p).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode(path), 0o600);
        assert_eq!(mode(session_dir(path)), 0o700);

        let target = temp.0.join("target.json");
        fs::rename(path, &target).unwrap();
        let original = fs::read(&target).unwrap();
        symlink(&target, path).unwrap();
        assert!(Session::read_from(path).is_err());
        assert!(write_session(path, &session).is_err());
        assert!(remove_session(path).is_err());
        assert_eq!(fs::read_link(path).unwrap(), target);
        assert_eq!(fs::read(&target).unwrap(), original);
        assert_eq!(fs::read_dir(session_dir(path)).unwrap().count(), 1);

        fs::remove_file(target).unwrap();
        assert!(write_session(path, &session).is_err());
        assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn skips_non_utf8_paths() {
        use std::os::unix::ffi::OsStringExt;

        let mut workspace = Workspace::new();
        let pane = workspace
            .new_tab(&Grid, &mut FakeShells::default())
            .unwrap();
        workspace.pane_mut(pane).unwrap().meta.cwd =
            Some(std::ffi::OsString::from_vec(b"/\xff".to_vec()).into());
        assert_eq!(
            Session::capture(&workspace, true).tabs[0].nodes,
            [leaf(None)]
        );
    }

    #[test]
    fn session_restore_defaults_on_and_can_be_disabled() {
        use crate::config::TwittyConfig;

        assert!(
            serde_json::from_str::<TwittyConfig>("{}")
                .unwrap()
                .restore_session
        );
        assert!(
            !serde_json::from_str::<TwittyConfig>(r#"{"restore_session":false}"#)
                .unwrap()
                .restore_session
        );
    }
}
