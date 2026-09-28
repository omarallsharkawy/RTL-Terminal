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

    pub fn to_linear(self) -> [f32; 4] {
        let conv = |c: f32| {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        [conv(self.r), conv(self.g), conv(self.b), self.a]
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
        // Deep dark terminal palette (Tokyo Night / OpenCode compatible)
        Self {
            background: Rgba::from_rgb8(21, 22, 30),     // #15161e (crisp dark background)
            foreground: Rgba::from_rgb8(192, 202, 245),  // #c0caf5
            cursor: Rgba::from_rgb8(245, 224, 220),      // #f5e0dc
            ansi: [
                Rgba::from_rgb8(21, 22, 30),     // 0: Black
                Rgba::from_rgb8(247, 118, 142),  // 1: Red
                Rgba::from_rgb8(158, 206, 106),  // 2: Green
                Rgba::from_rgb8(224, 175, 104),  // 3: Yellow
                Rgba::from_rgb8(122, 162, 247),  // 4: Blue
                Rgba::from_rgb8(187, 154, 247),  // 5: Magenta
                Rgba::from_rgb8(125, 207, 255),  // 6: Cyan
                Rgba::from_rgb8(169, 177, 214),  // 7: White
                Rgba::from_rgb8(65, 72, 104),    // 8: Bright Black (gutter/borders)
                Rgba::from_rgb8(247, 118, 142),  // 9: Bright Red
                Rgba::from_rgb8(158, 206, 106),  // 10: Bright Green
                Rgba::from_rgb8(224, 175, 104),  // 11: Bright Yellow
                Rgba::from_rgb8(122, 162, 247),  // 12: Bright Blue
                Rgba::from_rgb8(187, 154, 247),  // 13: Bright Magenta
                Rgba::from_rgb8(125, 207, 255),  // 14: Bright Cyan
                Rgba::from_rgb8(192, 202, 245),  // 15: Bright White
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
