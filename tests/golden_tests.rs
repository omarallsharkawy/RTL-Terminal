//! Golden images: whole frames drawn on a software GPU adapter and compared with reference
//! PNGs in `tests/golden/`.
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

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use alacritty_terminal::index::{Direction, Side};
use cosmic_text::{fontdb, FontSystem};
use twitty::color::Palette;
use twitty::layout::{PaneId, Rect};
use twitty::render::gpu::offscreen::Offscreen;
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::frame::{build_frame, FrameOptions};
use twitty::render::scene::preedit::PreeditState;
use twitty::render::{CellMetrics, PaneFrame, RowCache};
use twitty::terminal::Terminal;

/// Largest difference allowed in any channel of a pixel (rasterizers may round blending
/// differently by a step or two).
const CHANNEL_TOLERANCE: u8 = 12;
/// Pixels allowed beyond `CHANNEL_TOLERANCE` (antialiased glyph edges on another llvmpipe
/// version), as a fraction of the image.
const MAX_BAD_FRACTION: f64 = 0.0005;

const NASKH: &[u8] = include_bytes!("../assets/fonts/NotoNaskhArabic-Regular.ttf");
const MONO: &[u8] = include_bytes!("golden/fonts/NotoSansMono-Subset.ttf");
const CJK: &[u8] = include_bytes!("golden/fonts/NotoSansCJK-Subset.otf");
const EMOJI: &[u8] = include_bytes!("golden/fonts/NotoColorEmoji-Subset.ttf");

/// A font system with only the bundled fonts.
fn font_system() -> FontSystem {
    let mut db = fontdb::Database::new();
    for font in [MONO, NASKH, CJK, EMOJI] {
        db.load_font_data(font.to_vec());
    }
    db.set_monospace_family("Noto Sans Mono");
    db.set_sans_serif_family("Noto Sans Mono");
    db.set_serif_family("Noto Sans Mono");
    FontSystem::new_with_locale_and_db("en-US".into(), db)
}

/// llvmpipe is not thread-safe enough to share between many devices at once on every Mesa
/// version, and the scenes are small: draw them one at a time.
static GPU: Mutex<()> = Mutex::new(());

/// A cell and the side of it a pointer is on: (col, row, side).
type Point = (usize, usize, Side);

/// What a scene needs besides its text.
#[derive(Default)]
struct Scene {
    name: &'static str,
    cols: usize,
    rows: usize,
    /// Bytes written to the terminal.
    input: String,
    /// Keep the cursor visible (block); otherwise it is hidden.
    cursor: bool,
    /// Select from one (col, row, side) to another.
    selection: Option<(Point, Point)>,
    /// Search the scrollback for this text.
    search: Option<&'static str>,
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn failure_dir() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("golden-failures")
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| !v.is_empty() && v != "0")
}

/// Draw `scene` and return its size and RGBA pixels, or `None` when no adapter is available.
fn render(scene: &Scene) -> Option<(u32, u32, Vec<u8>)> {
    let mut font_system = font_system();
    let palette = Palette::default();
    let metrics = CellMetrics::measure(&mut font_system, None, 15.0, 1.0);
    let width = (metrics.padding_left * 2.0 + scene.cols as f32 * metrics.char_width).ceil();
    let height = (metrics.padding_top * 2.0 + scene.rows as f32 * metrics.line_height).ceil();

    let mut term = Terminal::new(scene.cols, scene.rows, |_| {});
    if !scene.cursor {
        term.process_bytes(b"\x1b[?25l");
    }
    term.process_bytes(scene.input.as_bytes());
    if let Some((from, to)) = scene.selection {
        term.start_selection(from.0, from.1, from.2, 1);
        term.update_selection(to.0, to.1, to.2);
    }
    if let Some(query) = scene.search {
        assert!(term.search(query, Direction::Left), "{query} not found");
    }
    let (lines, cursor) = term.snapshot();
    let panes = [PaneFrame {
        id: PaneId(1),
        rect: Rect::new(0.0, 0.0, width, height),
        lines,
        cursor,
        focused: true,
        status: None,
    }];

    let mut cache = RowCache::default();
    let mut ime = PreeditState::default();
    let mut ctx = DrawContext {
        font_system: &mut font_system,
        family: None,
        palette: &palette,
        metrics,
        is_srgb: false,
    };
    let opts = FrameOptions {
        window_focused: true,
        cursor_style: "block",
        cursor_blink_on: true,
        surface: (width, height),
    };
    let frame = build_frame(&mut ctx, &opts, &mut cache, &mut ime, &[], &panes, &[]);
    let runs = frame.text_runs(&panes, &cache, &ime);

    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let mut target = match Offscreen::new(width as u32, height as u32) {
        Ok(target) => target,
        Err(e) => {
            assert!(
                !env_flag("TWITTY_REQUIRE_GOLDEN"),
                "TWITTY_REQUIRE_GOLDEN is set but the golden tests cannot run: {e}"
            );
            eprintln!("note: skipping golden image {}: {e}", scene.name);
            return None;
        }
    };
    let pixels = target
        .render(&frame.quads, runs, &mut font_system, frame.background, 1.0)
        .expect("offscreen render");
    let (w, h) = target.size();
    Some((w, h, pixels))
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

fn check(scene: Scene) {
    let Some((width, height, actual)) = render(&scene) else {
        return;
    };
    let reference = golden_dir().join(format!("{}.png", scene.name));
    if env_flag("TWITTY_BLESS") {
        write_png(&reference, width, height, &actual);
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
    let actual_path = out.join(format!("{}.actual.png", scene.name));
    if (ew, eh) != (width, height) {
        write_png(&actual_path, width, height, &actual);
        panic!(
            "{}: size {width}x{height}, reference is {ew}x{eh} (actual: {})",
            scene.name,
            actual_path.display()
        );
    }
    let (bad, diff_image) = diff(&expected, &actual);
    let allowed = ((width * height) as f64 * MAX_BAD_FRACTION) as usize;
    if bad > allowed {
        let diff_path = out.join(format!("{}.diff.png", scene.name));
        write_png(&actual_path, width, height, &actual);
        write_png(&diff_path, width, height, &diff_image);
        panic!(
            "{}: {bad} pixels differ from {} by more than {CHANNEL_TOLERANCE} (allowed {allowed})\n\
             actual: {}\ndiff:   {}\nIf the change is intended, rerun with TWITTY_BLESS=1.",
            scene.name,
            reference.display(),
            actual_path.display(),
            diff_path.display()
        );
    }
}

#[test]
fn golden_mixed_arabic_english_with_hash() {
    check(Scene {
        name: "mixed_hash",
        cols: 44,
        rows: 3,
        input: "commit 7abaf40 (HEAD -> main)\r\n\
                تم دمج الفرع 7abaf40 في main بنجاح\r\n\
                قال: الإصدار v3.0.0 جاهز منذ 2026"
            .into(),
        ..Default::default()
    });
}

#[test]
fn golden_arabic_with_harakat() {
    check(Scene {
        name: "harakat",
        cols: 40,
        rows: 2,
        input: "بِسْمِ اللَّهِ الرَّحْمَٰنِ الرَّحِيمِ\r\nمَرْحَبًا بِكُمْ فِي تْوِيتِي".into(),
        ..Default::default()
    });
}

#[test]
fn golden_colored_background_arabic_line() {
    check(Scene {
        name: "colored_line",
        cols: 36,
        rows: 2,
        // Background color erase: `\e[K` paints the rest of the line too.
        input: "\x1b[48;2;40;80;160m\x1b[97mمرحبا بالعالم: نص على خلفية ملونة\x1b[K\x1b[0m\r\n\
                \x1b[42;30m ok \x1b[0m \x1b[43;30mتحذير\x1b[0m done"
            .into(),
        ..Default::default()
    });
}

#[test]
fn golden_selection_over_arabic() {
    check(Scene {
        name: "selection_arabic",
        cols: 32,
        rows: 2,
        input: "قال مرحبا بك في البيت\r\nhello عالم world".into(),
        selection: Some(((4, 0, Side::Left), (8, 1, Side::Right))),
        ..Default::default()
    });
}

#[test]
fn golden_box_drawing_and_blocks() {
    check(Scene {
        name: "box_blocks",
        cols: 24,
        rows: 6,
        input: "┌──────┬──────┐ ░▒▓█\r\n\
                │ name │ اسم  │ ▀▄▌▐\r\n\
                ├──────┼──────┤ ▁▂▃▄\r\n\
                │ 7a40 │ نعم  │ ▅▆▇█\r\n\
                ╰──────┴──────╯ ╔═╗\r\n\
                ━━┃━━ ╌╌ ╎ ║ ╚═╝"
            .into(),
        ..Default::default()
    });
}

#[test]
fn golden_cjk_emoji_with_block_cursor() {
    check(Scene {
        name: "cjk_emoji_cursor",
        cols: 30,
        rows: 2,
        input: "日本語 中文 漢字 ok\r\n😀 🚀 👍 世界 ".into(),
        cursor: true,
        ..Default::default()
    });
}

#[test]
fn golden_search_highlight() {
    check(Scene {
        name: "search_highlight",
        cols: 30,
        rows: 3,
        input: "سطر أول مرحبا هنا\r\nline two: no match\r\nثم مرحبا again مرحبا".into(),
        search: Some("مرحبا"),
        ..Default::default()
    });
}
