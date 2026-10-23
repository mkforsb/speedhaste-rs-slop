//! Computer-driven cars following the circuit path (game/cars.c).

use super::control::{ground_kind, off_road_effects, push_away, PLYF_ISCONSOLE};
use super::world::*;
use crate::fx::*;

pub struct Car {
    pub v: i32,
    pub av: i32,
    pub to_angle: u16,
    pub to_speed: i32,
    /// Skill: start grid position counted from the back.
    pub car_speed: i32,
    pub next_point: usize,
    pub tx: u32,
    pub ty: u32,
    pub racer: usize,
}

/// SetCarTarget: aim at the next path point, jittered if crowded.
fn set_target(w: &mut World, car: &mut Car) {
    let p = &mut w.path[car.next_point];
    car.tx = (p.x >> 1).wrapping_add(1 << 30);
    car.ty = (p.y >> 1).wrapping_add(1 << 30);
    p.ncars += 1;
    if !(p.speed > 8000 * 256 && p.ncars > 1) {
        return;
    }
    let s = fp_mult_div(p.speed - 8000 * 256, 1 << 24, 5000 * 256) as u32;
    if s == 0 {
        return;
    }
    car.tx = car.tx.wrapping_add((w.rng.next() << 16) % s).wrapping_sub(s / 2);
    car.ty = car.ty.wrapping_add((w.rng.next() << 16) % s).wrapping_sub(s / 2);
}

/// CAR_New.
pub fn new_car(w: &mut World, id: ThingId, spr: SpriteId) {
    let racer = w.racers.len();
    w.racers.push(Racer::new(id));
    let t = w.thing_mut(id);
    t.flags = THNF_SOLID;
    t.spr = Some(spr);
    t.bounds = Some(&CAR_BOUNDS);
    let mut car = Car { v: 0, av: 0, to_angle: t.angle, to_speed: 0, car_speed: 0, next_point: 0, tx: 0, ty: 0, racer };
    set_target(w, &mut car);
    w.thing_mut(id).kind = Kind::Car(Box::new(car));
}

fn gear_change(w: &mut World, id: ThingId, v: i32) {
    if w.rng.next() % 2048 != 0 {
        return;
    }
    let t = w.thing(id);
    let a = t.angle;
    let x = t.x.wrapping_sub(fp_mult(1 << 23, cos(a)) as u32);
    let y = t.y.wrapping_sub(fp_mult(1 << 23, sin(a)) as u32);
    if w.set.car_type == 0
        && let Some(s) = super::add_thing(w, x, y, 0, THNT_SPARKS, 0, 0)
        && let Kind::Sparks(s) = &mut w.thing_mut(s).kind
    {
        s.dx = fp_mult(v, cos(a)) / 4;
        s.dy = fp_mult(v, sin(a)) / 4;
    }
}

/// CAR_RunIt: one 70 Hz tick of a robot car.
pub fn run(w: &mut World, id: ThingId, car: &mut Car) {
    if w.paused || !w.race_started {
        return;
    }
    {
        let t = w.thing_mut(id);
        t.angle = t.angle.wrapping_add(fp_mult_div(car.av, car.v, 1 << 22) as u16);
        if t.angle >> 6 == car.to_angle >> 6 {
            t.angle = car.to_angle;
            car.av = 0;
        }
    }
    let (tx, ty, ta) = (w.thing(id).x, w.thing(id).y, w.thing(id).angle);
    w.move_thing(id, tx.wrapping_add(fp_mult(car.v, cos(ta)) as u32), ty.wrapping_add(fp_mult(car.v, sin(ta)) as u32));

    // Race progress and steering along the path.
    let (tx, ty) = (w.thing(id).x, w.thing(id).y);
    let ri = car.racer;
    let pt = w.path[w.racers[ri].npoint as usize];
    let dx = ((((pt.x >> 1).wrapping_add(1 << 30)) >> 18) as i32).wrapping_sub((tx >> 18) as i32);
    let dy = ((((pt.y >> 1).wrapping_add(1 << 30)) >> 18) as i32).wrapping_sub((ty >> 18) as i32);
    if abs32(get_angle(dx, -dy).wrapping_sub(pt.dir as u16) as i16 as i32) > 0x4000 {
        let r = &mut w.racers[ri];
        r.npoint += 1;
        if r.npoint as usize >= w.path.len() {
            r.npoint = 0;
            r.nlap += 1;
        }
    }
    let mut dx = ((car.tx >> 18) as i32).wrapping_sub((tx >> 18) as i32);
    let mut dy = ((car.ty >> 18) as i32).wrapping_sub((ty >> 18) as i32);
    let d2 = (dx.wrapping_mul(dx)).wrapping_add(dy.wrapping_mul(dy)) as u32;
    let ta = w.thing(id).angle;
    if d2 < (1 << 15) || (d2 < (1 << 19) && abs32(get_angle(dx, -dy).wrapping_sub(ta) as i16 as i32) > 0x4000) {
        w.path[car.next_point].ncars -= 1;
        car.next_point += 1;
        if car.next_point >= w.path.len() {
            car.next_point = 0;
        }
        set_target(w, car);
        dx = ((car.tx >> 18) as i32).wrapping_sub((tx >> 18) as i32);
        dy = ((car.ty >> 18) as i32).wrapping_sub((ty >> 18) as i32);
    }
    car.to_angle = get_angle(dx, -dy);
    let d = (car.to_angle.wrapping_sub(ta) as i32).wrapping_add(car.av) as i16;
    {
        let s = -sin((car.v >> (22 - 14)) as u16);
        let maxva = 40 - fp_mult(30, s);
        let maxa = 120 - fp_mult(105, s);
        let a = maxva;
        if d < 0 {
            car.av -= if car.av > 0 { 2 * a } else { a };
        } else if d > 0 {
            car.av += if car.av < 0 { 2 * a } else { a };
        }
        if car.av < -maxa * 8 {
            car.av = -maxa * 8;
        }
        if car.av > maxa * 8 {
            car.av = maxa * 8;
        }
    }
    {
        let t = w.thing_mut(id);
        t.tirerot = t.tirerot.wrapping_add(abs32(car.v / (32 * (1 << 4))) as u16);
        let tdir = if w.set.car_type == 0 {
            -4 * car.av - fp_mult_div(16 * car.av, car.v, 1 << 22)
        } else {
            -8 * car.av - fp_mult_div(32 * car.av, car.v, 1 << 22)
        };
        let t = w.thing_mut(id);
        t.tiredir = t.tiredir.wrapping_add(((tdir.wrapping_sub(t.tiredir as i32)) as i16 / 32) as u16);
    }
    car.to_speed = w.path[car.next_point].speed - 1800 * 256;
    if car.to_speed < car.v {
        car.v -= if w.set.car_type == 1 { 0x8000 * 3 / 4 } else { 0x8000 };
        gear_change(w, id, car.v);
    } else if car.to_speed > car.v {
        // Sic: `(type == 1) ? 0x1000 : 0x1100 - k` in the original.
        let acc = if w.set.car_type == 1 { 0x1000 } else { 0x1100 - fp_mult_div(0xC00, car.v, 1 << 22) };
        car.v += acc * (car.car_speed + 3) / 16;
        gear_change(w, id, car.v);
    }

    // Bump into other cars.
    if let Some(other) = w.calc_collide(id) {
        if let Kind::Player(p) = &w.thing(other).kind
            && p.flags & PLYF_ISCONSOLE != 0
        {
            w.eff_start(3, 256, 120, 0, w.sfx.crash);
            w.rumble.push((other, 0.6, 0.5, 180));
        }
        let (tx, ty) = (w.thing(id).x, w.thing(id).y);
        let (ox, oy) = (w.thing(other).x, w.thing(other).y);
        super::add_thing(w, (tx / 2).wrapping_add(ox / 2), (ty / 2).wrapping_add(oy / 2), 0, THNT_SPARKS, 0, 0);
        push_away(w, id, other);
        let mut other = other;
        if let Some(o2) = w.calc_collide(id) {
            push_away(w, id, o2);
            other = o2;
        }
        car.v = fp_mult_div(car.v, 7, 8);
        let t = w.thing(id);
        let o = w.thing(other);
        let da = get_angle(t.x.wrapping_sub(o.x) as i32, o.y.wrapping_sub(t.y) as i32).wrapping_sub(t.angle) as i16;
        let t = w.thing_mut(id);
        t.angle = t.angle.wrapping_add((da / 25) as u16);
    }

    // Ground detection.
    let (x, y) = (w.thing(id).x, w.thing(id).y);
    match ground_kind(w, x, y) {
        Some(base) => {
            car.v -= car.v >> 7;
            off_road_effects(w, id, car.v, base);
        }
        None => {
            let z = if car.v > (1 << 7) { 0x20000 - ((w.rng.next() % (car.v >> 7) as u32) << 4) } else { 0x20000 };
            w.thing_mut(id).z = z;
        }
    }
}
