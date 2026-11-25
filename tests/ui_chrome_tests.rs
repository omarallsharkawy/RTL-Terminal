//! Window-free coverage for the shared chrome geometry, typography, tokens and Git parsing.

use twitty::color::{Palette, Rgba};
use twitty::config::TwittyConfig;
use twitty::layout::{Node, PaneId, Rect, SidebarPosition};
use twitty::render::icons::Icon;
use twitty::render::scene::context::DrawContext;
use twitty::render::scene::frame::{build_frame, FrameOptions};
use twitty::render::scene::palette::{PaletteHit, PaletteLayout};
use twitty::render::scene::preedit::PreeditState;
use twitty::render::scene::window_chrome::ChromeView;
use twitty::render::shapes::{append_instances, Border, Shadow, Shape};
use twitty::render::text::fonts::new_font_system;
use twitty::render::text::ui::{label_width, UiAlign, UiFont, UiLabel, UiSize, UiWeight};
use twitty::render::ui_style::UiStyle;
use twitty::render::{CellMetrics, RowCache, TabLabel};
use twitty::ui::chrome::{
    count_badge, ChromeAction, PaneHeaderLayout, SearchContents, SearchPlaceholder, TopBarLayout,
    UiLanguage, WindowLayout, SEARCH_AR, SEARCH_EN, SEARCH_KEYCAP,
};
use twitty::ui::git_diff::{parse_numstat, DiffStat, MAX_FILES, MAX_LINES, MAX_OUTPUT};
use twitty::ui::hit::{hit_test, Hit, HitGeometry};

fn metrics(scale: f32) -> CellMetrics {
    CellMetrics {
        font_size: 14.0,
        scale,
        char_width: 10.0 * scale,
        line_height: 20.0 * scale,
        padding_left: 8.0 * scale,
        padding_top: 6.0 * scale,
    }
}

fn center(rect: Rect) -> (f64, f64) {
    (
        f64::from(rect.x + rect.w / 2.0),
        f64::from(rect.y + rect.h / 2.0),
    )
}

#[test]
fn shape_bounds_and_batches_preserve_painter_order_at_multiple_scales() {
    let color = [0.8, 0.3, 0.3, 0.8];
    for scale in [1.0, 1.5, 2.0] {
        let px = |logical: f32| logical * scale;
        let rounded = Shape::RoundedRect {
            rect: Rect::new(px(32.0), px(16.0), px(40.0), px(20.0)),
            radii: [px(6.0), px(3.0), 0.0, px(10.0)],
            fill: color,
            border: Some(Border {
                width: px(2.0),
                color,
            }),
            shadow: Some(Shadow {
                blur: px(4.0),
                offset: [px(3.0), px(-2.0)],
                color,
            }),
        };
        let capsule = Shape::Capsule {
            from: [px(5.0), px(5.0)],
            to: [px(8.0), px(3.0)],
            width: px(2.0),
            color,
        };
        let empty = Shape::rounded(Rect::new(0.0, 0.0, 10.0, 0.0), 0.0, color);
        let mut batch = Vec::new();
        assert_eq!(
            append_instances(&[rounded, empty, capsule], &mut batch),
            0..2
        );
        assert_eq!(append_instances(&[], &mut batch), 2..2);
        assert_eq!(append_instances(&[capsule], &mut batch), 2..3);
        assert_eq!(
            batch[0].bounds_rect(),
            Rect::new(
                px(31.0) - 1.0,
                px(10.0) - 1.0,
                px(48.0) + 2.0,
                px(28.0) + 2.0
            )
        );
        assert_eq!(
            batch[1].bounds_rect(),
            Rect::new(px(4.0) - 1.0, px(2.0) - 1.0, px(5.0) + 2.0, px(4.0) + 2.0)
        );
        assert_eq!(batch[1], batch[2]);
    }
}

#[test]
fn every_icon_stays_in_its_box() {
    for icon in [
        Icon::Menu,
        Icon::Search,
        Icon::Bell,
        Icon::SplitRight,
        Icon::SplitDown,
        Icon::Close,
        Icon::GitBranch,
        Icon::Folder,
        Icon::Plus,
        Icon::Check,
        Icon::Warning,
        Icon::Terminal,
        Icon::Agent,
        Icon::Diff,
        Icon::Copy,
        Icon::CopyCommand,
        Icon::Rerun,
        Icon::Zoom,
    ] {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for size in [12.0, 16.0, 20.0] {
                let rect = Rect::new(10.0, 20.0, size * scale, size * scale);
                let mut shapes = Vec::new();
                icon.draw(rect, [1.0; 4], &mut shapes);
                assert!(!shapes.is_empty());
                for bounds in shapes.into_iter().map(|shape| shape.bounds().unwrap()) {
                    assert!(
                        bounds.x >= rect.x && bounds.y >= rect.y,
                        "{icon:?}: {bounds:?}"
                    );
                    assert!(
                        bounds.right() <= rect.right() && bounds.bottom() <= rect.bottom(),
                        "{icon:?}: {bounds:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn theme_tokens_have_the_same_hierarchy_in_light_and_dark() {
    for dark in [true, false] {
        let palette = Palette::named(if dark { "tokyo-night" } else { "github-light" }).unwrap();
        let style = UiStyle::new(&palette, None, 1.25);
        let luminance = |r: Rgba| 0.2126 * r.r + 0.7152 * r.g + 0.0722 * r.b;
        for pair in [
            style.bg0,
            style.bg1,
            style.bg2,
            style.bg3,
            style.text_faint,
            style.text_dim,
            style.text,
        ]
        .windows(2)
        {
            assert_eq!(luminance(pair[0]) < luminance(pair[1]), dark);
        }
        assert_eq!(style.bg0, palette.background);
        assert_eq!(style.accent, palette.cursor);
        assert_eq!(
            (style.danger, style.warn, style.ok),
            (palette.ansi[1], palette.ansi[3], palette.ansi[2])
        );
        let override_color = Rgba::from_rgb8(55, 122, 90);
        assert_eq!(
            UiStyle::new(&palette, Some(override_color), 1.25).accent,
            override_color
        );
        assert_eq!(style.px(8.0), 10.0);
    }
}

#[test]
fn proportional_ui_text_scales_and_arabic_starts_on_the_right() {
    let mut fonts = new_font_system();
    let font = UiFont::new(&mut fonts, None);
    let shape = |fonts: &mut _, text, scale| {
        font.shape(fonts, UiSize::Body, UiWeight::Semibold, scale, text)
    };
    assert!(
        label_width(&shape(&mut fonts, "iiii", 1.0)) < label_width(&shape(&mut fonts, "WWWW", 1.0))
    );
    let normal = shape(&mut fonts, "مرحبًا بالعالم", 1.0);
    let double = shape(&mut fonts, "مرحبًا بالعالم", 2.0);
    assert!(normal.layout_runs().next().unwrap().rtl);
    assert!(double
        .layout_runs()
        .next()
        .unwrap()
        .glyphs
        .iter()
        .all(|glyph| glyph.glyph_id != 0));
    assert_eq!(double.metrics().font_size, 2.0 * normal.metrics().font_size);
    let text = UiLabel::new(
        normal,
        Rect::new(10.0, 10.0, 240.0, 28.0),
        UiAlign::Start,
        Rgba::from_rgb8(255, 255, 255),
    );
    assert!((text.left + label_width(&text.buffer) - 250.0).abs() <= 1.0);

    let mut db = cosmic_text::fontdb::Database::new();
    db.load_font_data(include_bytes!("golden/fonts/NotoSansMono-Subset.ttf").to_vec());
    db.load_font_data(include_bytes!("../assets/fonts/NotoNaskhArabic-Regular.ttf").to_vec());
    let mut fonts = cosmic_text::FontSystem::new_with_locale_and_db("en-US".into(), db);
    let font = UiFont::new(&mut fonts, Some("Noto Sans Mono"));
    let buffer = font.shape(&mut fonts, UiSize::Body, UiWeight::Semibold, 1.0, "عربي");
    let arabic = buffer.layout_runs().next().unwrap();
    assert!(!arabic.glyphs.is_empty());
    assert!(arabic.glyphs.iter().all(|glyph| {
        glyph.glyph_id != 0
            && fonts
                .db()
                .face(glyph.font_id)
                .unwrap()
                .families
                .iter()
                .any(|(family, _)| family == "Noto Naskh Arabic")
    }));
}

#[test]
fn grid_geometry_accounts_for_headers_and_sidebar() {
    let cfg: TwittyConfig = serde_json::from_str("{}").unwrap();
    assert!(cfg.top_bar && cfg.pane_headers);
    assert!(cfg.ui_font_family.is_none() && cfg.ui_accent.is_none());
    for scale in [1.0, 1.25, 2.0] {
        let metrics = metrics(scale);
        let size = (800.0 * scale, 600.0 * scale);
        let with_sidebar =
            WindowLayout::new(metrics, size, 2, (SidebarPosition::Right, 180.0), true);
        assert_eq!(with_sidebar.top_bar.h, 36.0 * scale);
        assert_eq!(with_sidebar.sidebar.unwrap().y, 36.0 * scale);
        assert!(with_sidebar.tabs.is_empty());
        assert_eq!(with_sidebar.tab_bar.h, 0.0);
        let with_tabs = WindowLayout::new(metrics, size, 2, (SidebarPosition::Hidden, 180.0), true);
        assert_eq!(with_tabs.tabs.len(), 2);
        assert_eq!(with_tabs.tabs[0].y, 40.0 * scale);
        assert_eq!(with_tabs.content.y, 68.0 * scale);
        let pane = PaneHeaderLayout::new(with_sidebar.content, scale, true);
        assert_eq!(pane.header.h, 28.0 * scale);
        assert_eq!(pane.grid.y, 64.0 * scale);
        assert_eq!(pane.grid.bottom(), with_sidebar.content.bottom());
        assert_eq!(metrics.grid_size(pane.grid), (60, 26));
        let no_chrome =
            WindowLayout::new(metrics, size, 0, (SidebarPosition::Hidden, 220.0), false);
        let pane = PaneHeaderLayout::new(no_chrome.content, scale, false);
        assert_eq!(pane.grid, Rect::new(0.0, 0.0, size.0, size.1));
        assert_eq!(metrics.grid_size(pane.grid), (78, 29));
    }
}

#[test]
fn chrome_hits_are_distinct_from_terminal_hits() {
    let scale = 1.25;
    let window = WindowLayout::new(
        metrics(scale),
        (1000.0, 700.0),
        2,
        (SidebarPosition::Hidden, 220.0),
        true,
    );
    let top = TopBarLayout::new(
        window.top_bar,
        scale,
        Some(DiffStat {
            added: 413,
            removed: 56,
        }),
    );
    assert_eq!(center(top.search).0, center(top.area).0);
    for (rect, action) in [
        (top.sidebar, ChromeAction::Sidebar),
        (top.search, ChromeAction::Search),
        (top.diff.unwrap(), ChromeAction::Diff),
        (top.notifications, ChromeAction::Notifications),
    ] {
        let (x, y) = center(rect);
        assert_eq!(top.hit(x, y), Some(Hit::Chrome(action)));
    }
    assert_eq!(top.hit(0.0, 0.0), Some(Hit::ChromeBackground));
    assert_eq!(top.hit(0.0, f64::from(top.area.bottom())), None);
    assert!(TopBarLayout::new(window.top_bar, scale, None)
        .diff
        .is_none());

    let pane = PaneHeaderLayout::new(window.content, scale, true);
    let id = PaneId(1);
    for (rect, action) in [
        (pane.split_right, ChromeAction::SplitRight(id)),
        (pane.split_down, ChromeAction::SplitDown(id)),
        (pane.close, ChromeAction::Close(id)),
        (pane.title, ChromeAction::FocusPane(id)),
    ] {
        let (x, y) = center(rect);
        assert_eq!(pane.hit(id, x, y), Some(action));
    }
    assert_eq!(pane.hit(id, 0.0, f64::from(pane.grid.y)), None);
    let (x, y) = center(window.tabs[0]);
    assert_eq!(window.tab_hit(x, y), Some(Hit::Tab(0)));
    let geometry = HitGeometry {
        tab_bar_height: 0.0,
        tabs: Vec::new(),
        content: window.content,
        gap: 1.0,
        slop: 2.0,
    };
    assert_eq!(
        hit_test(
            &geometry,
            Some(&Node::Leaf(id)),
            1.0,
            f64::from(pane.grid.y)
        ),
        Hit::Pane(id)
    );

    let overlay = PaletteLayout::new(scale, (1000.0, 380.0), 10);
    assert!(overlay.rows() > 0 && overlay.rows() < 10);
    let (x, y) = center(overlay.search);
    assert_eq!(overlay.hit(x, y), PaletteHit::Inside);
    for index in 0..overlay.rows() {
        let (x, y) = center(overlay.row(index).unwrap());
        assert_eq!(overlay.hit(x, y), PaletteHit::Item(index));
    }
    assert_eq!(overlay.hit(0.0, 0.0), PaletteHit::Outside);
}

#[test]
fn pill_tabs_have_disjoint_tab_close_and_new_tab_hits() {
    for (scale, width, count) in [(1.0, 800.0, 1), (1.25, 600.0, 4), (2.0, 380.0, 20)] {
        let layout = WindowLayout::new(
            metrics(scale),
            (width, 600.0),
            count,
            (SidebarPosition::Hidden, 220.0),
            true,
        );
        assert!(layout.new_tab.right() <= layout.tab_bar.right());
        let (x, y) = center(layout.new_tab);
        assert_eq!(
            layout.tab_hit(x, y),
            Some(Hit::Chrome(ChromeAction::NewTab))
        );
        for (i, &tab) in layout.tabs.iter().enumerate() {
            let close = layout.tab_close(tab);
            let (x, y) = center(close);
            assert_eq!(
                layout.tab_hit(x, y),
                Some(Hit::Chrome(ChromeAction::CloseTab(i)))
            );
            let x = f64::from((tab.x + close.x) / 2.0);
            assert_eq!(layout.tab_hit(x, y), Some(Hit::Tab(i)));
        }
        assert_eq!(
            layout.tab_hit(0.0, f64::from(layout.tab_bar.y)),
            Some(Hit::TabBar)
        );
        assert_eq!(layout.tab_hit(0.0, f64::from(layout.content.y)), None);
    }
}

#[test]
fn search_placeholder_and_badges_follow_width_locale_and_counts() {
    for (locale, expected) in [
        ("ar_EG.UTF-8", UiLanguage::Arabic),
        ("ar", UiLanguage::Arabic),
        ("en_US.UTF-8", UiLanguage::English),
        ("C", UiLanguage::English),
    ] {
        assert_eq!(UiLanguage::from_locale(locale), expected);
    }
    assert_eq!(
        SearchContents::fit(299.0, 300.0, 150.0, 75.0).placeholder,
        SearchPlaceholder::Local
    );
    assert_eq!(
        SearchContents::fit(375.0, 300.0, 150.0, 75.0),
        SearchContents {
            placeholder: SearchPlaceholder::Bilingual,
            keycap: true
        }
    );
    assert_eq!(
        SearchContents::fit(300.0, 300.0, 150.0, 75.0),
        SearchContents {
            placeholder: SearchPlaceholder::Bilingual,
            keycap: false
        }
    );
    assert_eq!(
        SearchContents::fit(74.0, 300.0, 150.0, 75.0),
        SearchContents {
            placeholder: SearchPlaceholder::LocalShortened,
            keycap: false
        }
    );
    assert_eq!(count_badge(0), None);
    assert_eq!(count_badge(2).as_deref(), Some("2"));
    assert_eq!(count_badge(100).as_deref(), Some("99+"));

    let area = Rect::new(0.0, 0.0, 800.0, 36.0);
    assert!(TopBarLayout::new(area, 1.0, Some(DiffStat::default()))
        .diff
        .is_none());
    for stat in [
        DiffStat {
            added: 1,
            removed: 0,
        },
        DiffStat {
            added: 0,
            removed: 1,
        },
    ] {
        assert!(TopBarLayout::new(area, 1.0, Some(stat)).diff.is_some());
    }
}

#[test]
fn rendered_search_prefers_complete_labels_and_bell_count_is_conditional() {
    let mut fonts = new_font_system();
    let font = UiFont::default();
    let palette = Palette::default();
    let metrics = metrics(1.0);
    let mut ctx = DrawContext {
        font_system: &mut fonts,
        family: None,
        palette: &palette,
        metrics,
        is_srgb: false,
    };
    let placeholder_w = |ctx: &mut DrawContext, text| {
        label_width(&font.shape(
            ctx.font_system,
            UiSize::Caption,
            UiWeight::Regular,
            1.0,
            text,
        ))
        .ceil()
    };
    let en_w = placeholder_w(&mut ctx, SEARCH_EN);
    let ar_w = placeholder_w(&mut ctx, SEARCH_AR);
    for (field_width, language, unread, expected) in [
        (
            456.0,
            UiLanguage::English,
            99,
            vec![SEARCH_KEYCAP, SEARCH_EN, "/", SEARCH_AR, "99"],
        ),
        (en_w + 39.0, UiLanguage::English, 0, vec![SEARCH_EN]),
        (ar_w + 39.0, UiLanguage::Arabic, 0, vec![SEARCH_AR]),
        (47.0, UiLanguage::Arabic, 0, vec![]),
    ] {
        let surface = (900.0, 300.0);
        let layout = WindowLayout::new(metrics, surface, 1, (SidebarPosition::Hidden, 220.0), true);
        let mut top = TopBarLayout::new(layout.top_bar, metrics.scale, Some(DiffStat::default()));
        top.search.w = field_width;
        let tabs = [TabLabel {
            title: "Shell".to_owned(),
            active: true,
        }];
        let view = ChromeView {
            layout: &layout,
            top,
            diff: Some(DiffStat::default()),
            unread,
            sidebar_visible: false,
            hover: None,
            hover_tab: Some(0),
            language,
            panes: &[],
            tabs: &tabs,
        };
        let opts = FrameOptions {
            window_focused: true,
            cursor_style: "block",
            cursor_blink_on: true,
            surface,
        };
        let mut rows = RowCache::default();
        let mut ime = PreeditState::default();
        let mut frame = build_frame(&mut ctx, &opts, &mut rows, &mut ime, &[], &[], &[]);
        frame.add_chrome(&mut ctx, UiStyle::new(&palette, None, 1.0), &font, &view);
        let runs = frame.text_runs(&[], &rows, &ime);
        let labels: Vec<_> = runs
            .iter()
            .filter(|run| run.bounds.bottom <= layout.top_bar.bottom() as i32)
            .map(|run| run.buffer.lines[0].text())
            .collect();
        assert_eq!(labels, expected, "search field {field_width}, {language:?}");
    }
}

#[test]
fn git_numstat_handles_binary_files_renames_and_bounds() {
    let expected = Some(DiffStat {
        added: 9,
        removed: 7,
    });
    assert_eq!(
        parse_numstat(b"7\t4\tsrc/old.rs => src/new.rs\n-\t-\timage.png\n2\t3\ta\n"),
        expected
    );
    assert_eq!(
        parse_numstat(b"7\t4\t\0old\nname\0new\tname\0-\t-\timage.png\x002\t3\ta\0"),
        expected
    );
    assert_eq!(parse_numstat(b""), Some(DiffStat::default()));
    assert_eq!(parse_numstat(b"7\t4\t\0old\0"), None);
    assert_eq!(parse_numstat(b"7\t4\tfile\0-\t-\tother"), None);
    assert_eq!(parse_numstat(b"-\t4\tfile\n"), None);
    assert_eq!(parse_numstat(&vec![b'a'; MAX_OUTPUT + 1]), None);
    assert!(parse_numstat(&b"1\t1\tf\n".repeat(MAX_FILES)).is_some());
    assert_eq!(parse_numstat(&b"1\t1\tf\n".repeat(MAX_FILES + 1)), None);
    let capped = parse_numstat(b"999999999999999999\t999999\ta\n1\t1\tb\n").unwrap();
    assert_eq!(
        capped,
        DiffStat {
            added: MAX_LINES + 1,
            removed: MAX_LINES + 1
        }
    );
    assert_eq!(capped.label(), "+999999+ −999999+");
}
