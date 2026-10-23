//! Record tables (game/records.c): best racers and track lap records, one
//! column per car type, stored in RECORDS.LST.

use crate::config::{clean_line, read_word, split_line};
use crate::game::Fonts;
use crate::gfx::Palette;
use crate::is2::Sprite;
use crate::keys;
use crate::sys::Sys;

const CAR_NAME: [&str; 2] = ["Formula  1 ", "Stock Car"];

#[derive(Clone, Debug, Default)]
pub struct BestRecord {
    pub name: [String; 2],
    pub ncirc: [i32; 2],
    pub nlaps: [i32; 2],
    pub time: [u32; 2],
}

#[derive(Clone, Debug, Default)]
pub struct CircRecord {
    pub name: [String; 2],
    pub ncirc: [i32; 2],
    pub time: [u32; 2],
}

#[derive(Clone, Debug, Default)]
pub struct Records {
    pub best: Vec<BestRecord>,
    pub circ: Vec<CircRecord>,
}

const STORAGE_KEY: &str = "records.lst";

impl Records {
    /// REC_Init: the saved table, else the defaults from the data file.
    pub fn load(sys: &Sys) -> Records {
        sys.storage
            .load(STORAGE_KEY)
            .and_then(|t| Records::parse(&t))
            .or_else(|| sys.data.get("records.lst").and_then(|b| Records::parse(&String::from_utf8_lossy(b))))
            .unwrap_or_default()
    }

    pub fn parse(text: &str) -> Option<Records> {
        let mut lines = text.lines().map(clean_line).filter(|l| !l.is_empty() && !l.starts_with(';'));
        let mut r = Records::default();
        let nbest = read_word(&lines.next()?) as usize;
        for line in lines.by_ref() {
            let t = split_line(&line, 10);
            if t.len() == 1 && t[0].eq_ignore_ascii_case("End") {
                break;
            }
            if t.len() == 7 && r.best.len() < nbest {
                let n = r.best.len() as i32 + 1;
                r.best.push(BestRecord {
                    name: [name8(&t[1]), name8(&t[4])],
                    ncirc: [n, n],
                    nlaps: [read_word(&t[2]), read_word(&t[5])],
                    time: [read_word(&t[3]) as u32, read_word(&t[6]) as u32],
                });
            }
        }
        let ncirc = read_word(&lines.next()?) as usize;
        for line in lines {
            let t = split_line(&line, 10);
            if t.len() == 1 && t[0].eq_ignore_ascii_case("End") {
                break;
            }
            if t.len() == 5 && r.circ.len() < ncirc {
                let n = r.circ.len() as i32 + 1;
                r.circ.push(CircRecord {
                    name: [name8(&t[1]), name8(&t[3])],
                    ncirc: [n, n],
                    time: [read_word(&t[2]) as u32, read_word(&t[4]) as u32],
                });
            }
        }
        (r.best.len() == nbest && r.circ.len() == ncirc).then_some(r)
    }

    pub fn to_text(&self) -> String {
        let mut s = format!("{}\n", self.best.len());
        for (i, b) in self.best.iter().enumerate() {
            s += &format!("{i} \"{}\" {} {} \"{}\" {} {}\n", b.name[0], b.nlaps[0], b.time[0], b.name[1], b.nlaps[1], b.time[1]);
        }
        s += &format!("End\n{}\n", self.circ.len());
        for (i, c) in self.circ.iter().enumerate() {
            s += &format!("{i} \"{}\" {} \"{}\" {}\n", c.name[0], c.time[0], c.name[1], c.time[1]);
        }
        s + "End\n"
    }

    pub fn save(&self, sys: &mut Sys) {
        sys.storage.save(STORAGE_KEY, &self.to_text());
    }

    /// REC_AddBest: slot this result would take, by average lap time.
    fn add_best(&mut self, ncirc: i32, nlaps: i32, time: u32, ncar: usize) -> Option<usize> {
        if ncar > 1 || time == 0 || nlaps <= 0 {
            return None;
        }
        let i = self.best.iter().position(|b| {
            ncirc == b.ncirc[ncar] && b.nlaps[ncar] > 0 && time / (nlaps as u32) < b.time[ncar] / (b.nlaps[ncar] as u32)
        })?;
        let b = &mut self.best[i];
        b.name[ncar].clear();
        b.time[ncar] = time;
        b.nlaps[ncar] = nlaps;
        b.ncirc[ncar] = ncirc;
        Some(i)
    }

    fn add_circ(&mut self, ncirc: i32, time: u32, ncar: usize) -> Option<usize> {
        if ncar > 1 || time == 0 {
            return None;
        }
        let i = self.circ.iter().position(|c| ncirc == c.ncirc[ncar] && time < c.time[ncar])?;
        self.circ[i].name[ncar].clear();
        self.circ[i].time[ncar] = time;
        Some(i)
    }
}

fn name8(s: &str) -> String {
    s.chars().take(8).collect()
}

fn fmt_time(t: u32) -> String {
    format!("{}'{:02}\"{:02}", t / 70 / 60, (t / 70) % 60, t % 70 * 100 / 70)
}

/// The shared look of the record screens; returns the composed background.
async fn record_screen(sys: &mut Sys, fonts: &Fonts, title: &str, cols: &[(i32, &str)]) -> Vec<u8> {
    sys.fade_out(3).await;
    if let Some(b) = sys.data.get("mbg_reco.pix") {
        let b = b.to_vec();
        sys.screen.blit_pix(&b);
    }
    let clr = sys.data.get("grafs.clr").map(<[u8]>::to_vec).unwrap_or_default();
    if let Ok(sp) = Sprite::load(&sys.data, "rec_txt.is2") {
        sp.draw(&mut sys.screen, 160 - sp.w / 2, 5, sp.w, sp.h, &clr[15 * 256..16 * 256]);
    }
    let w = fonts.yellow.text_width(title);
    fonts.yellow.write(&mut sys.screen, (320 - w) / 2, 60, title, 0);
    for &(x, t) in cols {
        fonts.white.write(&mut sys.screen, x, 73, t, 0);
    }
    let back = sys.screen.pix.clone();
    sys.update();
    let pal = Palette::load(&sys.data, "grafs.pal").unwrap_or_default();
    sys.start_fade(Some(pal), 3);
    sys.vsync(0).await;
    sys.last_scan = 0;
    back
}

async fn end_screen(sys: &mut Sys) {
    sys.fade.pos = 0;
    sys.pal = Palette::load(&sys.data, "grafs.pal").unwrap_or_default();
    sys.vsync(0).await;
    sys.vsync(1).await;
}

/// REC_ShowBest. Returns false if ESC was pressed.
pub async fn show_best(sys: &mut Sys, fonts: &Fonts, rec: &Records, ncar: usize) -> bool {
    if ncar > 1 {
        return false;
    }
    let cols = [(10, "Name"), (130, "Track"), (190, "Laps"), (240, "Time")];
    let back = record_screen(sys, fonts, &format!("{} Best Racers", CAR_NAME[ncar]), &cols).await;
    let mut l = 0;
    loop {
        sys.screen.pix.copy_from_slice(&back);
        for (i, b) in rec.best.iter().enumerate() {
            let y = 90 + 13 * i as i32;
            fonts.yellow.write(&mut sys.screen, 10, y, &b.name[ncar], 0);
            fonts.yellow.write(&mut sys.screen, 130, y, &b.ncirc[ncar].to_string(), 0);
            fonts.yellow.write(&mut sys.screen, 190, y, &b.nlaps[ncar].to_string(), 0);
            fonts.yellow.write(&mut sys.screen, 240, y, &fmt_time(b.time[ncar]), 0);
        }
        sys.update();
        l += sys.vsync(1).await;
        if l >= 10 * 70 || sys.last_scan != 0 {
            break;
        }
    }
    end_screen(sys).await;
    sys.last_scan != keys::ESC
}

/// REC_ShowCirc. Returns false if ESC was pressed.
pub async fn show_circ(sys: &mut Sys, fonts: &Fonts, rec: &Records, ncar: usize) -> bool {
    if ncar > 1 {
        return false;
    }
    let cols = [(10, "Name"), (140, "Track"), (200, "Best Lap")];
    let back = record_screen(sys, fonts, &format!("{} Track records", CAR_NAME[ncar]), &cols).await;
    let mut l = 0;
    loop {
        sys.screen.pix.copy_from_slice(&back);
        for (i, c) in rec.circ.iter().enumerate() {
            let y = 90 + 13 * i as i32;
            fonts.yellow.write(&mut sys.screen, 10, y, &c.name[ncar], 0);
            fonts.yellow.write(&mut sys.screen, 140, y, &c.ncirc[ncar].to_string(), 0);
            fonts.yellow.write(&mut sys.screen, 200, y, &fmt_time(c.time[ncar]), 0);
        }
        sys.update();
        l += sys.vsync(1).await;
        if l >= 10 * 70 || sys.last_scan != 0 {
            break;
        }
    }
    end_screen(sys).await;
    sys.last_scan != keys::ESC
}

/// The character typed for a scancode in name entry (KEYN_FindKey names of
/// length 1, plus space).
pub fn key_char(sc: u8) -> Option<char> {
    if sc == keys::SPACE {
        return Some(' ');
    }
    let n = keys::name(sc)?;
    match n {
        _ if n.len() == 1 => n.chars().next(),
        _ if n.len() == 2 && n.starts_with('K') && n.as_bytes()[1].is_ascii_digit() => n.chars().nth(1),
        "MINUS" => Some('-'),
        "EQUAL" => Some('='),
        "LBRACKET" => Some('['),
        "RBRACKET" => Some(']'),
        "SEMICOLON" => Some(';'),
        "TICK" => Some('^'),
        "APOSTROPHE" => Some('\''),
        "BACKSLASH" => Some('\\'),
        "COMMA" => Some(','),
        "PERIOD" => Some('.'),
        "SLASH" => Some('/'),
        _ => None,
    }
}

/// Name entry shared by REC_InputBest / REC_InputCirc: edits `get(rec)`.
async fn input_name(
    sys: &mut Sys,
    fonts: &Fonts,
    rec: &mut Records,
    back: Vec<u8>,
    nr: usize,
    draw_row: impl Fn(&mut Sys, &Records, usize, &crate::gfx::Font, bool),
    name: impl Fn(&mut Records) -> &mut String,
) {
    let mut l = 0u32;
    loop {
        sys.screen.pix.copy_from_slice(&back);
        for i in 0..rec.best.len().max(rec.circ.len()) {
            if i == nr {
                let f = if l & 16 != 0 { &fonts.yellow } else { &fonts.white };
                draw_row(sys, rec, i, f, l & 32 != 0);
            } else {
                draw_row(sys, rec, i, &fonts.yellow, false);
            }
        }
        l += sys.vsync(1).await;
        sys.update();
        let c = sys.last_scan;
        sys.last_scan = 0;
        let nm = name(rec);
        match c {
            0 => {}
            keys::BACKSPACE => {
                nm.pop();
            }
            keys::ESC | keys::ENTER | keys::KEYPADENTER => {
                if nm.is_empty() {
                    *nm = "Mr. Shy".into();
                }
                break;
            }
            _ => {
                if nm.len() < 8
                    && let Some(ch) = key_char(c)
                {
                    nm.push(ch);
                }
            }
        }
    }
}

/// REC_InputBest: if the result is a best-racer record, let the player
/// enter a name.
pub async fn input_best(sys: &mut Sys, fonts: &Fonts, rec: &mut Records, nplayer: usize, ncirc: i32, nlaps: i32, time: u32, ncar: usize) {
    let Some(nr) = rec.add_best(ncirc, nlaps, time, ncar) else { return };
    let cols = [(10, "Name"), (130, "Track"), (190, "Laps"), (240, "Time")];
    let back = record_screen(sys, fonts, &format!("Best racer player {}", nplayer + 1), &cols).await;
    let n_shown = rec.best.iter().take_while(|b| b.nlaps[ncar] > 0).count();
    input_name(
        sys,
        fonts,
        rec,
        back,
        nr,
        move |sys, rec, i, f, cursor| {
            if i >= n_shown {
                return;
            }
            let b = &rec.best[i];
            let y = 90 + 13 * i as i32;
            let x = f.write(&mut sys.screen, 10, y, &b.name[ncar], 0);
            if cursor {
                f.write(&mut sys.screen, x, y + 5, "-", 0);
            }
            f.write(&mut sys.screen, 130, y, &b.ncirc[ncar].to_string(), 0);
            f.write(&mut sys.screen, 190, y, &b.nlaps[ncar].to_string(), 0);
            f.write(&mut sys.screen, 240, y, &fmt_time(b.time[ncar]), 0);
        },
        move |rec| &mut rec.best[nr].name[ncar],
    )
    .await;
    rec.save(sys);
    end_screen(sys).await;
}

/// REC_InputCirc: same for track lap records.
pub async fn input_circ(sys: &mut Sys, fonts: &Fonts, rec: &mut Records, nplayer: usize, ncirc: i32, time: u32, ncar: usize) {
    let Some(nr) = rec.add_circ(ncirc, time, ncar) else { return };
    let cols = [(10, "Name"), (140, "Track"), (200, "Best Lap")];
    let back = record_screen(sys, fonts, &format!("Track Record player {}", nplayer + 1), &cols).await;
    let n_shown = rec.circ.len();
    input_name(
        sys,
        fonts,
        rec,
        back,
        nr,
        move |sys, rec, i, f, cursor| {
            if i >= n_shown {
                return;
            }
            let c = &rec.circ[i];
            let y = 90 + 13 * i as i32;
            let x = f.write(&mut sys.screen, 10, y, &c.name[ncar], 0);
            if cursor {
                f.write(&mut sys.screen, x, y + 5, "-", 0);
            }
            f.write(&mut sys.screen, 140, y, &c.ncirc[ncar].to_string(), 0);
            f.write(&mut sys.screen, 200, y, &fmt_time(c.time[ncar]), 0);
        },
        move |rec| &mut rec.circ[nr].name[ncar],
    )
    .await;
    rec.save(sys);
    end_screen(sys).await;
}
