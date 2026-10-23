//! Polygon rasterizer (wcgsl/polygonc.c + polygona.asm), transliterated so
//! the output matches the original pixel for pixel: same edge DDA, same
//! packed 6.18 texture stepping with its 32-pixel resyncs, same shading.

use crate::gfx::{Rect, Screen};

/// Light values are 16.16 with 32 levels (POLY_ShadeTable = GRAFS.CLR).
#[derive(Clone, Copy, Default, Debug)]
pub struct Vertex {
    pub x: i32,
    pub y: i32,
    /// Texture coordinates, 6.18 fixed point for 64x64 textures.
    pub tx: i32,
    pub ty: i32,
    /// Light, 16.16 (level in the integer part).
    pub l: i32,
}

#[derive(Clone, Copy, Default)]
struct Edge {
    x: [i32; 2],
    tx: [i32; 2],
    ty: [i32; 2],
    l: [i32; 2],
}

const EDGE: u32 = 1;
const TEXTURE: u32 = 2;
const SHADE: u32 = 4;

pub enum Fill<'a> {
    /// Flat colour, lit through the shade table with vertex 0's light.
    Solid(u8),
    /// Gouraud-shaded flat colour.
    Shade(u8),
    /// Unlit 64x64 texture.
    Texture(&'a [u8]),
    /// Unlit 64x64 texture where texel 0 is transparent.
    HoleTexture(&'a [u8]),
    /// Gouraud-lit 64x64 texture.
    ShadeTexture(&'a [u8]),
    /// Translucency: remap the screen through vertex 0's light level.
    Trans,
}

/// The polygon filler's scratch state.
pub struct Poly {
    edges: Vec<Edge>,
}

impl Default for Poly {
    fn default() -> Self {
        Poly { edges: vec![Edge::default(); 1024] }
    }
}

impl Poly {
    /// Draw a convex polygon clipped to `s.poly` (POLY_MinX..MaxY).
    /// `shade` is the 32x256 colour table (GL_ClrTable).
    pub fn draw(&mut self, s: &mut Screen, clip: Rect, verts: &[Vertex], fill: Fill, shade: &[u8]) {
        if verts.is_empty() {
            return;
        }
        // Never write outside the screen, whatever the clip zone says.
        let clip = Rect {
            min_x: clip.min_x.max(0),
            min_y: clip.min_y.max(0),
            max_x: clip.max_x.min(s.w as i32),
            max_y: clip.max_y.min(s.h as i32),
        };
        let mut miny_i = 0;
        let (mut minx, mut maxx, mut miny, mut maxy) = (verts[0].x, verts[0].x, verts[0].y, verts[0].y);
        for (i, v) in verts.iter().enumerate().skip(1) {
            minx = minx.min(v.x);
            maxx = maxx.max(v.x);
            if miny > v.y {
                miny = v.y;
                miny_i = i;
            }
            maxy = maxy.max(v.y);
        }
        if maxx <= clip.min_x || maxy <= clip.min_y || minx >= clip.max_x || miny >= clip.max_y || maxx <= minx || maxy <= miny {
            return;
        }
        let what = match fill {
            Fill::Solid(_) | Fill::Trans => EDGE,
            Fill::Shade(_) => EDGE | SHADE,
            Fill::Texture(_) | Fill::HoleTexture(_) => EDGE | TEXTURE,
            Fill::ShadeTexture(_) => EDGE | TEXTURE | SHADE,
        };
        let n = self.trace_poly(0, clip, verts, miny_i, -1, what);
        if n <= 0 || n as usize > self.edges.len() {
            return;
        }
        if n != self.trace_poly(1, clip, verts, miny_i, 1, what) {
            return;
        }
        let row0 = miny.max(clip.min_y);
        let lvl = |l: i32| ((l >> 16) & 0xff) as usize * 256;
        for k in 0..n as usize {
            let y = row0 + k as i32;
            if y < 0 || y >= s.h as i32 {
                continue;
            }
            let e = self.edges[k];
            let row = &mut s.pix[y as usize * s.w..(y as usize + 1) * s.w];
            match fill {
                Fill::Solid(c) => {
                    let c = *shade.get(lvl(verts[0].l) + c as usize).unwrap_or(&c);
                    let (x0, x1) = (e.x[0].max(clip.min_x), e.x[1].min(clip.max_x));
                    if x1 > x0 {
                        row[x0 as usize..x1 as usize].fill(c);
                    }
                }
                Fill::Trans => {
                    let t = lvl(verts[0].l);
                    let (x0, x1) = (e.x[0].max(clip.min_x), e.x[1].min(clip.max_x));
                    if e.x[0] < clip.max_x && e.x[1] > clip.min_x && x1 > x0 {
                        for p in &mut row[x0 as usize..x1 as usize] {
                            *p = *shade.get(t + *p as usize).unwrap_or(p);
                        }
                    }
                }
                Fill::Shade(c) => shade_scan(row, clip, &e, c, shade),
                Fill::Texture(t) => tex_scan(row, clip, &e, t, false),
                Fill::HoleTexture(t) => tex_scan(row, clip, &e, t, true),
                Fill::ShadeTexture(t) => shade_tex_scan(row, clip, &e, t, shade),
            }
        }
    }

    /// POLY_TracePoly for one side (0 = left/x0, 1 = right/x1).
    fn trace_poly(&mut self, side: usize, clip: Rect, verts: &[Vertex], start: usize, add: i32, what: u32) -> i32 {
        let size = verts.len();
        let roll = if add < 0 { size - 1 } else { 0 };
        let mut nscans = 0i32;
        let mut i = start;
        for _ in 0..size * 2 + 2 {
            let vi = verts[i];
            if vi.y >= clip.max_y {
                break;
            }
            let j = {
                let j = i as i64 + add as i64;
                if j < 0 || j as usize >= size { roll } else { j as usize }
            };
            let vj = verts[j];
            let dy = vj.y.wrapping_sub(vi.y);
            if dy == 0 {
                i = j;
                continue;
            }
            if dy < 0 {
                break;
            }
            if j == start {
                return 0;
            }
            if vj.y <= clip.min_y {
                i = j;
                continue;
            }
            let clipend = (vj.y - clip.max_y).max(0);
            let base = nscans.max(0) as usize;
            if what & EDGE != 0 {
                self.trace(side, base, clip, vi, vj, clipend, |v| v.x.wrapping_shl(16), true, |e, s, v| e.x[s] = v);
            }
            if what & TEXTURE != 0 {
                self.trace(side, base, clip, vi, vj, clipend, |v| v.tx, false, |e, s, v| e.tx[s] = v);
                self.trace(side, base, clip, vi, vj, clipend, |v| v.ty, false, |e, s, v| e.ty[s] = v);
            }
            if what & SHADE != 0 {
                self.trace(side, base, clip, vi, vj, clipend, |v| v.l, false, |e, s, v| e.l[s] = v);
            }
            let inc = if vi.y >= clip.min_y { dy - clipend } else { dy - (clip.min_y - vi.y) - clipend };
            nscans += inc;
            i = j;
        }
        nscans
    }

    /// One edge interpolator (POLY_TraceEdge/Texture/Shade). `is_x`
    /// selects the screen-x flavour: delta computed as (dx << 16) / dy and
    /// the stored value is the integer part.
    #[allow(clippy::too_many_arguments)]
    fn trace(
        &mut self,
        side: usize,
        base: usize,
        clip: Rect,
        v0: Vertex,
        v1: Vertex,
        clipend: i32,
        get: impl Fn(&Vertex) -> i32,
        is_x: bool,
        set: impl Fn(&mut Edge, usize, i32),
    ) {
        let mut dy = v1.y - v0.y;
        let delta = if is_x {
            v1.x.wrapping_sub(v0.x).wrapping_shl(16).checked_div(dy).unwrap_or(0)
        } else {
            get(&v1).wrapping_sub(get(&v0)).checked_div(dy).unwrap_or(0)
        };
        let mut val = get(&v0);
        let y0 = v0.y - clip.min_y;
        if y0 < 0 {
            val = val.wrapping_sub(delta.wrapping_mul(y0));
            dy += y0;
        }
        dy -= clipend;
        if dy <= 0 {
            return;
        }
        val = val.wrapping_add(delta >> 1);
        for k in 0..dy as usize {
            if let Some(e) = self.edges.get_mut(base + k) {
                set(e, side, if is_x { val >> 16 } else { val });
            }
            val = val.wrapping_add(delta);
        }
    }
}

/// Horizontal clipping shared by the interpolating scan fillers: returns
/// (first x, pixel count, length used for deltas, pixels skipped on the left).
fn clip_scan(clip: Rect, e: &Edge) -> Option<(i32, i32, i32, i32)> {
    let (x0, x1) = (e.x[0], e.x[1]);
    if x0 >= clip.max_x || x1 <= clip.min_x {
        return None;
    }
    let len = x1 - x0;
    if len <= 0 {
        return None;
    }
    let skip = (clip.min_x - x0).max(0);
    let start = x0 + skip;
    let n = x1.min(clip.max_x) - start;
    if n <= 0 {
        return None;
    }
    Some((start, n, len, skip))
}

#[inline]
fn advance(v0: i32, v1: i32, skip: i32, len: i32) -> i32 {
    if skip == 0 {
        v0
    } else {
        v0.wrapping_add(((v1.wrapping_sub(v0) as i64 * skip as i64) / len as i64) as i32)
    }
}

/// POLY_ShadeDump: Gouraud flat colour. Note the original steps the light
/// before fetching, so pixel i uses l0 + (i+1)*dl.
fn shade_scan(row: &mut [u8], clip: Rect, e: &Edge, color: u8, shade: &[u8]) {
    let Some((start, n, len, skip)) = clip_scan(clip, e) else { return };
    let dl = e.l[1].wrapping_sub(e.l[0]) / len;
    let l0 = advance(e.l[0], e.l[1], skip, len);
    let mut frac = (l0 as u32) << 16;
    let fstep = (dl as u32) << 16;
    let istep = ((dl as u32) >> 16) as u8;
    let mut level = ((l0 as u32) >> 16) as u8;
    for p in &mut row[start as usize..(start + n) as usize] {
        let (f, carry) = frac.overflowing_add(fstep);
        frac = f;
        level = level.wrapping_add(istep).wrapping_add(carry as u8);
        *p = *shade.get(level as usize * 256 + color as usize).unwrap_or(&color);
    }
}

#[inline]
fn texel(ebp: u32) -> usize {
    (((ebp >> 26) << 6) | ((ebp & 0xffff) >> 10)) as usize
}

/// `SHL EBP,8` / `MOV BP,WORD PTR [tx+1]`: ty's top 24 bits in the high
/// word (integer in bits 26..31), tx bits 8..23 replacing the low word.
#[inline]
fn pack(tx: i32, ty: i32) -> u32 {
    (((ty as u32) << 8) & 0xffff_0000) | (((tx as u32) >> 8) & 0xffff)
}

/// POLY_TextureDump / POLY_HoleTexDump.
fn tex_scan(row: &mut [u8], clip: Rect, e: &Edge, tex: &[u8], holes: bool) {
    let Some((start, n, len, skip)) = clip_scan(clip, e) else { return };
    let dtx = e.tx[1].wrapping_sub(e.tx[0]) / len;
    let dty = e.ty[1].wrapping_sub(e.ty[0]) / len;
    let tx0 = advance(e.tx[0], e.tx[1], skip, len);
    let ty0 = advance(e.ty[0], e.ty[1], skip, len);
    let mut ptx = tx0.wrapping_add(dtx >> 1);
    let mut pty = ty0.wrapping_add(dty >> 1);
    let ebx = pack(dtx, dty);
    // The first block starts from the unadjusted values (no half step).
    let mut ebp = pack(tx0, ty0);
    let mut x = start as usize;
    let mut left = n;
    let mut px = |x: usize, ebp: u32| {
        let t = *tex.get(texel(ebp)).unwrap_or(&0);
        if !holes || t != 0 {
            row[x] = t;
        }
    };
    while left >= 32 {
        ptx = ptx.wrapping_add(dtx << 5);
        pty = pty.wrapping_add(dty << 5);
        for i in 0..32 {
            px(x + i, ebp);
            ebp = ebp.wrapping_add(ebx);
        }
        left -= 32;
        x += 32;
        ebp = pack(ptx, pty);
    }
    for i in 0..left as usize {
        px(x + i, ebp);
        ebp = ebp.wrapping_add(ebx);
    }
}

/// POLY_ShadeTexDump: Gouraud-lit texture.
fn shade_tex_scan(row: &mut [u8], clip: Rect, e: &Edge, tex: &[u8], shade: &[u8]) {
    let Some((start, n, len, skip)) = clip_scan(clip, e) else { return };
    let dtx = e.tx[1].wrapping_sub(e.tx[0]) / len;
    let dty = e.ty[1].wrapping_sub(e.ty[0]) / len;
    let dl = e.l[1].wrapping_sub(e.l[0]) / len;
    let tx0 = advance(e.tx[0], e.tx[1], skip, len);
    let ty0 = advance(e.ty[0], e.ty[1], skip, len);
    let l0 = advance(e.l[0], e.l[1], skip, len);
    let mut ptx = tx0.wrapping_add(dtx >> 1);
    let mut pty = ty0.wrapping_add(dty >> 1);
    let mut pl = l0.wrapping_add(dl >> 1);
    let ebx = pack(dtx, dty);
    let mut ebp = pack(ptx, pty);
    let mut ecx = ((pl >> 8) as i16) as i32 as u32;
    let mut esi = ((dl >> 8) as i16) as i32;
    if esi < 0 {
        esi += 1;
    }
    let esi = esi as u32;
    let mut x = start as usize;
    let mut left = n;
    let mut px = |x: usize, ebp: u32, ecx: u32| {
        let t = *tex.get(texel(ebp)).unwrap_or(&0);
        let level = ((ecx >> 8) & 0xff) as usize;
        row[x] = *shade.get(level * 256 + t as usize).unwrap_or(&t);
    };
    while left >= 32 {
        ptx = ptx.wrapping_add(dtx << 5);
        pty = pty.wrapping_add(dty << 5);
        pl = pl.wrapping_add(dl << 5);
        for i in 0..32 {
            px(x + i, ebp, ecx);
            ebp = ebp.wrapping_add(ebx);
            ecx = ecx.wrapping_add(esi);
        }
        left -= 32;
        x += 32;
        ebp = pack(ptx, pty);
        ecx = (pl as u32) >> 8;
    }
    for i in 0..left as usize {
        px(x + i, ebp, ecx);
        ebp = ebp.wrapping_add(ebx);
        ecx = ecx.wrapping_add(esi);
    }
}
