//! Event-loop scheduling for the focused pane's bounded, off-thread Git probe.

use std::time::{Duration, Instant};

use super::{App, AppEvent};
use crate::layout::PaneId;
use crate::session::workspace::Workspace;
use crate::ui::git_diff::{scan, DiffStat, WorkingDirectory};

const INTERVAL: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Target {
    pub pane: PaneId,
    directory: WorkingDirectory,
}

pub(super) fn target(ws: &Workspace) -> Option<Target> {
    let id = ws.focused_id()?;
    let pane = ws.pane(id)?;
    if pane.remote.is_some() {
        return None;
    }
    let directory = pane
        .meta
        .cwd
        .clone()
        .map(WorkingDirectory::Reported)
        .or_else(|| {
            let pid = pane.pty.as_ref()?.pid()?;
            cfg!(target_os = "linux").then_some(WorkingDirectory::Process(pid))
        })?;
    Some(Target {
        pane: id,
        directory,
    })
}

#[derive(Default)]
pub(super) struct GitDiffWatch {
    target: Option<Target>,
    due: Option<Instant>,
    generation: u64,
    in_flight: bool,
    worker: Option<std::sync::mpsc::Sender<(u64, WorkingDirectory)>>,
}

impl GitDiffWatch {
    pub fn refresh(&mut self, id: PaneId) {
        if self.target.as_ref().is_some_and(|target| target.pane == id) {
            self.due = Some(Instant::now());
            self.generation = self.generation.wrapping_add(1);
        }
    }

    pub fn stop(&mut self) {
        self.worker = None;
        self.in_flight = false;
    }

    fn ensure_worker(
        &mut self,
        proxy: &winit::event_loop::EventLoopProxy<AppEvent>,
    ) -> Option<&std::sync::mpsc::Sender<(u64, WorkingDirectory)>> {
        if self.worker.is_none() {
            let (tx, rx) = std::sync::mpsc::channel::<(u64, WorkingDirectory)>();
            let proxy = proxy.clone();
            if std::thread::Builder::new()
                .name("twitty-git-diff".into())
                .spawn(move || {
                    while let Ok(mut job) = rx.recv() {
                        // Drain queued probes so only the latest target is scanned.
                        while let Ok(newer) = rx.try_recv() {
                            job = newer;
                        }
                        let (generation, directory) = job;
                        let stat = scan(directory);
                        if proxy
                            .send_event(AppEvent::GitDiff(generation, stat))
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .is_ok()
            {
                self.worker = Some(tx);
            }
        }
        self.worker.as_ref()
    }
}

impl App {
    pub(super) fn poll_git_diff(&mut self, now: Instant) -> Option<Instant> {
        let target = self
            .window
            .diff_visible(self.ws.tabs().len())
            .then(|| target(&self.ws))
            .flatten();
        let watch = &mut self.git_diff;
        if watch.target != target {
            watch.target = target;
            watch.due = watch.target.as_ref().map(|_| now);
            watch.generation = watch.generation.wrapping_add(1);
            self.window.set_diff(None);
        }
        if watch.in_flight {
            return None;
        }
        if let Some((target, _)) = watch
            .target
            .as_ref()
            .zip(watch.due.filter(|&due| now >= due))
        {
            let directory = target.directory.clone();
            let generation = watch.generation;
            watch.due = Some(now + INTERVAL);
            if let Some(tx) = watch.ensure_worker(&self.proxy) {
                if tx.send((generation, directory)).is_ok() {
                    watch.in_flight = true;
                } else {
                    watch.worker = None;
                }
            }
        }
        if watch.in_flight {
            None
        } else {
            watch.due
        }
    }

    pub(super) fn on_git_diff(&mut self, generation: u64, stat: Option<DiffStat>) {
        let watch = &mut self.git_diff;
        watch.in_flight = false;
        if generation == watch.generation
            && self.window.diff_visible(self.ws.tabs().len())
            && watch.target == target(&self.ws)
        {
            let stat = watch.target.clone().zip(stat);
            self.window.set_diff(stat);
            if watch.due.is_some_and(|due| due <= Instant::now()) {
                watch.due = Some(Instant::now() + INTERVAL);
            }
            self.refresh_pointer_hover();
        }
    }
}
