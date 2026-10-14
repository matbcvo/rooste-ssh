//! Colors: the 16 ANSI colors, the xterm 256-color cube and default
//! foreground, background, cursor and selection colors.

/// An sRGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Scale toward black, used for the SGR "dim" attribute.
    pub fn dimmed(self) -> Self {
        let f = |c: u8| (u16::from(c) * 2 / 3) as u8;
        Self::new(f(self.r), f(self.g), f(self.b))
    }
}

/// A color scheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub selection_background: Rgb,
    pub selection_foreground: Rgb,
    /// Black, red, green, yellow, blue, magenta, cyan, white, then the bright
    /// variants in the same order.
    pub ansi: [Rgb; 16],
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            foreground: Rgb::new(0xd8, 0xd8, 0xd8),
            background: Rgb::new(0x1c, 0x1c, 0x1c),
            cursor: Rgb::new(0xe0, 0xe0, 0xe0),
            selection_background: Rgb::new(0x44, 0x5a, 0x80),
            selection_foreground: Rgb::new(0xff, 0xff, 0xff),
            ansi: [
                Rgb::new(0x00, 0x00, 0x00),
                Rgb::new(0xcd, 0x31, 0x31),
                Rgb::new(0x0d, 0xbc, 0x79),
                Rgb::new(0xe5, 0xe5, 0x10),
                Rgb::new(0x24, 0x72, 0xc8),
                Rgb::new(0xbc, 0x3f, 0xbc),
                Rgb::new(0x11, 0xa8, 0xcd),
                Rgb::new(0xe5, 0xe5, 0xe5),
                Rgb::new(0x66, 0x66, 0x66),
                Rgb::new(0xf1, 0x4c, 0x4c),
                Rgb::new(0x23, 0xd1, 0x8b),
                Rgb::new(0xf5, 0xf5, 0x43),
                Rgb::new(0x3b, 0x8e, 0xea),
                Rgb::new(0xd6, 0x70, 0xd6),
                Rgb::new(0x29, 0xb8, 0xdb),
                Rgb::new(0xff, 0xff, 0xff),
            ],
        }
    }
}

impl Palette {
    /// Dark text on a light background.
    pub fn light() -> Self {
        Self {
            foreground: Rgb::new(0x2a, 0x2a, 0x2a),
            background: Rgb::new(0xfa, 0xfa, 0xfa),
            cursor: Rgb::new(0x30, 0x30, 0x30),
            selection_background: Rgb::new(0xb4, 0xcc, 0xf0),
            selection_foreground: Rgb::new(0x00, 0x00, 0x00),
            ansi: [
                Rgb::new(0x00, 0x00, 0x00),
                Rgb::new(0xc4, 0x1a, 0x16),
                Rgb::new(0x1a, 0x8f, 0x29),
                Rgb::new(0x9c, 0x7a, 0x00),
                Rgb::new(0x1f, 0x5f, 0xc4),
                Rgb::new(0xa3, 0x26, 0xa3),
                Rgb::new(0x0b, 0x86, 0x9c),
                Rgb::new(0xbf, 0xbf, 0xbf),
                Rgb::new(0x66, 0x66, 0x66),
                Rgb::new(0xe0, 0x2e, 0x2a),
                Rgb::new(0x2b, 0xa8, 0x3a),
                Rgb::new(0xb5, 0x8f, 0x00),
                Rgb::new(0x33, 0x75, 0xe0),
                Rgb::new(0xbf, 0x3f, 0xbf),
                Rgb::new(0x13, 0x9f, 0xb8),
                Rgb::new(0x1a, 0x1a, 0x1a),
            ],
        }
    }

    /// Color for an xterm 256-color index.
    pub fn indexed(&self, index: u8) -> Rgb {
        match index {
            0..=15 => self.ansi[usize::from(index)],
            16..=231 => {
                let i = index - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                Rgb::new(level(i / 36), level((i / 6) % 6), level(i % 6))
            }
            232..=255 => {
                let v = 8 + (index - 232) * 10;
                Rgb::new(v, v, v)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xterm_256_color_cube() {
        let p = Palette::default();
        assert_eq!(p.indexed(1), p.ansi[1]);
        assert_eq!(p.indexed(16), Rgb::new(0, 0, 0));
        assert_eq!(p.indexed(196), Rgb::new(255, 0, 0));
        assert_eq!(p.indexed(231), Rgb::new(255, 255, 255));
        assert_eq!(p.indexed(232), Rgb::new(8, 8, 8));
        assert_eq!(p.indexed(255), Rgb::new(238, 238, 238));
    }

    #[test]
    fn dim_darkens() {
        assert_eq!(Rgb::new(150, 30, 0).dimmed(), Rgb::new(100, 20, 0));
    }
}
