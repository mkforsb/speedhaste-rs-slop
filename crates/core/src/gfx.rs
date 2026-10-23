//! 8-bit indexed framebuffer, palettes, rectangles and bitmap fonts
//! (wcgsl/llscrc.c, graphics.c, text.c).

use crate::data::{Data, DataError, Reader, Result};

/// A 256-colour VGA palette with 6-bit components, as stored in .PAL files.
#[derive(Clone)]
pub struct Palette(pub [u8; 768]);

impl Default for Palette {
    fn default() -> Self {
        Palette([0; 768])
    }
}

impl Palette {
    pub fn load(data: &Data, name: &str) -> Result<Palette> {
        let b = data.load(name)?;
        let mut p = [0u8; 768];
        let n = b.len().min(768);
        p[..n].copy_from_slice(&b[..n]);
        Ok(Palette(p))
    }

    /// RGBA8 for colour `i`, expanding 6-bit DAC values the way VGA does.
    pub fn rgba(&self, i: u8) -> [u8; 4] {
        let c = &self.0[i as usize * 3..i as usize * 3 + 3];
        let x = |v: u8| ((v & 63) << 2) | ((v & 63) >> 4);
        [x(c[0]), x(c[1]), x(c[2]), 255]
    }

    pub fn to_rgba_table(&self) -> [[u8; 4]; 256] {
        std::array::from_fn(|i| self.rgba(i as u8))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub min_x: i32,
    pub min_y: i32,
    pub max_x: i32,
    pub max_y: i32,
}

/// The screen: an indexed framebuffer plus the current clip zone
/// (DRW_MinX..DRW_MaxY in the original; max is exclusive).
pub struct Screen {
    pub w: usize,
    pub h: usize,
    pub pix: Vec<u8>,
    pub clip: Rect,
}

impl Screen {
    pub fn new(w: usize, h: usize) -> Screen {
        Screen {
            w,
            h,
            pix: vec![0; w * h],
            clip: Rect { min_x: 0, min_y: 0, max_x: w as i32, max_y: h as i32 },
        }
    }

    pub fn reset_clip(&mut self) {
        self.clip = Rect { min_x: 0, min_y: 0, max_x: self.w as i32, max_y: self.h as i32 };
    }

    pub fn clear(&mut self, c: u8) {
        self.pix.fill(c);
    }

    #[inline]
    pub fn put(&mut self, x: i32, y: i32, c: u8) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            self.pix[y as usize * self.w + x as usize] = c;
        }
    }

    /// Copy a 320-wide image (like a .PIX) to the top-left corner.
    pub fn blit_pix(&mut self, src: &[u8]) {
        for (y, row) in src.chunks(320).enumerate().take(self.h) {
            let n = row.len().min(self.w);
            self.pix[y * self.w..y * self.w + n].copy_from_slice(&row[..n]);
        }
    }

    /// GFX_Rectangle: border colour and interior colour, negative = none.
    pub fn rectangle(&mut self, x0: i32, y0: i32, w: i32, h: i32, border: i32, interior: i32) {
        let (mut x0, mut y0, mut x1, mut y1) = (x0, y0, x0 + w, y0 + h);
        if x0 > x1 {
            std::mem::swap(&mut x0, &mut x1);
        }
        if y0 > y1 {
            std::mem::swap(&mut y0, &mut y1);
        }
        let (sw, sh) = (self.w as i32, self.h as i32);
        if x0 >= sw || x1 <= 0 || y0 >= sh || y1 <= 0 || x0 == x1 || y0 == y1 {
            return;
        }
        x0 = x0.max(0);
        y0 = y0.max(0);
        let (w, h) = (x1 - x0, y1 - y0);
        let interior = if w < 3 || h < 3 { -1 } else { interior };
        if interior < 0 && border < 0 {
            return;
        }
        let hline = |s: &mut Screen, y: i32, x: i32, n: i32, c: i32| {
            for i in x..x + n {
                s.put(i, y, c as u8);
            }
        };
        let first = if border >= 0 { border } else { interior };
        hline(self, y0, x0, w, first);
        for y in y0 + 1..y0 + h - 1 {
            if border >= 0 {
                self.put(x0, y, border as u8);
                self.put(x0 + w - 1, y, border as u8);
                if interior >= 0 {
                    hline(self, y, x0 + 1, w - 2, interior);
                }
            } else {
                hline(self, y, x0, w, interior);
            }
        }
        if border >= 0 || interior >= 0 {
            hline(self, y0 + h - 1, x0, w, first);
        }
    }
}

// ---------------------------------------------------------------------------
// Fonts

const FONT_FALPHA: u16 = 1;
const FONT_F128: u16 = 2;
const FONT_FTIGHT: u16 = 4;
const FONT_CHRMAP: u16 = 8;

/// A bitmap font. Fixed-width fonts store glyphs row-major; proportional
/// fonts (width 0) store per glyph a width byte then columns of `height`
/// pixels. Pixel 255 = ink colour, 254 = border colour, 0 = transparent.
#[derive(Clone, Default)]
pub struct Font {
    flags: u16,
    pub width: u8,
    pub height: u8,
    data: Vec<u8>,
    offs: Vec<u16>,
}

impl Font {
    pub fn load(d: &Data, name: &str) -> Result<Font> {
        let b = d.load(name)?;
        let mut r = Reader::new(b);
        let bad = || DataError::Format(name.to_string(), "font header");
        let flags = r.u16().ok_or_else(bad)?;
        let height = r.u8().ok_or_else(bad)?;
        let width = r.u8().ok_or_else(bad)?;
        let len = r.u32().ok_or_else(bad)? as usize;
        let mut data = r.buf[r.pos..].to_vec();
        data.resize(len.max(data.len()), 0);
        let mut offs = Vec::new();
        if width == 0 {
            let nchars = if flags & FONT_FALPHA != 0 {
                26 * 2 + 10
            } else if flags & FONT_F128 != 0 {
                128 - 32
            } else if flags & FONT_CHRMAP != 0 {
                data[..256].iter().filter(|&&c| c != 255).count()
            } else {
                256 - 32
            };
            let mut off: usize = if flags & FONT_CHRMAP != 0 { 256 } else { 0 };
            for _ in 0..nchars {
                offs.push(off as u16);
                off += height as usize * *data.get(off).unwrap_or(&0) as usize + 1;
            }
        }
        Ok(Font { flags, width, height, data, offs })
    }

    /// FONT_Index: glyph index → (advance width, glyph data offset or None for space).
    fn index(&self, c: u8) -> (i32, Option<usize>) {
        let idx: i32 = if self.flags & FONT_CHRMAP != 0 {
            let mut c = c;
            if self.data[c as usize] == 255 {
                c = c.to_ascii_uppercase();
            }
            let m = self.data[c as usize];
            if m == 255 { -1 } else { m as i32 }
        } else if self.flags & FONT_FALPHA != 0 {
            match c {
                b'A'..=b'Z' => (c - b'A') as i32,
                b'a'..=b'z' => (c - b'a') as i32 + 26,
                b'0'..=b'9' => (c - b'0') as i32 + 52,
                _ => -1,
            }
        } else if self.flags & FONT_F128 != 0 {
            // 0xA5/0xA4 (Ñ/ñ in CP437) map to glyph 128.
            let c = if c == 0xA5 || c == 0xA4 { 128 } else if c >= 128 { b' ' as i32 } else { c as i32 };
            c - 32 - 1
        } else {
            c as i32 - 32
        };
        if idx < 0 {
            return (self.height as i32 / 2, None);
        }
        if self.width == 0 {
            let o = *self.offs.get(idx as usize).unwrap_or(&0) as usize;
            (*self.data.get(o).unwrap_or(&0) as i32, Some(o + 1))
        } else {
            (self.width as i32, Some(self.width as usize * self.height as usize * idx as usize))
        }
    }

    /// The greetings effect's own Write (intro.c): column-major glyphs of
    /// 31 pixels, non-zero pixels drawn as `pixel + ink`, spaces 6 wide.
    /// Returns the final x.
    pub fn write_greets(&self, s: &mut Screen, mut x: i32, y: i32, text: &str, ink: u8) -> i32 {
        for &c in text.as_bytes() {
            if c == b' ' {
                x += 6;
                continue;
            }
            let (width, glyph) = self.index(c);
            let g = glyph.unwrap_or(0);
            for i in 0..width {
                for j in 0..31 {
                    let k = *self.data.get(g + (i * 31 + j) as usize).unwrap_or(&0);
                    if k > 0 {
                        s.put(x, y + j, k.wrapping_add(ink));
                    }
                }
                x += 1;
            }
            x += 1;
        }
        x - 1
    }

    fn advance_pad(&self) -> bool {
        self.width == 0 && self.flags & (FONT_FTIGHT | FONT_CHRMAP) == 0
    }

    /// TEXT_Write (without the embedded control codes, which the game
    /// doesn't use with these fonts). Returns the final x.
    pub fn write(&self, s: &mut Screen, mut x: i32, mut y: i32, text: &str, ink: u8) -> i32 {
        let lmar = x;
        let rmar = s.w as i32;
        let h = self.height as i32;
        for &c in text.as_bytes() {
            let (width, glyph) = self.index(c);
            if x > rmar - width {
                x = lmar;
                y += h;
            }
            if let Some(g) = glyph
                && x <= s.w as i32 - width
                && y <= s.h as i32 - h
            {
                let px = |s: &mut Screen, xx: i32, yy: i32, k: u8| match k {
                    255 => s.put(xx, yy, ink),
                    254 => s.put(xx, yy, 0),
                    0 => {}
                    k => s.put(xx, yy, k),
                };
                if self.width == 0 {
                    for i in 0..width {
                        for j in 0..h {
                            let k = *self.data.get(g + (i * h + j) as usize).unwrap_or(&0);
                            px(s, x + i, y + j, k);
                        }
                    }
                    if self.advance_pad() {
                        x += 1;
                    }
                } else {
                    for j in 0..h {
                        for i in 0..width {
                            let k = *self.data.get(g + (j * width + i) as usize).unwrap_or(&0);
                            px(s, x + i, y + j, k);
                        }
                    }
                }
            }
            x += width;
        }
        x
    }

    /// TEXT_GetExtent: width of `text` drawn from x on a screen `scr_w` wide.
    pub fn extent(&self, text: &str, scr_w: i32, scr_h: i32) -> (i32, i32) {
        let (mut x, mut y) = (0, 0);
        let h = self.height as i32;
        for &c in text.as_bytes() {
            let (width, glyph) = self.index(c);
            if x > scr_w - width {
                x = 0;
                y += h;
            }
            if glyph.is_some() && x <= scr_w - width && y <= scr_h - h && self.advance_pad() {
                x += 1;
            }
            x += width;
        }
        (x, y)
    }

    pub fn text_width(&self, text: &str) -> i32 {
        self.extent(text, 320, 200).0
    }
}
