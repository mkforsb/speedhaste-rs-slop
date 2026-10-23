//! In-race heads-up display (game/hud.c).

use super::control::{Player, Status};
use super::render::Geometry;
use super::world::World;
use crate::data::{Data, Result};
use crate::fx::*;
use crate::gfx::Screen;
use crate::is2::Sprite;
use crate::poly::{Fill, Poly, Vertex};

pub struct Hud {
    gears: Vec<Sprite>,
    gear: Sprite,
    revo: Sprite,
    time: Sprite,
    laps: Sprite,
    pos: Sprite,
    big_gold: Vec<Sprite>,
    big_gold_b: Sprite,
    med_gold: Vec<Sprite>,
    big_white: Vec<Sprite>,
    med_white: Vec<Sprite>,
    /// Digits plus ' and " for lap times.
    little_white: Vec<Sprite>,
    little_gold: Vec<Sprite>,
    lap: Sprite,
    best: Sprite,
    pos_bar: Sprite,
    pause: Sprite,
    ext_time: Sprite,
    fin_lap: Sprite,
    timeout: Sprite,
    end_race: Sprite,
    you_win: Sprite,
    /// Cockpit overlay for the in-car camera (SALPn.PIX), 320 wide.
    salp: Vec<u8>,
    poly: Poly,
}

/// What the HUD needs to know about a player.
pub struct PlayerView<'a> {
    pub p: &'a Player,
    pub pos: i32,
    pub total: i32,
}

impl Hud {
    /// HUD_Init.
    pub fn load(d: &Data, car_type: i32) -> Result<Hud> {
        let l = |n: &str| Sprite::load(d, n);
        let font = |n: &str| (0..10).map(|i| Sprite::load(d, &format!("{n}{i}.is2"))).collect::<Result<Vec<_>>>();
        let mut little_white = font("mflw")?;
        little_white.push(l("mwquote.is2")?);
        little_white.push(l("mwdquote.is2")?);
        let mut little_gold = font("mflg")?;
        little_gold.push(l("mgquote.is2")?);
        little_gold.push(l("mgdquote.is2")?);
        Ok(Hud {
            gears: (0..7).map(|i| l(&format!("mg{i}.is2"))).collect::<Result<_>>()?,
            gear: l("mgear.is2")?,
            revo: l(&format!("mrevo{car_type}.is2"))?,
            time: l("mtime.is2")?,
            laps: l("mlaps.is2")?,
            pos: l("mpos.is2")?,
            big_gold: font("mfbg")?,
            big_gold_b: l("mfbgb.is2")?,
            med_gold: font("mfmg")?,
            big_white: font("mfbw")?,
            med_white: font("mfmw")?,
            little_white,
            little_gold,
            lap: l("mlap.is2")?,
            best: l("mbest.is2")?,
            pos_bar: l("mposbar.is2")?,
            pause: l("pause.is2")?,
            ext_time: l("rexttime.is2")?,
            fin_lap: l("rfinlap.is2")?,
            timeout: l("rtimeout.is2")?,
            end_race: l("endrace.is2")?,
            you_win: l("youwin.is2")?,
            salp: d.get(&format!("salp{car_type}.pix")).map(<[u8]>::to_vec).unwrap_or_default(),
            poly: Poly::default(),
        })
    }

    /// DrawNum: mode 0 = left aligned at x, 1 = centred, 2 = right aligned.
    fn draw_num(s: &mut Screen, n: u32, font: &[Sprite], mut x: i32, y: i32, mode: i32) -> i32 {
        let digits: Vec<usize> = n.to_string().bytes().map(|c| (c - b'0') as usize).collect();
        if mode != 0 {
            let w: i32 = digits.iter().map(|&d| font[d].w + 1).sum::<i32>() - 1;
            if mode == 1 {
                x -= w / 2;
            } else if mode == 2 {
                x -= w;
            }
        }
        for d in digits {
            font[d].draw_horizontal(s, x, y);
            x += font[d].w + 1;
        }
        x - 1
    }

    /// DrawTime: mm'ss"hh from 70 Hz ticks.
    fn draw_time(s: &mut Screen, n: u32, font: &[Sprite], mut x: i32, y: i32) {
        let txt = format!("{:02}'{:02}\"{:02}", n / 70 / 60, (n / 70) % 60, (n % 70) * 100 / 70);
        for c in txt.bytes() {
            let sp = match c {
                b'0'..=b'9' => Some(&font[(c - b'0') as usize]),
                b'\'' => Some(&font[10]),
                b'"' => Some(&font[11]),
                _ => None,
            };
            match sp {
                Some(sp) => {
                    sp.draw_horizontal(s, x, y);
                    x += sp.w;
                }
                None => x += 4,
            }
        }
    }

    /// The rev counter needle: a thin quad pointing at angle `l`.
    fn needle(&mut self, s: &mut Screen, clr: &[u8], cx: i32, cy: i32, r: i32, l: i32, color: u8) {
        let mut v = [Vertex::default(); 4];
        for (i, (vv, rr)) in v.iter_mut().zip([7 * r, r, r, r]).enumerate() {
            let a = (l - i as i32 * 65536 / 4) as u16;
            vv.x = cx + fp_mult(rr, cos(a));
            vv.y = cy + fp_mult(rr, sin(a));
        }
        v[0].l = 0xF0000;
        let clip = s.clip;
        self.poly.draw(s, clip, &v, Fill::Solid(color), clr);
    }

    /// HUD_DrawSalp: the in-car cockpit overlay, bottom aligned at `cy`.
    pub fn draw_salp(&self, s: &mut Screen, g: &Geometry, cy: i32) {
        if self.salp.is_empty() {
            return;
        }
        let rows = self.salp.len() as i32 / 320;
        let mut cy = cy - rows * if g.w == 320 { 1 } else { 2 };
        for row in self.salp.chunks_exact(320) {
            if cy >= s.clip.max_y {
                break;
            }
            for dy in 0..(if g.w == 320 { 1 } else { 2 }) {
                if (0..s.h as i32).contains(&cy) {
                    let base = cy as usize * s.w;
                    for (i, &c) in row.iter().enumerate() {
                        if c != 0 {
                            if g.w == 320 {
                                s.pix[base + i] = c;
                            } else {
                                s.pix[base + 2 * i] = c;
                                s.pix[base + 2 * i + 1] = c;
                            }
                        }
                    }
                }
                if dy == 0 && g.w != 320 {
                    cy += 1;
                }
            }
            cy += 1;
        }
    }

    fn revo_of(p: &Player) -> i32 {
        fp_mult_div(p.revo, 13000, 1 << 22)
    }

    fn speed_of(p: &Player) -> i32 {
        abs32(fp_mult_div(p.v, p.maxspeed, 1 << 22))
    }

    /// HUD_DrawPlayer: full-screen HUD for one player.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_player(&mut self, s: &mut Screen, w: &World, clr: &[u8], g: &Geometry, pv: &PlayerView, mode: i32, salp: bool) {
        let p = pv.p;
        let r = &w.racers[p.racer];
        let laps = (r.nlap + 1) as u32;
        let time = (r.laptimeleft / 70).max(0) as u32;
        let gear = p.gear.saturating_sub(1).min(6) as usize;
        let revo = Self::revo_of(p);
        let speed = Self::speed_of(p);
        let car_type = w.set.car_type;
        if salp {
            let l = 0xC000 - fp_mult_div(revo, 0xC000, 13000);
            let (mut cx, mut cy) = match car_type {
                0 => (182, 160),
                1 => (119, 175),
                _ => (98, 175),
            };
            let mut rr = 3;
            if g.w != 320 {
                cx = cx * 2 + g.min_x;
                cy = cy * 2 + g.min_y;
                rr = 6;
            }
            self.needle(s, clr, cx, cy, rr, l, if revo < 10000 { 14 } else { 4 });
            if car_type != 2 {
                let l = 0xC000 - fp_mult_div(speed, 0xC000, p.maxspeed);
                let (mut cx, mut cy) = if car_type == 0 { (132, 160) } else { (70, 175) };
                let mut rr = 3;
                if g.w != 320 {
                    cx = cx * 2 + g.min_x;
                    cy = cy * 2 + g.min_y;
                    rr = 6;
                }
                self.needle(s, clr, cx, cy, rr, l, 7);
            }
        }
        self.gear.draw_horizontal(s, g.max_x - 22, g.max_y - 77);
        self.gears[gear].draw_horizontal(s, g.max_x - 21, g.max_y - 77 + 12 + 8 * gear as i32);
        if !salp {
            self.revo.draw_horizontal(s, g.max_x - 92, g.max_y - 62);
            let yy = if car_type != 1 { 197 } else { 191 };
            Self::draw_num(s, speed as u32, &self.med_white, g.max_x - 320 + 263, g.max_y - 200 + yy - self.med_white[0].h, 0);
        }
        if mode < 2 {
            self.pos.draw_horizontal(s, 8, g.max_y - 200 + 158);
            let i = Self::draw_num(s, pv.pos as u32, &self.big_white, 3, g.max_y - 200 + 197 - self.big_white[0].h, 0);
            self.pos_bar.draw_horizontal(s, i + 1, g.max_y - 200 + 197 - self.pos_bar.h);
            let i = i + self.pos_bar.w + 1;
            Self::draw_num(s, pv.total as u32, &self.med_white, i, g.max_y - 200 + 197 - self.med_white[0].h, 0);
        }
        let bg0h = self.big_gold[0].h;
        if !w.set.timed_race {
            self.laps.draw_horizontal(s, g.cx - self.laps.w / 2, g.min_y + 8);
            if mode < 2 {
                self.big_gold_b.draw_horizontal(s, g.cx - self.big_gold_b.w / 2, g.min_y + 20 + bg0h - self.big_gold_b.h);
                Self::draw_num(s, laps, &self.big_gold, g.cx - self.big_gold_b.w / 2, g.min_y + 20, 2);
                Self::draw_num(s, w.set.race_laps as u32, &self.med_gold, g.cx + self.big_gold_b.w / 2, g.min_y + 20 + bg0h - self.med_gold[0].h, 0);
            } else {
                Self::draw_num(s, laps, &self.big_gold, g.cx, g.min_y + 20, 1);
            }
        } else {
            self.time.draw_horizontal(s, g.cx - self.time.w / 2, g.min_y + 8);
            Self::draw_num(s, time, &self.big_gold, g.cx, g.min_y + 20, 1);
            self.laps.draw_horizontal(s, g.cx - self.time.w / 2 - 30 - self.laps.w, g.min_y + 8);
            Self::draw_num(s, laps, &self.big_gold, g.cx - self.laps.w - 30, g.min_y + 20, 1);
        }
        let ident: Vec<u8> = (0..=255).collect();
        if r.bestlaptime != 0 {
            self.best.draw(s, 4, g.min_y + 65, self.best.w, self.best.h, &ident);
            if p.status == Status::Racing || p.clock & 0x40 != 0 {
                Self::draw_time(s, r.bestlaptime, &self.little_white, 10, g.min_y + 75);
            }
        }
        if p.status == Status::Racing {
            self.lap.draw(s, 4, g.min_y + 85, self.lap.w, self.lap.h, &ident);
            let nl = (r.nlap as usize).min(r.laptime.len() - 1);
            for (i, k) in (0..=nl).rev().enumerate() {
                Self::draw_time(s, r.laptime[k], &self.little_gold, 10, g.min_y + 95 + i as i32 * 10);
            }
        }
        if !salp {
            let l = 0xC000 - fp_mult_div(revo, 0xC000, 13000);
            let (cx, cy) = if g.w == 320 { (258, 165) } else { (g.max_x - (320 - 258), g.max_y - (200 - 165)) };
            self.needle(s, clr, cx, cy, 3, l, if revo < 10000 { 14 } else { 4 });
        }
    }

    /// HUD_DrawPlayerLogos (cy is the view centre, h its height).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_logos(&self, s: &mut Screen, w: &World, p: &Player, paused: bool, is_first: bool, cx: i32, pause_y: i32, msg_y: i32) {
        let ident: Vec<u8> = (0..=255).collect();
        let r = &w.racers[p.racer];
        let at = |s: &mut Screen, sp: &Sprite, y: i32| sp.draw(s, cx, y, sp.w, sp.h, &ident);
        if paused {
            at(s, &self.pause, pause_y);
        }
        if r.clexttime & 0x10 != 0 {
            at(s, &self.ext_time, msg_y);
        }
        if r.clfinallap & 0x10 != 0 {
            at(s, &self.fin_lap, msg_y);
        }
        if p.status == Status::Won && p.clock & 0x20 != 0 {
            at(s, if is_first { &self.you_win } else { &self.end_race }, msg_y);
        }
        if p.status == Status::Timeout && p.clock & 0x20 != 0 {
            at(s, &self.timeout, msg_y);
        }
    }

    /// HUD_Draw2Player: split-screen HUD for the half starting at `y`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_2player(&mut self, s: &mut Screen, w: &World, clr: &[u8], g: &Geometry, pv: &PlayerView, mode: i32, y: i32) {
        let p = pv.p;
        let r = &w.racers[p.racer];
        let laps = (r.nlap + 1) as u32;
        let time = (r.laptimeleft / 70).max(0) as u32;
        let gear = p.gear.saturating_sub(1).min(6) as usize;
        let revo = Self::revo_of(p);
        let speed = Self::speed_of(p);
        let hh = g.h / 2;
        self.gear.draw_horizontal(s, g.max_x - 22, y + hh - 77);
        self.gears[gear].draw_horizontal(s, g.max_x - 21, y + hh - 77 + 12 + 8 * gear as i32);
        self.revo.draw_horizontal(s, g.max_x - 92, y + hh - 62);
        let yy = if w.set.car_type != 1 { 197 } else { 191 };
        Self::draw_num(s, speed as u32, &self.med_white, g.max_x - 320 + 263, y + hh - 200 + yy - self.med_white[0].h, 0);
        if mode < 2 {
            self.pos.draw_horizontal(s, 8, y + hh - 200 + 158);
            let i = Self::draw_num(s, pv.pos as u32, &self.big_white, 3, y + hh - 200 + 197 - self.big_white[0].h, 0);
            self.pos_bar.draw_horizontal(s, i + 1, y + hh - 200 + 197 - self.pos_bar.h);
            let i = i + self.pos_bar.w + 1;
            Self::draw_num(s, pv.total as u32, &self.med_white, i, y + hh - 200 + 197 - self.med_white[0].h, 0);
        }
        let bg0h = self.big_gold[0].h;
        if !w.set.timed_race {
            self.laps.draw_horizontal(s, g.cx - self.laps.w / 2, y + 1);
            if mode < 2 {
                self.big_gold_b.draw_horizontal(s, g.cx - self.big_gold_b.w / 2, y + 10 + bg0h - self.big_gold_b.h);
                Self::draw_num(s, laps, &self.big_gold, g.cx - self.big_gold_b.w / 2, y + 10, 2);
                Self::draw_num(s, w.set.race_laps as u32, &self.med_gold, g.cx + self.big_gold_b.w / 2, y + 10 + bg0h - self.med_gold[0].h, 0);
            } else {
                Self::draw_num(s, laps, &self.big_gold, g.cx, y + 10, 1);
            }
        } else {
            self.time.draw_horizontal(s, g.cx - self.time.w / 2, y);
            Self::draw_num(s, time, &self.big_gold, g.cx, y + 10, 1);
            self.laps.draw_horizontal(s, g.cx - self.time.w / 2 - 30 - self.laps.w, y);
            Self::draw_num(s, laps, &self.big_gold, g.cx - self.laps.w - 30, y + 10, 1);
        }
        let ident: Vec<u8> = (0..=255).collect();
        if r.bestlaptime != 0 {
            self.best.draw(s, 4, y + 5, self.best.w, self.best.h, &ident);
            if p.status == Status::Racing || p.clock & 0x40 != 0 {
                Self::draw_time(s, r.bestlaptime, &self.little_white, 10, y + 15);
            }
        }
        if p.status == Status::Racing {
            self.lap.draw(s, 4, y + 25, self.lap.w, self.lap.h, &ident);
            let nl = (r.nlap as usize).min(r.laptime.len() - 1);
            Self::draw_time(s, r.laptime[nl], &self.little_gold, 10, y + 35);
        }
        let l = 0xC000 - fp_mult_div(revo, 0xC000, 13000);
        let (cx, cy) = if g.w == 320 { (258, y + 67) } else { (g.max_x - (320 - 258), y + hh - (100 - 67)) };
        self.needle(s, clr, cx, cy, 3, l, if revo < 10000 { 14 } else { 4 });
    }
}
