//! Offscreen window previews for design review. Set TWITTY_SHOWCASE_DIR to write static previews and one transition sample.
use std::path::Path;

use twitty::blocks::{BlockDecor, BlockStatus};
use twitty::color::Palette;
use twitty::layout::{self, Node, PaneId, Rect, SidebarPosition, SplitDir};
use twitty::render::gpu::offscreen::Offscreen;
use twitty::render::icons::Icon;
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::frame::{build_frame_with_cards, FrameOptions};
use twitty::render::scene::palette::{PaletteIcons, PaletteItem, PaletteView};
use twitty::render::scene::preedit::PreeditState;
use twitty::render::scene::sidebar::LabelCache;
use twitty::render::scene::window_chrome::{ChromeView, PaneHeaderView};
use twitty::render::text::{fonts::new_font_system, ui::UiFont};
use twitty::render::ui_style::UiStyle;
use twitty::render::{CellMetrics, PaneFrame, RowCache};
use twitty::sidebar::cards::ScrollState;
use twitty::sidebar::{AgentState, AgentStatus, Entry, Notice};
use twitty::terminal::Terminal;
use twitty::ui::chrome::{PaneHeaderLayout, TopBarLayout, UiLanguage, WindowLayout};
use twitty::ui::git_diff::DiffStat;
use twitty::ui::motion::{Tween, PANEL};

#[test]
fn render_window_showcase() {
    let Some(dir) = std::env::var_os("TWITTY_SHOWCASE_DIR").filter(|value| !value.is_empty())
    else {
        return;
    };
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).expect("create showcase output directory");
    for theme in ["twitty-dark", "twitty-light"] {
        let palette = Palette::named(theme).unwrap();
        for (width, height, scale, language) in [
            (1280, 800, 1.0, UiLanguage::English),
            (951, 1002, 1.5, UiLanguage::Arabic),
        ] {
            let path = dir.join(format!("{theme}-{width}x{height}.png"));
            render(&path, width, height, scale, language, &palette, false);
        }
    }
    render(
        &dir.join("twitty-dark-palette-opening-1280x800.png"),
        1280,
        800,
        1.0,
        UiLanguage::English,
        &Palette::named("twitty-dark").unwrap(),
        true,
    );
}

fn render(
    path: &Path,
    width: u32,
    height: u32,
    scale: f32,
    language: UiLanguage,
    palette: &Palette,
    in_motion: bool,
) {
    let mut fonts = new_font_system();
    let font = UiFont::default();
    let metrics = CellMetrics::measure(&mut fonts, None, 14.0, scale);
    let style = UiStyle::new(palette, None, scale);
    let surface = (width as f32, height as f32);
    let (side, dir) = if language == UiLanguage::Arabic {
        (SidebarPosition::Right, SplitDir::Vertical)
    } else {
        (SidebarPosition::Left, SplitDir::Horizontal)
    };
    let entries = workspaces(language);
    let window = WindowLayout::new(metrics, surface, entries.len(), (side, 256.0), true)
        .with_language(language);
    let tree = Node::Split {
        dir,
        ratio: 0.54,
        a: Box::new(Node::Leaf(PaneId(1))),
        b: Box::new(Node::Leaf(PaneId(2))),
    };
    let gap = style.hairline();
    let mut headers = Vec::new();
    let mut panes = Vec::new();
    for (id, rect) in layout::layout(&tree, window.content, gap) {
        let layout = PaneHeaderLayout::new(rect, scale, true).with_language(language);
        let focused = id == PaneId(1);
        headers.push(PaneHeaderView {
            id,
            layout,
            title: if focused {
                language.text("twitty / workspace", "طرفية مساحة العمل")
            } else {
                language.text("test runner", "تشغيل الاختبارات")
            }
            .into(),
            agent: false,
            focused,
            attention: false,
            zoomed: false,
            flash: 0.0,
        });
        panes.push(terminal_pane(id, layout.grid, metrics, focused));
    }
    let dividers: Vec<_> = layout::dividers(&tree, window.content, gap)
        .into_iter()
        .map(|divider| divider.rect)
        .collect();
    let diff = Some(DiffStat {
        added: 34,
        removed: 8,
    });
    let chrome = ChromeView {
        layout: &window,
        top: TopBarLayout::new(window.top_bar, scale, diff).with_language(language),
        diff,
        unread: 1,
        sidebar_visible: true,
        hover: None,
        hover_tab: None,
        language,
        panes: &headers,
        tabs: &[],
    };
    let mut ctx = DrawContext {
        font_system: &mut fonts,
        family: None,
        palette,
        metrics,
        is_srgb: false,
    };
    let area = window.sidebar.unwrap();
    let mut labels = LabelCache::default();
    let cards = labels.cards(&font, ctx.font_system, entries, area, scale);
    let sidebar = ScrollState::default().view(&cards, 0, area, scale, side, language);
    let mut rows = RowCache::default();
    let mut ime = PreeditState::default();
    let opts = FrameOptions {
        window_focused: true,
        cursor_style: "beam",
        cursor_blink_on: true,
        surface,
    };
    let mut frame = build_frame_with_cards(
        &mut ctx,
        &opts,
        &mut rows,
        &mut ime,
        &[],
        &panes,
        &dividers,
        style,
        &font,
        None,
        language,
    );
    frame.add_sidebar(&mut ctx, &sidebar, &mut labels, style, &font);
    frame.add_chrome(&mut ctx, style, &font, &chrome);
    if in_motion {
        let start = std::time::Instant::now();
        let mut transition = Tween::default();
        transition.set(1.0, start, PANEL);
        let commands = [
            "composer.open",
            "project.open",
            "layout.save",
            "input.broadcast",
        ];
        let rows: Vec<_> = twitty::commands::all()
            .into_iter()
            .filter(|command| commands.contains(&command.name))
            .collect();
        let icons = PaletteIcons {
            leading: Icon::Search,
            rows: rows
                .iter()
                .map(|command| Icon::for_command(command.name))
                .collect(),
        };
        let view = PaletteView {
            items: rows
                .into_iter()
                .map(|command| PaletteItem {
                    title_en: command.title_en.into(),
                    title_ar: command.title_ar.into(),
                    shortcut: command.shortcut.map(str::to_owned),
                    dimmed: false,
                })
                .collect(),
            ..PaletteView::default()
        };
        frame.add_palette_motion(
            &mut ctx,
            style,
            &font,
            &view,
            &icons,
            transition.value(start + PANEL / 3),
        );
    }

    let pixels = Offscreen::new(width, height)
        .expect("allocate offscreen showcase target")
        .render_layers(
            (&frame.quads, &frame.shapes),
            frame.text_runs(&panes, &rows, &ime),
            ctx.font_system,
            frame.background,
            1.0,
        )
        .expect("render window showcase");
    let file = std::fs::File::create(path).expect("create showcase PNG");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&pixels)
        .unwrap();
}

fn workspaces(language: UiLanguage) -> Vec<Entry> {
    let mut active = Entry::new(
        language.text("Terminal rendering", "مراجعة عرض الطرفية"),
        Some(Path::new("/src/twitty")),
        true,
        None,
        [],
    );
    active.branch = Some("look-v2".into());
    active.last_command = Some("cargo test --test bidi_layout_tests".into());

    let mut done = Entry::new(
        language.text("Shell integration", "تكامل الصدفة"),
        Some(Path::new("/src/shell")),
        false,
        Some(0),
        [],
    );
    done.branch = Some("main".into());

    let mut error = Entry::new(
        language.text("Release build", "بناء الإصدار"),
        Some(Path::new("/src/twitty")),
        false,
        Some(1),
        [],
    );
    error.branch = Some("release".into());

    let mut waiting = Entry::new(
        "مراجعة التغييرات",
        Some(Path::new("/src/docs")),
        false,
        None,
        [Notice::new(
            None,
            language.text("Approve the proposed changes", "هل أتابع تعديل التوثيق؟"),
        )],
    )
    .with_agent_status(Some(AgentStatus::new(AgentState::NeedsInput, None)));
    waiting.branch = Some("docs".into());
    vec![active, done, error, waiting]
}

fn terminal_pane(id: PaneId, grid: Rect, metrics: CellMetrics, focused: bool) -> PaneFrame {
    let (cols, rows) = metrics.grid_size(grid);
    let mut term = Terminal::new(cols, rows, |_| {});
    let (script, blocks) = if focused {
        (
            concat!(
                "\x1b[36m~/twitty\x1b[0m $ git status --short\r\n",
                " \x1b[33mM\x1b[0m  src/render/ui_style.rs\r\n",
                " \x1b[33mM\x1b[0m  docs/design/LOOK.md\r\n\r\n",
                "\x1b[36m~/twitty\x1b[0m $ cat notes.txt\r\n",
                "مَرْحَبًا — عرض النص العربي\r\n",
                "Bidirectional rendering ready\r\n\r\n",
                "\x1b[36m~/twitty\x1b[0m $ ",
            ),
            vec![
                block(1, 0, 2, BlockStatus::Succeeded, None),
                block(2, 4, 6, BlockStatus::Succeeded, Some("0.4s")),
            ],
        )
    } else {
        (
            concat!(
                "\x1b[36m~/twitty\x1b[0m $ cargo test --lib\r\n",
                "running 24 tests\r\n",
                "\x1b[31mFAILED\x1b[0m paths::expand_home\r\n",
                "fixture directory missing\r\n\r\n",
                "\x1b[36m~/twitty\x1b[0m $ ",
            ),
            vec![block(
                3,
                0,
                3,
                BlockStatus::Failed(101),
                Some("2.3s · exit 101"),
            )],
        )
    };
    term.process_bytes(script.as_bytes());
    let (lines, cursor) = term.snapshot();
    PaneFrame {
        id,
        rect: grid,
        lines,
        cursor,
        focused,
        status: None,
        blocks,
    }
}

fn block(
    id: u64,
    first_row: i64,
    last_row: i64,
    status: BlockStatus,
    meta: Option<&str>,
) -> BlockDecor {
    BlockDecor {
        id,
        first_row,
        last_row,
        status,
        selected: false,
        badge: None,
        meta: meta.map(str::to_owned),
        has_command: true,
    }
}
