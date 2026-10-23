//! Human-controlled cars: input sampling and driving physics
//! (game/userctl.c).

use super::map::to_sec;
use super::world::*;
use crate::fx::*;
use crate::pad::{self, Pad};

pub const MAX_USERCONTROLS: usize = 32;

pub const UCTL_LEFT: u8 = 0x01;
pub const UCTL_RIGHT: u8 = 0x02;
pub const UCTL_UP: u8 = 0x04;
pub const UCTL_DOWN: u8 = 0x08;
pub const UCTL_GEAR: u8 = 0x10;
pub const UCTL_GEARUP: u8 = 0x20;
pub const UCTL_GEARDN: u8 = 0x40;

pub const PLYF_ISCONSOLE: u16 = 0x0002;
pub const PLYF_SLIDING: u16 = 0x8000;
pub const PLYF_TRACTING: u16 = 0x4000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Racing,
    Broken,
    Won,
    Timeout,
}

/// One 70 Hz tick of input (UCT_TUserControl).
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct UserControl {
    pub scancode: u8,
    pub control: u8,
    /// Analog stick, -16..16 (the original's calibrated joystick range).
    pub analog_x: i8,
    pub analog_y: i8,
}

/// Input configuration (UCT_TInputConfig): key bindings plus the gamepad
/// (0 or 1) that also drives this player, if any.
#[derive(Clone, Copy, Default, Debug)]
pub struct InputConfig {
    pub pad: Option<usize>,
    pub kleft: u8,
    pub kright: u8,
    pub kup: u8,
    pub kdn: u8,
    pub kgear: u8,
}

/// UCT_GetUserControl: sample one tick of input. `pad` is the player's
/// gamepad and the buttons newly pressed on it since the last tick.
pub fn get_user_control(keys: &[bool; 256], last_scan: u8, cfg: &InputConfig, pad: Option<(&Pad, u32)>) -> UserControl {
    let mut c = UserControl { scancode: last_scan, ..Default::default() };
    let k = |sc: u8| keys[sc as usize];
    if k(cfg.kup) {
        c.control |= UCTL_UP;
    }
    if k(cfg.kdn) {
        c.control |= UCTL_DOWN;
    }
    if k(cfg.kleft) {
        c.control |= UCTL_LEFT;
    }
    if k(cfg.kright) {
        c.control |= UCTL_RIGHT;
    }
    if k(cfg.kgear) {
        c.control |= UCTL_GEAR;
    }
    // The original's joystick path filled analog_x/y; a pad steers through
    // analog_x but uses the digital controls for everything else.
    if let Some((p, pressed)) = pad {
        let bits = [
            (p.buttons & pad::LEFT != 0, UCTL_LEFT),
            (p.buttons & pad::RIGHT != 0, UCTL_RIGHT),
            (p.accelerate(), UCTL_UP),
            (p.brake(), UCTL_DOWN),
            (pressed & pad::RB != 0, UCTL_GEARUP),
            (pressed & pad::LB != 0, UCTL_GEARDN),
        ];
        for (on, bit) in bits {
            if on {
                c.control |= bit;
            }
        }
        if c.control & (UCTL_LEFT | UCTL_RIGHT) == 0 {
            c.analog_x = p.steer();
        }
    }
    c
}

pub struct Player {
    pub flags: u16,
    pub gear: u16,
    pub ma: u16,
    pub revo: i32,
    pub va: i32,
    pub v: i32,
    /// Input ring write position (ticks of input received).
    pub clock: u32,
    pub racer: usize,
    pub cfg: InputConfig,
    pub ctl: [UserControl; MAX_USERCONTROLS],
    pub status: Status,
    pub slidcounter: i32,
    pub slidspeed: i32,
    pub slidva: i32,
    pub movangle: i32,
    pub cartype: i32,
    pub maxspeed: i32,
    pub automatic: bool,
}

struct GearInfo {
    ratio: i32,
    accel: i32,
    minrevo: i32,
    maxrevo: i32,
}

const GEARS: [GearInfo; 8] = [
    GearInfo { ratio: -0x4000, accel: 0x8000, minrevo: 0, maxrevo: 1 << 28 },
    GearInfo { ratio: 0x0000, accel: 0x20000, minrevo: 0, maxrevo: 1 << 15 },
    GearInfo { ratio: 0x2000, accel: 0x10000, minrevo: 0, maxrevo: 0x0D << 18 },
    GearInfo { ratio: 0x4000, accel: 0x07000, minrevo: 4 << 18, maxrevo: 0x0D << 18 },
    GearInfo { ratio: 0x6000, accel: 0x04000, minrevo: 5 << 18, maxrevo: 0x0E << 18 },
    GearInfo { ratio: 0x8000, accel: 0x02800, minrevo: 5 << 18, maxrevo: 0x0D << 18 },
    GearInfo { ratio: 0xC000, accel: 0x01C00, minrevo: 5 << 18, maxrevo: 0x0D << 18 },
    GearInfo { ratio: 0x10000, accel: 0x01000, minrevo: 6 << 18, maxrevo: 0x0D << 18 },
];

/// UCT_New: set up a human player's thing.
pub fn new_player(w: &mut World, id: ThingId, flags: u16, spr: SpriteId) {
    let racer = w.racers.len();
    w.racers.push(Racer::new(id));
    if w.set.timed_race {
        w.racers[racer].laptimeleft = w.set.race_time[0];
    }
    let t = w.thing_mut(id);
    t.flags = THNF_SOLID;
    t.spr = Some(spr);
    t.bounds = Some(&CAR_BOUNDS);
    let a = t.angle;
    t.kind = Kind::Player(Box::new(Player {
        flags,
        gear: 1,
        ma: a,
        revo: 0,
        va: 0,
        v: 0,
        clock: 0,
        racer,
        cfg: InputConfig::default(),
        ctl: [UserControl::default(); MAX_USERCONTROLS],
        status: Status::Racing,
        slidcounter: 0,
        slidspeed: 0,
        slidva: 0,
        movangle: a as i32,
        cartype: 0,
        maxspeed: 0,
        automatic: false,
    }));
}

/// GearChange: sometimes throw sparks from the exhaust (F1 cars).
fn gear_change(w: &mut World, id: ThingId, p: &Player) {
    if w.rng.next() % 2 != 0 {
        return;
    }
    let t = w.thing(id);
    let x = t.x.wrapping_sub(fp_mult(1 << 23, cos(t.angle)) as u32);
    let y = t.y.wrapping_sub(fp_mult(1 << 23, sin(t.angle)) as u32);
    let a = t.angle;
    if p.cartype == 0 {
        let s = super::add_thing(w, x, y, 0, THNT_SPARKS, 0, 0);
        if let Some(Kind::Sparks(s)) = s.map(|s| &mut w.thing_mut(s).kind) {
            s.dx = fp_mult(p.v, cos(a)) / 4;
            s.dy = fp_mult(p.v, sin(a)) / 4;
        }
    }
}

/// Ground under a car: returns the smoke type (dirt/sand/grass) if off-road.
pub fn ground_kind(w: &World, x: u32, y: u32) -> Option<u32> {
    let c = w.floor.texel(x, y);
    if (160..192).contains(&c) {
        None
    } else if (48..63).contains(&c) {
        Some(1)
    } else if (64..=79).contains(&c) {
        Some(2)
    } else {
        Some(0)
    }
}

/// Off-road bumping and dust (shared by players and robots).
pub fn off_road_effects(w: &mut World, id: ThingId, v: i32, base: u32) {
    let (x, y, a) = {
        let t = w.thing(id);
        (t.x, t.y, t.angle)
    };
    if v > (1 << 3) {
        let z = 0x20000 + ((w.rng.next() % (1 << 5)) << 14);
        w.thing_mut(id).z = z;
        if w.rng.next() & 7 == 0 {
            // The shipped code draws y's offset first (0x41c29 / 0x4dcde).
            let dy = ((w.rng.next() & 0x7FFF) << 7).wrapping_sub(1 << 21);
            let dx = ((w.rng.next() & 0x7FFF) << 7).wrapping_sub(1 << 21);
            let s = super::add_thing(w, x.wrapping_add(dx), y.wrapping_add(dy), 0, THNT_SMOKE + base, 0, 0);
            if let Some(Kind::Smoke(s)) = s.map(|s| &mut w.thing_mut(s).kind) {
                s.dx = fp_mult(v, cos(a)) / 2;
                s.dy = fp_mult(v, sin(a)) / 2;
            }
        }
    } else {
        w.thing_mut(id).z = 0x20000;
    }
}

/// UCT_DoControl: one 70 Hz tick of a human car.
pub fn run(w: &mut World, id: ThingId, p: &mut Player, is_leader: impl Fn(&World, ThingId) -> bool) {
    if w.paused {
        return;
    }
    let console = p.flags & PLYF_ISCONSOLE != 0;
    let ri = p.racer;
    if w.race_started && p.status == Status::Racing {
        let r = &mut w.racers[ri];
        let nl = (r.nlap as usize).min(r.laptime.len() - 1);
        r.totaltime += 1;
        r.laptime[nl] += 1;
        if r.laptimeleft > 0 {
            r.laptimeleft -= 1;
            if r.laptimeleft == 0 {
                r.cltimeout = 5 * 70;
                r.finished = true;
                p.status = Status::Timeout;
                if console {
                    w.eff_start(4, 256, 400, 0, w.sfx.timeout);
                }
            }
        }
    }
    if w.race_started {
        let r = &mut w.racers[ri];
        r.cltimeout = (r.cltimeout - 1).max(0);
        r.clexttime = (r.clexttime - 1).max(0);
        r.clfinallap = (r.clfinallap - 1).max(0);
        r.clendrace = (r.clendrace - 1).max(0);
        if r.clendrace <= 0 {
            let _ = w.audio.lock().map(|mut a| a.eff_unlock(4));
        }
    }

    let k = (w.global_clock as usize) % MAX_USERCONTROLS;
    if p.status == Status::Broken {
        p.ctl[k] = UserControl::default();
    }
    let ctl = p.ctl[k];
    let cfg = p.cfg;
    let top_speed = w.car_models.last().map_or(1, |m| m.max_speed);

    // Speed changes.
    if ctl.control & UCTL_UP != 0 || ctl.analog_y > 5 {
        let g = &GEARS[p.gear as usize];
        if g.ratio >= 0 {
            if p.flags & PLYF_SLIDING == 0 {
                p.revo = p.revo.wrapping_add(fp_mult_div(g.accel, p.maxspeed, top_speed));
            } else if p.flags & PLYF_TRACTING != 0 {
                p.revo = p.revo.wrapping_add(g.accel / 2);
            }
        } else if w.race_started {
            p.revo -= g.accel;
            if p.revo <= 0 {
                p.gear += 1;
            }
        }
    }
    let mut braking = 0;
    p.v = fp_mult_div(p.v, top_speed, p.maxspeed);
    if ctl.control & UCTL_DOWN != 0 || ctl.analog_y < -5 {
        braking = 1;
        let g = &GEARS[p.gear as usize];
        if g.ratio > 0 {
            let k = (1 << 12) + (p.v >> 8);
            p.revo = fp16_div(p.v.wrapping_sub(k), g.ratio);
        } else if w.race_started && p.gear > 0 && g.ratio == 0 {
            p.gear -= 1;
            p.revo = p.revo.wrapping_add(GEARS[p.gear as usize].accel);
        } else if g.ratio < 0 {
            p.revo = p.revo.wrapping_add(g.accel);
        }
    }
    if p.flags & PLYF_SLIDING == 0 {
        p.revo -= GEARS[p.gear as usize].accel / 8;
    }
    if p.revo <= 0 {
        p.gear = 1;
        p.revo = 0;
    }
    // Gear changes only once the race is on.
    if w.race_started {
        let g = &GEARS[p.gear as usize];
        if (ctl.scancode == cfg.kgear && ctl.control & UCTL_UP != 0)
            || (ctl.scancode == cfg.kup && ctl.control & UCTL_GEAR != 0)
            || ctl.control & UCTL_GEARUP != 0
            || (p.automatic && p.revo > g.maxrevo)
        {
            if g.ratio >= 0 && (p.gear as usize) < GEARS.len() - 1 {
                p.gear += 1;
                if p.revo > (0x0D << 18) {
                    gear_change(w, id, p);
                }
                let r = GEARS[p.gear as usize].ratio;
                if r > 0 {
                    p.revo = fp16_div(p.v, r);
                }
            }
        }
        let g = &GEARS[p.gear as usize];
        if (ctl.scancode == cfg.kgear && ctl.control & UCTL_UP == 0) || ctl.control & UCTL_GEARDN != 0 || (p.automatic && p.revo < g.minrevo) {
            if p.gear > 2 && GEARS[p.gear as usize - 1].ratio != 0 {
                let r = fp16_div(p.v, GEARS[p.gear as usize - 1].ratio).min(1 << 22);
                p.revo = r;
                p.gear -= 1;
                if GEARS[p.gear as usize].ratio > 0 && p.revo > (0x0D << 18) {
                    gear_change(w, id, p);
                }
            }
        }
    }
    p.revo = p.revo.min(1 << 22);

    let (maxva, mut maxa) = {
        let s = (p.v << 8).max(0);
        let mut maxva = if p.cartype == 1 { 40 - fp_mult(55, s) } else { 40 - fp_mult(45, s) };
        maxva = maxva.max(7);
        if p.slidcounter > 0 {
            maxva = (maxva - fp_mult_div(maxva, p.slidcounter, 1 << 8)).max(0);
        }
        let maxa = (8 * (120 - fp_mult(160, s))).max(25 * 8);
        (maxva, maxa)
    };
    let mut a = maxva;
    let ax = ctl.analog_x as i32;
    if ctl.control & UCTL_LEFT != 0 || ax < -4 {
        if p.va < 0 {
            p.va = 0;
        }
    }
    if ctl.control & UCTL_RIGHT != 0 || ax > 4 {
        if p.va > 0 {
            p.va = 0;
        }
        a = -a;
    }
    if ctl.control & (UCTL_RIGHT | UCTL_LEFT) == 0 && abs32(ax) <= 4 {
        a = 0;
    }
    p.va += a;
    if abs32(ax) > 4 {
        maxa = maxa * -ax / 16;
    }
    // Note maxa can be negative with analog input; the clamps then apply
    // in the original's order.
    if p.va < -maxa {
        p.va = -maxa;
    }
    if p.va > maxa {
        p.va = maxa;
    }
    if ctl.control & UCTL_LEFT == 0 && ctl.control & UCTL_RIGHT == 0 && abs32(ax) <= 4 {
        p.va /= 2;
    }

    p.v = fp16_mult(p.revo, GEARS[p.gear as usize].ratio);
    p.v = fp_mult_div(p.v, p.maxspeed, top_speed);

    // Sliding / out of control.
    {
        let t_angle = w.thing(id).angle;
        if p.slidcounter > 0 {
            p.revo -= p.revo >> 6;
            p.slidcounter -= 1;
            w.thing_mut(id).angle = t_angle.wrapping_add(p.slidva as u16);
            if p.slidva > 2 {
                p.slidva -= 2;
            } else if p.slidva < -2 {
                p.slidva += 2;
            }
            p.slidspeed -= p.slidspeed >> 7;
        } else {
            let doslide = p.cartype == 1 && p.v > (3 << 19) && abs32((1 + 2 * braking) * p.va) >= abs32(maxa) - 8;
            if doslide {
                p.flags |= PLYF_SLIDING;
                p.ma = p.ma.wrapping_add(fp_mult_div(p.va, p.v, 1 << (22 + braking)) as u16);
                let na = t_angle.wrapping_add(fp_mult_div(p.va * (1 + braking) / 2, p.v, 1 << 22) as u16);
                w.thing_mut(id).angle = na;
                p.slidspeed -= fp_mult(p.slidspeed >> 9, cos(na.wrapping_sub(p.ma))) + (p.slidspeed >> 9);
                p.slidva = p.va;
            } else if p.flags & PLYF_SLIDING != 0 {
                p.flags |= PLYF_TRACTING;
                p.slidspeed += fp_mult((p.v - p.slidspeed) / 128, cos(t_angle.wrapping_sub(p.ma)));
                let k = (t_angle.wrapping_sub(p.ma) as i16 as i32) / 32;
                p.ma = p.ma.wrapping_add(k as u16);
                if abs32(k) < 0x400 / 64
                    && (p.slidspeed == 0 || (p.v - p.slidspeed) < (1 << 19) || (p.v << 4).wrapping_div(p.slidspeed) < (6 << 4) / 5)
                {
                    p.flags &= !(PLYF_SLIDING | PLYF_TRACTING);
                }
            } else {
                p.flags &= !(PLYF_SLIDING | PLYF_TRACTING);
                p.ma = t_angle;
                p.slidspeed = p.v;
                p.slidva = 0;
            }
        }
    }

    let dx = fp_mult(p.slidspeed, cos(p.ma));
    let dy = fp_mult(p.slidspeed, sin(p.ma));
    {
        let t = w.thing_mut(id);
        t.angle = t.angle.wrapping_add(fp_mult_div(p.va, p.v, 1 << 22) as u16);
        // Advance tires.
        t.tirerot = t.tirerot.wrapping_add((p.v / (32 * (1 << 4))) as u16);
        t.tiredir = if p.cartype == 0 {
            (-4 * p.va - fp_mult_div(16 * p.va, p.v, 1 << 22)) as u16
        } else {
            (-8 * p.va - fp_mult_div(32 * p.va, p.v, 1 << 22)) as u16
        };
    }
    let sec = w.thing(id).sec;

    if p.slidcounter <= 0 {
        p.movangle = if abs32(p.v) > (1 << 9) {
            get_angle(dx, -dy) as i32
        } else if p.v > (1 << 17) {
            get_angle(dx >> 5, -dy >> 5) as i32
        } else {
            w.thing(id).angle as i32
        };
    }

    {
        let t = w.thing(id);
        let (nx, ny) = (t.x.wrapping_add(dx as u32), t.y.wrapping_add(dy as u32));
        w.move_thing(id, nx, ny);
    }

    if w.sec.secs[sec].flags == 0 {
        // Somehow went off road.
        p.slidcounter = 0;
        p.slidspeed = 0;
        p.ma = w.thing(id).angle;
    } else {
        // Collide with walls: probe four corners.
        for (cx, cy) in [(3 << 21, 3 << 21), (-(3 << 21), 3 << 21), (3 << 21, -(3 << 21)), (-(3 << 21), -(3 << 21))] {
            let (tx, ty, tsec) = {
                let t = w.thing(id);
                (t.x, t.y, t.sec)
            };
            let vs = w.sec.find(Some(tsec), to_sec(tx.wrapping_add(cx as u32)), to_sec(ty.wrapping_add(cy as u32)));
            if sec == vs || w.sec.secs[vs].flags != 0 {
                continue;
            }
            if console {
                w.eff_start(3, 256, 120, 0, w.sfx.crash);
                w.rumble.push((id, 0.9, 0.6, 250));
            }
            super::add_thing(w, tx.wrapping_add((cx * 3 / 4) as u32), ty.wrapping_add((cy * 3 / 4) as u32), 0, THNT_SPARKS, 0, 0);
            let mut mini = -1i32;
            let mut mindist = 0x3FFFFFFF;
            for (i, side) in w.sec.secs[sec].sides.iter().enumerate() {
                if side.tex.is_none() {
                    continue;
                }
                let v0 = w.sec.verts[side.v0];
                let dist = fp_mult(cos(side.angle), to_sec(tx).wrapping_sub(v0.0) as i32)
                    .wrapping_add(fp_mult(sin(side.angle), to_sec(ty).wrapping_sub(v0.1) as i32));
                if abs32(dist) < mindist {
                    mindist = abs32(dist);
                    mini = i as i32;
                }
            }
            let ca: i32;
            if mini < 0 {
                ca = 0x1000;
                p.slidspeed = p.v >> 1;
                w.move_thing(id, tx.wrapping_sub(dx as u32).wrapping_sub((cx / 16) as u32), ty.wrapping_sub(dy as u32).wrapping_sub((cy / 16) as u32));
            } else {
                let wall = w.sec.secs[sec].sides[mini as usize].angle;
                ca = (2u32.wrapping_mul((wall as u32).wrapping_sub(p.ma as u32))) as i16 as i32;
                p.slidspeed = p.v >> 1;
                w.move_thing(id, tx.wrapping_sub(dx as u32).wrapping_sub((cx / 4) as u32), ty.wrapping_sub(dy as u32).wrapping_sub((cy / 4) as u32));
            }
            p.ma = p.ma.wrapping_add(ca as u16);
            p.slidcounter = 10 + fp_mult_div(abs32(p.v >> 14), abs32(ca), 0x10000);
            p.slidva = fp_mult_div(ca >> 4, p.slidspeed, 1 << 22);
            break;
        }

        // Then with other things.
        if !w.set.no_collide
            && let Some(other) = w.calc_collide(id)
        {
            if console {
                w.eff_start(3, 256, 120, 0, w.sfx.crash);
                w.rumble.push((id, 0.6, 0.5, 180));
            }
            let (ox, oy) = (w.thing(other).x, w.thing(other).y);
            let (tx, ty) = (w.thing(id).x, w.thing(id).y);
            super::add_thing(w, (tx / 2).wrapping_add(ox / 2), (ty / 2).wrapping_add(oy / 2), 0, THNT_SPARKS, 0, 0);
            push_away(w, id, other);
            // Second try for deep collisions; like the original, the bounce
            // angle below then uses whatever was hit last.
            let mut other = other;
            if let Some(o2) = w.calc_collide(id) {
                push_away(w, id, o2);
                other = o2;
            }
            let k = fp_mult_div(p.revo, 7, 8);
            if k != 0 {
                p.revo = k;
            }
            let (tx, ty, ta) = (w.thing(id).x, w.thing(id).y, w.thing(id).angle);
            let (ox, oy) = (w.thing(other).x, w.thing(other).y);
            let da = get_angle(tx.wrapping_sub(ox) as i32, oy.wrapping_sub(ty) as i32).wrapping_sub(ta) as i16;
            p.ma = p.ma.wrapping_add((da / 25) as u16);
        }
    }

    // Race progress: path points, laps, records.
    if p.status == Status::Racing {
        let r = &w.racers[ri];
        let pt = w.path[r.npoint as usize];
        let t = w.thing(id);
        let dx = ((((pt.x >> 1).wrapping_add(1 << 30)) >> 18) as i32).wrapping_sub((t.x >> 18) as i32);
        let dy = ((((pt.y >> 1).wrapping_add(1 << 30)) >> 18) as i32).wrapping_sub((t.y >> 18) as i32);
        if abs32(get_angle(dx, -dy).wrapping_sub(pt.dir as u16) as i16 as i32) > 0x4000 {
            let np = w.racers[ri].npoint as usize;
            w.path[np].ncars -= 1;
            w.racers[ri].npoint += 1;
            if w.racers[ri].npoint as usize >= w.path.len() {
                lap_done(w, id, p, console, &is_leader);
            }
            let np = w.racers[ri].npoint as usize;
            w.path[np].ncars += 1;
        }
    }

    // Ground detection.
    if w.thing(id).flags & THNF_SOLID != 0 {
        let (x, y) = (w.thing(id).x, w.thing(id).y);
        match ground_kind(w, x, y) {
            Some(base) => {
                p.revo -= p.revo >> 7;
                off_road_effects(w, id, p.v, base);
                if console && p.v > (1 << 3) {
                    w.rumble.push((id, 0.0, 0.3, 60));
                }
            }
            None => w.thing_mut(id).z = 0x20000,
        }
    }
}

/// Bounce off another thing: move away by 1/6 of the centre distance.
pub fn push_away(w: &mut World, id: ThingId, other: ThingId) {
    let (tx, ty) = (w.thing(id).x, w.thing(id).y);
    let (ox, oy) = (w.thing(other).x, w.thing(other).y);
    let nx = tx.wrapping_add((tx.wrapping_sub(ox) as i32 / 6) as u32);
    let ny = ty.wrapping_add((ty.wrapping_sub(oy) as i32 / 6) as u32);
    w.move_thing(id, nx, ny);
}

fn lap_done(w: &mut World, id: ThingId, p: &mut Player, console: bool, is_leader: &impl Fn(&World, ThingId) -> bool) {
    let ri = p.racer;
    let r = &mut w.racers[ri];
    let nl = (r.nlap as usize).min(r.laptime.len() - 1);
    let mut record = false;
    if r.bestlaptime == 0 || r.bestlaptime > r.laptime[nl] {
        record = true;
        r.bestlaptime = r.laptime[nl];
    }
    if nl >= r.laptime.len() - 1 {
        r.laptime.copy_within(1.., 0);
        r.laptime[nl] = 0;
    }
    if record && console {
        w.eff_start(4, 256, 400, 0, w.sfx.lap_record);
    }
    let r = &mut w.racers[ri];
    if p.status == Status::Racing && w.set.timed_race {
        let nl = ((r.nlap + 1) as usize).min(w.set.race_time.len() - 1);
        r.laptimeleft = (r.laptimeleft + w.set.race_time[nl]).min(w.set.race_time[0]);
        r.clexttime = 3 * 70;
        if console {
            w.eff_start(4, 256, 400, 0, w.sfx.extended);
        }
    }
    let r = &mut w.racers[ri];
    r.npoint = 0;
    if p.status == Status::Racing && (w.set.timed_race || r.nlap < w.set.race_laps) {
        r.nlap += 1;
        if r.nlap == w.set.race_laps - 1 {
            r.clfinallap = 3 * 70;
            r.clexttime = 0;
        }
        if r.nlap == w.set.race_laps {
            p.status = Status::Won;
            r.clexttime = 0;
            r.clendrace = 5 * 70;
            r.finished = true;
            r.nlap -= 1;
            if is_leader(w, id) && console {
                w.eff_start(4, 256, 500, 0, w.sfx.you_win);
                let _ = w.audio.lock().map(|mut a| a.eff_lock(4));
            }
        }
    }
}
