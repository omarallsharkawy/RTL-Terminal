//! Listening TCP ports of the processes started from each pane, for the sidebar
//! (a dev server on 3000 shows as `:3000` under its tab).
//!
//! On Linux the scan reads `/proc/net/tcp{,6}` for sockets in the LISTEN state, builds the
//! process tree from `/proc/<pid>/stat`, and maps the sockets to the descendants of each
//! pane's shell through their `/proc/<pid>/fd/*` links (`socket:[inode]`). Other platforms
//! report nothing.
//!
//! The scan runs on a [`PortWatcher`] thread, never on the event loop, and every step is
//! bounded: processes that cannot be read are skipped, and the numbers of processes, of
//! descendants per pane and of descriptors per process looked at are capped. The parsing
//! functions are pure so they are tested with fixture text.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::sync::mpsc;
use std::time::Duration;

/// How often the ports are scanned while the sidebar is shown.
pub const SCAN_INTERVAL: Duration = Duration::from_secs(2);
/// Most ports listed for one tab before the rest are counted as `+N`.
pub const MAX_SHOWN: usize = 4;
/// Most processes read from `/proc` in one scan.
pub const MAX_PROCESSES: usize = 8192;
/// Most descendants of one shell looked at.
pub const MAX_DESCENDANTS: usize = 256;
/// Most file descriptors of one process looked at.
pub const MAX_FDS: usize = 1024;

/// TCP state `0A` in `/proc/net/tcp`.
const TCP_LISTEN: &str = "0A";

/// `(port, socket inode)` of every listening socket in a `/proc/net/tcp` or `/proc/net/tcp6`
/// table. The header, other states and malformed lines are skipped.
pub fn parse_listening(table: &str) -> Vec<(u16, u64)> {
    table
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            // sl local rem st tx:rx tr:when retrnsmt uid timeout inode ...
            if fields.len() < 10 || !fields[0].ends_with(':') || fields[3] != TCP_LISTEN {
                return None;
            }
            let (address, port) = fields[1].rsplit_once(':')?;
            if !matches!(address.len(), 8 | 32) || !address.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            if port.len() != 4 {
                return None;
            }
            let port = u16::from_str_radix(port, 16).ok().filter(|&p| p != 0)?;
            let inode: u64 = fields[9].parse().ok().filter(|&i| i != 0)?;
            Some((port, inode))
        })
        .collect()
}

/// The parent pid in the text of `/proc/<pid>/stat`. The command name sits in parentheses
/// and may itself contain spaces and parentheses, so the fields are read after the last `)`.
pub fn parse_ppid(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut fields = rest.split_whitespace();
    let _state = fields.next()?;
    fields.next()?.parse().ok()
}

/// The inode of a `socket:[inode]` descriptor link.
pub fn socket_inode(link: &str) -> Option<u64> {
    link.strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

/// `root` and its descendants in a `(pid, ppid)` table, breadth first, at most `limit`.
pub fn descendants(processes: &[(u32, u32)], root: u32, limit: usize) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(pid, ppid) in processes {
        if pid != ppid {
            children.entry(ppid).or_default().push(pid);
        }
    }
    let mut out = vec![root];
    let mut seen = HashSet::from([root]);
    let mut next = 0;
    while next < out.len() && out.len() < limit {
        let pid = out[next];
        next += 1;
        for &child in children.get(&pid).into_iter().flatten() {
            if out.len() >= limit {
                break;
            }
            if seen.insert(child) {
                out.push(child);
            }
        }
    }
    out.truncate(limit);
    out
}

/// The sidebar line for a tab's ports: sorted, without repeats, at most [`MAX_SHOWN`] and then
/// `+N` for the rest. Empty when there are none.
pub fn format_ports(ports: &[u16]) -> String {
    format_ports_limit(ports, MAX_SHOWN)
}

pub fn format_ports_limit(ports: &[u16], limit: usize) -> String {
    let mut ports = ports.to_vec();
    ports.sort_unstable();
    ports.dedup();
    let mut text = ports
        .iter()
        .take(limit)
        .map(|p| format!(":{p}"))
        .collect::<Vec<_>>()
        .join(" ");
    if ports.len() > limit {
        text.push_str(&format!(" +{}", ports.len() - limit));
    }
    text
}

/// Where a scan reads process information from: `/proc` on Linux, fixtures in tests.
pub trait ProcSource {
    /// The text of each TCP table (`/proc/net/tcp`, `/proc/net/tcp6`).
    fn tcp_tables(&self) -> Vec<String>;
    /// Pids of running processes, at most `limit`.
    fn pids(&self, limit: usize) -> Vec<u32>;
    /// The text of `/proc/<pid>/stat`, if readable.
    fn stat(&self, pid: u32) -> Option<String>;
    /// Targets of the descriptor links of `pid`, at most `limit`; empty if unreadable.
    fn fd_links(&self, pid: u32, limit: usize) -> Vec<String>;
}

/// Listening ports of each root process and its descendants, sorted and without repeats.
/// Every key of `roots` is in the result, with no ports when nothing is listening.
pub fn scan<K: Copy + Eq + Hash>(
    source: &impl ProcSource,
    roots: &[(K, u32)],
) -> HashMap<K, Vec<u16>> {
    let mut out: HashMap<K, Vec<u16>> = roots.iter().map(|&(k, _)| (k, Vec::new())).collect();
    if roots.is_empty() {
        return out;
    }
    let listening: HashMap<u64, u16> = source
        .tcp_tables()
        .iter()
        .flat_map(|t| parse_listening(t))
        .map(|(port, inode)| (inode, port))
        .collect();
    if listening.is_empty() {
        return out;
    }
    let processes: Vec<(u32, u32)> = source
        .pids(MAX_PROCESSES)
        .into_iter()
        .filter_map(|pid| Some((pid, parse_ppid(&source.stat(pid)?)?)))
        .collect();
    let mut per_process: HashMap<u32, Vec<u16>> = HashMap::new();
    for &(key, root) in roots {
        let ports = out.entry(key).or_default();
        for pid in descendants(&processes, root, MAX_DESCENDANTS) {
            let found = per_process.entry(pid).or_insert_with(|| {
                source
                    .fd_links(pid, MAX_FDS)
                    .iter()
                    .filter_map(|link| listening.get(&socket_inode(link)?).copied())
                    .collect()
            });
            ports.extend_from_slice(found);
        }
        ports.sort_unstable();
        ports.dedup();
    }
    out
}

/// The real `/proc` (Linux only; elsewhere every call finds nothing).
pub struct SystemProc;

#[cfg(target_os = "linux")]
impl ProcSource for SystemProc {
    fn tcp_tables(&self) -> Vec<String> {
        ["/proc/net/tcp", "/proc/net/tcp6"]
            .iter()
            .filter_map(|path| read_bounded(path, 4 << 20))
            .collect()
    }

    fn pids(&self, limit: usize) -> Vec<u32> {
        let Ok(dir) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
            .take(limit)
            .collect()
    }

    fn stat(&self, pid: u32) -> Option<String> {
        read_bounded(&format!("/proc/{pid}/stat"), 4096)
    }

    fn fd_links(&self, pid: u32, limit: usize) -> Vec<String> {
        let Ok(dir) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
            return Vec::new();
        };
        dir.take(limit)
            .filter_map(|e| {
                let link = std::fs::read_link(e.ok()?.path()).ok()?;
                Some(link.to_str()?.to_owned())
            })
            .collect()
    }
}

#[cfg(not(target_os = "linux"))]
impl ProcSource for SystemProc {
    fn tcp_tables(&self) -> Vec<String> {
        Vec::new()
    }
    fn pids(&self, _: usize) -> Vec<u32> {
        Vec::new()
    }
    fn stat(&self, _: u32) -> Option<String> {
        None
    }
    fn fd_links(&self, _: u32, _: usize) -> Vec<String> {
        Vec::new()
    }
}

/// At most `max` bytes of a file, as text.
#[cfg(target_os = "linux")]
fn read_bounded(path: &str, max: u64) -> Option<String> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(max)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// Scans ports on its own thread while it has roots to watch, and hands each changed result
/// to `deliver`. Dropping it stops the thread.
pub struct PortWatcher<K> {
    tx: mpsc::Sender<Vec<(K, u32)>>,
    watching: Vec<(K, u32)>,
}

impl<K: Copy + Eq + Hash + Send + 'static> PortWatcher<K> {
    /// Start the thread. `deliver` returns false once nobody listens any more, which ends it.
    pub fn spawn(
        source: impl ProcSource + Send + 'static,
        interval: Duration,
        mut deliver: impl FnMut(HashMap<K, Vec<u16>>) -> bool + Send + 'static,
    ) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Vec<(K, u32)>>();
        std::thread::Builder::new()
            .name("twitty-ports".into())
            .spawn(move || {
                let mut roots: Vec<(K, u32)> = Vec::new();
                let mut last: Option<HashMap<K, Vec<u16>>> = None;
                loop {
                    // Idle (nothing to watch): sleep until told otherwise.
                    let next = if roots.is_empty() {
                        rx.recv().map_err(|_| ())
                    } else {
                        match rx.recv_timeout(interval) {
                            Ok(roots) => Ok(roots),
                            Err(mpsc::RecvTimeoutError::Timeout) => Ok(roots.clone()),
                            Err(mpsc::RecvTimeoutError::Disconnected) => Err(()),
                        }
                    };
                    let Ok(mut next) = next else { return };
                    // Only the latest of several queued updates matters.
                    while let Ok(newer) = rx.try_recv() {
                        next = newer;
                    }
                    roots = next;
                    if roots.is_empty() {
                        continue;
                    }
                    let result = scan(&source, &roots);
                    if last.as_ref() != Some(&result) {
                        if !deliver(result.clone()) {
                            return;
                        }
                        last = Some(result);
                    }
                }
            })?;
        Ok(Self {
            tx,
            watching: Vec::new(),
        })
    }

    /// Watch these `(key, shell pid)` roots; an empty list pauses the scanning. Nothing is
    /// sent when the list is unchanged.
    pub fn watch(&mut self, roots: Vec<(K, u32)>) {
        if roots != self.watching {
            self.watching = roots.clone();
            let _ = self.tx.send(roots);
        }
    }

    /// The roots being watched (empty while paused).
    pub fn watching(&self) -> &[(K, u32)] {
        &self.watching
    }
}
