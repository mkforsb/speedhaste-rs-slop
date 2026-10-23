//! Intro logos, the fire title effect (game/intro.c, fire.asm) and the
//! waving-flag credits (game/credits.c).

use crate::fx::*;
use crate::gfx::{Font, Palette};
use crate::is2::{Sprite, TRANSPARENT};
use crate::keys;
use crate::sys::Sys;

/// FadeIn/FadeOut of intro.c: fade with `speed` steps per tick, abortable.
async fn fade_to(sys: &mut Sys, target: Palette, speed: i32) {
    sys.start_fade(Some(target.clone()), speed);
    sys.wait_fade(true).await;
    sys.fade.pos = 0;
    sys.pal = target;
}

/// Wait(n): up to `n` ticks or a key. Returns true on ESC.
async fn wait(sys: &mut Sys, n: u32) -> bool {
    let start = sys.clock();
    while sys.clock().wrapping_sub(start) < n && sys.last_scan == 0 {
        sys.frame().await;
    }
    let esc = sys.last_scan == keys::ESC;
    sys.last_scan = 0;
    esc
}

/// Intro1: a full-screen picture faded in.
async fn picture(sys: &mut Sys, name: &str) {
    let pal = Palette::load(&sys.data, &format!("{name}.pal")).unwrap_or_default();
    sys.screen.clear(0);
    if let Some(p) = sys.data.get(&format!("{name}.pix")) {
        let p = p.to_vec();
        sys.screen.blit_pix(&p);
    }
    sys.update();
    fade_to(sys, pal, 3).await;
}

/// INTRO_DoIntro as shipped (checked with REA): friend, noria, fire.
pub async fn intro(sys: &mut Sys) {
    sys.last_scan = 0;
    sys.pal = Palette::default();
    'bye: {
        picture(sys, "friend").await;
        if wait(sys, 200).await {
            break 'bye;
        }
        fade_to(sys, Palette::default(), 6).await;
        picture(sys, "noria").await;
        if wait(sys, 200).await {
            break 'bye;
        }
        fade_to(sys, Palette::default(), 6).await;
        if fire(sys).await {
            break 'bye;
        }
        fade_to(sys, Palette::default(), 6).await;
    }
    sys.pal = Palette::default();
    sys.screen.clear(0);
    sys.update();
    sys.last_scan = 0;
}

const FIREW: usize = 160;
const FIREH: usize = 115;
const MAXC: u32 = 255 - 16 - 1;

/// FIRE_Move: each cell is the average of 8 neighbours two rows below,
/// minus `dec`.
fn fire_move(dst: &mut [u32], src: &[u32], dec: u32) {
    let w = FIREW;
    for i in 0..w * (FIREH - 3) {
        let s = |k: usize| src[k];
        let sum = s(i + w - 1) + s(i + w) + s(i + w + 1) + s(i + 3 * w - 1) + s(i + 3 * w) + s(i + 3 * w + 1) + s(i + 2 * w + 1) + s(i + 2 * w - 1);
        let v = sum >> 3;
        dst[i] = v.saturating_sub(dec);
    }
}

/// DumpIs2: draw the first `n` columns of a title sprite; the last one
/// sets the fire alight along its opaque runs.
fn dump_is2(sys: &mut Sys, fire: &mut [u32], sp: &Sprite, x0: i32, y0: i32, n: i32) {
    for i in 0..n.min(sp.w) {
        let x = x0 + i;
        let col = &sp.pix[(i * sp.h) as usize..((i + 1) * sp.h) as usize];
        let mut j = 0;
        while j < sp.h {
            if col[j as usize] == TRANSPARENT {
                j += 1;
                continue;
            }
            let start = j;
            while j < sp.h && col[j as usize] != TRANSPARENT {
                sys.screen.put(x, y0 + j, col[j as usize] as u8);
                j += 1;
            }
            if i == n - 1 {
                let (dy, c) = (y0 + start, j - start);
                for row in dy / 2..(dy + c) / 2 {
                    let v = (MAXC - (sys.rng.next() & 0x3F)) * 256;
                    if let Some(cell) = fire.get_mut(row as usize * FIREW + (x / 2) as usize) {
                        *cell = v;
                    }
                }
            }
        }
    }
}

/// DoFire: the burning title. Returns true on ESC.
async fn fire(sys: &mut Sys) -> bool {
    let (Ok(t1), Ok(t2)) = (Sprite::load(&sys.data, "firet1.is2"), Sprite::load(&sys.data, "firet2.is2")) else { return false };
    // FIRE_Move reads one cell past the end; pad like the heap did.
    let mut b1 = vec![0u32; FIREW * FIREH + 2];
    let mut b2 = vec![0u32; FIREW * FIREH + 2];
    // Blue-black, red, orange to yellow, yellow to white, white, then greys.
    let mut cols: Vec<[u8; 3]> = Vec::with_capacity(256);
    cols.extend((0..10).map(|j| [0, 0, 2 * j]));
    cols.extend((0..8).map(|j| [j * 8, 0, 0]));
    cols.extend((0..64).step_by(2).map(|j| [63, j, 0]));
    cols.extend((0..64).map(|j| [63, 63, j]));
    while cols.len() <= MAXC as usize {
        cols.push([63, 63, 63]);
    }
    cols.extend((0..16).map(|j| [j * 4 + 3, j * 4 + 3, j * 4 + 3]));
    let mut pal = Palette::default();
    for (i, c) in cols.iter().take(256).enumerate() {
        pal.0[i * 3..i * 3 + 3].copy_from_slice(c);
    }
    sys.last_scan = 0;
    sys.pal = Palette::default();
    sys.screen.clear(0);
    sys.vsync(0).await;
    sys.vsync(2).await;
    sys.start_fade(Some(pal), 1);
    let mut dec = 256 * MAXC;
    let (mut f, mut w2, mut nf, mut nflames) = (1, 0, 0, 200);
    while sys.last_scan == 0 {
        for _ in 0..f.min(3) {
            std::mem::swap(&mut b1, &mut b2);
            if !sys.keys[keys::SPACE as usize] {
                for _ in 0..nflames {
                    let s = (sys.rng.next() & 255) as usize;
                    if s < FIREW {
                        b1[FIREW * (FIREH - 4) + s] = MAXC * 256;
                    }
                }
            }
            fire_move(&mut b2, &b1, dec);
        }
        // FIRE_Dump: byte 1 of each cell, 2x2 pixels.
        for y in 0..100 {
            for x in 0..FIREW {
                let c = (b2[y * FIREW + x] >> 8) as u8;
                let o = y * 2 * 320 + x * 2;
                sys.screen.pix[o] = c;
                sys.screen.pix[o + 1] = c;
                sys.screen.pix[o + 320] = c;
                sys.screen.pix[o + 321] = c;
            }
        }
        dump_is2(sys, &mut b2, &t1, 160 - t1.w / 2, 40, t1.w + 1);
        if nf > 70 * 9 / 3 {
            break;
        } else if nflames > 0 && nf > 70 * 7 / 3 {
            nflames -= 10;
        } else if dec > 380 {
            dec -= dec / 16;
        }
        if nf > 70 / 3 {
            dump_is2(sys, &mut b2, &t2, 160 - t2.w / 2, 80, w2);
            if w2 <= t2.w {
                w2 += 3;
            }
        }
        sys.update();
        f = sys.vsync(3).await / 3;
        nf += f;
    }
    sys.pal = Palette::default();
    sys.fade.pos = 0;
    sys.vsync(0).await;
    sys.vsync(2).await;
    sys.last_scan == keys::ESC
}

// ---------------------------------------------------------------------------

const CREDITS: &[(usize, i32, Option<&str>)] = &[
    (0, 40, None), (0, 60, Some("Game Design")), (1, 120, Some("Javier Arevalo")), (1, 140, Some("Noriaworks")),
    (0, 40, None), (0, 40, None), (0, 60, Some("Programming")), (1, 130, Some("Javier Arevalo")),
    (0, 40, None), (0, 40, Some("Additional programming")), (1, 80, Some("Juan Carlos Arevalo")), (1, 100, Some("Alejandro Luengo")),
    (0, 150, Some("VTAL Sound System")), (1, 170, Some("Juan Carlos Arevalo")),
    (0, 40, None), (0, 60, Some("Graphics")), (1, 120, Some("Cesar Valencia")), (1, 140, Some("Josete")),
    (0, 40, None), (0, 50, Some("Modelling")), (1, 115, Some("Jorge Rosado")), (1, 135, Some("Josete")), (1, 155, Some("Rafael Sanchez")),
    (0, 40, None), (0, 50, Some("Map Design")), (1, 80, Some("Leticia Krahe")), (0, 130, Some("Map Decoration")), (1, 160, Some("Jorge Rosado")),
    (0, 40, None), (0, 50, Some("Music and Sound Effects")), (1, 80, Some("Victor Segura")), (0, 130, Some("Voices")), (1, 160, Some("Emilio Mahoney")),
    (0, 40, None), (0, 50, Some("Moral Support")), (1, 110, Some("Friendware")), (1, 130, Some("Our families")), (1, 150, Some("Our friends")),
    (0, 40, None), (0, 50, Some("Moral Support")), (1, 110, Some("Friendware")), (1, 130, Some("Our families")), (1, 150, Some("Our friends")),
    (1, 175, Some("And, of course, you")), (0, 40, None),
];

/// MENU_DoCredits: credits over a waving flag. Returns false on ESC.
pub async fn credits(sys: &mut Sys, game_pal: &Palette) -> bool {
    let d = sys.data.clone();
    let (Ok(f0), Ok(f1)) = (Font::load(&d, "fontcr_1.fnt"), Font::load(&d, "fontcr_2.fnt")) else { return true };
    let fonts = [f0, f1];
    sys.pal = Palette::default();
    sys.vsync(1).await;
    let dtable: Vec<[i32; 1024]> = (0..32).map(|i| std::array::from_fn(|j| fp_mult(i, cos((j * 256 + 128) as u16)))).collect();
    let stable: Vec<[i32; 1024]> = (0..32)
        .map(|i| std::array::from_fn(|j| fp_mult(3 * i, fp_mult(cos((j * 256 + 128) as u16), cos((j * 256 + 128) as u16)))))
        .collect();
    // 256 rows of 512, the picture centred; reads may fall slightly outside.
    let mut image = vec![0u8; 256 * 512];
    let put_image = |image: &mut Vec<u8>, src: &[u8]| {
        for i in 0..200 {
            image[(i + 28) * 512 + 96..(i + 28) * 512 + 416].copy_from_slice(&src[i * 320..i * 320 + 320]);
        }
    };
    if let Some(p) = d.get("credflag.pix") {
        put_image(&mut image, p);
    }
    let clr = d.get("credflag.clr").map(<[u8]>::to_vec).unwrap_or_else(|| vec![0; 8192]);
    let img = |image: &[u8], k: isize| *image.get(k.max(0) as usize).unwrap_or(&0);
    let draw = |sys: &mut Sys, image: &[u8], a: usize, b: usize, idt: usize, shade: bool| {
        let (dt, st) = (&dtable[idt], &stable[idt]);
        let mut ia = a;
        for i in 0..200usize {
            let mut jb = (b + 2 * i) & 255;
            let kj = dt[ia] as isize;
            let base = ((i + 28) * 512) as isize + kj + 96;
            for j in 0..320usize {
                let k = base + j as isize + ((dt[jb] as isize) << 9);
                let v = img(image, k);
                sys.screen.pix[i * 320 + j] = if shade { clr[(31 - st[jb]) as usize * 256 + v as usize] } else { v };
                jb += 2;
            }
            ia = (ia + 2) & 255;
        }
    };
    sys.start_fade(Some(game_pal.clone()), 1);
    let (mut a, mut b, mut idt) = (17usize, 93usize, 6usize);
    let (mut ncr, mut clock) = (0usize, 0u32);
    sys.last_scan = 0;
    sys.vsync(0).await;
    while sys.last_scan == 0 && ncr < CREDITS.len() {
        draw(sys, &image, a, b, idt, true);
        let mut nc = ncr + 1;
        let mut k = ncr;
        while k < CREDITS.len() {
            let (font, y, text) = CREDITS[k];
            let Some(text) = text else { break };
            let w = fonts[font].text_width(text);
            fonts[font].write(&mut sys.screen, (320 - w) / 2, y, text, 15);
            k += 1;
            nc += 1;
        }
        clock += sys.vsync(3).await;
        let hold = if CREDITS[ncr].2.is_some() { 5 } else { 1 };
        if clock > hold * 70 {
            ncr = nc;
            clock = 0;
        }
        sys.update();
        a = (a + 5) & 255;
        b = (b + 9) & 255;
    }
    sys.last_scan = 0;
    idt = 5;
    for j in 0..40 {
        if sys.last_scan != 0 {
            break;
        }
        let shot = sys.screen.pix.clone();
        put_image(&mut image, &shot);
        draw(sys, &image, a, b, idt, false);
        sys.vsync(3).await;
        sys.update();
        if j == 40 - 22 {
            sys.start_full_fade(None, 1);
        }
        a = (a + 5) & 255;
        b = (b + 9) & 255;
    }
    sys.screen.clear(0);
    sys.update();
    sys.last_scan != keys::ESC
}

// ---- Greetings (INTRO_DoGreets): the hidden I+G+N starfield ----------------

const MAXDOTS: usize = 12000;
const XRANGE: i32 = 1 << 22;
const YRANGE: i32 = XRANGE;
const ZMIN: i32 = 2;
const ZRANGE: i32 = 16384;
const ZMAX: i32 = ZMIN + ZRANGE;
const CTABLEW: i32 = 380;

#[derive(Clone, Copy, Default)]
struct Star {
    x: i32,
    y: i32,
    z: i32,
}

/// AddStar1..5: where a (re)spawned star appears.
fn add_star(generator: usize, st: &mut Star, rng: &mut Rng, pos: i32) {
    let rnd = |rng: &mut Rng, range: i32| rng.below(range as u32) as i32;
    match generator {
        0 => {
            st.x = rnd(rng, XRANGE) - XRANGE / 2;
            st.y = rnd(rng, YRANGE) - YRANGE / 2;
        }
        1 => {
            let r = rng.next() as u16;
            st.x = fp_mult(XRANGE / 16, cos(r));
            st.y = fp_mult(YRANGE / 16, sin(r));
        }
        2 => {
            rng.next();
            st.x = rnd(rng, XRANGE) - XRANGE / 2;
            st.y = YRANGE / 8;
        }
        3 => {
            rng.next();
            st.x = rnd(rng, XRANGE) - XRANGE / 2;
            st.y = fp_mult(YRANGE / 16, cos((pos * 8) as u16) / 2 + cos((st.x / 16) as u16) / 2);
        }
        _ => {
            if rng.next() & 1 != 0 {
                st.x = (rnd(rng, XRANGE) & !0xFFFFF) - XRANGE / 2 + 0x100000 / 2;
                st.y = rnd(rng, YRANGE) - YRANGE / 2;
            } else {
                st.x = rnd(rng, XRANGE) - XRANGE / 2;
                st.y = (rnd(rng, YRANGE) & !0xFFFFF) - YRANGE / 2 + 0x100000 / 2;
            }
        }
    }
}

/// INTRO_DoGreets: a rotating starfield with Jare's greetings, until ESC.
/// Arrows change speed and spin, 1-5 pick the star pattern.
pub async fn greets(sys: &mut Sys) {
    let Ok(font) = Font::load(&sys.data, "fontgrt.fnt") else { return };
    // One entry more than the original's InvTbl: a star at exactly ZMAX
    // read past its end there.
    let inv: Vec<i32> = (0..=ZRANGE).map(|i| (1 << 30) / (ZMIN + i)).collect();
    let mut rng = std::mem::take(&mut sys.rng);

    // Mode Y with a 48-colour palette: greys, oranges, blues.
    sys.screen = crate::gfx::Screen::new(320, 200);
    sys.pal = Palette::default();
    for i in 0..16u8 {
        let k = i as usize * 3;
        sys.pal.0[k..k + 3].copy_from_slice(&[i * 4, i * 4, i * 4]);
        sys.pal.0[48 + k..48 + k + 3].copy_from_slice(&[16 + 3 * i, i * 2, 0]);
        sys.pal.0[96 + k..96 + k + 3].copy_from_slice(&[0, 2 * i, 16 + i * 2]);
    }

    let mut generator = 4;
    let mut gentime = 4 * 70;
    let mut stars = vec![Star::default(); MAXDOTS];
    for st in stars.iter_mut() {
        st.z = rng.below(ZRANGE as u32) as i32 + ZMIN;
        add_star(generator, st, &mut rng, 0);
    }
    let (mut starsv, mut destsv, mut destva) = (1i32, 100i32, 200i32);
    let (mut pos, mut angle, mut va) = (0i32, 0i32, 0i32);
    let (mut prevnf, mut f, mut ndots) = (1u32, 0u32, MAXDOTS);
    let mut planes: [Vec<(usize, u8)>; 4] = Default::default();
    let mut ct = vec![0i32; CTABLEW as usize * 2];
    let mut stt = vec![0i32; CTABLEW as usize * 2];
    sys.last_scan = 0;
    while sys.last_scan != keys::ESC {
        let held = |k: u8| sys.keys[k as usize];
        if held(keys::UARROW) {
            starsv += 1;
        }
        if held(keys::DARROW) {
            starsv -= 1;
        }
        if held(keys::LARROW) {
            va -= 1;
        }
        if held(keys::RARROW) {
            va += 1;
        }
        for (g, k) in [keys::K1, keys::K2, keys::K3, keys::K4, keys::K5].into_iter().enumerate() {
            if held(k) {
                generator = g;
            }
        }

        // GenTables.
        angle += va;
        for i in -CTABLEW..CTABLEW {
            ct[(i + CTABLEW) as usize] = fp_mult(2 * i, cos(angle as u16));
            stt[(i + CTABLEW) as usize] = fp_mult(2 * i, sin(angle as u16));
        }

        sys.screen.clear(0);
        planes.iter_mut().for_each(Vec::clear);
        for ps in stars.iter_mut().take(ndots) {
            let z = inv[(ps.z - ZMIN) as usize];
            let x = CTABLEW + fp_mult(2 * ps.x, z);
            let y = CTABLEW + fp_mult(2 * ps.y, z);
            if (0..CTABLEW * 2).contains(&x) && (0..CTABLEW * 2).contains(&y) {
                let rx = 160 + (ct[x as usize] + stt[y as usize]) / 4;
                let ry = 100 + (ct[y as usize] - stt[x as usize]) / 4;
                if (0..320).contains(&rx) && (0..200).contains(&ry) {
                    let c = if ps.z < ZMIN + ZRANGE / 2 { 15 } else { 15 - (ps.z - ZMIN - ZRANGE / 2) * 16 / (ZRANGE / 2) };
                    planes[(rx & 3) as usize].push((ry as usize * 320 + rx as usize, c as u8));
                }
            }
            ps.z -= starsv;
            if ps.z < ZMIN {
                ps.z = ZMAX - rng.below(1024) as i32;
                add_star(generator, ps, &mut rng, pos);
            } else if ps.z > ZMAX {
                ps.z = ZMIN + rng.below(1024) as i32;
                add_star(generator, ps, &mut rng, pos);
            }
        }
        pos += starsv;
        // DumpDots: plane by plane, each in reverse order.
        for plane in &planes {
            for &(o, c) in plane.iter().rev() {
                sys.screen.pix[o] = c;
            }
        }

        if f > 70 {
            font.write_greets(&mut sys.screen, 0, 0, "Greets from Jare to", 16);
            font.write_greets(&mut sys.screen, 0, 60, "All my friends and", 32);
            font.write_greets(&mut sys.screen, 0, 100, "everyone in the scene", 32);
        }

        if starsv < destsv {
            starsv += 1;
        } else if starsv > destsv {
            starsv -= 1;
        } else {
            destsv = (rng.next() % 200) as i32 - 100;
            destsv += sgn(destsv) * 200;
        }
        if va < destva {
            va += 1;
        } else if va > destva {
            va -= 1;
        } else {
            destva = (rng.next() % 200) as i32 - 100;
            destva += sgn(destva) * 100;
        }
        gentime -= 1;
        if gentime < 0 {
            gentime = 70 + (rng.next() % 200) as i32;
            generator = (rng.next() % 5) as usize;
        }

        sys.update();
        let nf = sys.vsync(1).await;
        // The original thinned the stars when it couldn't keep up.
        if ndots > MAXDOTS / 10 && nf > 1 && prevnf > 1 {
            ndots -= ndots / 16;
        } else if ndots < MAXDOTS && nf == 1 && prevnf == 1 {
            ndots = (ndots + ndots / 64).min(MAXDOTS);
        }
        prevnf = nf;
        f += nf;
    }
    sys.rng = rng;
    sys.pal = Palette::default();
    sys.screen.clear(0);
    sys.update();
    sys.vsync(0).await;
    sys.vsync(2).await;
}
