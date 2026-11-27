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
const UI_LATIN_REG: &[u8] = include_bytes!("../assets/fonts/TwittyLatinUI-Regular.ttf");
const UI_LATIN_SEMI: &[u8] = include_bytes!("../assets/fonts/TwittyLatinUI-SemiBold.ttf");
const UI_ARABIC_REG: &[u8] = include_bytes!("../assets/fonts/TwittyArabicUI-Regular.ttf");
const UI_ARABIC_SEMI: &[u8] = include_bytes!("../assets/fonts/TwittyArabicUI-SemiBold.ttf");
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
    // These fixtures isolate terminal rendering and retain their original palette.
    let palette = Palette::named("tokyo-night").unwrap();
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
        blocks: Vec::new(),
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

#[test]
fn sdf_shapes_render_rounded_border_shadow_and_capsule() {
    use twitty::render::shapes::{Border, Shadow, Shape, ShapeLayers};

    let _gpu = GPU.lock().unwrap_or_else(|p| p.into_inner());
    let mut target = match Offscreen::new(64, 48) {
        Ok(target) => target,
        Err(err) if !env_flag("TWITTY_REQUIRE_GOLDEN") => {
            eprintln!("shape pixels skipped: {err:#}");
            return;
        }
        Err(err) => panic!("shape pixels require an offscreen adapter: {err:#}"),
    };
    let layers = ShapeLayers {
        underlay: Vec::new(),
        chrome: vec![Shape::rounded(
            Rect::new(0.0, 0.0, 64.0, 48.0),
            0.0,
            [0.4, 0.4, 0.4, 1.0],
        )],
        overlay: vec![
            Shape::RoundedRect {
                rect: Rect::new(12.0, 8.0, 36.0, 24.0),
                radii: [8.0, 0.0, 4.0, 6.0],
                fill: [0.1, 0.2, 0.9, 1.0],
                border: Some(Border {
                    width: 2.0,
                    color: [0.9, 0.2, 0.1, 1.0],
                }),
                shadow: Some(Shadow {
                    blur: 6.0,
                    offset: [2.0, 4.0],
                    color: [0.0, 0.0, 0.0, 0.5],
                }),
            },
            Shape::Capsule {
                from: [22.5, 20.5],
                to: [34.5, 20.5],
                width: 2.0,
                color: [1.0, 1.0, 1.0, 0.5],
            },
        ],
    };
    let pixels = target
        .render_layers((&[], &layers), [], &mut font_system(), [0.0; 4], 1.0)
        .unwrap();
    let pixel = |x: usize, y: usize| &pixels[(y * 64 + x) * 4..][..4];
    let fill = pixel(30, 25);
    assert!(fill[2] > 220 && fill[0] < 40);
    let border = pixel(30, 8);
    assert!(border[0] > 220 && border[2] < 40);
    assert_eq!(pixel(12, 8), pixel(2, 2), "large corner is cut away");
    assert_eq!(pixel(47, 8), border, "sharp corner reaches the edge");
    let shadow_near = pixel(30, 34)[0];
    let shadow_far = pixel(30, 39)[0];
    assert!(shadow_near < shadow_far && shadow_far < pixel(30, 44)[0]);
    let blended = pixel(28, 20);
    assert!(blended[0].abs_diff(140) <= 2 && blended[2].abs_diff(242) <= 2);
}

#[test]
fn workspace_cards_draw_in_dark_and_light_themes() {
    use twitty::color::Rgba;
    use twitty::layout::SidebarPosition;
    use twitty::render::scene::sidebar::LabelCache;
    use twitty::render::scene::window_chrome::{ChromeView, PaneHeaderView};
    use twitty::render::text::ui::UiFont;
    use twitty::render::ui_style::UiStyle;
    use twitty::sidebar::cards::ScrollState;
    use twitty::sidebar::{AgentState, AgentStatus, Entry, Notice};
    use twitty::ui::chrome::{PaneHeaderLayout, TopBarLayout, UiLanguage, WindowLayout};

    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let (width, height) = (360, 280);
    let mut target = match Offscreen::new(width, height) {
        Ok(target) => target,
        Err(err) if !env_flag("TWITTY_REQUIRE_GOLDEN") => {
            eprintln!("workspace card pixels skipped: {err:#}");
            return;
        }
        Err(err) => panic!("workspace cards require an offscreen adapter: {err:#}"),
    };
    let mut fonts = font_system();
    let font = UiFont::default();
    let metrics = CellMetrics::measure(&mut fonts, None, 15.0, 1.0);
    let selected = Entry::new(
        "Review change",
        Some(Path::new("/tmp/project")),
        false,
        None,
        [Notice::new(None, "Ready to merge")],
    )
    .with_ports(vec![3000]);
    let waiting = Entry::new("Build", None, false, None, []).with_agent_status(Some(
        AgentStatus::new(AgentState::NeedsInput, Some("Approve the change")),
    ));
    let arabic = Entry::new(
        "مراجعة التغييرات",
        None,
        false,
        None,
        [Notice::new(None, "هل أتابع النشر؟")],
    );
    let surface = (width as f32, height as f32);
    let window = WindowLayout::new(metrics, surface, 3, (SidebarPosition::Right, 250.0), true);
    let top = window.top_bar.bottom();
    let area = Rect::new(110.0, top, 250.0, height as f32 - top);
    let header = PaneHeaderLayout::new(Rect::new(0.0, top, area.x, area.h), 1.0, true);
    let pane_headers = [PaneHeaderView {
        id: PaneId(1),
        layout: header,
        title: "Terminal".into(),
        agent: false,
        focused: false,
        attention: false,
        needs_input: false,
        zoomed: false,
        flash: 0.0,
    }];
    let chrome = ChromeView {
        layout: &window,
        top: TopBarLayout::new(window.top_bar, 1.0, None),
        diff: None,
        unread: 0,
        sidebar_visible: true,
        hover: None,
        hover_tab: None,
        language: UiLanguage::English,
        panes: &pane_headers,
        tabs: &[],
    };
    let entries = [selected, waiting, arabic];
    for palette in [Palette::default(), Palette::named("github-light").unwrap()] {
        let style = UiStyle::new(&palette, Some(Rgba::from_rgb8(54, 86, 170)), 1.0);
        let mut ctx = DrawContext {
            font_system: &mut fonts,
            family: None,
            palette: &palette,
            metrics,
            is_srgb: false,
        };
        let mut labels = LabelCache::default();
        let cards = labels.cards(&font, ctx.font_system, entries.to_vec(), area, 1.0);
        let mut view = ScrollState::default().view(
            &cards,
            0,
            area,
            1.0,
            SidebarPosition::Right,
            UiLanguage::English,
        );
        assert_eq!(view.items.len(), 3);
        let waiting = view.items[1].rect;
        view.update_hover((
            f64::from(waiting.x + 10.0),
            f64::from(waiting.bottom() - 2.0),
        ));
        let opts = FrameOptions {
            window_focused: true,
            cursor_style: "block",
            cursor_blink_on: true,
            surface: (width as f32, height as f32),
        };
        let mut rows = RowCache::default();
        let mut ime = PreeditState::default();
        let mut frame = build_frame(&mut ctx, &opts, &mut rows, &mut ime, &[], &[], &[]);
        frame.add_sidebar(&mut ctx, &view, &mut labels, style, &font);
        frame.add_chrome(&mut ctx, style, &font, &chrome);
        let runs = frame.text_runs(&[], &rows, &ime);
        let ar = runs
            .iter()
            .find(|run| run.buffer.lines[0].text() == "مراجعة التغييرات")
            .unwrap();
        assert!(ar.buffer.layout_runs().next().unwrap().rtl);
        let ar = ar.bounds;
        let pixels = target
            .render_layers(
                (&frame.quads, &frame.shapes),
                runs,
                ctx.font_system,
                frame.background,
                1.0,
            )
            .unwrap();
        let pixel = |x: usize, y: usize| &pixels[((y * width as usize + x) * 4)..][..4];
        let near = |pixel: &[u8], color: Rgba| {
            pixel
                .iter()
                .zip(color.to_array())
                .all(|(&a, b)| a.abs_diff((b * 255.0).round() as u8) <= 2)
        };
        let sample = |rect: Rect| pixel((rect.x + 10.0) as usize, (rect.bottom() - 5.0) as usize);
        assert!(near(sample(view.items[0].rect), style.bg3));
        assert!(near(sample(waiting), style.bg2));
        assert!(near(sample(view.items[2].rect), style.bg1));
        let terminal_sample = (1, header.grid.y as usize + 1);
        let top_sample = (1, 1);
        let header_sample = (1, top as usize + 1);
        let sidebar_sample = (area.x as usize + 1, top as usize + 1);
        assert!(near(pixel(terminal_sample.0, terminal_sample.1), style.bg0));
        for (x, y) in [top_sample, header_sample, sidebar_sample] {
            assert!(near(pixel(x, y), style.bg1));
        }
        let first = view.items[0].rect;
        assert!(
            near(pixel(first.x as usize, first.y as usize), style.bg1),
            "rounded corner"
        );
        // Accumulated card fills and the sidebar base share terminal opacity.
        for opacity in [0.78, 0.5] {
            let translucent = target
                .render_layers(
                    (&frame.quads, &frame.shapes),
                    frame.text_runs(&[], &rows, &ime),
                    ctx.font_system,
                    frame.background,
                    opacity,
                )
                .unwrap();
            for (x, y) in [
                terminal_sample,
                top_sample,
                header_sample,
                sidebar_sample,
                ((first.x + 10.0) as usize, (first.bottom() - 5.0) as usize),
            ] {
                let off = (y * width as usize + x) * 4;
                for (&actual, &solid) in translucent[off..off + 4].iter().zip(pixel(x, y)) {
                    assert!(actual.abs_diff((f32::from(solid) * opacity).round() as u8) <= 1);
                }
            }
        }
        let title = view.items[0].layout(&view).title;
        assert!(
            near(
                pixel((first.x + 5.0) as usize, (title.y + title.h / 2.0) as usize),
                style.accent,
            ),
            "active card accent strip"
        );
        let state = view.items[1].layout(&view).state;
        let warn = style
            .warn
            .to_array()
            .map(|value| (value * 255.0).round() as u8);
        assert!(
            (state.y as usize..state.bottom() as usize).any(|y| {
                (state.x as usize..(state.x + 12.0) as usize).any(|x| {
                    // The 12 px icon's thin SDF strokes are antialiased even in the center.
                    pixel(x, y)
                        .iter()
                        .zip(warn)
                        .all(|(&actual, expected)| actual.abs_diff(expected) <= 40)
                })
            }),
            "needs-input status icon"
        );
        assert!(
            (ar.top as usize..ar.bottom as usize).any(|y| {
                (ar.left as usize..ar.right as usize).any(|x| near(pixel(x, y), style.text))
            }),
            "Arabic glyphs must reach the output pixels"
        );
    }
}

#[derive(Clone, Copy)]
enum AgentGoldenKind {
    Empty,
    Running,
    WaitingCommand,
    PatchExpanded,
    Denied,
}

fn build_agent_golden_state(
    kind: AgentGoldenKind,
    lang: twitty::ui::chrome::UiLanguage,
) -> twitty::ui::agent_panel::AgentPanelState {
    use twitty::ui::agent_panel::{AgentPanelState, CardButton};
    use twitty::ui::chrome::UiLanguage;
    use twitty_ai::engine::model::{
        AgentEvent, ContextBundle, ContextItem, ContextKind, DenyReason, PatchFile, ProviderInfo,
        ProviderKind, ReadTool, SessionId, Step, StepId, StepKind, StepStatus, Usage,
    };

    let mut panel = AgentPanelState::new();
    panel.open();
    panel.language = lang;
    panel.set_project_context(PathBuf::from("/workspace/twitty"));
    panel.set_context_bundle(ContextBundle {
        project_root: Some(PathBuf::from("/workspace/twitty")),
        budget_tokens: 8000,
        items: vec![
            ContextItem {
                kind: ContextKind::Rule,
                label: "AGENTS.md".into(),
                text: "Rules".into(),
                est_tokens: 120,
                included: true,
            },
            ContextItem {
                kind: ContextKind::Memory,
                label: "fact #1".into(),
                text: "Memory".into(),
                est_tokens: 80,
                included: true,
            },
            ContextItem {
                kind: ContextKind::Block,
                label: "block: cargo test".into(),
                text: "Block".into(),
                est_tokens: 210,
                included: true,
            },
        ],
    });
    if matches!(kind, AgentGoldenKind::Empty) {
        return panel;
    }

    let sid = SessionId(1);
    panel.apply_event(AgentEvent::Started {
        session: sid,
        provider: ProviderInfo {
            kind: ProviderKind::Anthropic,
            model: "claude-sonnet".into(),
            display: "Claude · API".into(),
        },
    });
    panel.apply_event(AgentEvent::Usage {
        session: sid,
        usage: Usage {
            input_tokens: 1450,
            output_tokens: 380,
            cost_micros_usd: Some(18_000),
        },
    });

    match kind {
        AgentGoldenKind::Empty => {}
        AgentGoldenKind::Running => {
            panel.apply_event(AgentEvent::StepAdded {
                session: sid,
                step: Step {
                    id: StepId(1),
                    kind: StepKind::Plan {
                        text: match lang {
                            UiLanguage::English => {
                                "1. Inspect failing test\n2. Check src/ui/agent_panel.rs".into()
                            }
                            UiLanguage::Arabic => {
                                "١. فحص الاختبار الفاشل\n٢. مراجعة src/ui/agent_panel.rs".into()
                            }
                        },
                    },
                    status: StepStatus::Done,
                },
            });
            panel.apply_event(AgentEvent::StepAdded {
                session: sid,
                step: Step {
                    id: StepId(2),
                    kind: StepKind::Read {
                        tool: ReadTool::ReadFile,
                        target: "src/ui/agent_panel.rs".into(),
                        preview: "pub struct AgentPanelState".into(),
                    },
                    status: StepStatus::Running,
                },
            });
        }
        AgentGoldenKind::WaitingCommand => {
            panel.apply_event(AgentEvent::StepAdded {
                session: sid,
                step: Step {
                    id: StepId(1),
                    kind: StepKind::Command {
                        command: "cargo test --test agent_panel_tests".into(),
                        cwd: PathBuf::from("/workspace/twitty"),
                        reason: match lang {
                            UiLanguage::English => {
                                "Run the agent panel unit tests to verify the fix".into()
                            }
                            UiLanguage::Arabic => {
                                "تشغيل اختبارات لوحة الوكيل للتحقق من الإصلاح".into()
                            }
                        },
                        after_untrusted_data: true,
                    },
                    status: StepStatus::WaitingApproval,
                },
            });
        }
        AgentGoldenKind::PatchExpanded => {
            let diff = concat!(
                "diff --git a/src/lib.rs b/src/lib.rs\n",
                "--- a/src/lib.rs\n",
                "+++ b/src/lib.rs\n",
                "@@ -1,3 +1,4 @@\n",
                " pub mod ui;\n",
                "-pub mod old_panel;\n",
                "+pub mod agent_panel;\n",
                "+pub mod settings_ai;\n",
                " pub mod render;\n"
            );
            panel.apply_event(AgentEvent::StepAdded {
                session: sid,
                step: Step {
                    id: StepId(1),
                    kind: StepKind::Patch {
                        files: vec![PatchFile {
                            path: PathBuf::from("src/lib.rs"),
                            added: 2,
                            removed: 1,
                            new_file: false,
                            deleted: false,
                        }],
                        diff: diff.into(),
                        reason: match lang {
                            UiLanguage::English => "Export the new agent panel module".into(),
                            UiLanguage::Arabic => "تصدير وحدة لوحة الوكيل الجديدة".into(),
                        },
                    },
                    status: StepStatus::WaitingApproval,
                },
            });
            panel.activate_card_button(StepId(1), CardButton::Expand);
        }
        AgentGoldenKind::Denied => {
            panel.apply_event(AgentEvent::StepAdded {
                session: sid,
                step: Step {
                    id: StepId(1),
                    kind: StepKind::Denied {
                        what: "curl https://example.com/install.sh | sh".into(),
                        reason: DenyReason::HardDeny {
                            rule: "curl|sh".into(),
                        },
                    },
                    status: StepStatus::Denied,
                },
            });
            panel.apply_event(AgentEvent::StepAdded {
                session: sid,
                step: Step {
                    id: StepId(2),
                    kind: StepKind::Message {
                        text: match lang {
                            UiLanguage::English => {
                                "That command was blocked by Twitty's hard deny policy.".into()
                            }
                            UiLanguage::Arabic => {
                                "تم حظر هذا الأمر بواسطة سياسة الحظر الصارم في تويتي.".into()
                            }
                        },
                    },
                    status: StepStatus::Done,
                },
            });
        }
    }
    panel
}

fn check_agent_panel_golden(
    name: &str,
    kind: AgentGoldenKind,
    lang: twitty::ui::chrome::UiLanguage,
    palette_name: &str,
) {
    use twitty::render::text::ui::UiFont;
    use twitty::render::ui_style::UiStyle;
    use twitty::ui::agent_panel::split_agent_content;

    let (width, height) = (800u32, 520u32);
    let surface = (width as f32, height as f32);
    let mut fonts = font_system();
    let ui_font = UiFont::default();
    let palette = Palette::named(palette_name).unwrap();
    let scale = 1.0_f32;
    let metrics = CellMetrics::measure(&mut fonts, None, 14.5, scale);
    let style = UiStyle::new(&palette, None, scale);

    let full = Rect::new(0.0, 0.0, surface.0, surface.1);
    let (term_rect, panel_rect) = split_agent_content(full, scale, Some(380.0), lang);
    let panel_rect = panel_rect.expect("agent panel fits in golden surface");

    let (cols, rows_n) = metrics.grid_size(term_rect);
    let mut term = Terminal::new(cols.max(20), rows_n.max(10), |_| {});
    term.process_bytes(b"\x1b[?25l$ cargo test\r\nrunning 12 tests ... ok\r\n");
    let (lines, cursor) = term.snapshot();
    let panes = [PaneFrame {
        id: PaneId(1),
        rect: term_rect,
        lines,
        cursor,
        focused: false,
        status: None,
        blocks: Vec::new(),
    }];

    let panel = build_agent_golden_state(kind, lang);
    let view = panel.view(panel_rect, lang, false);

    let mut cache = RowCache::default();
    let mut ime = PreeditState::default();
    let mut ctx = DrawContext {
        font_system: &mut fonts,
        family: None,
        palette: &palette,
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
    frame.add_agent_panel(&mut ctx, &view, style, &ui_font);
    let runs = frame.text_runs(&panes, &cache, &ime);

    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let mut target = match Offscreen::new(width, height) {
        Ok(target) => target,
        Err(e) => {
            assert!(
                !env_flag("TWITTY_REQUIRE_GOLDEN"),
                "TWITTY_REQUIRE_GOLDEN is set but golden test {name} cannot run: {e}"
            );
            eprintln!("note: skipping golden image {name}: {e}");
            return;
        }
    };
    let actual = target
        .render_layers(
            (&frame.quads, &frame.shapes),
            runs,
            ctx.font_system,
            frame.background,
            1.0,
        )
        .expect("offscreen render");
    let (w, h) = target.size();
    compare_golden(name, w, h, &actual);
}

/// Compare `actual` with `tests/golden/<name>.png`, or write it under `TWITTY_BLESS`.
fn compare_golden(name: &str, w: u32, h: u32, actual: &[u8]) {
    let reference = golden_dir().join(format!("{name}.png"));
    if env_flag("TWITTY_BLESS") {
        write_png(&reference, w, h, actual);
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
    if (ew, eh) != (w, h) {
        write_png(&actual_path, w, h, actual);
        panic!("{name}: size {w}x{h}, reference is {ew}x{eh}");
    }
    let (bad, diff_image) = diff(&expected, actual);
    let allowed = ((w * h) as f64 * MAX_BAD_FRACTION) as usize;
    if bad > allowed {
        let diff_path = out.join(format!("{name}.diff.png"));
        write_png(&actual_path, w, h, actual);
        write_png(&diff_path, w, h, &diff_image);
        panic!(
            "{name}: {bad} pixels differ from {} by more than {CHANNEL_TOLERANCE} (allowed {allowed})\nactual: {}\ndiff:   {}",
            reference.display(),
            actual_path.display(),
            diff_path.display()
        );
    }
}

#[test]
fn golden_agent_panel_states_bilingual_dark_and_light() {
    use twitty::ui::chrome::UiLanguage;
    let states = [
        ("empty", AgentGoldenKind::Empty),
        ("running", AgentGoldenKind::Running),
        ("waiting_command", AgentGoldenKind::WaitingCommand),
        ("patch_expanded", AgentGoldenKind::PatchExpanded),
        ("denied", AgentGoldenKind::Denied),
    ];
    let langs = [("en", UiLanguage::English), ("ar", UiLanguage::Arabic)];
    let themes = [("dark", "twitty-dark"), ("light", "twitty-light")];

    for (sname, kind) in states {
        for (lname, lang) in langs {
            for (tname, theme) in themes {
                let name = format!("agent_panel_{sname}_{lname}_{tname}");
                check_agent_panel_golden(&name, kind, lang, theme);
            }
        }
    }
}

fn memory_ui_font_system() -> FontSystem {
    let mut db = fontdb::Database::new();
    for font in [
        UI_LATIN_REG,
        UI_LATIN_SEMI,
        UI_ARABIC_REG,
        UI_ARABIC_SEMI,
        NASKH,
        MONO,
    ] {
        db.load_font_data(font.to_vec());
    }
    db.set_sans_serif_family("Twitty Latin UI");
    db.set_monospace_family("Noto Sans Mono");
    db.set_serif_family("Twitty Latin UI");
    FontSystem::new_with_locale_and_db("en-US".into(), db)
}

fn check_memory_screen_golden(
    name: &'static str,
    theme: &'static str,
    language: twitty::ui::chrome::UiLanguage,
) {
    use twitty::render::scene::memory::MemoryView;
    use twitty::render::text::ui::UiFont;
    use twitty::render::ui_style::UiStyle;
    use twitty::ui::memory_panel::MemoryPanelState;
    use twitty_store::{
        FrequentCommand, MemoryEntry, MemoryKind, ProjectMemorySnapshot, RulesFileInfo,
        RulesFileState, AUTO_COMMAND_SOURCE, AUTO_ENV_SOURCE,
    };

    let (width, height) = (920u32, 660u32);
    let surface = (width as f32, height as f32);
    let mut fonts = memory_ui_font_system();
    let palette = Palette::named(theme).expect("named theme");
    let metrics = CellMetrics::measure(&mut fonts, None, 14.5, 1.0);
    let style = UiStyle::new(&palette, None, 1.0);
    let font = UiFont::default();

    let mut term = Terminal::new(96, 28, |_| {});
    term.process_bytes(b"\x1b[?25l");
    term.process_bytes(
        "twitty$ cargo test --workspace\r\n\
         تم تشغيل الاختبارات بنجاح (0 failures)\r\n\
         twitty$ git status --short\r\n"
            .as_bytes(),
    );
    let (lines, cursor) = term.snapshot();
    let panes = [PaneFrame {
        id: PaneId(1),
        rect: Rect::new(0.0, 0.0, surface.0, surface.1),
        lines,
        cursor,
        focused: true,
        status: None,
        blocks: Vec::new(),
    }];

    let mut state = MemoryPanelState::new(
        Some("/home/user/projects/twitty".into()),
        vec!["~/secrets".into(), "**/private/**".into()],
        language,
    );
    state.apply_snapshot(ProjectMemorySnapshot {
        project_root: "/home/user/projects/twitty".into(),
        excluded: false,
        rules_files: vec![
            RulesFileInfo {
                rel_path: "AGENTS.md".into(),
                current_sha256: "a1b2c3d4e5f600001111222233334444".into(),
                approved_sha256: Some("a1b2c3d4e5f600001111222233334444".into()),
                state: RulesFileState::Approved,
                approved_content: Some("Keep grid logical".into()),
            },
            RulesFileInfo {
                rel_path: "CLAUDE.md".into(),
                current_sha256: "ff00ee11dd22cc33bb44aa5599887766".into(),
                approved_sha256: Some("00000000000000000000000000000000".into()),
                state: RulesFileState::Changed,
                approved_content: None,
            },
            RulesFileInfo {
                rel_path: "TWITTY.md".into(),
                current_sha256: "1234567890abcdef1234567890abcdef".into(),
                approved_sha256: None,
                state: RulesFileState::Unapproved,
                approved_content: None,
            },
        ],
        entries: vec![
            MemoryEntry {
                id: 1,
                project_root: "/home/user/projects/twitty".into(),
                kind: MemoryKind::Rule,
                text: match language {
                    twitty::ui::chrome::UiLanguage::English => {
                        "Every UI string must have English and Arabic".into()
                    }
                    twitty::ui::chrome::UiLanguage::Arabic => {
                        "كل نص في الواجهة بالعربية والإنجليزية".into()
                    }
                },
                source: "user".into(),
                created_at: 100,
                updated_at: 200,
                approved: true,
            },
            MemoryEntry {
                id: 2,
                project_root: "/home/user/projects/twitty".into(),
                kind: MemoryKind::Fact,
                text: "Run golden tests with llvmpipe".into(),
                source: "agent".into(),
                created_at: 110,
                updated_at: 210,
                approved: false,
            },
            MemoryEntry {
                id: 3,
                project_root: "/home/user/projects/twitty".into(),
                kind: MemoryKind::Command,
                text: "`cargo test --workspace` (12 runs, 92% ok)".into(),
                source: AUTO_COMMAND_SOURCE.into(),
                created_at: 120,
                updated_at: 220,
                approved: true,
            },
            MemoryEntry {
                id: 4,
                project_root: "/home/user/projects/twitty".into(),
                kind: MemoryKind::Env,
                text: "Rust toolchain: 1.90.0 · Package managers: cargo".into(),
                source: AUTO_ENV_SOURCE.into(),
                created_at: 130,
                updated_at: 230,
                approved: true,
            },
        ],
        frequent_commands: vec![FrequentCommand {
            command: "cargo test --workspace".into(),
            runs: 12,
            successes: 11,
            known_exits: 12,
            success_rate_pct: Some(92),
            last_run_at: 220,
        }],
    });

    let mut ctx = DrawContext {
        font_system: &mut fonts,
        family: None,
        palette: &palette,
        metrics,
        is_srgb: false,
    };
    let opts = FrameOptions {
        window_focused: true,
        cursor_style: "block",
        cursor_blink_on: true,
        surface,
    };
    let mut cache = RowCache::default();
    let mut ime = PreeditState::default();
    let mut frame = build_frame(&mut ctx, &opts, &mut cache, &mut ime, &[], &panes, &[]);
    frame.add_memory(&mut ctx, style, &font, &MemoryView { state });
    let runs = frame.text_runs(&panes, &cache, &ime);

    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let mut target = match Offscreen::new(width, height) {
        Ok(target) => target,
        Err(e) => {
            assert!(
                !env_flag("TWITTY_REQUIRE_GOLDEN"),
                "TWITTY_REQUIRE_GOLDEN is set but the golden tests cannot run: {e}"
            );
            eprintln!("note: skipping golden image {name}: {e}");
            return;
        }
    };
    let actual = target
        .render_layers(
            (&frame.quads, &frame.shapes),
            runs,
            ctx.font_system,
            frame.background,
            1.0,
        )
        .expect("offscreen memory screen render");

    let reference = golden_dir().join(format!("{name}.png"));
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
    let actual_path = out.join(format!("{name}.actual.png"));
    if (ew, eh) != (width, height) {
        write_png(&actual_path, width, height, &actual);
        panic!(
            "{name}: size {width}x{height}, reference is {ew}x{eh} (actual: {})",
            actual_path.display()
        );
    }
    let (bad, diff_image) = diff(&expected, &actual);
    let allowed = ((width * height) as f64 * MAX_BAD_FRACTION) as usize;
    if bad > allowed {
        let diff_path = out.join(format!("{name}.diff.png"));
        write_png(&actual_path, width, height, &actual);
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

#[test]
fn golden_memory_screen_en_dark() {
    check_memory_screen_golden(
        "memory_en_dark",
        "twitty-dark",
        twitty::ui::chrome::UiLanguage::English,
    );
}

#[test]
fn golden_memory_screen_en_light() {
    check_memory_screen_golden(
        "memory_en_light",
        "twitty-light",
        twitty::ui::chrome::UiLanguage::English,
    );
}

#[test]
fn golden_memory_screen_ar_dark() {
    check_memory_screen_golden(
        "memory_ar_dark",
        "twitty-dark",
        twitty::ui::chrome::UiLanguage::Arabic,
    );
}

#[test]
fn golden_memory_screen_ar_light() {
    check_memory_screen_golden(
        "memory_ar_light",
        "twitty-light",
        twitty::ui::chrome::UiLanguage::Arabic,
    );
}

/// The composer's `/` menu over a shell prompt: a Claude skill, an MCP server and Twitty
/// commands, with English or Arabic details.
fn check_slash_menu_golden(name: &str, palette_name: &str, lang: twitty::ui::chrome::UiLanguage) {
    use twitty::composer::slash::{
        suggestions, Agent, Foreground, SlashCatalog, SlashInputs, SlashItem, SlashSource,
    };
    use twitty::render::scene::composer::{ComposerPopupItem, ComposerPopupView, ComposerView};
    use twitty::render::text::ui::UiFont;
    use twitty::render::ui_style::UiStyle;

    let (width, height) = (760u32, 420u32);
    let surface = (width as f32, height as f32);
    let mut fonts = font_system();
    let ui_font = UiFont::default();
    let palette = Palette::named(palette_name).unwrap();
    let metrics = CellMetrics::measure(&mut fonts, None, 14.5, 1.0);
    let style = UiStyle::new(&palette, None, 1.0);
    let rect = Rect::new(0.0, 0.0, surface.0, surface.1);
    let (cols, rows) = metrics.grid_size(rect);
    let mut term = Terminal::new(cols.max(20), rows.max(10), |_| {});
    term.process_bytes(b"\x1b[?25l~/projects/twitty $ \r\n");
    let (lines, cursor) = term.snapshot();
    let panes = [PaneFrame {
        id: PaneId(1),
        rect,
        lines,
        cursor,
        focused: true,
        status: None,
        blocks: Vec::new(),
    }];

    let item = |name: &str, source, description: &str| SlashItem {
        name: name.into(),
        source,
        description: description.into(),
        description_ar: None,
    };
    let catalog = SlashCatalog {
        items: vec![
            item(
                "review",
                SlashSource::Skill(Agent::Claude),
                "Review the current diff for bugs",
            ),
            item(
                "release-notes",
                SlashSource::Command(Agent::Claude),
                "Draft release notes",
            ),
            item("rea", SlashSource::Mcp(Agent::Claude), "MCP · rea"),
        ],
        project_root: None,
    };
    let inputs = SlashInputs {
        catalog: &catalog,
        workflows: &[],
        foreground: Foreground::Shell,
    };
    let query = "re";
    let items = suggestions(query, 1 + query.len(), &inputs);
    let popup = ComposerPopupView {
        items: items
            .iter()
            .take(8)
            .map(|i| ComposerPopupItem {
                kind: i.kind,
                label: i.label.clone(),
                detail: i.localized_detail(lang).map(str::to_owned),
            })
            .collect(),
        selected: 0,
    };

    let mut cache = RowCache::default();
    let mut ime = PreeditState::default();
    let mut ctx = DrawContext {
        font_system: &mut fonts,
        family: None,
        palette: &palette,
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
    frame.add_composer(
        &mut ctx,
        style,
        &ui_font,
        &ComposerView {
            context: None,
            pane_rect: rect,
            lines: vec![format!("/{query}")],
            caret: (0, 1 + query.len()),
            selection: None,
            preedit: None,
            ghost: None,
            highlights: Vec::new(),
            lints: Vec::new(),
            focused: true,
            cursor_blink_on: false,
            popup: Some(popup),
            language: lang,
            ai: Default::default(),
        },
    );
    let runs = frame.text_runs(&panes, &cache, &ime);
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let mut target = match Offscreen::new(width, height) {
        Ok(target) => target,
        Err(e) => {
            assert!(
                !env_flag("TWITTY_REQUIRE_GOLDEN"),
                "TWITTY_REQUIRE_GOLDEN is set but golden test {name} cannot run: {e}"
            );
            eprintln!("note: skipping golden image {name}: {e}");
            return;
        }
    };
    let actual = target
        .render_layers(
            (&frame.quads, &frame.shapes),
            runs,
            ctx.font_system,
            frame.background,
            1.0,
        )
        .expect("offscreen render");
    let (w, h) = target.size();
    compare_golden(name, w, h, &actual);
}

#[test]
fn golden_slash_menu_en_dark_and_ar_light() {
    use twitty::ui::chrome::UiLanguage;
    check_slash_menu_golden("slash_menu_en_dark", "twitty-dark", UiLanguage::English);
    check_slash_menu_golden("slash_menu_ar_light", "twitty-light", UiLanguage::Arabic);
}

/// One Settings section over an empty terminal.
fn check_settings_golden(
    name: &str,
    palette_name: &str,
    lang: twitty::ui::chrome::UiLanguage,
    section: twitty::ui::settings_panel::SettingsSection,
    prepare: impl FnOnce(
        &mut twitty::ui::settings_panel::SettingsPanelState,
        &twitty::config::TwittyConfig,
    ),
) {
    use twitty::config::TwittyConfig;
    use twitty::render::scene::settings::SettingsView;
    use twitty::render::text::ui::UiFont;
    use twitty::render::ui_style::UiStyle;
    use twitty::ui::settings_panel::SettingsPanelState;

    let (width, height) = (900u32, 640u32);
    let surface = (width as f32, height as f32);
    let mut fonts = font_system();
    let ui_font = UiFont::default();
    let palette = Palette::named(palette_name).unwrap();
    let metrics = CellMetrics::measure(&mut fonts, None, 14.5, 1.0);
    let style = UiStyle::new(&palette, None, 1.0);
    let rect = Rect::new(0.0, 0.0, surface.0, surface.1);
    let (cols, rows) = metrics.grid_size(rect);
    let mut term = Terminal::new(cols.max(20), rows.max(10), |_| {});
    term.process_bytes(b"\x1b[?25l");
    let (lines, cursor) = term.snapshot();
    let panes = [PaneFrame {
        id: PaneId(1),
        rect,
        lines,
        cursor,
        focused: true,
        status: None,
        blocks: Vec::new(),
    }];

    let mut cfg = TwittyConfig::default();
    cfg.theme = Some(palette_name.into());
    cfg.project_roots = vec!["~/Projects".into(), "~/src".into()];
    cfg.commands = [
        serde_json::json!({"name": "Run tests", "name_ar": "شغّل الاختبارات", "command": "cargo test --workspace"}),
        serde_json::json!({"name": "Deploy staging", "name_ar": "انشر على التجربة", "command": "./scripts/deploy.sh staging", "mode": "run"}),
    ]
    .iter()
    .map(|v| twitty::workflows::parse_entry(v).unwrap())
    .collect();
    let mut state = SettingsPanelState::new(Vec::new(), Vec::new(), None, lang);
    state.select_section(section, &cfg);
    prepare(&mut state, &cfg);

    let mut cache = RowCache::default();
    let mut ime = PreeditState::default();
    let mut ctx = DrawContext {
        font_system: &mut fonts,
        family: None,
        palette: &palette,
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
    frame.add_settings(
        &mut ctx,
        style,
        &ui_font,
        &SettingsView { state, config: cfg },
    );
    let runs = frame.text_runs(&panes, &cache, &ime);
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let mut target = match Offscreen::new(width, height) {
        Ok(target) => target,
        Err(e) => {
            assert!(
                !env_flag("TWITTY_REQUIRE_GOLDEN"),
                "TWITTY_REQUIRE_GOLDEN is set but golden test {name} cannot run: {e}"
            );
            eprintln!("note: skipping golden image {name}: {e}");
            return;
        }
    };
    let actual = target
        .render_layers(
            (&frame.quads, &frame.shapes),
            runs,
            ctx.font_system,
            frame.background,
            1.0,
        )
        .expect("offscreen render");
    let (w, h) = target.size();
    compare_golden(name, w, h, &actual);
}

#[test]
fn golden_settings_behaviour_ar_light_and_ai_en_dark() {
    use twitty::ui::chrome::UiLanguage;
    use twitty::ui::settings_panel::SettingsSection;
    check_settings_golden(
        "settings_behaviour_ar_light",
        "twitty-light",
        UiLanguage::Arabic,
        SettingsSection::Behaviour,
        |_, _| {},
    );
    check_settings_golden(
        "settings_ai_en_dark",
        "twitty-dark",
        UiLanguage::English,
        SettingsSection::Ai,
        |_, _| {},
    );
}

#[test]
fn golden_settings_commands_en_dark_and_edit_form_ar_light() {
    use twitty::ui::chrome::UiLanguage;
    use twitty::ui::settings_panel::{FocusZone, SettingsSection};
    check_settings_golden(
        "settings_commands_en_dark",
        "twitty-dark",
        UiLanguage::English,
        SettingsSection::Commands,
        |state, _| state.focus = FocusZone::Content,
    );
    check_settings_golden(
        "settings_commands_form_ar_light",
        "twitty-light",
        UiLanguage::Arabic,
        SettingsSection::Commands,
        |state, cfg| {
            state.focus = FocusZone::Content;
            state.commands_editor.start_edit(1, cfg);
        },
    );
}
