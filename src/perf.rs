//! Opt-in timing for the event loop and renderer.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

const SAMPLE_COUNT: usize = 120;
const REPORT_INTERVAL: Duration = Duration::from_secs(3);

pub(crate) struct Perf {
    started: Instant,
    next_report: Instant,
    gpu_init: Duration,
    font_db: Duration,
    samples: VecDeque<(Duration, Duration)>,
    frames: usize,
    first_frame: bool,
}

impl Perf {
    pub fn from_env() -> Option<Self> {
        if std::env::var_os("TWITTY_PERF").as_deref() != Some(std::ffi::OsStr::new("1")) {
            return None;
        }
        let started = Instant::now();
        Some(Self {
            started,
            next_report: started + REPORT_INTERVAL,
            gpu_init: Duration::ZERO,
            font_db: Duration::ZERO,
            samples: VecDeque::with_capacity(SAMPLE_COUNT),
            frames: 0,
            first_frame: true,
        })
    }

    pub fn startup_phases(&mut self, gpu_init: Duration, font_db: Duration) {
        self.gpu_init = gpu_init;
        self.font_db = font_db;
    }

    pub fn record_frame(&mut self, build: Duration, render: Duration) {
        if self.first_frame {
            self.first_frame = false;
            eprintln!(
                "twitty perf startup gpu_init_ms={:.3} font_db_ms={:.3} first_frame_ms={:.3}",
                ms(self.gpu_init),
                ms(self.font_db),
                ms(self.started.elapsed()),
            );
        }
        if self.samples.len() == SAMPLE_COUNT {
            self.samples.pop_front();
        }
        self.samples.push_back((build, render));
        self.frames += 1;
    }

    /// Periodic sampling also runs on idle windows, without requesting a frame.
    pub fn poll(&mut self, now: Instant) -> Instant {
        if now >= self.next_report {
            self.next_report = now + REPORT_INTERVAL;
            let rss = rss_kib().map_or_else(|| "n/a".into(), |kb| kb.to_string());
            let build = percentiles(self.samples.iter().map(|&(build, _)| build));
            let render = percentiles(self.samples.iter().map(|&(_, render)| render));
            match (build, render) {
                (Some((b50, b95)), Some((r50, r95))) => eprintln!(
                    "twitty perf frames={} samples={} build_ms_p50={:.3} build_ms_p95={:.3} render_ms_p50={:.3} render_ms_p95={:.3} rss_kib={rss}",
                    self.frames, self.samples.len(), ms(b50), ms(b95), ms(r50), ms(r95),
                ),
                _ => eprintln!("twitty perf frames=0 samples=0 rss_kib={rss}"),
            }
            self.frames = 0;
        }
        self.next_report
    }
}

fn percentiles(samples: impl Iterator<Item = Duration>) -> Option<(Duration, Duration)> {
    let mut values: Vec<_> = samples.collect();
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let at = |p: usize| values[(p * values.len()).div_ceil(100) - 1];
    Some((at(50), at(95)))
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[cfg(target_os = "linux")]
fn rss_kib() -> Option<usize> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages = statm.split_whitespace().nth(1)?.parse::<usize>().ok()?;
    pages
        .checked_mul(rustix::param::page_size())
        .map(|bytes| bytes / 1024)
}

#[cfg(not(target_os = "linux"))]
fn rss_kib() -> Option<usize> {
    None
}
