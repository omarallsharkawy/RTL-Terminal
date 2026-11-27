//! Throughput and latency of the CPU side: parsing PTY output, snapshotting the grid, shaping
//! rows and building whole frames. Run with `cargo bench --bench perf`.
//!
//! Set `TWITTY_PERF_STARTUP=1` to build this workspace once and time the first completed
//! offscreen frame, including pixel readback. Run in a fresh process for each sample.
//! The live window uses `TWITTY_PERF=1` for timing and RSS reports.

use std::hint::black_box;
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use criterion::{BatchSize, Criterion, Throughput};
use twitty::blocks::{BlockDecor, BlockStatus};
use twitty::color::Palette;
use twitty::layout::{PaneId, Rect, SidebarPosition};
use twitty::pty::schedule::{OutputSchedule, CHUNK_BYTES};
use twitty::pty::OutputQueue;
use twitty::render::gpu::offscreen::Offscreen;
use twitty::render::icons::Icon;
use twitty::render::scene::composer::ComposerView;
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::frame::{build_frame, build_frame_with_cards, FrameOptions};
use twitty::render::scene::palette::{PaletteIcons, PaletteItem, PaletteView};
use twitty::render::scene::preedit::PreeditState;
use twitty::render::scene::sidebar::LabelCache;
use twitty::render::scene::window_chrome::{ChromeView, PaneHeaderView};
use twitty::render::text::fonts::new_font_system;
use twitty::render::text::labels::{LabelCache as UiLabelCache, UiLabels};
use twitty::render::text::ui::UiFont;
use twitty::render::ui_style::UiStyle;
use twitty::render::{CellMetrics, PaneFrame, RowCache, RowShaper};
use twitty::sidebar::cards::ScrollState;
use twitty::sidebar::Entry;
use twitty::terminal::Terminal;
use twitty::ui::chrome::{PaneHeaderLayout, TopBarLayout, UiLanguage, WindowLayout};
use twitty::ui::git_diff::DiffStat;
use twitty::ui::search_all::{search_all, PaneInfo, Query};

const COLS: usize = 200;
const ROWS: usize = 60;
/// Fixed chunk size for reproducible parser throughput samples.
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

/// A reader thread feeding the bounded PTY queue and a window-free event loop draining it.
fn feed_scheduled(term: &mut Terminal, bytes: &[u8]) {
    let queue = OutputQueue::default();
    let producer = queue.clone();
    let (wake, events) = mpsc::channel();
    thread::scope(|scope| {
        scope.spawn(move || {
            for chunk in bytes.chunks(CHUNK) {
                if producer.push(chunk) {
                    wake.send(()).unwrap();
                }
            }
        });

        let pane = PaneId(1);
        let mut schedule = OutputSchedule::default();
        let mut remaining = bytes.len();
        let mut scratch = Vec::new();
        let mut deadline: Option<Instant> = None;
        while remaining > 0 {
            if let Some(deadline) = deadline {
                let delay = deadline.saturating_duration_since(Instant::now());
                if !delay.is_zero() {
                    thread::sleep(delay);
                }
            } else {
                events.recv().expect("reader wake while bytes remain");
                schedule.queue(pane);
            }
            for () in events.try_iter() {
                schedule.queue(pane);
            }
            while let Some(id) = schedule.next_pane(Instant::now) {
                let more = queue.take_into(&mut scratch, CHUNK_BYTES);
                term.process_bytes(&scratch);
                remaining -= scratch.len();
                schedule.chunk_drained(id, more);
            }
            deadline = schedule.end_turn(Instant::now());
        }
    });
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
    let cat = ascii_log(50 * 1024 * 1024);
    group.throughput(Throughput::Bytes(cat.len() as u64));
    group.bench_function("cat_50mib", |b| {
        b.iter_batched_ref(
            || Terminal::new(COLS, ROWS, |_| {}),
            |term| feed(term, black_box(&cat)),
            BatchSize::LargeInput,
        )
    });
    group.bench_function("cat_50mib_scheduled", |b| {
        b.iter_batched_ref(
            || Terminal::new(COLS, ROWS, |_| {}),
            |term| feed_scheduled(term, black_box(&cat)),
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
            blocks: Vec::new(),
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

fn bench_full_screen(c: &mut Criterion) {
    let mut fonts = new_font_system();
    let m = metrics(&mut fonts);
    let palette = Palette::default();
    let (rows, _) = full_terminal().snapshot();
    c.bench_function("shape_mixed_screen_200x60", |b| {
        let mut shaper = RowShaper {
            font_system: &mut fonts,
            palette: &palette,
            font_px: m.font_px(),
            char_width: m.char_width,
            line_height: m.line_height,
            family: None,
        };
        b.iter(|| {
            for row in &rows {
                black_box(shaper.build_row(black_box(row), None));
            }
        });
    });
}

fn bench_scroll(c: &mut Criterion) {
    const HISTORY: usize = 10_000;
    const STEP: usize = 50;
    let mut term = Terminal::new(COLS, ROWS, |_| {});
    let script = mixed_sgr(32 * 1024);
    for line in script
        .split_inclusive(|&byte| byte == b'\n')
        .filter(|line| line.ends_with(b"\n"))
        .cycle()
        .take(HISTORY + ROWS)
    {
        term.process_bytes(line);
    }
    let mut fonts = new_font_system();
    let m = metrics(&mut fonts);
    let palette = Palette::default();
    let surface = (COLS as f32 * m.char_width, ROWS as f32 * m.line_height);
    let mut ctx = context(&mut fonts, &palette, m);
    let opts = options(surface);
    let mut rows = RowCache::default();
    let mut ime = PreeditState::default();
    let mut group = c.benchmark_group("scrollback");
    group.sample_size(10);
    group.bench_function("10k_lines", |b| {
        b.iter(|| {
            term.scroll_to_bottom();
            for _ in (0..HISTORY).step_by(STEP) {
                term.scroll_display(STEP as i32);
                let (lines, cursor) = term.snapshot();
                let pane = PaneFrame {
                    id: PaneId(1),
                    rect: Rect::new(0.0, 0.0, surface.0, surface.1),
                    lines,
                    cursor,
                    focused: true,
                    status: None,
                    blocks: Vec::new(),
                };
                black_box(build_frame(
                    &mut ctx,
                    &opts,
                    &mut rows,
                    &mut ime,
                    &[],
                    &[pane],
                    &[],
                ));
            }
        });
    });
    group.finish();
}

fn bench_search(c: &mut Criterion) {
    let script = "INFO build succeeded · تم حفظ الملف\r\n".repeat(999);
    let mut panes: Vec<_> = (0..20)
        .map(|i| {
            let mut term = Terminal::new(80, 24, |_| {});
            for _ in 0..2 {
                feed(&mut term, script.as_bytes());
                term.process_bytes("release handoff · مُرَاجَعَة\r\n".as_bytes());
            }
            let info = PaneInfo {
                pane: PaneId(i as u64),
                tab: i / 4,
                tab_title: format!("workspace {}", i / 4),
                pane_title: Some(format!("pane {i}")),
            };
            (info, term)
        })
        .collect();
    let query = Query::new("مراجعة").unwrap();
    c.bench_function("search_all_20_panes", |b| {
        b.iter(|| {
            let hits = search_all(
                panes.iter_mut().map(|(info, term)| (info.clone(), term)),
                &query,
            );
            assert_eq!(hits.len(), 40);
            black_box(hits);
        })
    });
}

fn context<'a>(
    fonts: &'a mut cosmic_text::FontSystem,
    palette: &'a Palette,
    metrics: CellMetrics,
) -> DrawContext<'a> {
    DrawContext {
        font_system: fonts,
        family: None,
        palette,
        metrics,
        is_srgb: false,
    }
}

fn options(surface: (f32, f32)) -> FrameOptions<'static> {
    FrameOptions {
        window_focused: true,
        cursor_style: "block",
        cursor_blink_on: true,
        surface,
    }
}

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 800;
const SURFACE: (f32, f32) = (WIDTH as f32, HEIGHT as f32);
const SIDE: SidebarPosition = SidebarPosition::Right;
const LANGUAGE: UiLanguage = UiLanguage::Arabic;

fn entries() -> Vec<Entry> {
    (0..20)
        .map(|i| {
            let mut entry = Entry::new(
                &format!("مراجعة مساحة العمل {i}"),
                Some(Path::new("/src/twitty")),
                i == 0,
                Some(0),
                [],
            );
            entry.branch = Some(format!("feature/{i}"));
            entry.last_command = Some("cargo test --test bidi_layout_tests".into());
            entry
        })
        .collect()
}

struct WorkspaceFrame {
    layout: WindowLayout,
    panes: Vec<PaneFrame>,
    headers: Vec<PaneHeaderView>,
    composer: ComposerView,
}

impl WorkspaceFrame {
    fn new(m: CellMetrics) -> Self {
        let layout = WindowLayout::new(m, SURFACE, 20, (SIDE, 256.0), true).with_language(LANGUAGE);
        let mut panes = Vec::new();
        let mut headers = Vec::new();
        let area = layout.content;
        let output = mixed_sgr(8192);
        for i in 0..4 {
            let id = PaneId(i + 1);
            let w = area.w / 2.0;
            let h = area.h / 2.0;
            let rect = Rect::new(
                area.x + (i % 2) as f32 * w,
                area.y + (i / 2) as f32 * h,
                w,
                h,
            );
            let header = PaneHeaderLayout::new(rect, m.scale, true).with_language(LANGUAGE);
            let (cols, rows) = m.grid_size(header.grid);
            let mut term = Terminal::new(cols, rows, |_| {});
            term.process_bytes(&output);
            let (lines, cursor) = term.snapshot();
            let focused = i == 0;
            headers.push(PaneHeaderView {
                id,
                layout: header,
                title: format!("مخرجات build {i}"),
                agent: false,
                focused,
                attention: false,
                needs_input: false,
                zoomed: false,
                flash: 0.0,
            });
            panes.push(PaneFrame {
                id,
                rect: header.grid,
                lines,
                cursor,
                focused,
                status: None,
                blocks: [(0, 4), (6, 12)]
                    .into_iter()
                    .enumerate()
                    .map(|(i, (first_row, last_row))| BlockDecor {
                        id: i as u64,
                        first_row,
                        last_row,
                        status: BlockStatus::Succeeded,
                        selected: false,
                        badge: None,
                        meta: Some("0.4s".into()),
                        has_command: true,
                        bookmarked: false,
                    })
                    .collect(),
            });
        }
        let composer = ComposerView {
            context: None,
            pane_rect: panes[0].rect,
            lines: vec!["git log --grep='تم حفظ الملف'".into()],
            caret: (0, 3),
            selection: None,
            preedit: None,
            ghost: None,
            highlights: Vec::new(),
            lints: Vec::new(),
            focused: true,
            cursor_blink_on: true,
            popup: None,
            language: LANGUAGE,
            ai: Default::default(),
        };
        Self {
            layout,
            panes,
            headers,
            composer,
        }
    }

    fn chrome(&self, scale: f32) -> ChromeView<'_> {
        let diff = Some(DiffStat {
            added: 34,
            removed: 8,
        });
        ChromeView {
            layout: &self.layout,
            top: TopBarLayout::new(self.layout.top_bar, scale, diff).with_language(LANGUAGE),
            diff,
            unread: 2,
            sidebar_visible: true,
            hover: None,
            hover_tab: None,
            language: LANGUAGE,
            panes: &self.headers,
            tabs: &[],
        }
    }
}

fn bench_workspace(c: &mut Criterion) {
    let mut fonts = new_font_system();
    let m = metrics(&mut fonts);
    let palette = Palette::default();
    let font = UiFont::default();
    let style = UiStyle::new(&palette, None, m.scale);
    let workspace = WorkspaceFrame::new(m);
    let chrome = workspace.chrome(m.scale);
    let area = workspace.layout.sidebar.unwrap();
    let mut labels = LabelCache::default();
    let mut ui_labels = UiLabelCache::default();
    let mut scroll = ScrollState::default();
    let entries = entries();
    let cards = labels.cards(&font, &mut fonts, entries.clone(), area, m.scale);
    let mut ctx = context(&mut fonts, &palette, m);
    let mut rows = RowCache::default();
    let mut ime = PreeditState::default();
    let opts = options(SURFACE);
    c.bench_function("frame_sidebar_four_panes_blocks_composer", |b| {
        b.iter(|| {
            let cards = labels.cards(&font, ctx.font_system, entries.clone(), area, m.scale);
            let sidebar = scroll.view(&cards, 0, area, m.scale, SIDE, LANGUAGE);
            let mut frame = build_frame_with_cards(
                &mut ctx,
                &opts,
                &mut rows,
                &mut ime,
                &[],
                &workspace.panes,
                &[],
                style,
                &font,
                None,
                LANGUAGE,
            );
            frame.add_sidebar(&mut ctx, &sidebar, &mut labels, style, &font);
            frame.add_chrome(
                &mut ctx,
                style,
                UiLabels::cached(&font, &mut ui_labels),
                &chrome,
            );
            frame.add_composer(&mut ctx, style, &font, &workspace.composer);
            black_box(frame);
        })
    });
    c.bench_function("sidebar_layout_20_cards", |b| {
        b.iter(|| {
            black_box(scroll.view(black_box(&cards), 0, area, m.scale, SIDE, LANGUAGE));
        })
    });
    let mut frame = build_frame(&mut ctx, &opts, &mut rows, &mut ime, &[], &[], &[]);
    c.bench_function("chrome_labels", |b| {
        b.iter(|| {
            frame.shapes.chrome.clear();
            frame.add_chrome(
                &mut ctx,
                style,
                UiLabels::cached(&font, &mut ui_labels),
                &chrome,
            );
            black_box(&frame);
        })
    });
    let items: Vec<_> = twitty::commands::all().into_iter().take(10).collect();
    let icons = PaletteIcons {
        leading: Icon::Search,
        rows: items
            .iter()
            .map(|cmd| Icon::for_command(cmd.name))
            .collect(),
    };
    let view = PaletteView {
        items: items
            .into_iter()
            .map(|cmd| PaletteItem {
                title_en: cmd.title_en.into(),
                title_ar: cmd.title_ar.into(),
                shortcut: cmd.shortcut.map(str::to_owned),
                dimmed: false,
            })
            .collect(),
        ..Default::default()
    };
    let mut setup_fonts = cosmic_text::FontSystem::new_with_fonts([]);
    c.bench_function("palette_labels_10_items", |b| {
        b.iter_batched(
            || {
                build_frame(
                    &mut context(&mut setup_fonts, &palette, m),
                    &opts,
                    &mut RowCache::default(),
                    &mut PreeditState::default(),
                    &[],
                    &[],
                    &[],
                )
            },
            |mut frame| {
                frame.add_palette_motion(
                    &mut ctx,
                    style,
                    UiLabels::cached(&font, &mut ui_labels),
                    &view,
                    &icons,
                    1.0,
                );
                black_box(frame);
            },
            BatchSize::SmallInput,
        )
    });
}

fn measure_startup() {
    let start = Instant::now();
    let mut target = Offscreen::new(WIDTH, HEIGHT).expect("initialize headless GPU");
    let gpu = start.elapsed();
    let font_start = Instant::now();
    let mut fonts = new_font_system();
    let _ui_fonts = twitty::render::text::fonts::ui_font_system(&fonts);
    let font_db = font_start.elapsed();
    let m = metrics(&mut fonts);
    let palette = Palette::default();
    let font = UiFont::default();
    let style = UiStyle::new(&palette, None, m.scale);
    let ws = WorkspaceFrame::new(m);
    let mut ctx = context(&mut fonts, &palette, m);
    let mut rows = RowCache::default();
    let mut ime = PreeditState::default();
    let mut labels = LabelCache::default();
    let mut ui_labels = UiLabelCache::default();
    let area = ws.layout.sidebar.unwrap();
    let cards = labels.cards(&font, ctx.font_system, entries(), area, m.scale);
    let sidebar = ScrollState::default().view(&cards, 0, area, m.scale, SIDE, LANGUAGE);
    let mut frame = build_frame_with_cards(
        &mut ctx,
        &options(SURFACE),
        &mut rows,
        &mut ime,
        &[],
        &ws.panes,
        &[],
        style,
        &font,
        None,
        LANGUAGE,
    );
    frame.add_sidebar(&mut ctx, &sidebar, &mut labels, style, &font);
    frame.add_chrome(
        &mut ctx,
        style,
        UiLabels::cached(&font, &mut ui_labels),
        &ws.chrome(m.scale),
    );
    frame.add_composer(&mut ctx, style, &font, &ws.composer);
    black_box(
        target
            .render_layers(
                (&frame.quads, &frame.shapes),
                frame.text_runs(&ws.panes, &rows, &ime),
                ctx.font_system,
                frame.background,
                1.0,
            )
            .expect("draw and read back the first frame"),
    );
    let first_frame = start.elapsed();
    eprintln!(
        "headless startup gpu_ms={:.3} font_db_ms={:.3} first_frame_ms={:.3}",
        gpu.as_secs_f64() * 1000.0,
        font_db.as_secs_f64() * 1000.0,
        first_frame.as_secs_f64() * 1000.0,
    );
}

fn main() {
    if std::env::var_os("TWITTY_PERF_STARTUP").as_deref() == Some(std::ffi::OsStr::new("1")) {
        measure_startup();
        return;
    }
    let mut c = Criterion::default()
        .warm_up_time(std::time::Duration::from_millis(750))
        .measurement_time(std::time::Duration::from_secs(2))
        .sample_size(20)
        .configure_from_args();
    bench_process(&mut c);
    bench_snapshot(&mut c);
    bench_build_row(&mut c);
    bench_frame(&mut c);
    bench_full_screen(&mut c);
    bench_scroll(&mut c);
    bench_search(&mut c);
    bench_workspace(&mut c);
    c.final_summary();
}
