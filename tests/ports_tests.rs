//! Listening ports in the sidebar: `/proc` parsing from fixture text, the process tree,
//! formatting, the scanner thread, and the sidebar line. No real `/proc` is needed.
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use cosmic_text::FontSystem;
use twitty::color::Palette;
use twitty::layout::{Rect, SidebarPosition};
use twitty::ports::{
    descendants, format_ports, parse_listening, parse_ppid, scan, socket_inode, PortWatcher,
    ProcSource, MAX_DESCENDANTS,
};
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::sidebar::{build, LabelCache};
use twitty::render::CellMetrics;
use twitty::sidebar::{Entry, ScrollState};

const TCP: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 111 1 0000000000000000 100 0 0 10 0
   1: 00000000:1435 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 222 1 0000000000000000 100 0 0 10 0
   2: 0100007F:A1B2 0100007F:0BB8 01 00000000:00000000 00:00000000 00000000  1000        0 333 1 0000000000000000 20 4 30 10 -1
   3: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 0 1 0000000000000000 100 0 0 10 0
";

const TCP6: &str = "\
  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000001000000:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 444 1 0000000000000000 100 0 0 10 0
   1: 00470626B5871001DE997DD8310D35D3:C650 60480120000442480000000000000000:01BB 01 00000000:00000000 02:000008AF 00000000  1000        0 555 2 0000000000000000 26 4 25 10 -1
";

#[test]
fn ipv4_listen_lines_only() {
    // Header skipped, ESTABLISHED (01) skipped, inode 0 (no owner) skipped.
    assert_eq!(parse_listening(TCP), vec![(3000, 111), (5173, 222)]);
}

#[test]
fn ipv6_listen_lines_only() {
    assert_eq!(parse_listening(TCP6), vec![(8080, 444)]);
}

#[test]
fn malformed_lines_are_skipped() {
    let table = "\
garbage
   0: 0100007F 00000000:0000 0A 0 0 0 0 0 777
   1: 0100007G:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000 0 0 778
   2: 0100007F:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000 0 0 notanumber
   3: 0100007F:0BB 00000000:0000 0A 00000000:00000000 00:00000000 00000000 0 0 779
   4: 0100:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000 0 0 780
   5: 0100007F:0000 00000000:0000 0A 00000000:00000000 00:00000000 00000000 0 0 781
   6: 0100007F:0BB8 00000000:0000 0A
0100007F:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000 0 0 782 1
   7: 0100007F:FFFF 00000000:0000 0A 00000000:00000000 00:00000000 00000000 0 0 783 1
";
    assert_eq!(parse_listening(table), vec![(65535, 783)]);
    assert!(parse_listening("").is_empty());
}

#[test]
fn ppid_survives_spaces_and_parentheses_in_the_command_name() {
    assert_eq!(
        parse_ppid("1317108 (cat) R 1317101 1317108 1317101 0"),
        Some(1317101)
    );
    assert_eq!(parse_ppid("42 (my (odd) prog) S 7 42 42 0 -1"), Some(7));
    assert_eq!(parse_ppid("42 (سيرفر تجربة) S 9 42"), Some(9));
    assert_eq!(parse_ppid("42 (no close S 7"), None);
    assert_eq!(parse_ppid("42 (x) S"), None);
    assert_eq!(parse_ppid("42 (x) S abc"), None);
}

#[test]
fn socket_links() {
    assert_eq!(socket_inode("socket:[111]"), Some(111));
    assert_eq!(socket_inode("pipe:[111]"), None);
    assert_eq!(socket_inode("socket:[]"), None);
    assert_eq!(socket_inode("/dev/pts/3"), None);
    assert_eq!(socket_inode("socket:[12"), None);
}

#[test]
fn descendants_follow_the_tree_and_stop_at_the_limit() {
    // 10 ─┬─ 11 ── 13
    //     └─ 12
    // 20 ── 21        (another pane)
    // 13 claims to be its own parent's parent: no loop.
    let procs = [
        (1, 0),
        (10, 1),
        (11, 10),
        (12, 10),
        (13, 11),
        (20, 1),
        (21, 20),
        (10, 13),
    ];
    let mut d = descendants(&procs, 10, 100);
    d.sort_unstable();
    assert_eq!(d, vec![10, 11, 12, 13]);
    assert_eq!(descendants(&procs, 20, 100), vec![20, 21]);
    assert_eq!(descendants(&procs, 99, 100), vec![99]);
    assert_eq!(descendants(&procs, 10, 2).len(), 2);
    // A process that is its own parent (pid 0 style) does not repeat.
    assert_eq!(descendants(&[(5, 5)], 5, 10), vec![5]);
    // A huge flat tree stays bounded.
    let wide: Vec<(u32, u32)> = (2..10_000).map(|p| (p, 1)).collect();
    assert_eq!(
        descendants(&wide, 1, MAX_DESCENDANTS).len(),
        MAX_DESCENDANTS
    );
}

#[test]
fn port_lines() {
    assert_eq!(format_ports(&[]), "");
    assert_eq!(format_ports(&[5173, 3000, 3000]), ":3000 :5173");
    assert_eq!(
        format_ports(&[9000, 8080, 3000, 5173, 22, 443]),
        ":22 :443 :3000 :5173 +2"
    );
    assert_eq!(format_ports(&[1, 2, 3, 4]), ":1 :2 :3 :4");
}

/// A fake `/proc`.
#[derive(Clone, Default)]
struct FakeProc {
    tables: Vec<String>,
    stats: HashMap<u32, String>,
    fds: HashMap<u32, Vec<String>>,
}

impl FakeProc {
    fn process(mut self, pid: u32, ppid: u32, fds: &[&str]) -> Self {
        self.stats
            .insert(pid, format!("{pid} (p {pid}) S {ppid} 1 1 0"));
        self.fds
            .insert(pid, fds.iter().map(|s| s.to_string()).collect());
        self
    }
}

impl ProcSource for FakeProc {
    fn tcp_tables(&self) -> Vec<String> {
        self.tables.clone()
    }
    fn pids(&self, limit: usize) -> Vec<u32> {
        let mut pids: Vec<u32> = self.stats.keys().copied().collect();
        pids.sort_unstable();
        pids.truncate(limit);
        pids
    }
    fn stat(&self, pid: u32) -> Option<String> {
        self.stats.get(&pid).cloned()
    }
    fn fd_links(&self, pid: u32, limit: usize) -> Vec<String> {
        let mut fds = self.fds.get(&pid).cloned().unwrap_or_default();
        fds.truncate(limit);
        fds
    }
}

fn fixture() -> FakeProc {
    FakeProc {
        tables: vec![TCP.into(), TCP6.into()],
        ..Default::default()
    }
    // Pane A: shell 100 → npm 101 → node 102 (listens on 3000 and 5173).
    .process(100, 1, &["/dev/pts/1", "/dev/pts/1"])
    .process(101, 100, &["pipe:[9]"])
    .process(102, 101, &["socket:[111]", "socket:[222]", "socket:[333]"])
    // Pane B: shell 200 with a python server on 8080 (IPv6).
    .process(200, 1, &[])
    .process(201, 200, &["socket:[444]"])
    // Not a pane's descendant: a daemon that also holds socket 222.
    .process(300, 1, &["socket:[222]"])
}

#[test]
fn scan_attributes_ports_to_the_pane_that_started_them() {
    // A child of pane A's shell whose stat cannot be read: skipped, so its socket (8080) is
    // not attributed to pane A.
    let mut proc = fixture().process(400, 100, &["socket:[444]"]);
    proc.stats.remove(&400);
    let result = scan(&proc, &[("a", 100), ("b", 200), ("c", 999)]);
    assert_eq!(result["a"], vec![3000, 5173]);
    assert_eq!(result["b"], vec![8080]);
    assert_eq!(result["c"], Vec::<u16>::new(), "every root is answered");
    assert_eq!(result.len(), 3);
}

#[test]
fn scan_without_listeners_or_roots_reads_no_processes() {
    struct Strict;
    impl ProcSource for Strict {
        fn tcp_tables(&self) -> Vec<String> {
            vec![TCP.lines().next().unwrap().to_string()]
        }
        fn pids(&self, _: usize) -> Vec<u32> {
            panic!("no listening sockets: processes need not be read")
        }
        fn stat(&self, _: u32) -> Option<String> {
            unreachable!()
        }
        fn fd_links(&self, _: u32, _: usize) -> Vec<String> {
            unreachable!()
        }
    }
    assert_eq!(scan(&Strict, &[(1u8, 100)])[&1], Vec::<u16>::new());
    assert!(scan::<u8>(&Strict, &[]).is_empty());
}

#[test]
fn scan_fd_limit_is_passed_to_the_source() {
    let mut proc = fixture();
    let many: Vec<String> = (0..5000)
        .map(|i| format!("socket:[{}]", 10_000 + i))
        .collect();
    proc.fds.get_mut(&102).unwrap().splice(0..0, many);
    // The listening sockets come after more than MAX_FDS other descriptors: not looked at.
    assert_eq!(scan(&proc, &[("a", 100)])["a"], Vec::<u16>::new());
}

#[test]
fn watcher_scans_while_watching_and_sends_only_changes() {
    let (tx, rx) = mpsc::channel();
    let mut watcher = PortWatcher::spawn(fixture(), Duration::from_millis(20), move |ports| {
        tx.send(ports).is_ok()
    })
    .unwrap();
    // Nothing to watch: no scan.
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    watcher.watch(vec![("a", 100)]);
    let first = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(first["a"], vec![3000, 5173]);
    // Same result on the next scans: nothing more is sent.
    assert!(rx.recv_timeout(Duration::from_millis(150)).is_err());
    watcher.watch(vec![("a", 100), ("b", 200)]);
    let second = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(second["b"], vec![8080]);
    // Paused (the sidebar was hidden).
    watcher.watch(Vec::new());
    assert!(watcher.watching().is_empty());
    assert!(rx.recv_timeout(Duration::from_millis(150)).is_err());
    drop(watcher);
}

#[test]
fn sidebar_entry_gets_a_ports_line_next_to_an_arabic_title() {
    let mut fs = FontSystem::new();
    let palette = Palette::default();
    let mut ctx = DrawContext {
        font_system: &mut fs,
        family: None,
        palette: &palette,
        metrics: CellMetrics {
            font_size: 14.0,
            scale: 1.0,
            char_width: 9.0,
            line_height: 22.0,
            padding_left: 8.0,
            padding_top: 6.0,
        },
        is_srgb: false,
    };
    let plain = Entry::new(
        "سيرفر التطوير",
        Some(Path::new("/srv/مشروع")),
        true,
        None,
        [],
    );
    let with_ports = plain.clone().with_ports(vec![3000, 5173, 8080, 9000, 9229]);
    assert_eq!(with_ports.height(22.0) - plain.height(22.0), 22.0);
    let entries = [with_ports, plain];
    let area = Rect::new(500.0, 0.0, 320.0, 400.0);
    let view = ScrollState::default().view(&entries, 0, area, 22.0, SidebarPosition::Right);
    // The second entry starts below the first one's extra line.
    assert_eq!(view.items[1].rect.y, entries[0].height(22.0));
    let scene = build(&mut ctx, &view, &mut LabelCache::default());
    let texts: Vec<String> = scene
        .labels
        .iter()
        .map(|l| l.buffer.lines[0].text().to_string())
        .collect();
    let ports = texts
        .iter()
        .position(|t| t == ":3000 :5173 :8080 :9000 +1")
        .expect("ports line");
    let title = texts.iter().position(|t| t == "سيرفر التطوير").unwrap();
    let (p, t) = (&scene.labels[ports], &scene.labels[title]);
    assert!(p.top > t.top, "ports below the title");
    // Right sidebar: both end at the right padding.
    let right = |l: &twitty::render::scene::sidebar::Label| {
        l.left + l.buffer.layout_runs().next().unwrap().line_w
    };
    assert!((right(p) - (area.right() - 8.0)).abs() < 0.1);
    assert!(t.buffer.layout_runs().next().unwrap().rtl);
    // Only one ports line in the whole scene (the second tab has none).
    assert_eq!(texts.iter().filter(|t| t.starts_with(':')).count(), 1);
}

#[cfg(target_os = "linux")]
#[test]
fn real_proc_finds_a_socket_this_process_listens_on() {
    use twitty::ports::SystemProc;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let me = std::process::id();
    let found = scan(&SystemProc, &[("me", me)]);
    assert!(
        found["me"].contains(&port),
        "{port} not in {:?}",
        found["me"]
    );
    drop(listener);
    let found = scan(&SystemProc, &[("me", me)]);
    assert!(!found["me"].contains(&port));
}
