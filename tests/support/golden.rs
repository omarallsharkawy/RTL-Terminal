//! Shared harness for golden images: bundled fonts, the software GPU target, and comparing a
//! frame with its reference PNG in `tests/golden/`.
//!
//! A new golden test is usually one `GoldenScene`:
//!
//! ```ignore
//! GoldenScene::new("thing_en_dark", 640, 400, "twitty-dark")
//!     .terminal(b"$ cargo test\r\n")
//!     .check(|frame, ctx, style, ui_font| frame.add_thing(ctx, style, ui_font, &view));
//! ```
//!
//! - Fonts are the embedded Noto Naskh Arabic plus subsets of Noto Sans Mono, Noto Sans CJK
//!   and Noto Color Emoji bundled in `tests/golden/fonts/`; system fonts are never loaded, so
//!   the shaping is the same on every machine.
//! - Only Mesa's llvmpipe (lavapipe on Vulkan, or llvmpipe on GL) is used by default. Without
//!   it the tests print a note and pass, unless `TWITTY_REQUIRE_GOLDEN=1` (CI) makes that a
//!   failure. `TWITTY_GPU_ADAPTER=any` (or part of an adapter name) uses another adapter.
//!   The references were made with llvmpipe on GL; on a desktop whose EGL goes to another
//!   vendor, `__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json
//!   LIBGL_ALWAYS_SOFTWARE=1` reaches it.
//! - `TWITTY_BLESS=1` rewrites the references from the current output.
//! - On a mismatch the actual image and a diff (mismatching pixels in red) are written under
//!   the cargo target dir and their paths printed.

#![allow(dead_code)] // Each test binary uses part of it.

use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use cosmic_text::{fontdb, FontSystem};
use twitty::color::Palette;
use twitty::layout::{PaneId, Rect};
use twitty::render::gpu::offscreen::Offscreen;
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::frame::{build_frame, Frame, FrameOptions};
use twitty::render::scene::preedit::PreeditState;
use twitty::render::text::ui::UiFont;
use twitty::render::ui_style::UiStyle;
use twitty::render::{CellMetrics, PaneFrame, RowCache};
use twitty::terminal::Terminal;

/// Largest per-channel difference that still counts as the same pixel.
pub const CHANNEL_TOLERANCE: u8 = 12;
/// Share of pixels allowed to differ by more than that (antialiasing noise between Mesa
/// versions).
pub const MAX_BAD_FRACTION: f64 = 0.0005;

pub const NASKH: &[u8] = include_bytes!("../../assets/fonts/NotoNaskhArabic-Regular.ttf");
pub const UI_LATIN_REG: &[u8] = include_bytes!("../../assets/fonts/TwittyLatinUI-Regular.ttf");
pub const UI_LATIN_SEMI: &[u8] = include_bytes!("../../assets/fonts/TwittyLatinUI-SemiBold.ttf");
pub const UI_ARABIC_REG: &[u8] = include_bytes!("../../assets/fonts/TwittyArabicUI-Regular.ttf");
pub const UI_ARABIC_SEMI: &[u8] = include_bytes!("../../assets/fonts/TwittyArabicUI-SemiBold.ttf");
pub const MONO: &[u8] = include_bytes!("../golden/fonts/NotoSansMono-Subset.ttf");
pub const CJK: &[u8] = include_bytes!("../golden/fonts/NotoSansCJK-Subset.otf");
pub const EMOJI: &[u8] = include_bytes!("../golden/fonts/NotoColorEmoji-Subset.ttf");

fn font_db(fonts: &[&[u8]]) -> fontdb::Database {
    let mut db = fontdb::Database::new();
    for font in fonts {
        db.load_font_data(font.to_vec());
    }
    db
}

/// The bundled terminal fonts only; the interface falls back to Noto Sans Mono.
pub fn font_system() -> FontSystem {
    let mut db = font_db(&[MONO, NASKH, CJK, EMOJI]);
    db.set_monospace_family("Noto Sans Mono");
    db.set_sans_serif_family("Noto Sans Mono");
    db.set_serif_family("Noto Sans Mono");
    FontSystem::new_with_locale_and_db("en-US".into(), db)
}

/// The bundled interface fonts (Twitty Latin and Arabic UI) plus the terminal ones.
pub fn ui_font_system() -> FontSystem {
    let mut db = font_db(&[
        UI_LATIN_REG,
        UI_LATIN_SEMI,
        UI_ARABIC_REG,
        UI_ARABIC_SEMI,
        NASKH,
        MONO,
    ]);
    db.set_sans_serif_family("Twitty Latin UI");
    db.set_monospace_family("Noto Sans Mono");
    db.set_serif_family("Twitty Latin UI");
    FontSystem::new_with_locale_and_db("en-US".into(), db)
}

pub fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| !v.is_empty() && v != "0")
}

/// llvmpipe is not thread-safe enough to share between many devices at once on every Mesa
/// version, and the scenes are small: draw them one at a time.
static GPU: Mutex<()> = Mutex::new(());

/// An offscreen target, held alone while it lives.
pub struct GpuTarget {
    target: Offscreen,
    _lock: MutexGuard<'static, ()>,
}

impl Deref for GpuTarget {
    type Target = Offscreen;
    fn deref(&self) -> &Offscreen {
        &self.target
    }
}

impl DerefMut for GpuTarget {
    fn deref_mut(&mut self) -> &mut Offscreen {
        &mut self.target
    }
}

/// A `width`×`height` software GPU target for test `name`, or `None` (with a note) when no
/// adapter is available. Under `TWITTY_REQUIRE_GOLDEN` a missing adapter fails the test.
pub fn offscreen(name: &str, width: u32, height: u32) -> Option<GpuTarget> {
    let lock = GPU.lock().unwrap_or_else(|e| e.into_inner());
    match Offscreen::new(width, height) {
        Ok(target) => Some(GpuTarget {
            target,
            _lock: lock,
        }),
        Err(e) => {
            assert!(
                !env_flag("TWITTY_REQUIRE_GOLDEN"),
                "TWITTY_REQUIRE_GOLDEN is set but golden test {name} cannot run: {e:#}"
            );
            eprintln!("note: skipping golden image {name}: {e:#}");
            None
        }
    }
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn failure_dir() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("golden-failures")
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::High);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(rgba).unwrap();
}

fn read_png(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let file = std::fs::File::open(path).ok()?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    assert_eq!(
        (info.color_type, info.bit_depth),
        (png::ColorType::Rgba, png::BitDepth::Eight),
        "{} must be 8-bit RGBA",
        path.display()
    );
    buf.truncate(info.buffer_size());
    Some((info.width, info.height, buf))
}

/// Pixels whose channels differ by more than `CHANNEL_TOLERANCE`, and a diff image: the
/// reference dimmed, mismatches in red.
fn diff(expected: &[u8], actual: &[u8]) -> (usize, Vec<u8>) {
    let mut bad = 0;
    let mut image = Vec::with_capacity(actual.len());
    for (e, a) in expected.chunks(4).zip(actual.chunks(4)) {
        let worst = e.iter().zip(a).map(|(x, y)| x.abs_diff(*y)).max().unwrap();
        if worst > CHANNEL_TOLERANCE {
            bad += 1;
            image.extend_from_slice(&[255, 0, 0, 255]);
        } else {
            let gray = ((e[0] as u32 + e[1] as u32 + e[2] as u32) / 3 / 3) as u8;
            image.extend_from_slice(&[gray, gray, gray, 255]);
        }
    }
    (bad, image)
}

/// Compare `actual` with `tests/golden/<name>.png`, or write it under `TWITTY_BLESS`.
pub fn compare(name: &str, width: u32, height: u32, actual: &[u8]) {
    let reference = golden_dir().join(format!("{name}.png"));
    if env_flag("TWITTY_BLESS") {
        write_png(&reference, width, height, actual);
        eprintln!("blessed {}", reference.display());
        return;
    }
    let Some((ew, eh, expected)) = read_png(&reference) else {
        panic!(
            "no reference image {}; run with TWITTY_BLESS=1 to create it",
            reference.display()
        );
    };
    let out = failure_dir();
    let actual_path = out.join(format!("{name}.actual.png"));
    if (ew, eh) != (width, height) {
        write_png(&actual_path, width, height, actual);
        panic!(
            "{name}: size {width}x{height}, reference is {ew}x{eh} (actual: {})",
            actual_path.display()
        );
    }
    let (bad, diff_image) = diff(&expected, actual);
    let allowed = ((width * height) as f64 * MAX_BAD_FRACTION) as usize;
    if bad > allowed {
        let diff_path = out.join(format!("{name}.diff.png"));
        write_png(&actual_path, width, height, actual);
        write_png(&diff_path, width, height, &diff_image);
        panic!(
            "{name}: {bad} pixels differ from {} by more than {CHANNEL_TOLERANCE} (allowed {allowed})\n\
             actual: {}\ndiff:   {}\nIf the change is intended, rerun with TWITTY_BLESS=1.",
            reference.display(),
            actual_path.display(),
            diff_path.display()
        );
    }
}

/// A window of one terminal pane with interface layers drawn over it, compared with its
/// reference image.
pub struct GoldenScene {
    name: String,
    width: u32,
    height: u32,
    palette: Palette,
    fonts: FontSystem,
    font_size: f32,
    text: Vec<u8>,
    pane: Option<Rect>,
    grid: Option<(usize, usize)>,
    focused: bool,
}

impl GoldenScene {
    /// A `width`×`height` window in theme `theme` (a `Palette::named` name) with the bundled
    /// terminal fonts at 14.5 pt and an empty, focused terminal filling it.
    pub fn new(name: impl Into<String>, width: u32, height: u32, theme: &str) -> Self {
        Self {
            name: name.into(),
            width,
            height,
            palette: Palette::named(theme).unwrap_or_else(|| panic!("unknown theme {theme}")),
            fonts: font_system(),
            font_size: 14.5,
            text: Vec::new(),
            pane: None,
            grid: None,
            focused: true,
        }
    }

    /// Use these fonts (e.g. `ui_font_system()`).
    pub fn fonts(mut self, fonts: FontSystem) -> Self {
        self.fonts = fonts;
        self
    }

    /// Bytes written to the terminal (its cursor is hidden).
    pub fn terminal(mut self, bytes: &[u8]) -> Self {
        self.text = bytes.to_vec();
        self
    }

    /// Where the pane sits; the whole window by default.
    pub fn pane(mut self, rect: Rect) -> Self {
        self.pane = Some(rect);
        self
    }

    /// Terminal size in cells; by default what fits the pane (at least 20×10).
    pub fn grid(mut self, cols: usize, rows: usize) -> Self {
        self.grid = Some((cols, rows));
        self
    }

    /// The pane does not have focus.
    pub fn unfocused(mut self) -> Self {
        self.focused = false;
        self
    }

    pub fn window(&self) -> Rect {
        Rect::new(0.0, 0.0, self.width as f32, self.height as f32)
    }

    /// Build the frame, let `draw` add layers over the terminal, render and compare.
    pub fn check(mut self, draw: impl FnOnce(&mut Frame, &mut DrawContext, UiStyle, &UiFont)) {
        let window = self.window();
        let surface = (window.w, window.h);
        let metrics = CellMetrics::measure(&mut self.fonts, None, self.font_size, 1.0);
        let style = UiStyle::new(&self.palette, None, 1.0);
        let ui_font = UiFont::default();
        let rect = self.pane.unwrap_or(window);
        let (cols, rows) = self.grid.unwrap_or_else(|| {
            let (cols, rows) = metrics.grid_size(rect);
            (cols.max(20), rows.max(10))
        });
        let mut term = Terminal::new(cols, rows, |_| {});
        term.process_bytes(b"\x1b[?25l");
        term.process_bytes(&self.text);
        let (lines, cursor) = term.snapshot();
        let panes = [PaneFrame {
            id: PaneId(1),
            rect,
            lines,
            cursor,
            focused: self.focused,
            status: None,
            blocks: Vec::new(),
        }];

        let mut cache = RowCache::default();
        let mut ime = PreeditState::default();
        let mut ctx = DrawContext {
            font_system: &mut self.fonts,
            family: None,
            palette: &self.palette,
            metrics,
            is_srgb: false,
        };
        let opts = FrameOptions {
            window_focused: true,
            cursor_style: "block",
            cursor_blink_on: false,
            surface,
        };
        let mut frame = build_frame(&mut ctx, &opts, &mut cache, &mut ime, &[], &panes, &[]);
        draw(&mut frame, &mut ctx, style, &ui_font);
        let text_layers = frame.text_layers(&panes, &cache, &ime);
        let Some(mut target) = offscreen(&self.name, self.width, self.height) else {
            return;
        };
        let actual = target
            .render_layered(
                (&frame.quads, &frame.shapes),
                frame.overlay_splits(),
                text_layers,
                ctx.font_system,
                frame.background,
                1.0,
            )
            .expect("offscreen render");
        let (w, h) = target.size();
        compare(&self.name, w, h, &actual);
    }
}
