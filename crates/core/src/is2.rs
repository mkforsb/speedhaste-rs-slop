//! IS2 run-length sprites (wcgsl/is2code.c, vertdrwa.asm, hordrawa.asm).
//!
//! On disk each column (or row, for "horizontal" sprites) is a sequence of
//! `[transparent count] [data count] [data...]` runs ending with a zero count.
//! We decode into a plain pixel grid with a transparency sentinel; drawing
//! then reproduces the original DDA stepping pixel for pixel.

use crate::data::{Data, DataError, Reader, Result};
use crate::fx::fp_mult_div;
use crate::gfx::Screen;

pub const TRANSPARENT: i16 = -1;
const IS2F_HORIZONTAL: u32 = 1;

pub struct Sprite {
    pub w: i32,
    pub h: i32,
    pub dx: i32,
    pub dy: i32,
    pub x_ratio: i32,
    pub y_ratio: i32,
    pub horizontal: bool,
    /// Pixels, `w * h`, column-major for vertical sprites and row-major for
    /// horizontal ones. `TRANSPARENT` marks holes.
    pub pix: Vec<i16>,
    /// The raw RLE data and each column's offset in it (vertical sprites),
    /// which the original's column drawer walks directly.
    rle: Vec<u8>,
    offsets: Vec<usize>,
}

impl Sprite {
    pub fn parse(name: &str, b: &[u8]) -> Result<Sprite> {
        let bad = |why| DataError::Format(name.to_string(), why);
        let mut r = Reader::new(b);
        if r.bytes(4) != Some(b".IS2") {
            return Err(bad("signature"));
        }
        let w = r.u16().ok_or(bad("header"))? as i32;
        let h = r.u16().ok_or(bad("header"))? as i32;
        let dx = r.i16().ok_or(bad("header"))? as i32;
        let dy = r.i16().ok_or(bad("header"))? as i32;
        let x_ratio = r.u16().ok_or(bad("header"))? as i32;
        let y_ratio = r.u16().ok_or(bad("header"))? as i32;
        let flags = r.u32().ok_or(bad("header"))?;
        let _len = r.u32().ok_or(bad("header"))?;
        let horizontal = flags & IS2F_HORIZONTAL != 0;
        let (nlines, linelen) = if horizontal { (h, w) } else { (w, h) };
        let mut pix = vec![TRANSPARENT; (w * h) as usize];
        let src = &b[r.pos..];
        let mut p = 0usize;
        let at = |i: usize| *src.get(i).unwrap_or(&0) as usize;
        let mut offsets = Vec::new();
        for line in 0..nlines as usize {
            offsets.push(p);
            let base = line * linelen as usize;
            let mut pos = 0usize;
            // First transparent run always exists (may be 0).
            pos += at(p);
            p += 1;
            loop {
                let n = at(p);
                p += 1;
                if n == 0 {
                    break;
                }
                for i in 0..n {
                    if pos + i < linelen as usize {
                        pix[base + pos + i] = at(p + i) as i16;
                    }
                }
                p += n;
                pos += n;
                let t = at(p);
                p += 1;
                if t == 0 {
                    break;
                }
                pos += t;
            }
        }
        let rle = if horizontal { Vec::new() } else { src[..p.min(src.len())].to_vec() };
        Ok(Sprite { w, h, dx, dy, x_ratio, y_ratio, horizontal, pix, rle, offsets })
    }

    pub fn load(d: &Data, name: &str) -> Result<Sprite> {
        Sprite::parse(name, d.load(name)?)
    }

    #[inline]
    fn col(&self, x: i32) -> &[i16] {
        &self.pix[(x * self.h) as usize..((x + 1) * self.h) as usize]
    }

    /// IS2_Draw: scaled for vertical sprites, unscaled (w/h act as clip
    /// extents) for horizontal ones. `xlat` is the colour translation table
    /// (DRW_TranslatePtr) applied to vertical sprites.
    pub fn draw(&self, s: &mut Screen, x: i32, y: i32, w: i32, h: i32, xlat: &[u8]) {
        if self.horizontal {
            self.draw_horizontal_tile(s, x, y, w, h);
        } else {
            self.draw_vertical(s, x, y, w, h, xlat);
        }
    }

    /// IS2_DrawHorizontal: unscaled, untranslated, anchored at the hot spot.
    pub fn draw_horizontal(&self, s: &mut Screen, x: i32, y: i32) {
        self.draw_horizontal_tile(s, x, y, self.w, self.h);
    }

    fn draw_horizontal_tile(&self, s: &mut Screen, x: i32, y: i32, w: i32, h: i32) {
        let x = x - self.dx;
        let y = y - self.dy;
        let c = s.clip;
        if x >= c.max_x || y >= c.max_y || x + w <= c.min_x || y + h <= c.min_y {
            return;
        }
        if self.horizontal {
            for j in y.max(c.min_y)..(y + h).min(c.max_y) {
                let row = j - y;
                if row >= self.h {
                    break;
                }
                for i in x.max(c.min_x)..(x + w.min(self.w)).min(c.max_x) {
                    let v = self.pix[(row * self.w + i - x) as usize];
                    if v != TRANSPARENT {
                        s.pix[j as usize * s.w + i as usize] = v as u8;
                    }
                }
            }
        } else {
            // Vertical data drawn unscaled (used for a few HUD sprites).
            for i in x.max(c.min_x)..(x + w.min(self.w)).min(c.max_x) {
                let col = self.col(i - x);
                for j in y.max(c.min_y)..(y + h.min(self.h)).min(c.max_y) {
                    let v = col[(j - y) as usize];
                    if v != TRANSPARENT {
                        s.pix[j as usize * s.w + i as usize] = v as u8;
                    }
                }
            }
        }
    }

    /// IS2_DrawVertical: (x, y) is the hot spot, (w, h) the on-screen size;
    /// negative `w` mirrors horizontally.
    pub fn draw_vertical(&self, s: &mut Screen, x: i32, y: i32, w: i32, h: i32, xlat: &[u8]) {
        if w == 0 || h == 0 || self.horizontal {
            return;
        }
        let (w, mut ddapos, ddainc): (i32, u32, u32) = if w < 0 {
            let w = -w;
            let inc = (((self.w as u32) << 16) / w as u32).wrapping_neg();
            (w, ((self.w as u32) << 16).wrapping_add(((inc as i32) / 2) as u32).wrapping_sub(1), inc)
        } else {
            let inc = ((self.w as u32) << 16) / w as u32;
            (w, inc >> 1, inc)
        };
        let x = x - fp_mult_div(self.dx, w, self.w);
        let y = y - fp_mult_div(self.dy, h, self.h);
        let c = s.clip;
        let mut i = 0;
        if x < c.min_x {
            i = c.min_x - x;
            ddapos = ddapos.wrapping_add((i as u32).wrapping_mul(ddainc));
        }
        let sw = if x + w >= c.max_x { c.max_x - x } else { w };
        while i < sw {
            let d = (ddapos >> 16) as i32;
            if (0..self.w).contains(&d) {
                self.draw_column(s, x + i, y, h, d, xlat);
            }
            ddapos = ddapos.wrapping_add(ddainc);
            i += 1;
        }
    }

    /// Column `col`'s RLE data, as handed to DRW_DoVerticalDraw.
    pub fn column_rle(&self, col: usize) -> &[u8] {
        self.offsets.get(col).and_then(|&o| self.rle.get(o..)).unwrap_or(&[])
    }

    /// DRW_SetVerticalSpan + DRW_DoVerticalDraw for one screen column.
    fn draw_column(&self, s: &mut Screen, x: i32, y: i32, hscr: i32, col: i32, xlat: &[u8]) {
        let Some(span) = VSpan::new(s, x, y, hscr, self.h) else { return };
        span.draw_rle(s, x, y, self.column_rle(col as usize), xlat);
    }
}

/// The clipped vertical DDA set up by DRW_SetVerticalSpan.
pub struct VSpan {
    pub skip: i32,
    pub height: i32,
    pub start: u32,
    pub inc: u32,
}

impl VSpan {
    pub fn new(s: &Screen, x: i32, y: i32, hscr: i32, hdata: i32) -> Option<VSpan> {
        let c = s.clip;
        if hscr <= 0 || x < c.min_x || x >= c.max_x {
            return None;
        }
        let mut skip = c.min_y - y;
        if skip < 0 {
            skip = 0;
        } else if skip >= hscr {
            return None;
        }
        if y + hscr <= c.min_y || y + skip >= c.max_y {
            return None;
        }
        let height = if y + hscr >= c.max_y { c.max_y - y - skip } else { hscr - skip };
        let dda = ((hdata as u32) << 16) / hscr as u32;
        if dda < 4 {
            return None;
        }
        Some(VSpan { skip, height, start: (dda >> 1).wrapping_add(dda.wrapping_mul(skip as u32)), inc: dda })
    }

    /// DRW_DoVerticalDraw (vertdrwa.asm), register for register: it walks the
    /// RLE column run by run, skipping transparent runs with a rounded-up
    /// division and stepping data runs with the fraction's low word doubling
    /// as the pixel counter, so its sampling differs from plain stepping by
    /// a texel now and then. `col` starts at the column's first transparent
    /// run byte. (DRW_Tile is never set in this game, so no tiling.)
    pub fn draw_rle(&self, s: &mut Screen, x: i32, y: i32, col: &[u8], xlat: &[u8]) {
        #[derive(Clone, Copy)]
        enum St {
            DoDraw,
            DrawTrans,
            DrawData,
            GetData,
        }
        let b = |i: usize| *col.get(i).unwrap_or(&0) as u32;
        let (w, h) = (s.w, s.h);
        let mut plot = |row: usize, esi: usize, ecx: u32| {
            let v = *col.get(esi.wrapping_add(ecx as i32 as isize as usize)).unwrap_or(&0);
            if row < h && (x as usize) < w {
                s.pix[row * w + x as usize] = xlat[v as usize];
            }
        };
        let set_bx = |ebx: &mut u32, bx: u16| *ebx = (*ebx & 0xffff_0000) | bx as u32;
        // ADD EBP,EBX / ADC ECX,EDX; returns the final carry (end of run).
        let advance = |ebp: &mut u32, ebx: u32, ecx: &mut u32, edx: u32| {
            let (nbp, c1) = ebp.overflowing_add(ebx);
            *ebp = nbp;
            let (t, c2) = ecx.overflowing_add(edx);
            let (t2, c3) = t.overflowing_add(c1 as u32);
            *ecx = t2;
            c2 || c3
        };
        let (mut esi, mut ebp, mut ebx, mut edx, mut ecx) = (0usize, self.start, self.inc, self.height as u32, 0u32);
        let mut row = (y + self.skip) as usize;
        let mut st = St::DoDraw;
        if self.skip != 0 {
            // Skip whole runs up to the starting position.
            ecx = ebp >> 16;
            ebp &= 0xffff;
            let mut first = true;
            st = loop {
                if !(first && b(esi) == 0) {
                    let eax = b(esi);
                    if eax == 0 {
                        return;
                    }
                    let (r, borrow) = ecx.overflowing_sub(eax);
                    ecx = r;
                    if borrow {
                        ecx = ecx.wrapping_neg();
                        break St::DrawTrans;
                    }
                }
                first = false;
                let eax = b(esi + 1);
                if eax == 0 {
                    return;
                }
                esi += eax as usize + 2;
                let (r, borrow) = ecx.overflowing_sub(eax);
                ecx = r;
                if borrow {
                    ecx = ecx.wrapping_neg();
                    break St::GetData;
                }
            };
        }
        loop {
            match st {
                St::DoDraw => {
                    ecx = b(esi);
                    st = if ecx == 0 { St::DrawData } else { St::DrawTrans };
                }
                St::DrawTrans => {
                    let (r, borrow) = (ecx << 16).overflowing_sub(ebp);
                    ecx = r;
                    if borrow {
                        ebp = ecx.wrapping_neg();
                        st = St::DrawData;
                        continue;
                    }
                    let eax = ecx.wrapping_add(ebx).wrapping_sub(1);
                    let (q, rem) = (eax / ebx, eax % ebx);
                    let (left, below) = edx.overflowing_sub(q);
                    if below {
                        return;
                    }
                    row += q as usize;
                    ebp = ebx.wrapping_sub(rem);
                    edx = left;
                    st = St::DrawData;
                }
                St::DrawData => {
                    esi += 1;
                    ecx = b(esi);
                    if ecx & 0xffff == 0 {
                        return;
                    }
                    esi += ecx as usize + 1;
                    st = St::GetData;
                }
                St::GetData => {
                    let eax = (ebp >> 16).wrapping_sub(ecx);
                    if eax as i32 >= 0 {
                        ebp = ebp.wrapping_sub(ecx << 16);
                        ecx = b(esi);
                        if ecx == 0 {
                            return;
                        }
                        st = St::DrawTrans;
                        continue;
                    }
                    edx = (edx << 16) | (ebx >> 16);
                    ebx = (ebx << 16) | (edx >> 16);
                    ecx = eax;
                    edx &= 0xffff;
                    ebp <<= 16;
                    let (bx, mut slow) = (ebx as u16).overflowing_sub(8);
                    set_bx(&mut ebx, bx);
                    let mut run_end = false;
                    while !slow {
                        let mut eof = None;
                        for i in 0..8 {
                            plot(row + i, esi, ecx);
                            if advance(&mut ebp, ebx, &mut ecx, edx) {
                                eof = Some(i);
                                break;
                            }
                        }
                        if let Some(i) = eof {
                            row += i + 1;
                            let bx = (ebx as u16).wrapping_add((8 - (i + 1)) as u16);
                            set_bx(&mut ebx, bx);
                            if bx == 0 {
                                return;
                            }
                            run_end = true;
                            break;
                        }
                        row += 8;
                        let (bx, cf) = (ebx as u16).overflowing_sub(8);
                        set_bx(&mut ebx, bx);
                        slow = cf;
                    }
                    if !run_end {
                        let bx = (ebx as u16).wrapping_add(8);
                        set_bx(&mut ebx, bx);
                        if bx == 0 {
                            return;
                        }
                        for _ in 0..8 {
                            plot(row, esi, ecx);
                            row += 1;
                            let bx = (ebx as u16).wrapping_sub(1);
                            set_bx(&mut ebx, bx);
                            if bx == 0 {
                                return;
                            }
                            if advance(&mut ebp, ebx, &mut ecx, edx) {
                                run_end = true;
                                break;
                            }
                        }
                        if !run_end {
                            return;
                        }
                    }
                    // fixdraw: back to a 16.16 position past the run's end.
                    ebp = (ebp >> 16) | (ecx << 16);
                    let eax = ebx;
                    ebx = (ebx >> 16) | (edx << 16);
                    edx = eax & 0xffff;
                    ecx = b(esi);
                    if ecx == 0 {
                        return;
                    }
                    st = St::DrawTrans;
                }
            }
        }
    }

    /// Draw `height` pixels from row `y + skip`, sampling source rows through
    /// the DDA. `src(row)` returns a colour or `TRANSPARENT`.
    #[inline]
    pub fn draw(&self, s: &mut Screen, x: i32, y: i32, src: impl Fn(usize) -> i16, xlat: &[u8]) {
        let mut pos = self.start;
        let mut o = (y + self.skip) as usize * s.w + x as usize;
        for _ in 0..self.height {
            let v = src((pos >> 16) as usize);
            if v != TRANSPARENT {
                s.pix[o] = xlat[v as usize];
            }
            pos = pos.wrapping_add(self.inc);
            o += s.w;
        }
    }
}
