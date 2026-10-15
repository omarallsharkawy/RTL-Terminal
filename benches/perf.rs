//! Throughput and latency of the CPU side: parsing PTY output, snapshotting the grid, shaping
//! rows and building whole frames. Run with `cargo bench --bench perf`.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use twitty::color::Palette;
use twitty::layout::{PaneId, Rect};
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::frame::{build_frame, FrameOptions};
use twitty::render::scene::preedit::PreeditState;
use twitty::render::text::fonts::new_font_system;
use twitty::render::{CellMetrics, PaneFrame, RowCache, RowShaper};
use twitty::terminal::Terminal;

const COLS: usize = 200;
const ROWS: usize = 60;
/// What the PTY reader hands the terminal at a time.
const CHUNK: usize = 8192;

const ARABIC: [&str; 6] = [
    "مرحبا بالعالم",
    "تم حفظ الملف",
    "الإصدار",
    "خطأ في الاتصال",
    "اختبار",
    "عَرَبِيّ",
];
const LATIN: [&str; 6] = ["commit", "7abaf40", "v3.0.0", "error:", "build", "2026"];
const LOG_WORDS: [&str; 7] = [
    "request",
    "/api/v1/items",
    "took",
    "ms",
    "ok",
    "cache",
    "miss",
];

/// Cheap deterministic generator, so every run feeds the same bytes.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as usize
    }
}

/// `len` bytes of plain log lines (timestamps, levels, paths), like `cat build.log`.
fn ascii_log(len: usize) -> Vec<u8> {
    let mut rng = Lcg(1);
    let levels = ["INFO", "WARN", "DEBUG", "ERROR"];
    let mut out = String::with_capacity(len + 256);
    let mut n = 0;
    while out.len() < len {
        n += 1;
        out.push_str(&format!(
            "2026-10-16T12:{:02}:{:02}.{:03}Z {} [worker-{}] ",
            n / 60 % 60,
            n % 60,
            n % 1000,
            levels[rng.next() % 4],
            rng.next() % 16
        ));
        for _ in 0..4 + rng.next() % 14 {
            out.push_str(LOG_WORDS[rng.next() % LOG_WORDS.len()]);
            out.push(' ');
        }
        out.push_str("\r\n");
    }
    out.truncate(len);
    out.into_bytes()
}

/// `len` bytes of mixed Arabic/English lines with SGR colors, like `git log` with Arabic
/// subjects.
fn mixed_sgr(len: usize) -> Vec<u8> {
    let mut rng = Lcg(2);
    let mut out = String::with_capacity(len + 256);
    while out.len() < len {
        for _ in 0..4 + rng.next() % 12 {
            let color = 31 + rng.next() % 6;
            let word = if rng.next().is_multiple_of(2) {
                ARABIC[rng.next() % ARABIC.len()]
            } else {
                LATIN[rng.next() % LATIN.len()]
            };
            out.push_str(&format!("\x1b[{color}m{word}\x1b[0m "));
        }
        out.push_str("\r\n");
    }
    // Never cut a UTF-8 sequence.
    let mut end = len.min(out.len());
    while !out.is_char_boundary(end) {
        end -= 1;
    }
    out.truncate(end);
    out.into_bytes()
}

/// A terminal whose screen is full of mixed text.
fn full_terminal() -> Terminal {
    let mut term = Terminal::new(COLS, ROWS, |_| {});
    term.process_bytes(&mixed_sgr(64 * 1024));
    term
}

fn feed(term: &mut Terminal, bytes: &[u8]) {
    for chunk in bytes.chunks(CHUNK) {
        term.process_bytes(chunk);
    }
}

fn bench_process(c: &mut Criterion) {
    let mut group = c.benchmark_group("process_bytes");
    group.sample_size(10);
    let ascii = ascii_log(10 * 1024 * 1024);
    group.throughput(Throughput::Bytes(ascii.len() as u64));
    group.bench_function("ascii_log_10mb", |b| {
        b.iter_batched_ref(
            || Terminal::new(COLS, ROWS, |_| {}),
            |term| feed(term, black_box(&ascii)),
            BatchSize::LargeInput,
        )
    });
    let mixed = mixed_sgr(2 * 1024 * 1024);
    group.throughput(Throughput::Bytes(mixed.len() as u64));
    group.bench_function("mixed_arabic_sgr_2mb", |b| {
        b.iter_batched_ref(
            || Terminal::new(COLS, ROWS, |_| {}),
            |term| feed(term, black_box(&mixed)),
            BatchSize::LargeInput,
        )
    });
    group.finish();
}

fn bench_snapshot(c: &mut Criterion) {
    let term = full_terminal();
    c.bench_function("snapshot/200x60", |b| b.iter(|| black_box(term.snapshot())));
}

fn metrics(font_system: &mut cosmic_text::FontSystem) -> CellMetrics {
    CellMetrics::measure(font_system, None, 14.0, 1.0)
}

fn bench_build_row(c: &mut Criterion) {
    let mut font_system = new_font_system();
    let palette = Palette::default();
    let m = metrics(&mut font_system);
    let row_of = |text: &str| {
        let mut term = Terminal::new(COLS, 2, |_| {});
        term.process_bytes(text.as_bytes());
        term.snapshot().0.remove(0)
    };
    let mixed = row_of(
        "\x1b[33mcommit 7abaf40\x1b[0m تم حفظ الملف في المجلد v3.0.0 بنجاح  \x1b[32mok\x1b[0m \
         مرحبا بالعالم 2026 build خطأ في الاتصال",
    );
    let ascii = row_of(
        "2026-10-16T12:00:00.000Z INFO [worker-3] request /api/v1/items took 12 ms ok cache \
         miss request /api/v1/items took 7 ms",
    );
    let mut group = c.benchmark_group("build_row");
    for (name, line) in [("mixed_arabic", &mixed), ("ascii", &ascii)] {
        group.bench_function(name, |b| {
            b.iter(|| {
                let mut shaper = RowShaper {
                    font_system: &mut font_system,
                    palette: &palette,
                    font_px: m.font_px(),
                    char_width: m.char_width,
                    line_height: m.line_height,
                    family: None,
                };
                black_box(shaper.build_row(black_box(line), None))
            })
        });
    }
    group.finish();
}

fn bench_frame(c: &mut Criterion) {
    let mut font_system = new_font_system();
    let palette = Palette::default();
    let m = metrics(&mut font_system);
    let surface = (
        COLS as f32 * m.char_width + 2.0 * m.padding_left,
        ROWS as f32 * m.line_height + 2.0 * m.padding_top,
    );
    let rect = Rect::new(0.0, 0.0, surface.0, surface.1);
    let opts = FrameOptions {
        window_focused: true,
        cursor_style: "block",
        cursor_blink_on: true,
        surface,
    };
    let pane = |term: &Terminal| {
        let (lines, cursor) = term.snapshot();
        PaneFrame {
            id: PaneId(1),
            rect,
            lines,
            cursor,
            focused: true,
            status: None,
        }
    };
    let mut ime = PreeditState::default();
    let mut run =
        |font_system: &mut cosmic_text::FontSystem, cache: &mut RowCache, panes: &[PaneFrame]| {
            let mut ctx = DrawContext {
                font_system,
                family: None,
                palette: &palette,
                metrics: m,
                is_srgb: true,
            };
            black_box(build_frame(
                &mut ctx,
                &opts,
                cache,
                &mut ime,
                &[],
                panes,
                &[],
            ));
        };

    let mut term = full_terminal();
    let mut group = c.benchmark_group("frame_200x60");
    group.sample_size(20);
    // Every row shaped from scratch (first frame, font change).
    let panes = [pane(&term)];
    group.bench_function("cold", |b| {
        b.iter(|| run(&mut font_system, &mut RowCache::default(), &panes))
    });
    // Nothing changed since the last frame (cursor blink, focus change): snapshot + build.
    let mut cache = RowCache::default();
    run(&mut font_system, &mut cache, &panes);
    group.bench_function("idle", |b| {
        b.iter(|| {
            let panes = [pane(&term)];
            run(&mut font_system, &mut cache, &panes)
        })
    });
    // One new line of output scrolls the whole screen up (`cat`, build logs).
    let text = mixed_sgr(16 * 1024);
    let mut lines = text.split_inclusive(|&b| b == b'\n').cycle();
    group.bench_function("scroll_one_line", |b| {
        b.iter(|| {
            term.process_bytes(lines.next().unwrap_or_default());
            let panes = [pane(&term)];
            run(&mut font_system, &mut cache, &panes)
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_process,
    bench_snapshot,
    bench_build_row,
    bench_frame
);
criterion_main!(benches);
