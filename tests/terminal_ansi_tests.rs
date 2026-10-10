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
}
