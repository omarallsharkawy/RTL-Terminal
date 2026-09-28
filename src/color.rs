use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor};

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    #[allow(dead_code)]
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub fn from_rgb8(r: u8, g: u8, b: u8) -> Self {
        Self {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: 1.0,
        }
    }

    pub fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    pub fn to_glyphon(self) -> glyphon::Color {
        glyphon::Color::rgba(
            (self.r * 255.0) as u8,
            (self.g * 255.0) as u8,
            (self.b * 255.0) as u8,
            (self.a * 255.0) as u8,
        )
    }
}

pub struct Palette {
    pub background: Rgba,
    pub foreground: Rgba,
    pub cursor: Rgba,
    pub ansi: [Rgba; 16],
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            background: Rgba::from_rgb8(30, 30, 46),
            foreground: Rgba::from_rgb8(205, 214, 244),
            cursor: Rgba::from_rgb8(245, 224, 220),
            ansi: [
                Rgba::from_rgb8(69, 71, 90),
                Rgba::from_rgb8(243, 139, 168),
                Rgba::from_rgb8(166, 227, 161),
                Rgba::from_rgb8(249, 226, 175),
                Rgba::from_rgb8(137, 180, 250),
                Rgba::from_rgb8(245, 194, 231),
                Rgba::from_rgb8(148, 226, 213),
                Rgba::from_rgb8(186, 194, 222),
                Rgba::from_rgb8(88, 91, 112),
                Rgba::from_rgb8(243, 139, 168),
                Rgba::from_rgb8(166, 227, 161),
                Rgba::from_rgb8(249, 226, 175),
                Rgba::from_rgb8(137, 180, 250),
                Rgba::from_rgb8(245, 194, 231),
                Rgba::from_rgb8(148, 226, 213),
                Rgba::from_rgb8(166, 173, 200),
            ],
        }
    }
}

impl Palette {
    pub fn resolve(&self, color: AnsiColor, is_bg: bool) -> Rgba {
        match color {
            AnsiColor::Named(named) => match named {
                NamedColor::Black => self.ansi[0],
                NamedColor::Red => self.ansi[1],
                NamedColor::Green => self.ansi[2],
                NamedColor::Yellow => self.ansi[3],
                NamedColor::Blue => self.ansi[4],
                NamedColor::Magenta => self.ansi[5],
                NamedColor::Cyan => self.ansi[6],
                NamedColor::White => self.ansi[7],
                NamedColor::BrightBlack => self.ansi[8],
                NamedColor::BrightRed => self.ansi[9],
                NamedColor::BrightGreen => self.ansi[10],
                NamedColor::BrightYellow => self.ansi[11],
                NamedColor::BrightBlue => self.ansi[12],
                NamedColor::BrightMagenta => self.ansi[13],
                NamedColor::BrightCyan => self.ansi[14],
                NamedColor::BrightWhite => self.ansi[15],
                NamedColor::Foreground => self.foreground,
                NamedColor::Background => self.background,
                NamedColor::Cursor => self.cursor,
                _ => {
                    if is_bg {
                        self.background
                    } else {
                        self.foreground
                    }
                }
            },
            AnsiColor::Spec(rgb) => Rgba::from_rgb8(rgb.r, rgb.g, rgb.b),
            AnsiColor::Indexed(idx) => {
                if (idx as usize) < 16 {
                    self.ansi[idx as usize]
                } else if idx >= 16 && idx <= 231 {
                    let i = idx - 16;
                    let r = (i / 36) % 6;
                    let g = (i / 6) % 6;
                    let b = i % 6;
                    let conv = |v: u8| if v == 0 { 0 } else { v * 40 + 55 };
                    Rgba::from_rgb8(conv(r), conv(g), conv(b))
                } else {
                    let gray = (idx - 232) * 10 + 8;
                    Rgba::from_rgb8(gray, gray, gray)
                }
            }
        }
    }
}
