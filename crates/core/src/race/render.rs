//! Race view rendering: background, Mode-7 floor (game/3dfloor.c,
//! fla.asm), sector walls (game/sectors.c), sprites and 3D objects
//! (game/flsprs.c), and the overhead map (race.c DrawMap).
//!
//! Rendering reads the world but never changes it; the projection caches
//! the original kept inside its data structures live in [`Renderer`].

use super::map::{to_map, to_sec};
use super::world::*;
use crate::fx::*;
use crate::gfx::Screen;
use crate::is2::{Sprite, VSpan};
use crate::mesh::{FF_FLAT, FF_NORDER, FF_VISIBLE, MF_HOLES, MF_NOSHADE, MF_TRANS, OF_HASTIRES};
use crate::poly::{Fill, Poly, Vertex};

pub mod scene;

pub const CAMF_STATIC: u16 = 0x0001;

#[derive(Clone, Copy, Debug, Default)]
pub struct CamData {
    pub radius: i32,
    pub h: i32,
    pub focus: i32,
    pub horizon: i32,
    pub flags: u16,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Camera {
    pub data: CamData,
    pub x: u32,
    pub y: u32,
    pub angle: u16,
    pub tilt: i32,
    pub dx: u32,
    pub dy: u32,
    pub sec: usize,
}

/// GL_Screen*: the play area and its scale relative to 320x200.
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub w: i32,
    pub h: i32,
    pub min_x: i32,
    pub min_y: i32,
    pub max_x: i32,
    pub max_y: i32,
    pub cx: i32,
    pub cy: i32,
    pub xratio: i32,
    pub yratio: i32,
}

impl Geometry {
    /// InitScreenParms.
    pub fn new(sw: i32, sh: i32) -> Geometry {
        let (h, min_y) = if sh == 480 { (400, 40) } else { (sh, 0) };
        Geometry {
            w: sw,
            h,
            min_x: 0,
            min_y,
            max_x: sw,
            max_y: min_y + h,
            cx: sw / 2,
            cy: min_y + h / 2,
            xratio: (sw / 320).max(1),
            yratio: (h / 200).max(1),
        }
    }
}

#[derive(Clone, Copy, Default)]
struct VProj {
    magic: u32,
    rx: i32,
    rz: i32,
    px: i32,
    py: i32,
}

/// An entry of the per-sector, depth-sorted object list (FSP_TObject).
#[derive(Clone, Copy)]
struct Obj {
    x: i32,
    y: i32,
    z: i32,
    depth: i32,
    spr: SpriteId,
    w: i32,
    h: i32,
    view: (usize, usize),
    angle: u16,
    tirerot: u16,
    tiredir: u16,
}

#[derive(Clone, Copy, Default)]
struct MV {
    rx: i32,
    ry: i32,
    rz: i32,
    px: i32,
    py: i32,
    l: i32,
}

/// The wheels of a car mesh: its first `4 * tirev` vertices roll and the
/// front two steer.
struct TireRot {
    roll: (i32, i32),
    steer: (i32, i32),
}

impl TireRot {
    fn new(o: &Obj) -> TireRot {
        TireRot { roll: (cos(o.tirerot), sin(o.tirerot)), steer: (cos(o.tiredir), sin(o.tiredir)) }
    }

    fn vertex(&self, lod: &MeshLod, i: usize) -> [i32; 3] {
        let v = lod.mesh.verts[i];
        let (mut x, mut y, mut z) = (v[0], v[1], v[2]);
        if lod.mesh.flags & OF_HASTIRES != 0 && lod.tirev > 0 && i < lod.tirev * 4 {
            let ((ctr, str_), (ctd, std)) = (self.roll, self.steer);
            let nt = i / lod.tirev;
            let tc = lod.tirecenter[nt];
            let (ry, rz) = (y - tc[1], z - tc[2]);
            y = tc[1] + fp_mult(ry, ctr) - fp_mult(rz, str_);
            z = tc[2] + fp_mult(rz, ctr) + fp_mult(ry, str_);
            if nt < 2 {
                let (rx, rz) = (x - tc[0], z - tc[2]);
                x = tc[0] + fp_mult(rx, ctd) - fp_mult(rz, std);
                z = tc[2] + fp_mult(rz, ctd) + fp_mult(rx, std);
            }
        }
        [x, y, z]
    }
}

/// How face vertices clipped at the near plane get projected.
#[derive(Clone, Copy)]
enum Proj {
    Race { focus: i32, scrx: i32, scry: i32 },
    R3d { fx: i32, fy: i32, cx: i32, cy: i32 },
}

const MAX_OBJS: usize = 140;
const ZLIM: i32 = 8 << 12;
const PAL_LEVELS: i32 = 16;

#[derive(Default)]
pub struct Renderer {
    poly: Poly,
    vproj: Vec<VProj>,
    sec_magic: Vec<u32>,
    magic: u32,
    objs: Vec<Obj>,
    mv: Vec<MV>,
    visible: Vec<bool>,
    order: Vec<(i32, usize)>,
    buckets: Vec<Vec<usize>>,
    scrap: Vec<Vertex>,
    /// ExtraDetail: better quality for external (static) cameras.
    extra_detail: bool,
    /// Effective PolygonDetail (lowered for distant meshes).
    polygon_detail: bool,
    /// Size multiplier for unscaled sprites (position markers): 1 up to
    /// the original's 640x400, then grown to keep that look.
    noscale_mul: i32,
}

/// Floor scanline sampler (FL_DrawRaster / FL_DrawRasterLo).
fn floor_raster(w: &World, row: &mut [u8], mut x: u32, mut y: u32, dx: i32, dy: i32, xlat: &[u8], hi: bool) {
    if hi {
        for p in row.iter_mut() {
            *p = xlat[w.floor.texel(x, y) as usize];
            x = x.wrapping_add(dx as u32);
            y = y.wrapping_add(dy as u32);
        }
    } else {
        x = x.wrapping_add(dx as u32);
        y = y.wrapping_add(dy as u32);
        for p in row.chunks_mut(2) {
            p.fill(xlat[w.floor.texel(x, y) as usize]);
            x = x.wrapping_add((dx as u32).wrapping_mul(2));
            y = y.wrapping_add((dy as u32).wrapping_mul(2));
        }
    }
}

impl Renderer {
    /// RenderView: draw the view of `cam` into rows `y..y+h` of the play area.
    #[allow(clippy::too_many_arguments)]
    pub fn render_view(&mut self, s: &mut Screen, w: &World, clr: &[u8], g: &Geometry, cam: &mut Camera, tst_angle: u16, y: i32, h: i32) {
        self.extra_detail = cam.data.flags & CAMF_STATIC != 0;
        self.noscale_mul = (g.xratio / 2).max(1);
        let camh = cam.data.horizon;
        cam.tilt = cam.tilt.max(0);
        let hsk = if cam.tilt > w.bg.sky_h {
            cam.data.horizon += cam.tilt - w.bg.sky_h;
            0
        } else {
            w.bg.sky_h - cam.tilt
        };
        cam.angle = cam.angle.wrapping_add(tst_angle);
        cam.x = cam.dx.wrapping_sub(fp_mult(cam.data.radius, cos(cam.angle)) as u32);
        cam.y = cam.dy.wrapping_sub(fp_mult(cam.data.radius, sin(cam.angle)) as u32);

        // Clouds, then mountains (colour 0 transparent). The original tiles
        // the 320-wide pictures unscaled at 640x400; beyond that the port
        // stretches them to keep that look.
        let sw = s.w;
        let hs = (g.xratio / 2).max(1);
        let o = (3 * cam.angle as i32 * 320 / 65536) % 320;
        for i in 0..h.min(hsk) {
            let src = &w.bg.sky[((cam.tilt + i) * 320) as usize..][..320];
            let dst = ((y + i * g.yratio) as usize) * sw;
            for x in 0..sw {
                s.pix[dst + x] = src[((x as i32 / hs) - o).rem_euclid(320) as usize];
            }
        }
        let (mut i, mut prow, hmnt) = if cam.tilt > w.bg.sky_h - w.bg.mount_h {
            (0, cam.tilt - (w.bg.sky_h - w.bg.mount_h), w.bg.sky_h - cam.tilt)
        } else {
            let i = (w.bg.sky_h - w.bg.mount_h) - cam.tilt;
            (i, 0, i + w.bg.mount_h)
        };
        let o = (4 * cam.angle as i32 * 320 / 65536) % 320;
        while i < h && i < hmnt {
            let src = &w.bg.mount[(prow * 320) as usize..][..320];
            let dst = ((y + i * g.yratio) as usize) * sw;
            for x in 0..sw {
                let c = src[((x as i32 / hs) - o).rem_euclid(320) as usize];
                if c != 0 {
                    s.pix[dst + x] = c;
                }
            }
            prow += 1;
            i += 1;
        }
        for i in 0..hsk {
            let r = ((y + g.yratio * i) as usize) * sw;
            for k in 1..g.yratio as usize {
                s.pix.copy_within(r..r + sw, r + k * sw);
            }
        }

        // Floor.
        let hfl = h - g.yratio * hsk;
        if hfl > 0 {
            self.draw_floor(s, w, clr, cam, g.yratio * hsk + y, g.min_x, g.w, hfl);
        }

        // Walls and objects.
        self.render_sectors(s, w, clr, cam, g.cx, g.yratio * hsk + y);

        cam.angle = cam.angle.wrapping_sub(tst_angle);
        cam.data.horizon = camh;
    }

    /// F3D_Draw3D: perspective floor with distance fog.
    #[allow(clippy::too_many_arguments)]
    fn draw_floor(&mut self, s: &mut Screen, w: &World, clr: &[u8], cam: &Camera, y0: i32, x0: i32, width: i32, h: i32) {
        let mut yy = cam.data.horizon;
        for r in 0..h {
            let rad = fp_mult_div(cam.data.h, cam.data.focus, yy) as u32;
            let t = if rad < (4 << 16) {
                PAL_LEVELS - 1
            } else if rad >= (36 << 16) {
                0
            } else {
                PAL_LEVELS - 2 - ((((PAL_LEVELS - 2) as u32 * (rad - (4 << 16)) / (36 - 4)) >> 16) as i32)
            };
            let xlat = &clr[((31 - t) as usize) * 256..][..256];
            let k = fp_mult_div(cam.data.h, 0x1000, yy);
            let vx = fp_mult(k, cos(cam.angle.wrapping_sub(16384)));
            let vy = fp_mult(k, sin(cam.angle.wrapping_sub(16384)));
            let px = cam.x.wrapping_add(fpn_mult(rad as i32, cos(cam.angle), 22) as u32).wrapping_sub(fpn_mult(vx, width, 1) as u32);
            let py = cam.y.wrapping_add(fpn_mult(rad as i32, sin(cam.angle), 22) as u32).wrapping_sub(fpn_mult(vy, width, 1) as u32);
            let row = (y0 + r) as usize * s.w + x0 as usize;
            floor_raster(w, &mut s.pix[row..row + width as usize], px, py, vx, vy, xlat, w.set.floor_detail);
            yy += 1;
        }
    }

    /// SEC_Render: collect sectors around the camera breadth first, then
    /// draw them back to front (sector 0, the off-road "rest of the world",
    /// first).
    fn render_sectors(&mut self, s: &mut Screen, w: &World, clr: &[u8], cam: &mut Camera, cx: i32, cy: i32) {
        self.magic = self.magic.wrapping_add(1);
        let m = self.magic;
        self.vproj.resize(w.sec.verts.len(), VProj::default());
        self.sec_magic.resize(w.sec.secs.len(), 0);
        let s2 = w.sec.find(Some(cam.sec), to_sec(cam.x), to_sec(cam.y));
        if s2 != 0 {
            cam.sec = s2;
        }
        let start = cam.sec;
        let mut list = vec![start];
        self.sec_magic[0] = m;
        self.sec_magic[start] = m;
        let maxsectors = (20 + w.set.background_detail * 2 + self.extra_detail as i32 * 10) as usize;
        let mut base = 0;
        loop {
            if list.len() >= maxsectors {
                break;
            }
            let n = list.len();
            for i in base..n {
                for side in &w.sec.secs[list[i]].sides {
                    if self.sec_magic[side.other] != m {
                        self.sec_magic[side.other] = m;
                        list.push(side.other);
                    }
                }
            }
            base = n;
            if base >= list.len() {
                break;
            }
        }
        list.push(0);
        for &sec in list.iter().rev() {
            self.render_walls(s, w, clr, cam, sec, cx, cy, false);
            self.render_objects(s, w, clr, cam, sec, cx, cy);
            self.render_walls(s, w, clr, cam, sec, cx, cy, true);
        }
    }

    /// SEC_RenderSectorWalls: textured vertical walls, one screen column at
    /// a time with perspective-correct texture columns.
    #[allow(clippy::too_many_arguments)]
    fn render_walls(&mut self, s: &mut Screen, w: &World, clr: &[u8], cam: &Camera, sec: usize, cx: i32, cy: i32, inv: bool) {
        let (sa, ca) = (sin(cam.angle), cos(cam.angle));
        let sides = &w.sec.secs[sec].sides;
        for side in sides {
            let vp = &mut self.vproj[side.v0];
            if vp.magic != self.magic {
                vp.magic = self.magic;
                let (vx, vy) = w.sec.verts[side.v0];
                let x = ((to_map(vx) >> 6) as i32).wrapping_sub((cam.x >> 6) as i32);
                let y = ((to_map(vy) >> 6) as i32).wrapping_sub((cam.y >> 6) as i32);
                let rx = fp_mult(y, ca).wrapping_sub(fp_mult(x, sa));
                let ry = fp_mult(x, ca).wrapping_add(fp_mult(y, sa));
                vp.rx = rx;
                vp.rz = ry;
                let pz = ry.max(ZLIM);
                vp.py = cy + fp_mult_div(cam.data.h << 2, cam.data.focus, pz) - cam.data.horizon;
                vp.px = cx + fp_mult_div(rx >> 4, cam.data.focus, pz);
            }
        }
        let clip = s.clip;
        let xlat = &clr[16 * 256..17 * 256];
        for (si, side) in sides.iter().enumerate() {
            let Some(tex) = side.tex else { continue };
            let (i0, i1) = if inv { (side.v1, side.v0) } else { (side.v0, side.v1) };
            let (mut v0, mut v1) = (self.vproj[i0], self.vproj[i1]);
            if v0.rz < ZLIM && v1.rz < ZLIM {
                continue;
            }
            let dx = v1.rx.wrapping_sub(v0.rx);
            let dz = v1.rz.wrapping_sub(v0.rz);
            if v0.rz < ZLIM {
                if v1.px <= clip.min_x {
                    continue;
                }
                let ix = v0.rx.wrapping_add(fp_mult_div(ZLIM - v0.rz, dx, dz));
                let ix = cx + fp_mult_div(ix >> 4, cam.data.focus, ZLIM);
                if ix >= v1.px {
                    continue;
                }
                v0.px = ix;
                self.vproj[i0].px = ix;
            } else if v1.rz < ZLIM {
                if v0.px >= clip.max_x {
                    continue;
                }
                let ix = v1.rx.wrapping_add(fp_mult_div(ZLIM - v1.rz, dx, dz));
                let ix = cx + fp_mult_div(ix >> 4, cam.data.focus, ZLIM);
                if ix <= v0.px {
                    continue;
                }
                v1.px = ix;
                self.vproj[i1].px = ix;
            }
            let t: &Sprite = &w.sec.textures[tex];
            let tw = t.w << 12;
            let th = t.h << 8;
            let ox = v0.rx >> 8;
            let oz = v0.rz >> 4;
            let tx = fp_mult(tw, sin(side.angle.wrapping_sub(cam.angle))) >> 8;
            let tz = fp_mult(tw, cos(side.angle.wrapping_sub(cam.angle))) >> 4;
            let den = fpn_mult(v0.rx, dz, 16).wrapping_sub(fpn_mult(v0.rz, dx, 16));
            let mut px0 = if v0.px < clip.min_x || v0.rz < ZLIM { clip.min_x } else { v0.px };
            let px1 = if v1.px > clip.max_x || v1.rz < ZLIM { clip.max_x } else { v1.px };
            let h4 = cam.data.h << 2;
            while px0 < px1 {
                let num = fpn_mult(px0 - cx, dz, 12).wrapping_sub(fpn_mult(cam.data.focus, dx, 16));
                let y1 = cy - cam.data.horizon + fp_mult_div(h4, num, den);
                if y1 < cy {
                    px0 += 1;
                    continue;
                }
                let y0 = cy - cam.data.horizon + fp_mult_div(h4 - th, num, den);
                if let Some(span) = VSpan::new(s, px0, y0, y1 - y0, t.h) {
                    if w.set.walls_detail {
                        let b1 = (px0 - cx).wrapping_mul(oz);
                        let b2 = cam.data.focus.wrapping_mul(ox);
                        let b3 = cam.data.focus.wrapping_mul(tx);
                        let b4 = (px0 - cx).wrapping_mul(tz);
                        if b3 != b4 {
                            let col = (abs32(fp_mult_div(b1.wrapping_sub(b2), t.w, b3.wrapping_sub(b4))) % t.w) as usize;
                            span.draw_rle(s, px0, y0, t.column_rle(col), xlat);
                        }
                    } else {
                        let c = ((si * 20) & 0xC) as i16;
                        let ident: [u8; 256] = std::array::from_fn(|i| i as u8);
                        span.draw(s, px0, y0, |_| c, &ident);
                    }
                }
                px0 += 1;
            }
        }
    }

    /// SEC_RenderSector: queue the sector's things (FSP_AddObj) and draw
    /// them far to near (FSP_DumpObjs).
    #[allow(clippy::too_many_arguments)]
    fn render_objects(&mut self, s: &mut Screen, w: &World, clr: &[u8], cam: &Camera, sec: usize, cx: i32, cy: i32) {
        self.objs.clear();
        for id in w.sec_lists[sec].iter() {
            let t = w.thing(id);
            if let Some(spr) = t.spr {
                self.add_obj(w, cam, cx, cy, t.x, t.y, t.z, t.angle, spr, t.tirerot, t.tiredir);
            }
        }
        let objs = std::mem::take(&mut self.objs);
        for o in &objs {
            match &w.sprites[o.spr] {
                Fs3::Bitmap { views, .. } => {
                    let sp = &views[o.view.0][o.view.1].sp;
                    sp.draw(s, o.x, o.y, o.w & !1, o.h & !1, &clr[16 * 256..17 * 256]);
                }
                Fs3::Mesh(lods) => self.draw_mesh(s, w, clr, cam, o, lods, cx, cy),
            }
        }
        self.objs = objs;
    }

    #[allow(clippy::too_many_arguments)]
    fn add_obj(&mut self, w: &World, cam: &Camera, scrx: i32, scry: i32, x: u32, y: u32, z: u32, angle: u16, spr: SpriteId, tirerot: u16, tiredir: u16) {
        let x = ((x >> 6) as i32).wrapping_sub((cam.x >> 6) as i32);
        let y = ((y >> 6) as i32).wrapping_sub((cam.y >> 6) as i32);
        let a = cam.angle;
        let ry = fp_mult(x, cos(a)).wrapping_add(fp_mult(y, sin(a)));
        if ry < (4 << 14) {
            return;
        }
        let sp = &w.sprites[spr];
        let vector = matches!(sp, Fs3::Mesh(_));
        if vector && ry >= (20 << 20) {
            return;
        }
        let rx = fp_mult(y, cos(a)).wrapping_sub(fp_mult(x, sin(a)));
        if abs32(rx / 3) > ry {
            return;
        }
        let z = z as i32;
        let h4 = cam.data.h << 2;
        let mut o = Obj { x: rx, y: ry, z, depth: ry, spr, w: 0, h: 0, view: (0, 0), angle, tirerot, tiredir };
        if let Fs3::Bitmap { noscale, views } = sp {
            let sy = scry + fp_mult_div(h4 - (z >> 10), cam.data.focus, ry) - cam.data.horizon;
            if z < 0x100000 && sy < scry {
                return;
            }
            o.x = scrx + fp_mult_div(rx >> 4, cam.data.focus, ry);
            o.y = sy;
            const ROT9: [(usize, i32); 16] = [
                (0, 1), (1, 1), (2, 1), (3, 1), (4, 1), (5, 1), (6, 1), (7, 1),
                (8, -1), (7, -1), (6, -1), (5, -1), (4, -1), (3, -1), (2, -1), (1, -1),
            ];
            let view_angle = |div: u32| {
                (angle as i32 - get_angle(rx >> 4, ry >> 4) as i32 / 2 - cam.angle as i32 + 65536 / div as i32 / 2 + 16384 / 2) as u16
            };
            let (nh, mut ww) = match views.len() {
                9 => ROT9[(view_angle(16) >> 12) as usize],
                17 => {
                    let a = (view_angle(32) >> 11) as usize;
                    if a < 16 { (a, 1) } else { (32 - a, -1) }
                }
                _ => (0, 1),
            };
            let nv = if views[nh].len() == 5 {
                let a = if h4 - (z >> 10) < (1 << 13) { 0 } else { get_angle(ry >> 4, h4 - (z >> 10)) as i32 };
                match a {
                    a if a < 65536 * 20 / 360 => 0,
                    a if a < 65536 * 25 / 360 => 1,
                    a if a < 65536 * 30 / 360 => 2,
                    a if a < 65536 * 35 / 360 => 3,
                    _ => 4,
                }
            } else {
                0
            };
            let v = &views[nh][nv];
            if *noscale {
                ww *= v.sp.w * self.noscale_mul;
                o.h = v.sp.h * self.noscale_mul;
            } else {
                ww *= fp_mult_div(v.w, cam.data.focus, ry);
                o.h = fp_mult_div(v.h, cam.data.focus, ry);
            }
            o.w = ww;
            o.view = (nh, nv);
        }
        // Insert keeping farthest first; when full, drop the farthest.
        let pos = self.objs.iter().position(|p| p.depth <= ry).unwrap_or(self.objs.len());
        if self.objs.len() >= MAX_OBJS {
            if pos == 0 {
                return;
            }
            self.objs.remove(0);
            self.objs.insert(pos - 1, o);
        } else {
            self.objs.insert(pos, o);
        }
    }

    /// Draw3D: transform, light and draw one 3D object.
    #[allow(clippy::too_many_arguments)]
    fn draw_mesh(&mut self, s: &mut Screen, w: &World, clr: &[u8], cam: &Camera, o: &Obj, lods: &[MeshLod], scrx: i32, scry: i32) {
        let Some(li) = self.prepare_mesh(w, cam, o, lods, scrx, scry, s.clip) else { return };
        let mesh = &lods[li].mesh;
        let scry = scry - cam.data.horizon;
        let order = std::mem::take(&mut self.order);
        let proj = Proj::Race { focus: cam.data.focus, scrx, scry };
        for &(_, fi) in &order {
            self.draw_face(s, clr, proj, &mesh.faces[fi], mesh);
        }
        self.order = order;
    }

    /// The shared part of drawing a mesh: projection, visibility, lighting
    /// and face order (in `mv`, `visible`, `order`). Returns the LOD used, or
    /// None if the mesh is off screen.
    #[allow(clippy::too_many_arguments)]
    fn prepare_mesh(&mut self, w: &World, cam: &Camera, o: &Obj, lods: &[MeshLod], scrx: i32, scry: i32, clip: crate::gfx::Rect) -> Option<usize> {
        self.polygon_detail = w.set.polygon_detail;
        let a = 0x8000u16.wrapping_sub(cam.angle).wrapping_add(o.angle);
        let spx = o.x >> 4;
        let spz = (cam.data.h << 2) - (o.z >> 10);
        let scry = scry - cam.data.horizon;
        let (ca, sa) = (cos(a), sin(a));
        let rot = TireRot::new(o);
        let li = if lods.len() > 1 && !self.extra_detail && o.y > ((2 + w.set.background_detail / 2) << 20) {
            self.polygon_detail = false;
            1
        } else {
            0
        };
        let lod = &lods[li];
        let mesh = &lod.mesh;
        self.mv.clear();
        let (mut minx, mut maxx) = (i32::MAX, -i32::MAX);
        for i in 0..mesh.verts.len() {
            let [x, y, z] = rot.vertex(lod, i);
            let mut dz = (o.y >> 4).wrapping_add(fp_mult(z, ca)).wrapping_sub(fp_mult(x, sa));
            let rz = dz;
            dz = dz.max(90 << 4);
            let rx = spx.wrapping_add(fp_mult(x, ca)).wrapping_add(fp_mult(z, sa)) >> 6;
            let ry = spz.wrapping_sub(y) >> 6;
            let px = scrx + fp_mult_div(rx, cam.data.focus, dz >> 2);
            let py = scry + fp_mult_div(ry, cam.data.focus, dz >> 2);
            minx = minx.min(px);
            maxx = maxx.max(px);
            self.mv.push(MV { rx, ry, rz, px, py, l: 0 });
        }
        if minx >= clip.max_x || maxx <= clip.min_x {
            return None;
        }
        // O3DM_ParseVisibility.
        self.visible.clear();
        for f in &mesh.faces {
            let vis = if f.verts.len() < 3 || f.flags & FF_VISIBLE != 0 {
                true
            } else if f.verts.iter().all(|v| self.mv[v.vert].rz <= 300) {
                false
            } else {
                let (v0, v1, v2) = (self.mv[f.verts[0].vert], self.mv[f.verts[1].vert], self.mv[f.verts[2].vert]);
                (v2.px - v1.px).wrapping_mul(v0.py - v1.py).wrapping_sub((v2.py - v1.py).wrapping_mul(v0.px - v1.px)) < 0
            };
            self.visible.push(vis);
        }
        let order_by_z = o.y < (1 << (32 - 6 - 4 + self.extra_detail as i32));
        if self.polygon_detail {
            // O3DMCalcLights: fake per-vertex lighting from the object angle.
            let n1 = if w.set.car_type == 0 && mesh.flags & OF_HASTIRES != 0 { 14 * 2 * 4 } else { 0 };
            let mut l: u32 = 0;
            for (i, v) in self.mv.iter_mut().enumerate() {
                let amp = if i < n1 { 13 << 15 } else { 15 << 16 };
                v.l = (16 << 16) + fp_mult(amp, sin(o.angle.wrapping_add(l as u16)));
                l = l.wrapping_add(5 << 10);
            }
        }
        self.order_faces(mesh, order_by_z);
        Some(li)
    }

    /// The generic object pipeline used by the menus (O3DM_RotateObjVerts,
    /// TranslateObj, ProjectObject, ParseVisibility, CalcLights, Draw), for
    /// an object rotated by `yaw` around the vertical axis at `pos`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_object(&mut self, s: &mut Screen, clr: &[u8], mesh: &crate::mesh::Mesh, yaw: u16, pos: [i32; 3], focus: (i32, i32), center: (i32, i32)) {
        let (sc, cc) = (sin(yaw), cos(yaw));
        self.mv.clear();
        let mut l: u32 = 0;
        for v in &mesh.verts {
            let rx = fp_mult(cc, v[0]).wrapping_add(fp_mult(sc, v[2])).wrapping_add(pos[0]);
            let ry = v[1].wrapping_add(pos[1]);
            let rz = fp_mult(sc.wrapping_neg(), v[0]).wrapping_add(fp_mult(cc, v[2])).wrapping_add(pos[2]);
            let z = rz.max(60);
            let px = fp_mult_div(rx, focus.0, z) + center.0;
            let py = fp_mult_div(ry.wrapping_neg(), focus.1, z) + center.1;
            let lv = (16 << 16) + fp_mult(15 << 16, sin(yaw.wrapping_add(l as u16)));
            l = l.wrapping_add(5 << 10);
            self.mv.push(MV { rx, ry, rz, px, py, l: lv });
        }
        self.visible.clear();
        for f in &mesh.faces {
            let vis = if f.verts.len() < 3 || f.flags & FF_VISIBLE != 0 {
                true
            } else if f.verts.iter().all(|v| self.mv[v.vert].rz <= 300) {
                false
            } else {
                let (v0, v1, v2) = (self.mv[f.verts[0].vert], self.mv[f.verts[1].vert], self.mv[f.verts[2].vert]);
                (v2.px - v1.px).wrapping_mul(v0.py - v1.py).wrapping_sub((v2.py - v1.py).wrapping_mul(v0.px - v1.px)) < 0
            };
            self.visible.push(vis);
        }
        self.polygon_detail = true;
        self.order_faces(mesh, true);
        let order = std::mem::take(&mut self.order);
        let proj = Proj::R3d { fx: focus.0, fy: focus.1, cx: center.0, cy: center.1 };
        for &(_, fi) in &order {
            self.draw_face(s, clr, proj, &mesh.faces[fi], mesh);
        }
        self.order = order;
    }

    /// O3DM_OrderFaces: bucketed back-to-front ordering of visible faces.
    fn order_faces(&mut self, mesh: &crate::mesh::Mesh, by_z: bool) {
        self.order.clear();
        let usable = |fi: usize| mesh.faces[fi].material.is_some() && self.visible[fi];
        if !by_z {
            for fi in (0..mesh.faces.len()).rev() {
                if usable(fi) {
                    self.order.push((0, fi));
                }
            }
            return;
        }
        self.buckets.resize(1024, Vec::new());
        for b in self.buckets.iter_mut() {
            b.clear();
        }
        let mut depths = vec![0i32; mesh.faces.len()];
        let mut maxfi = 0;
        for (fi, f) in mesh.faces.iter().enumerate() {
            if !usable(fi) {
                continue;
            }
            let z = if f.flags & FF_NORDER != 0 {
                0x7FFFFFFF
            } else {
                let sum = f.verts.iter().fold(0i32, |acc, v| {
                    let m = self.mv[v.vert];
                    acc.wrapping_add(m.rz.wrapping_add(abs32(m.rx) / 4).wrapping_add(abs32(m.ry) / 4))
                });
                sum / f.verts.len() as i32
            };
            depths[fi] = z;
            let mut b = z >> 8;
            if b >= 1023 {
                b = 1023;
            } else if b < 0 {
                b = 0;
            } else if maxfi < b {
                maxfi = b;
            }
            let list = &mut self.buckets[b as usize];
            // Each bucket is kept nearest-last; insertion goes before the
            // first entry that is not farther (bucket 1023 is unordered).
            let pos = if b != 1023 { list.iter().position(|&o| depths[o] <= z).unwrap_or(list.len()) } else { 0 };
            list.insert(pos, fi);
        }
        for &fi in &self.buckets[1023] {
            self.order.push((depths[fi], fi));
        }
        for b in (0..=maxfi as usize).rev() {
            for &fi in &self.buckets[b] {
                self.order.push((depths[fi], fi));
            }
        }
    }

    /// O3DMDraw (race) / O3DM_DrawFace (menus) for one face: near-plane
    /// clipping, then the fill mode.
    fn draw_face(&mut self, s: &mut Screen, clr: &[u8], proj: Proj, f: &crate::mesh::Face, mesh: &crate::mesh::Mesh) {
        let plane = if matches!(proj, Proj::Race { .. }) { 4000 } else { 300 };
        let n = f.verts.len();
        self.scrap.clear();
        let mv = &self.mv;
        let fv = |i: usize| (&f.verts[i], mv[f.verts[i].vert]);
        let cut = |a: usize, b: usize| {
            // Point on edge a->b where rz == PLANE (a is behind).
            let ((fa, va), (fb, vb)) = (fv(a), fv(b));
            let d1 = vb.rz - va.rz;
            let d2 = plane - va.rz;
            let x = va.rx + fp_mult_div(vb.rx - va.rx, d2, d1);
            let y = va.ry + fp_mult_div(vb.ry - va.ry, d2, d1);
            let tx = fa.tx + fp_mult_div(fb.tx - fa.tx, d2, d1);
            let ty = fa.ty + fp_mult_div(fb.ty - fa.ty, d2, d1);
            let (px, py) = match proj {
                Proj::Race { focus, scrx, scry } => (scrx + fp_mult_div(x, focus, plane >> 2), scry + fp_mult_div(y, focus, plane >> 2)),
                Proj::R3d { fx, fy, cx, cy } => (fp_mult_div(x, fx, plane) + cx, -fp_mult_div(y, fy, plane) + cy),
            };
            (px, py, tx, ty)
        };
        let mut p = n as i32 - 1;
        let mut j = n as i32 - 1;
        let mut lastv = 0usize;
        while j >= 0 {
            if fv(p as usize).1.rz < plane {
                if fv(lastv).1.rz > plane {
                    let (x, y, tx, ty) = cut(p as usize, lastv);
                    self.scrap.push(Vertex { x, y, tx, ty, l: fv(p as usize).1.l });
                    lastv = p as usize;
                    p -= 1;
                    j -= 1;
                }
                while j >= 0 && fv(p as usize).1.rz < plane {
                    lastv = p as usize;
                    p -= 1;
                    j -= 1;
                }
                if j < 0 {
                    p = n as i32 - 1;
                }
                if fv(p as usize).1.rz > plane {
                    let (x, y, tx, ty) = cut(lastv, p as usize);
                    self.scrap.push(Vertex { x, y, tx, ty, l: fv(p as usize).1.l });
                }
                if j < 0 {
                    break;
                }
            }
            let (fvp, vp) = fv(p as usize);
            self.scrap.push(Vertex { x: vp.px, y: vp.py, tx: fvp.tx, ty: fvp.ty, l: vp.l });
            lastv = p as usize;
            p -= 1;
            j -= 1;
        }
        if self.scrap.len() < 3 {
            return;
        }
        let Some(mat) = f.material.map(|m| &mesh.materials[m]) else { return };
        let clip = s.clip;
        let generic = matches!(proj, Proj::R3d { .. });
        let fill = if mat.flags & MF_TRANS != 0 {
            self.scrap[0].l = (mat.ambient & 31) << 16;
            Fill::Trans
        } else if generic && mat.texture.is_none() {
            if mat.flags & MF_NOSHADE != 0 || f.flags & FF_FLAT != 0 {
                if mat.flags & MF_NOSHADE != 0 {
                    self.scrap[0].l = 31 << 16;
                }
                Fill::Solid(mat.color)
            } else {
                Fill::Shade(mat.color)
            }
        } else if let Some(tex) = &mat.texture {
            if !self.polygon_detail || f.flags & FF_FLAT != 0 || mat.flags & MF_NOSHADE != 0 {
                if mat.flags & MF_HOLES != 0 { Fill::HoleTexture(tex) } else { Fill::Texture(tex) }
            } else {
                Fill::ShadeTexture(tex)
            }
        } else if !self.polygon_detail {
            self.scrap[0].l = 15 << 16;
            Fill::Solid(mat.color)
        } else {
            Fill::Shade(mat.color)
        };
        self.poly.draw(s, clip, &self.scrap, fill, clr);
    }

    /// DrawMap: rotating overhead map with racer dots.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_map(&mut self, s: &mut Screen, w: &World, clr: &[u8], center: (u32, u32, u16), x: i32, y: i32, mw: i32, mh: i32, focus: i32, dots: &[(u32, u32, usize)], dot_sprites: &[Sprite]) {
        let (cx, cy, angle) = center;
        let dx = fp_mult(cos(angle.wrapping_sub(16384)), focus);
        let dy = fp_mult(sin(angle.wrapping_sub(16384)), focus);
        let mut px = cx.wrapping_sub(fpn_mult(dx, mw, 1) as u32).wrapping_add(fpn_mult(dy, mh, 1) as u32);
        let mut py = cy.wrapping_sub(fpn_mult(dy, mh, 1) as u32).wrapping_sub(fpn_mult(dx, mw, 1) as u32);
        for r in 0..mh {
            let row = (y + r) as usize * s.w + x as usize;
            let (mut ux, mut uy) = (px, py);
            for p in &mut s.pix[row..row + mw as usize] {
                let c = w.floor.texel(ux, uy);
                *p = if (160..192).contains(&c) { 185 } else { clr[0x1800 + *p as usize] };
                ux = ux.wrapping_add(dx as u32);
                uy = uy.wrapping_add(dy as u32);
            }
            px = px.wrapping_sub(dy as u32);
            py = py.wrapping_add(dx as u32);
        }
        let k = (1 << 30) / (focus >> 8);
        let ca = fp_mult(cos(16384u16.wrapping_sub(angle)), k);
        let sa = fp_mult(sin(16384u16.wrapping_sub(angle)), k);
        for &(tx, ty, nsp) in dots {
            let rx = ((tx >> 8) as i32).wrapping_sub((cx >> 8) as i32);
            let ry = ((ty >> 8) as i32).wrapping_sub((cy >> 8) as i32);
            let ppx = mw / 2 + fp_mult(rx, ca) - fp_mult(ry, sa);
            let ppy = mh / 2 + fp_mult(ry, ca) + fp_mult(rx, sa);
            if ppx >= 0 && ppx < mw && ppy >= 0 && ppy < mh {
                dot_sprites[nsp].draw_horizontal(s, x + ppx, y + ppy);
            }
        }
        dot_sprites[5].draw_horizontal(s, x + mw / 2, y + mh / 2);
    }
}
