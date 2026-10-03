use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor};
use twitty::color::{Palette, Rgba};
use twitty::terminal::Terminal;

#[test]
fn test_terminal_ansi_colors() {
    let mut term = Terminal::new(80, 24, |_| {});

    // Output green text
    term.process_bytes(b"\x1b[32mGreen\x1b[0m\r\n");
    let (lines, _) = term.snapshot();

    assert_eq!(lines[0].cells[0].c, 'G');
    assert_eq!(lines[0].cells[0].fg, AnsiColor::Named(NamedColor::Green));

    // Output TrueColor RGB text (r=123, g=45, b=67)
    term.process_bytes(b"\x1b[38;2;123;45;67mCustomRGB\x1b[0m\r\n");
    let (lines, _) = term.snapshot();
    assert_eq!(lines[1].cells[0].c, 'C');
    if let AnsiColor::Spec(rgb) = lines[1].cells[0].fg {
        assert_eq!(rgb.r, 123);
        assert_eq!(rgb.g, 45);
        assert_eq!(rgb.b, 67);
    } else {
        panic!("Expected TrueColor RGB");
    }
}

#[test]
fn test_terminal_cursor_tracking() {
    let mut term = Terminal::new(80, 24, |_| {});

    // Move cursor to row 5, col 10 (1-based in ANSI: \x1b[5;10H)
    term.process_bytes(b"\x1b[5;10H");
    let (_, cursor) = term.snapshot();
    assert_eq!(cursor.row, 4); // 0-based
    assert_eq!(cursor.col, 9); // 0-based
}

#[test]
fn test_palette_resolution() {
    let palette = Palette::default();

    let fg = palette.resolve(AnsiColor::Named(NamedColor::Foreground), false);
    assert_eq!(fg, palette.foreground);

    let bg = palette.resolve(AnsiColor::Named(NamedColor::Background), true);
    assert_eq!(bg, palette.background);

    let red = palette.resolve(AnsiColor::Named(NamedColor::Red), false);
    assert_eq!(red, palette.ansi[1]);

    let rgb = palette.resolve(
        AnsiColor::Spec(alacritty_terminal::vte::ansi::Rgb {
            r: 255,
            g: 0,
            b: 128,
        }),
        false,
    );
    assert_eq!(rgb, Rgba::from_rgb8(255, 0, 128));

    // Exact 256 color cube resolution: index 196 is standard xterm pure red #ff0000
    let color_196 = palette.resolve(AnsiColor::Indexed(196), false);
    assert_eq!(
        color_196,
        Rgba::from_rgb8(255, 0, 0),
        "Index 196 must resolve to exact pure red #ff0000"
    );

    // Index 21 is pure blue (r=0, g=0, b=5)
    let color_21 = palette.resolve(AnsiColor::Indexed(21), false);
    assert_eq!(color_21, Rgba::from_rgb8(0, 0, 255));

    // Grayscale ramp test: index 232 is 8, 8, 8
    let color_232 = palette.resolve(AnsiColor::Indexed(232), false);
    assert_eq!(color_232, Rgba::from_rgb8(8, 8, 8));
}

#[test]
fn test_ansi_muted_and_black_contrast() {
    let palette = Palette::default();

    // ANSI 0 (Black) must be clearly distinct from the background color
    let ansi_0 = palette.resolve(AnsiColor::Named(NamedColor::Black), false);
    assert_ne!(
        ansi_0, palette.background,
        "ANSI 0 must be distinct from terminal background"
    );

    // ANSI 8 (Bright Black / Muted Gray) used for CLI secondary info must have crisp luminance
    let ansi_8 = palette.resolve(AnsiColor::Named(NamedColor::BrightBlack), false);
    assert!(
        ansi_8.r > 0.4 && ansi_8.g > 0.4 && ansi_8.b > 0.5,
        "ANSI 8 must have high visibility for CLI muted labels, got: {:?}",
        ansi_8
    );
}
