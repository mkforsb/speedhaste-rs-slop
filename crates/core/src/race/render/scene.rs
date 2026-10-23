//! GPU scene extraction — an addition of the port, not in the original.
//!
//! Describes the race views as triangles in camera space plus the static
//! data to draw them, for a GPU renderer (crates/gpu). It reuses the
//! software renderer's decisions — which objects show and how big, mesh
//! culling, face order, lighting and fill types — so both look alike. The
//! floor and the sky are per-pixel passes on the GPU, from the parameters
//! given here.
//!
//! Camera space: `x` lateral (right), `y` depth, `z` up from the camera, in
//! the renderer's world unit (1/1024 of a map unit). A point projects to
//! (cx + focus * x / y, cy - focus * z / y) on the 320x200 frame, where
//! `focus` is [`SceneView::focus`] (the camera's focus / 16).

use std::collections::HashSet;
use std::sync::Arc;

use super::*;
use crate::is2::TRANSPARENT;

/// Shade level meaning "the palette as is" (unlit textures).
pub const LEVEL_RAW: f32 = 32.0;
/// Texel value for transparent pixels.
pub const TEXEL_CLEAR: u16 = 0xFFFF;

/// [`SceneVertex::flags`]: sample `tex` (else the flat colour `color`).
pub const SV_TEXTURE: u32 = 1;
/// Texel 0 is transparent (MF_HOLES textures).
pub const SV_HOLES: u32 = 2;
/// Clamp texel rows (else they wrap, like texel columns by default).
pub const SV_CLAMP_V: u32 = 4;
/// Clamp texel columns.
pub const SV_CLAMP_U: u32 = 8;

/// An indexed texture, row-major; [`TEXEL_CLEAR`] marks holes.
pub struct SceneTexture {
    /// What texture replacements are looked up by: the IS2 name, or
    /// `I3DNAME_n` for material n of a mesh (uppercase, no extension).
    pub name: String,
    pub w: u32,
    pub h: u32,
    pub pix: Vec<u16>,
}

/// Everything that stays the same during a race.
pub struct SceneStatic {
    /// GRAFS.CLR: 32 shade levels x 256 colours.
    pub clr: Vec<u8>,
    /// Floor tiles (64x64 each) and the 128x128 cell map of tile indices.
    pub floor_tiles: Vec<u8>,
    pub floor_map: Vec<u32>,
    /// Per floor tile: the GRAFS.DAT tile | quarter turns << 14.
    pub floor_ids: Vec<u16>,
    /// The background set (NUBESn / MOUNTn).
    pub sky_set: i32,
    /// Clouds and mountains, 320 wide (mountain colour 0 is transparent).
    pub sky: Vec<u8>,
    pub sky_h: i32,
    pub mount: Vec<u8>,
    pub mount_h: i32,
    pub textures: Vec<SceneTexture>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SceneVertex {
    /// Camera space.
    pub pos: [f32; 3],
    /// Texel coordinates (wrapping).
    pub uv: [f32; 2],
    /// Shade level (row of GRAFS.CLR), or [`LEVEL_RAW`].
    pub level: f32,
    /// Depth for hidden surface removal, or 0 for the true depth (`pos`'s
    /// y). Objects use their centre's: the software paints them back to
    /// front as wholes, and a mesh's faces in its own order, so each face
    /// gets the centre depth nudged nearer by its rank.
    pub depth: f32,
    pub tex: u32,
    pub color: u32,
    pub flags: u32,
}

/// One race view.
pub struct SceneView {
    /// x, y, w, h on the 320x200 frame.
    pub rect: [i32; 4],
    pub cam_x: u32,
    pub cam_y: u32,
    /// The game's cos/sin of the camera angle (sin is the original's).
    pub cos: f32,
    pub sin: f32,
    /// Camera height above the floor.
    pub height: f32,
    /// Projection scale (the camera's focus / 16, see the module doc).
    pub focus: f32,
    pub cx: f32,
    pub cy: f32,
    /// Sky band: rows at the top of the view, first source row, and the
    /// horizontal offsets of the clouds and mountains.
    pub sky_rows: i32,
    pub tilt: i32,
    pub sky_off: i32,
    pub mount_off: i32,
    /// Opaque (or alpha-tested) triangles.
    pub tris: Vec<SceneVertex>,
    /// Translucent triangles: darken what's behind to `level`'s shade.
    pub trans: Vec<SceneVertex>,
}

/// A race frame for the GPU renderer (drawn with the frame's palette).
pub struct Scene {
    pub stat: Arc<SceneStatic>,
    pub views: Vec<SceneView>,
    /// HUD and messages, 320x200: colour, and per pixel one of the `OV_*`
    /// kinds.
    pub overlay: Vec<u8>,
    pub overlay_kind: Vec<u8>,
}

/// [`Scene::overlay_kind`]: nothing drawn here.
pub const OV_NONE: u8 = 0;
/// An opaque overlay pixel.
pub const OV_OPAQUE: u8 = 1;
/// Darkened through shade level [`OV_DARK_LEVEL`] (the translucent minimap).
pub const OV_DARK: u8 = 2;
pub const OV_DARK_LEVEL: usize = 24;

impl Scene {
    /// Classify the overlays from two renderings of them, over screens
    /// cleared to 0 and to 255: pixels that agree are opaque, pixels that
    /// show the darkening of 0 and 255 are darkened, the rest see-through.
    pub fn overlay_from(a: &[u8], b: &[u8], clr: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let dark = &clr[OV_DARK_LEVEL * 256..][..256];
        a.iter()
            .zip(b)
            .map(|(&a, &b)| {
                if a == b {
                    (a, OV_OPAQUE)
                } else if a == dark[0] && b == dark[255] {
                    (0, OV_DARK)
                } else {
                    (0, OV_NONE)
                }
            })
            .unzip()
    }
}

/// Where each game texture went in [`SceneStatic::textures`].
pub struct SceneIndex {
    walls: Vec<u32>,
    /// Per sprite: bitmap views [h][v], or mesh materials [lod][material].
    sprites: Vec<Vec<Vec<Option<u32>>>>,
}

fn sprite_texture(sp: &Sprite, name: &str) -> SceneTexture {
    let (w, h) = (sp.w.max(0) as usize, sp.h.max(0) as usize);
    let mut pix = vec![TEXEL_CLEAR; w * h];
    for y in 0..h {
        for x in 0..w {
            let v = if sp.horizontal { sp.pix[y * w + x] } else { sp.pix[x * h + y] };
            if v != TRANSPARENT {
                pix[y * w + x] = v as u16;
            }
        }
    }
    SceneTexture { name: name.to_string(), w: w as u32, h: h as u32, pix }
}

impl SceneStatic {
    pub fn build(w: &World, clr: &[u8]) -> (SceneStatic, SceneIndex) {
        let mut textures = Vec::new();
        let mut add = |t: SceneTexture| {
            textures.push(t);
            (textures.len() - 1) as u32
        };
        let walls = w.sec.textures.iter().zip(&w.sec.tex_names).map(|(t, n)| add(sprite_texture(t, n))).collect();
        let sprites = w
            .sprites
            .iter()
            .map(|s| match s {
                Fs3::Bitmap { views, .. } => views.iter().map(|vv| vv.iter().map(|v| Some(add(sprite_texture(&v.sp, &v.name)))).collect()).collect(),
                Fs3::Mesh(lods) => lods
                    .iter()
                    .map(|l| {
                        l.mesh
                            .materials
                            .iter()
                            .enumerate()
                            .map(|(i, m)| {
                                let name = format!("{}_{i}", l.name);
                                m.texture.as_ref().map(|t| add(SceneTexture { name, w: 64, h: 64, pix: t.iter().map(|&c| c as u16).collect() }))
                            })
                            .collect()
                    })
                    .collect(),
            })
            .collect();
        let stat = SceneStatic {
            clr: clr.to_vec(),
            floor_tiles: w.floor.grafs.clone(),
            floor_map: w.floor.map.clone(),
            floor_ids: w.floor.tiles.clone(),
            sky_set: w.bg.n,
            sky: w.bg.sky.clone(),
            sky_h: w.bg.sky_h,
            mount: w.bg.mount.clone(),
            mount_h: w.bg.mount_h,
            textures,
        };
        (stat, SceneIndex { walls, sprites })
    }
}

/// Camera-space position of a world point (map units) at height `z`.
fn cam_space(cam: &Camera, x: u32, y: u32, z: f32, h: f32) -> [f32; 3] {
    let dx = x.wrapping_sub(cam.x) as i32 as f32 / 1024.0;
    let dy = y.wrapping_sub(cam.y) as i32 as f32 / 1024.0;
    let (c, s) = (cos(cam.angle) as f32 / (1 << 30) as f32, sin(cam.angle) as f32 / (1 << 30) as f32);
    [dy * c - dx * s, dx * c + dy * s, z - h]
}

impl Renderer {
    /// RenderView, but producing a [`SceneView`] for the GPU. `rect` is the
    /// view on the 320x200 frame; the camera is updated like render_view.
    pub fn scene_view(&mut self, w: &World, idx: &SceneIndex, cam: &mut Camera, tst_angle: u16, rect: [i32; 4]) -> SceneView {
        self.extra_detail = cam.data.flags & CAMF_STATIC != 0;
        self.noscale_mul = 1;
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
        let s2 = w.sec.find(Some(cam.sec), to_sec(cam.x), to_sec(cam.y));
        if s2 != 0 {
            cam.sec = s2;
        }
        let cx = rect[0] + rect[2] / 2;
        // What render_sectors gets as cy (the sky band's bottom), and the
        // projection centre after the horizon.
        let sky_bottom = rect[1] + hsk;
        let cy = sky_bottom - cam.data.horizon;
        let h4 = (cam.data.h << 2) as f32;
        let mut v = SceneView {
            rect,
            cam_x: cam.x,
            cam_y: cam.y,
            cos: cos(cam.angle) as f32 / (1 << 30) as f32,
            sin: sin(cam.angle) as f32 / (1 << 30) as f32,
            height: h4,
            focus: cam.data.focus as f32 / 16.0,
            cx: cx as f32,
            cy: cy as f32,
            sky_rows: hsk.min(rect[3]),
            tilt: cam.tilt,
            sky_off: (3 * cam.angle as i32 * 320 / 65536) % 320,
            mount_off: (4 * cam.angle as i32 * 320 / 65536) % 320,
            tris: Vec::new(),
            trans: Vec::new(),
        };
        self.scene_walls(w, idx, cam, &mut v);
        let clip = crate::gfx::Rect { min_x: rect[0], min_y: rect[1], max_x: rect[0] + rect[2], max_y: rect[1] + rect[3] };
        for sec in 0..w.sec.secs.len() {
            self.objs.clear();
            for id in w.sec_lists[sec].iter() {
                let t = w.thing(id);
                if let Some(spr) = t.spr {
                    self.add_obj(w, cam, cx, sky_bottom, t.x, t.y, t.z, t.angle, spr, t.tirerot, t.tiredir);
                }
            }
            let objs = std::mem::take(&mut self.objs);
            for o in &objs {
                match &w.sprites[o.spr] {
                    Fs3::Bitmap { views, .. } => scene_sprite(&mut v, idx, o, &views[o.view.0][o.view.1].sp, cx, cy),
                    Fs3::Mesh(lods) => self.scene_mesh(w, idx, cam, o, lods, cx, sky_bottom, clip, &mut v),
                }
            }
            self.objs = objs;
        }
        cam.angle = cam.angle.wrapping_sub(tst_angle);
        cam.data.horizon = camh;
        v
    }

    fn scene_walls(&mut self, w: &World, idx: &SceneIndex, cam: &Camera, v: &mut SceneView) {
        let mut done = HashSet::new();
        for sec in &w.sec.secs {
            for side in &sec.sides {
                let Some(tex) = side.tex else { continue };
                if !done.insert((side.v0.min(side.v1), side.v0.max(side.v1), tex)) {
                    continue;
                }
                let t = &w.sec.textures[tex];
                let (a, b) = (w.sec.verts[side.v0], w.sec.verts[side.v1]);
                let (ax, ay, bx, by) = (to_map(a.0), to_map(a.1), to_map(b.0), to_map(b.1));
                let pa = cam_space(cam, ax, ay, 0.0, v.height);
                let pb = cam_space(cam, bx, by, 0.0, v.height);
                if pa[1] < 1.0 && pb[1] < 1.0 {
                    continue;
                }
                // Texture columns count from the end on the left on screen.
                let (p0, p1) = if pa[0] * pb[1] - pb[0] * pa[1] <= 0.0 { (pa, pb) } else { (pb, pa) };
                let len = ((bx.wrapping_sub(ax) as i32 as f32).powi(2) + (by.wrapping_sub(ay) as i32 as f32).powi(2)).sqrt() / 1024.0;
                let (u1, top) = (len / 256.0, (t.h << 8) as f32);
                let tid = idx.walls[tex];
                let vert = |p: [f32; 3], u: f32, up: bool| SceneVertex {
                    pos: [p[0], p[1], if up { p[2] + top } else { p[2] }],
                    uv: [u, if up { 0.0 } else { t.h as f32 }],
                    level: 16.0,
                    tex: tid,
                    flags: SV_TEXTURE | SV_CLAMP_V,
                    ..Default::default()
                };
                let q = [vert(p0, 0.0, false), vert(p1, u1, false), vert(p1, u1, true), vert(p0, 0.0, true)];
                v.tris.extend([q[0], q[1], q[2], q[0], q[2], q[3]]);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn scene_mesh(&mut self, w: &World, idx: &SceneIndex, cam: &Camera, o: &Obj, lods: &[MeshLod], cx: i32, cy: i32, clip: crate::gfx::Rect, v: &mut SceneView) {
        let Some(li) = self.prepare_mesh(w, cam, o, lods, cx, cy, clip) else { return };
        let lod = &lods[li];
        let mesh = &lod.mesh;
        let a = 0x8000u16.wrapping_sub(cam.angle).wrapping_add(o.angle);
        let (ca, sa) = (cos(a) as f64 / (1u64 << 30) as f64, sin(a) as f64 / (1u64 << 30) as f64);
        let spz = ((cam.data.h << 2) - (o.z >> 10)) as f64;
        let rot = TireRot::new(o);
        let pos: Vec<[f32; 3]> = (0..mesh.verts.len())
            .map(|i| {
                let [x, y, z] = rot.vertex(lod, i);
                let (x, y, z) = (x as f64, y as f64, z as f64);
                [(o.x as f64 / 16.0 + x * ca + z * sa) as f32, (o.y as f64 / 16.0 + z * ca - x * sa) as f32, (y - spz) as f32]
            })
            .collect();
        let tex = idx.sprites.get(o.spr).and_then(|s| s.get(li));
        let depth = o.depth as f32 / 16.0;
        for (rank, &(_, fi)) in self.order.iter().enumerate() {
            let f = &mesh.faces[fi];
            let Some(mi) = f.material else { continue };
            let mat = &mesh.materials[mi];
            let level = |k: usize| self.mv[f.verts[k].vert].l as f32 / 65536.0;
            // As draw_face: translucent, unlit or lit texture, flat or shaded colour.
            let (trans, lit, flags, tid) = if mat.flags & MF_TRANS != 0 {
                (true, Some((mat.ambient & 31) as f32), 0, 0)
            } else if mat.texture.is_some() {
                let tid = tex.and_then(|t| t.get(mi).copied().flatten()).unwrap_or(0);
                let holes = if mat.flags & MF_HOLES != 0 { SV_HOLES } else { 0 };
                if !self.polygon_detail || f.flags & FF_FLAT != 0 || mat.flags & MF_NOSHADE != 0 {
                    (false, Some(LEVEL_RAW), SV_TEXTURE | holes, tid)
                } else {
                    (false, None, SV_TEXTURE, tid)
                }
            } else if !self.polygon_detail {
                (false, Some(15.0), 0, 0)
            } else {
                (false, None, 0, 0)
            };
            let fv = |k: usize| SceneVertex {
                pos: pos[f.verts[k].vert],
                uv: [f.verts[k].tx as f32 / (1 << 18) as f32, f.verts[k].ty as f32 / (1 << 18) as f32],
                level: lit.unwrap_or_else(|| level(k)),
                depth: depth * (1.0 - (rank + 1) as f32 * 1e-5),
                tex: tid,
                color: mat.color as u32,
                flags,
            };
            let out = if trans { &mut v.trans } else { &mut v.tris };
            let poly = clip_near(&(0..f.verts.len()).map(fv).collect::<Vec<_>>(), MESH_NEAR);
            for k in 1..poly.len().saturating_sub(1) {
                out.extend([poly[0], poly[k], poly[k + 1]]);
            }
        }
    }
}

/// The near plane mesh faces are cut at (O3DMDraw's PLANE).
const MESH_NEAR: f32 = 4000.0;

/// Cut a convex polygon to the part with depth (`pos[1]`) >= `near`.
fn clip_near(poly: &[SceneVertex], near: f32) -> Vec<SceneVertex> {
    if poly.iter().all(|v| v.pos[1] >= near) {
        return poly.to_vec();
    }
    let lerp = |a: &SceneVertex, b: &SceneVertex| {
        let t = (near - a.pos[1]) / (b.pos[1] - a.pos[1]);
        let l = |x: f32, y: f32| x + (y - x) * t;
        SceneVertex {
            pos: [l(a.pos[0], b.pos[0]), near, l(a.pos[2], b.pos[2])],
            uv: [l(a.uv[0], b.uv[0]), l(a.uv[1], b.uv[1])],
            level: l(a.level, b.level),
            ..*a
        }
    };
    let mut out = Vec::new();
    for (i, a) in poly.iter().enumerate() {
        let b = &poly[(i + 1) % poly.len()];
        let (ina, inb) = (a.pos[1] >= near, b.pos[1] >= near);
        if ina {
            out.push(*a);
        }
        if ina != inb {
            out.push(lerp(a, b));
        }
    }
    out
}

/// A billboard covering the screen rectangle the software would draw.
fn scene_sprite(v: &mut SceneView, idx: &SceneIndex, o: &Obj, sp: &Sprite, cx: i32, cy: i32) {
    let (w, h) = (o.w & !1, o.h & !1);
    if w == 0 || h == 0 || sp.w == 0 || sp.h == 0 {
        return;
    }
    let Some(tid) = idx.sprites.get(o.spr).and_then(|s| s.get(o.view.0)).and_then(|vv| vv.get(o.view.1)).copied().flatten() else { return };
    let (aw, flip) = (w.abs(), w < 0);
    let x0 = o.x - fp_mult_div(sp.dx, aw, sp.w);
    let y0 = o.y - fp_mult_div(sp.dy, h, sp.h);
    let d = o.depth as f32 / 16.0;
    let f = v.focus;
    let at = |sx: i32, sy: i32| [(sx - cx) as f32 * d / f, d, (cy - sy) as f32 * d / f];
    let (u0, u1) = if flip { (sp.w as f32, 0.0) } else { (0.0, sp.w as f32) };
    let vert = |p: [f32; 3], u: f32, t: f32| SceneVertex { pos: p, uv: [u, t], level: 16.0, depth: d, tex: tid, color: 0, flags: SV_TEXTURE | SV_CLAMP_U | SV_CLAMP_V };
    let q = [
        vert(at(x0, y0 + h), u0, sp.h as f32),
        vert(at(x0 + aw, y0 + h), u1, sp.h as f32),
        vert(at(x0 + aw, y0), u1, 0.0),
        vert(at(x0, y0), u0, 0.0),
    ];
    v.tris.extend([q[0], q[1], q[2], q[0], q[2], q[3]]);
}
