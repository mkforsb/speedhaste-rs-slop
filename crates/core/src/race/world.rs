//! The race world: things (game/things.c) and their sprites (game/flsprs.c).
//!
//! The original keeps things in intrusive doubly linked lists (global,
//! per-sector and per-64x64-block) with insertion at the head. Iteration
//! order matters (collision, update and draw order), so [`List`] mimics a
//! head-inserted list with a vector stored in reverse.

use std::collections::HashMap;

use super::map::{Backgrounds, Floor, PathPoint, SectorMap, StartPos, StaticCamera, to_sec};
use crate::data::{Data, Result};
use crate::fx::*;
use crate::is2::Sprite;
use crate::mesh::{Mesh, OF_HASTIRES};

pub type ThingId = usize;
pub type SpriteId = usize;

/// A head-inserted list: `iter()` yields the most recently added first.
#[derive(Default, Clone)]
pub struct List(Vec<ThingId>);

impl List {
    pub fn push_head(&mut self, t: ThingId) {
        self.0.push(t);
    }
    pub fn remove(&mut self, t: ThingId) {
        if let Some(i) = self.0.iter().rposition(|&x| x == t) {
            self.0.remove(i);
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = ThingId> + '_ {
        self.0.iter().rev().copied()
    }
    pub fn snapshot(&self) -> Vec<ThingId> {
        self.iter().collect()
    }
}

// ---------------------------------------------------------------------------
// Things

pub const THNF_SOLID: u8 = 0x01;

pub const THNT_PLAYER: u32 = 0x0100;
pub const THNT_CAR: u32 = 0x0500;
pub const THNT_OBSTACLE: u32 = 0x0600;
pub const THNT_SPARKS: u32 = 0x1000;
pub const THNT_SMOKE: u32 = 0x1100;
pub const THNT_RACEPOS: u32 = 0x1200;
pub const THNT_HUMANPOS: u32 = 0x1300;

/// Collision shape: bounding circle plus optional polygon (THN_TBounds).
pub struct Bounds {
    pub radius: u32,
    pub points: &'static [(i32, i32)],
}

/// Shared by player and robot cars.
pub static CAR_BOUNDS: Bounds = Bounds {
    radius: 0xB00000,
    points: &[(0xB00000, 0x380000), (0xB00000, -0x380000), (-0x800000, -0x380000), (-0x800000, 0x380000)],
};

pub struct Sparks {
    pub clock: u32,
    pub dx: i32,
    pub dy: i32,
}

pub struct Smoke {
    pub clock: u32,
    pub dx: i32,
    pub dy: i32,
    pub dz: i32,
}

pub enum Kind {
    None,
    Player(Box<super::control::Player>),
    Car(Box<super::robot::Car>),
    Sparks(Sparks),
    Smoke(Smoke),
}

pub struct Thing {
    pub ty: u32,
    pub magic: u32,
    pub x: u32,
    pub y: u32,
    pub z: u32,
    pub angle: u16,
    pub flags: u8,
    pub spr: Option<SpriteId>,
    pub bounds: Option<&'static Bounds>,
    pub kind: Kind,
    pub sec: usize,
    pub tirerot: u16,
    pub tiredir: u16,
}

impl Thing {
    pub fn is_thinker(&self) -> bool {
        !matches!(self.kind, Kind::None)
    }
}

// ---------------------------------------------------------------------------
// Sprites (FS3)

pub struct MeshLod {
    /// The I3D file (without extension), naming texture replacements.
    pub name: String,
    pub mesh: Mesh,
    pub tirev: usize,
    pub tirecenter: [[i32; 3]; 4],
}

/// One bitmap view with its world size (FS2_TSprite).
pub struct Fs2 {
    pub w: i32,
    pub h: i32,
    pub sp: Sprite,
    /// The IS2 file (without extension).
    pub name: String,
}

pub enum Fs3 {
    /// 3D object, one entry per level of detail.
    Mesh(Vec<MeshLod>),
    /// Bitmaps: [horizontal angle][vertical angle].
    Bitmap { noscale: bool, views: Vec<Vec<Fs2>> },
}

/// ProcessMesh: apply the object's scale/centre and find tire centres.
fn process_mesh(mut mesh: Mesh, name: &str) -> MeshLod {
    for s in mesh.sc.iter_mut() {
        if *s == 0 {
            *s = 1 << 19;
        }
        *s >>= 1;
    }
    for v in mesh.verts.iter_mut() {
        for k in 0..3 {
            v[k] = fpn_mult(v[k].wrapping_sub(mesh.dc[k]), mesh.sc[k], 16);
        }
    }
    let mut tirev = 0;
    let mut tirecenter = [[0; 3]; 4];
    if mesh.flags & OF_HASTIRES != 0 {
        tirev = match mesh.verts.len() {
            345 | 240 => 28,
            182 => 20,
            _ => 0,
        };
        if tirev > 0 {
            for (i, tc) in tirecenter.iter_mut().enumerate() {
                let mut c = [0i32; 3];
                for v in &mesh.verts[i * tirev..(i + 1) * tirev] {
                    for k in 0..3 {
                        c[k] = c[k].wrapping_add(v[k]);
                    }
                }
                *tc = c.map(|c| c / tirev as i32);
            }
        }
    }
    MeshLod { name: name.trim_end_matches(".I3D").to_string(), mesh, tirev, tirecenter }
}

fn fs2(sp: Sprite, name: &str) -> Fs2 {
    let w = fp_mult_div(sp.x_ratio, sp.w * 0x2000, 55 << 8);
    let h = fp_mult_div(sp.y_ratio, sp.h * 0x2000, 66 << 8);
    Fs2 { w, h, sp, name: name.trim_end_matches(".IS2").to_string() }
}

/// FS3_Load: a mesh (NAME.I3D or NAMEA/B/...I3D) or bitmap views
/// (NAME.IS2 or NAME[A-][A-].IS2).
pub fn load_fs3(d: &Data, name: &str) -> Result<Fs3> {
    let up = name.to_ascii_uppercase();
    if d.exists(&format!("{up}.I3D")) {
        let n = format!("{up}.I3D");
        return Ok(Fs3::Mesh(vec![process_mesh(Mesh::load(d, &n)?, &n)]));
    }
    let lods: Vec<String> = (b'A'..=b'Z').map(|c| format!("{up}{}.I3D", c as char)).take_while(|n| d.exists(n)).collect();
    if !lods.is_empty() {
        return Ok(Fs3::Mesh(lods.iter().map(|n| Mesh::load(d, n).map(|m| process_mesh(m, n))).collect::<Result<_>>()?));
    }
    let noscale = up.starts_with("RACEPOS") || up.starts_with("HUMANSP");
    if d.exists(&format!("{up}.IS2")) {
        let n = format!("{up}.IS2");
        return Ok(Fs3::Bitmap { noscale, views: vec![vec![fs2(Sprite::load(d, &n)?, &n)]] });
    }
    let base: String = up.chars().take(6).collect();
    let mut views = Vec::new();
    for h in b'A'..=b'Z' {
        let mut v = Vec::new();
        for vv in b'A'..=b'Z' {
            let n = format!("{base}{}{}.IS2", h as char, vv as char);
            if !d.exists(&n) {
                break;
            }
            v.push(fs2(Sprite::load(d, &n)?, &n));
        }
        if v.is_empty() {
            break;
        }
        views.push(v);
    }
    if views.is_empty() {
        return Err(crate::data::DataError::Missing(format!("object {name}")));
    }
    Ok(Fs3::Bitmap { noscale, views })
}

// ---------------------------------------------------------------------------

pub struct Racer {
    pub thn: ThingId,
    pub npoint: i32,
    pub nlap: i32,
    pub laptime: [u32; 5],
    pub laptimeleft: i32,
    pub bestlaptime: u32,
    pub totaltime: u32,
    pub cltimeout: i32,
    pub clexttime: i32,
    pub clfinallap: i32,
    pub clendrace: i32,
    pub finished: bool,
}

impl Racer {
    pub fn new(thn: ThingId) -> Racer {
        Racer {
            thn,
            npoint: 0,
            nlap: 0,
            laptime: [0; 5],
            laptimeleft: 0,
            bestlaptime: 0,
            totaltime: 0,
            cltimeout: 0,
            clexttime: 0,
            clfinallap: 0,
            clendrace: 0,
            finished: false,
        }
    }
}

/// Sound effects loaded for the race (RACE_Eff* in race.c).
#[derive(Default, Clone, Copy)]
pub struct Sfx {
    pub motor: Option<usize>,
    pub crash: Option<usize>,
    pub skid: Option<usize>,
    pub lap_record: Option<usize>,
    pub timeout: Option<usize>,
    pub extended: Option<usize>,
    pub you_win: Option<usize>,
    pub game_over: Option<usize>,
}

/// Settings that the original kept in globals.
pub struct Settings {
    pub car_type: i32,
    pub timed_race: bool,
    pub race_laps: i32,
    pub race_time: [i32; 5],
    pub decoration_detail: i32,
    pub floor_detail: bool,
    pub walls_detail: bool,
    pub polygon_detail: bool,
    pub background_detail: i32,
    pub no_collide: bool,
}

pub struct World {
    pub set: Settings,
    pub rng: Rng,
    pub audio: std::sync::Arc<std::sync::Mutex<crate::audio::Audio>>,
    pub sfx: Sfx,

    pub floor: Floor,
    pub bg: Backgrounds,
    pub sec: SectorMap,
    pub path: Vec<PathPoint>,
    pub start_pos: Vec<StartPos>,
    pub static_cams: Vec<StaticCamera>,
    pub car_models: Vec<crate::models::CarModel>,

    pub things: Vec<Option<Thing>>,
    pub thing_list: List,
    pub thinker_list: List,
    pub blocks: Vec<List>,
    pub sec_lists: Vec<List>,
    pub sprites: Vec<Fs3>,
    pub sprite_names: HashMap<String, SpriteId>,
    pub sparks: [SpriteId; 6],
    pub smoke: [SpriteId; 18],

    pub racers: Vec<Racer>,
    /// Race order: indices into `racers`, leader first (Map.racers).
    pub order: Vec<usize>,

    pub magic: u32,
    pub game_clock: u32,
    pub global_clock: u32,
    pub race_started: bool,
    pub paused: bool,
    /// Gamepad rumble requests for human players this frame: (player, strong
    /// motor, weak motor, ms). An addition of the port.
    pub rumble: Vec<(ThingId, f32, f32, u32)>,
}

impl World {
    pub fn thing(&self, t: ThingId) -> &Thing {
        self.things[t].as_ref().expect("dead thing")
    }

    pub fn thing_mut(&mut self, t: ThingId) -> &mut Thing {
        self.things[t].as_mut().expect("dead thing")
    }

    pub fn alive(&self, t: ThingId) -> bool {
        self.things.get(t).is_some_and(Option::is_some)
    }

    /// FS3_New: load a sprite once and share it by name.
    pub fn sprite(&mut self, d: &Data, name: &str) -> Result<SpriteId> {
        let key = name.to_ascii_uppercase();
        if let Some(&i) = self.sprite_names.get(&key) {
            return Ok(i);
        }
        self.sprites.push(load_fs3(d, name)?);
        self.sprite_names.insert(key, self.sprites.len() - 1);
        Ok(self.sprites.len() - 1)
    }

    pub fn eff_start(&self, chan: i32, rate: u32, vol: i32, pan: i32, eff: Option<usize>) -> i32 {
        self.audio.lock().map(|mut a| a.eff_start(chan, rate, vol, pan, eff)).unwrap_or(-1)
    }

    #[inline]
    fn block(x: u32, y: u32) -> usize {
        ((x >> 26) + 64 * (y >> 26)) as usize
    }

    /// The common part of THN_AddThing: create and link a bare thing.
    /// Type-specific initialisation is done by `race::add_thing`.
    pub fn new_thing(&mut self, x: u32, y: u32, z: u32, ty: u32, angle: u16) -> ThingId {
        let sec = self.sec.find(None, to_sec(x), to_sec(y));
        let t = Thing {
            ty,
            magic: 0,
            x,
            y,
            z,
            angle,
            flags: 0,
            spr: None,
            bounds: None,
            kind: Kind::None,
            sec,
            tirerot: 0,
            tiredir: 0,
        };
        let id = match self.things.iter().position(Option::is_none) {
            Some(i) => {
                self.things[i] = Some(t);
                i
            }
            None => {
                self.things.push(Some(t));
                self.things.len() - 1
            }
        };
        self.blocks[Self::block(x, y)].push_head(id);
        self.thing_list.push_head(id);
        self.sec_lists[sec].push_head(id);
        id
    }

    /// Move a freshly created thing from the passive to the thinker list.
    pub fn make_thinker(&mut self, id: ThingId) {
        self.thing_list.remove(id);
        self.thinker_list.push_head(id);
    }

    /// THN_DeleteThing.
    pub fn delete_thing(&mut self, id: ThingId) {
        let Some(t) = self.things[id].take() else { return };
        self.blocks[Self::block(t.x, t.y)].remove(id);
        if t.is_thinker() {
            self.thinker_list.remove(id);
        } else {
            self.thing_list.remove(id);
        }
        self.sec_lists[t.sec].remove(id);
    }

    /// THN_MoveThing.
    pub fn move_thing(&mut self, id: ThingId, nx: u32, ny: u32) {
        let (ox, oy, osec) = {
            let t = self.thing(id);
            (t.x, t.y, t.sec)
        };
        let (b0, b1) = (Self::block(ox, oy), Self::block(nx, ny));
        if b0 != b1 {
            self.blocks[b0].remove(id);
            self.blocks[b1].push_head(id);
        }
        let sec = self.sec.find(Some(osec), to_sec(nx), to_sec(ny));
        let t = self.thing_mut(id);
        t.x = nx;
        t.y = ny;
        if sec != osec {
            t.sec = sec;
            self.sec_lists[osec].remove(id);
            self.sec_lists[sec].push_head(id);
        }
    }

    /// THN_CalcCollide: first solid thing colliding with `id`.
    pub fn calc_collide(&self, id: ThingId) -> Option<ThingId> {
        let t = self.thing(id);
        if t.flags & THNF_SOLID == 0 {
            return None;
        }
        let r = t.bounds?.radius;
        let mx = (t.x.wrapping_sub(r) >> 26) as i32;
        let my = (t.y.wrapping_sub(r) >> 26) as i32;
        let mx2 = ((t.x.wrapping_add(r) >> 26) as i32).min(63);
        let my2 = ((t.y.wrapping_add(r) >> 26) as i32).min(63);
        for by in my.max(0)..=my2 {
            for bx in mx.max(0)..=mx2 {
                for p in self.blocks[(bx + 64 * by) as usize].iter() {
                    if p != id && collide_things(t, self.thing(p)) {
                        return Some(p);
                    }
                }
            }
        }
        None
    }
}

/// THN_CollideThing. In this game every solid thing is a car using
/// [`CAR_BOUNDS`], but the general shape-vs-circle cases are kept.
pub fn collide_things(t1: &Thing, t2: &Thing) -> bool {
    let (Some(b1), Some(b2)) = (t1.bounds, t2.bounds) else { return false };
    let x1 = (t1.x >> 8) as i32;
    let x2 = (t2.x >> 8) as i32;
    let y1 = (t1.y >> 8) as i32;
    let y2 = (t2.y >> 8) as i32;
    let r = ((b1.radius.wrapping_add(b2.radius)) >> 8) as i32;
    let dx = abs32(x2.wrapping_sub(x1));
    let dy = abs32(y2.wrapping_sub(y1));
    // Unsigned comparisons in the original.
    if dx as u32 >= r as u32 || dy as u32 >= r as u32 {
        return false;
    }
    let rr = fp16_pow2(r) as u32;
    let d = fp16_pow2(dx).wrapping_add(fp16_pow2(dy)) as u32;
    if d >= rr {
        return false;
    }
    if b1.points.is_empty() && b2.points.is_empty() {
        return true;
    }
    let (t1, t2, b1, b2) = if b1.points.is_empty() { (t2, t1, b2, b1) } else { (t1, t2, b1, b2) };
    let (x1, y1, x2, y2) = ((t1.x >> 8) as i32, (t1.y >> 8) as i32, (t2.x >> 8) as i32, (t2.y >> 8) as i32);
    let corner = |p: (i32, i32), x: i32, y: i32, ca: i32, sa: i32| {
        (
            x.wrapping_add(fp_mult(p.0 >> 8, ca)).wrapping_add(fp_mult(p.1 >> 8, sa)),
            y.wrapping_sub(fp_mult(p.0 >> 8, sa)).wrapping_add(fp_mult(p.1 >> 8, ca)),
        )
    };
    let (sa1, ca1) = (sin(t1.angle).wrapping_neg(), cos(t1.angle));
    // Count crossings of the polygon edges with the vertical through (cx, cy).
    let inside = |pts: &[(i32, i32)], x: i32, y: i32, ca: i32, sa: i32, cx: i32, cy: i32| -> Option<bool> {
        let (mut up, mut dn) = (0u32, 0u32);
        let (mut ax, mut ay) = corner(pts[pts.len() - 1], x, y, ca, sa);
        for &p in pts {
            let (bx, by) = corner(p, x, y, ca, sa);
            let (ddx, ddy) = (bx.wrapping_sub(ax), by.wrapping_sub(ay));
            if (ax > bx && ((ax <= cx && bx > cx) || (ax >= cx && bx < cx))) || (ax < bx && ((ax < cx && bx >= cx) || (ax > cx && bx <= cx))) {
                if ay > cy && by > cy {
                    up += 1;
                } else if ay < cy && by < cy {
                    dn += 1;
                } else {
                    let yp = ay.wrapping_add(fp_mult_div(cx.wrapping_sub(ax), ddy, ddx));
                    if yp > cy {
                        up += 1;
                    } else if yp < cy {
                        dn += 1;
                    } else {
                        return None; // On the edge: collision.
                    }
                }
            } else if ax == cx && bx == cx && ((ay >= cy && by <= cy) || (ay <= cy && by >= cy)) {
                return None;
            }
            ax = bx;
            ay = by;
        }
        Some(up & 1 != 0 && dn & 1 != 0)
    };
    if b2.points.is_empty() {
        // Shaped object against circular object.
        let r2 = fp16_pow2((b2.radius >> 8) as i32) as u32;
        if r2 >= d {
            return true;
        }
        let pts = b1.points;
        let (mut ax, mut ay) = corner(pts[pts.len() - 1], x1, y1, ca1, sa1);
        for &p in pts {
            if (fp16_pow2(ax.wrapping_sub(x2)).wrapping_add(fp16_pow2(ay.wrapping_sub(y2))) as u32) < r2 {
                return true;
            }
            let (bx, by) = corner(p, x1, y1, ca1, sa1);
            let (ddx, ddy) = (bx.wrapping_sub(ax), by.wrapping_sub(ay));
            let (a, b) = (ddy, ddx.wrapping_neg());
            let lhs = fp16_pow2(fp16_mult(a, x2.wrapping_sub(ax)).wrapping_add(fp16_mult(b, y2.wrapping_sub(ay))));
            if lhs < fp16_mult(fp16_pow2(a).wrapping_add(fp16_pow2(b)), r2 as i32) {
                if abs32(ddy) > abs32(ddx) {
                    let rr = y2.wrapping_sub(ay).wrapping_add(fp24_mult(fp24_div(ddx, ddy), x2.wrapping_sub(ax)));
                    if sgn(rr) == sgn(ddy) && abs32(rr) < abs32(ddy) {
                        return true;
                    }
                } else {
                    let rr = x2.wrapping_sub(ax).wrapping_add(fp24_mult(fp24_div(ddy, ddx), y2.wrapping_sub(ay)));
                    if sgn(rr) == sgn(ddx) && abs32(rr) < abs32(ddx) {
                        return true;
                    }
                }
            }
            ax = bx;
            ay = by;
        }
        return inside(pts, x1, y1, ca1, sa1, x2, y2) != Some(false);
    }
    // Two shaped objects.
    let (sa2, ca2) = (sin(t2.angle).wrapping_neg(), cos(t2.angle));
    match inside(b1.points, x1, y1, ca1, sa1, x2, y2) {
        None | Some(true) => return true,
        _ => {}
    }
    match inside(b2.points, x2, y2, ca2, sa2, x1, y1) {
        None | Some(true) => return true,
        _ => {}
    }
    // Edge against edge.
    let (p1, p2) = (b1.points, b2.points);
    let (mut ax, mut ay) = corner(p1[p1.len() - 1], x1, y1, ca1, sa1);
    for &pa in p1 {
        let (bx, by) = corner(pa, x1, y1, ca1, sa1);
        let (dx1, dy1) = (bx.wrapping_sub(ax), by.wrapping_sub(ay));
        let r1 = if abs32(dx1) > abs32(dy1) { fp24_div(dy1, dx1) } else { fp24_div(dx1, dy1) };
        let (mut cx, mut cy) = corner(p2[p2.len() - 1], x2, y2, ca2, sa2);
        for &pb in p2 {
            let (ex, ey) = corner(pb, x2, y2, ca2, sa2);
            let (dx2, dy2) = (ex.wrapping_sub(cx), ey.wrapping_sub(cy));
            let ok = |r: i32, rr: i32| sgn(r) == sgn(rr) && abs32(r) <= abs32(rr);
            let one = 1 << 24;
            if abs32(dx2) > abs32(dy2) {
                let r2 = fp24_div(dy2, dx2);
                if abs32(dx1) > abs32(dy1) {
                    let rr = fp24_mult(dx1, r1.wrapping_sub(r2));
                    if ok(cy.wrapping_sub(ay).wrapping_sub(fp24_mult(r2, cx.wrapping_sub(ax))), rr)
                        && ok(cy.wrapping_sub(ay).wrapping_sub(fp24_mult(r1, cx.wrapping_sub(ax))), fp24_mult(dx2, r1.wrapping_sub(r2)))
                    {
                        return true;
                    }
                } else {
                    let rr = fp24_mult(dy1, fp24_mult(r1, r2).wrapping_sub(one));
                    if ok(ay.wrapping_sub(cy).wrapping_add(fp24_mult(r2, cx.wrapping_sub(ax))), rr)
                        && ok(
                            ax.wrapping_sub(cx).wrapping_add(fp24_mult(r1, cy.wrapping_sub(ay))),
                            fp24_mult(dx2.wrapping_neg(), fp24_mult(r1, r2).wrapping_sub(one)),
                        )
                    {
                        return true;
                    }
                }
            } else {
                let r2 = fp24_div(dx2, dy2);
                if abs32(dx1) > abs32(dy1) {
                    let rr = fp24_mult(dx1, fp24_mult(r1, r2).wrapping_sub(one));
                    if ok(ax.wrapping_sub(cx).wrapping_add(fp24_mult(r2, cy.wrapping_sub(ay))), rr)
                        && ok(
                            ay.wrapping_sub(cy).wrapping_add(fp24_mult(r1, cx.wrapping_sub(ax))),
                            fp24_mult(dy2.wrapping_neg(), fp24_mult(r1, r2).wrapping_sub(one)),
                        )
                    {
                        return true;
                    }
                } else {
                    let rr = fp24_mult(dy1, r1.wrapping_sub(r2));
                    if ok(cx.wrapping_sub(ax).wrapping_sub(fp24_mult(r2, cy.wrapping_sub(ay))), rr)
                        && ok(cx.wrapping_sub(ax).wrapping_sub(fp24_mult(r1, cy.wrapping_sub(ay))), fp24_mult(dy2, r1.wrapping_sub(r2)))
                    {
                        return true;
                    }
                }
            }
            cx = ex;
            cy = ey;
        }
        ax = bx;
        ay = by;
    }
    false
}
