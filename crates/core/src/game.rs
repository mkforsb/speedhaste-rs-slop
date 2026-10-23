//! Top-level program flow (game/fl2.c `main`): intro, menus, races,
//! results and records.

use crate::config::Config;
use crate::data::{Data, Result};
use crate::gfx::{Font, Palette, Screen};
use crate::keys;
use crate::menus::{self, Ctx, Menu, Menus};
use crate::models::Circuit;
use crate::race::{self, RaceResult, RaceSetup};
use crate::records::{self, Records};
use crate::sys::Sys;

/// VGA BIOS default palette after a mode 13h set: colours 0-15 and 240-255.
pub const VGA_DEFAULT_LO: [[u8; 3]; 16] = [
    [0, 0, 0], [0, 0, 42], [0, 42, 0], [0, 42, 42], [42, 0, 0], [42, 0, 42], [42, 21, 0], [42, 42, 42],
    [21, 21, 21], [21, 21, 63], [21, 63, 21], [21, 63, 63], [63, 21, 21], [63, 21, 63], [63, 63, 21], [63, 63, 63],
];
pub const VGA_DEFAULT_HI: [[u8; 3]; 16] = [
    [11, 16, 11], [11, 16, 12], [11, 16, 13], [11, 16, 15], [11, 16, 16], [11, 15, 16], [11, 13, 16], [11, 12, 16],
    [0, 0, 0], [0, 0, 0], [0, 0, 0], [0, 0, 0], [0, 0, 0], [0, 0, 0], [0, 0, 0], [0, 0, 0],
];

/// The global fonts (GL_YFont etc. in globals.c).
pub struct Fonts {
    pub border: Font,
    pub yellow: Font,
    pub red: Font,
    pub white: Font,
}

impl Fonts {
    pub fn load(d: &Data) -> Result<Fonts> {
        Ok(Fonts {
            border: Font::load(d, "fontbord.fnt")?,
            yellow: Font::load(d, "mfontyel.fnt")?,
            red: Font::load(d, "mfontred.fnt")?,
            white: Font::load(d, "mfontwhi.fnt")?,
        })
    }
}

pub async fn run(mut sys: Sys) {
    if let Err(e) = main_flow(&mut sys).await {
        eprintln!("error: {e}");
    }
}

struct State {
    cfg: Config,
    fonts: Fonts,
    rec: Records,
    game_pal: Palette,
    clr: Vec<u8>,
    circuits: Vec<Circuit>,
    menus: Menus,
    sel_circuit: usize,
    sel_car: [usize; 2],
    seed: u32,
    /// The master's decoration detail in a network game.
    net_decoration: Option<i32>,
    /// GL_Registered: the full game's data, not the shareware's.
    registered: bool,
}

impl State {
    fn ctx(&mut self) -> (Ctx<'_>, &mut Menus) {
        (
            Ctx { fonts: &self.fonts, cfg: &mut self.cfg, rec: &self.rec, game_pal: &self.game_pal, clr: &self.clr, registered: self.registered },
            &mut self.menus,
        )
    }
}

fn apply_audio_cfg(sys: &Sys, cfg: &Config) {
    let mut a = sys.audio.lock().unwrap();
    a.cfg_music_volume = if cfg.music_on { cfg.music_volume } else { 0 };
    a.cfg_fx_volume = if cfg.fx_on { cfg.fx_volume } else { 0 };
}

async fn main_flow(sys: &mut Sys) -> Result<()> {
    let d = sys.data.clone();
    let cfg = sys.storage.load("speed.cfg").map(|t| Config::parse(&t)).unwrap_or_default();
    apply_audio_cfg(sys, &cfg);
    let seed = sys.arg_value("seed").and_then(|v| v.parse().ok()).unwrap_or(0x1234_5678);
    sys.rng.randomize(seed);
    let mut st = State {
        fonts: Fonts::load(&d)?,
        rec: Records::load(sys),
        game_pal: Palette::load(&d, "grafs.pal")?,
        clr: d.load("grafs.clr")?.to_vec(),
        circuits: crate::models::load_circuits(&d, cfg.car_type),
        menus: Menus::new(sys)?,
        cfg,
        sel_circuit: 0,
        sel_car: [0, 0],
        seed,
        net_decoration: None,
        registered: d.is_registered(),
    };

    if sys.arg("nologo").is_none() {
        crate::intro::intro(sys).await;
    }
    // grafs.pal has placeholders in colours 0-15 and 240-255; the original
    // reads them back from the DAC right after LLS_Init's BIOS mode 13h
    // set, i.e. the VGA BIOS default palette (checked against DOSBox).
    st.game_pal.0[..48].copy_from_slice(VGA_DEFAULT_LO.as_flattened());
    st.game_pal.0[720..].copy_from_slice(VGA_DEFAULT_HI.as_flattened());
    sys.screen = Screen::new(320, 200);
    sys.pal = st.game_pal.clone();

    let mut first_menu = false;
    loop {
        if first_menu {
            attract(sys, &mut st).await?;
            first_menu = false;
        } else if sys.song != "menus.s3m" {
            sys.done_music();
            sys.init_music(Some("menus.s3m")).await;
        }
        st.seed = sys.rng.next();
        // COM_EndGame: leave any network game.
        end_net(sys);
        st.menus.enter(Menu::Main);
        let mut rez = {
            let (mut ctx, m) = st.ctx();
            m.run(sys, &mut ctx).await
        };
        if rez >= 0 {
            let mut racemode = rez;
            let slave = sys.net.as_ref().is_some_and(|n| n.slave);
            if racemode == race::MODE_CHAMPIONSHIP && !slave {
                st.sel_circuit = 0;
            } else if !slave {
                if sys.arg("car").is_none() || sys.arg("track").is_none() {
                    sys.done_music();
                    sys.init_music(Some("scircuit.s3m")).await;
                }
                st.menus.enter(Menu::ChooseCirc);
                let (mut ctx, m) = st.ctx();
                rez = m.run(sys, &mut ctx).await;
                if rez >= 0 {
                    st.sel_circuit = rez as usize;
                }
            }
            if rez >= 0 && sys.net.is_some() {
                st.menus.console_players = 1;
                rez = if contact_players(sys, &mut st, &mut racemode).await { 0 } else { -1 };
            }
            if rez >= 0 {
                let players = st.menus.console_players;
                for p in 0..players {
                    st.menus.car_player = p;
                    st.menus.enter(Menu::ChooseCar);
                    let (mut ctx, m) = st.ctx();
                    rez = m.run(sys, &mut ctx).await;
                    if rez < 0 {
                        break;
                    }
                    st.sel_car[p] = rez as usize;
                }
                if rez >= 0
                    && let Some(mut net) = sys.net.take()
                {
                    if !crate::net::get_cars(sys, &st.fonts, &mut net, st.sel_car[0], st.cfg.p1_keys).await {
                        rez = -1;
                    }
                    sys.net = Some(net);
                }
                if rez >= 0 {
                    run_race(sys, &mut st, racemode).await?;
                }
            }
            st.menus.end();
            rez = 0;
        } else if rez == menus::MENU_IDLE {
            sys.done_music();
            sys.init_music(Some("haste9.s3m")).await;
            first_menu = true;
            rez = 0;
        } else if rez == menus::MENU_REINIT {
            rez = 0;
        } else if rez == menus::MENU_GREETS {
            st.menus.end();
            crate::intro::greets(sys).await;
            rez = 0;
        }
        apply_audio_cfg(sys, &st.cfg);
        if rez < 0 {
            break;
        }
    }

    // The farewell picture.
    sys.screen.clear(0);
    sys.update();
    sys.pal = Palette::default();
    sys.last_scan = 0;
    if let (Some(pix), Ok(pal)) = (d.get("sphlogo.pix").map(<[u8]>::to_vec), Palette::load(&d, "sphlogo.pal")) {
        sys.done_music();
        sys.init_music(Some("fin.s3m")).await;
        sys.vsync(0).await;
        sys.vsync(2).await;
        sys.screen.blit_pix(&pix);
        sys.update();
        sys.start_fade(Some(pal.clone()), 1);
        sys.wait_fade(true).await;
        sys.pal = pal;
        sys.vsync(0).await;
        sys.vsync(2).await;
        sys.last_scan = 0;
        let mut n = 0;
        while n < 22 * 70 && sys.last_scan == 0 {
            n += sys.vsync(1).await;
        }
        sys.audio.lock().unwrap().mus_set_fading(false, 150);
        sys.last_scan = 0;
        sys.start_fade(None, 1);
        sys.wait_fade(true).await;
    } else {
        sys.done_music();
    }
    sys.wait_music_fade().await;
    Ok(())
}

/// Leave the network game, if any.
fn end_net(sys: &mut Sys) {
    if let Some(mut n) = sys.net.take() {
        n.bye();
    }
}

/// NET_ContactPlayers: the master announces its choices, slaves take them.
async fn contact_players(sys: &mut Sys, st: &mut State, racemode: &mut i32) -> bool {
    let Some(mut net) = sys.net.take() else { return false };
    let mut setup = crate::net::Setup {
        racemode: *racemode,
        circuit: st.sel_circuit,
        laps: st.cfg.num_laps,
        timed: st.cfg.timed_race,
        car_type: st.cfg.car_type,
        seed: st.seed,
        decoration: st.cfg.decoration_detail,
        no_collide: st.menus.no_collide,
    };
    let ok = crate::net::contact_players(sys, &st.fonts, &mut net, &mut setup).await;
    if ok && net.slave {
        *racemode = setup.racemode;
        st.sel_circuit = setup.circuit;
        st.cfg.num_laps = setup.laps;
        st.cfg.timed_race = setup.timed;
        st.seed = setup.seed;
        st.menus.no_collide = setup.no_collide;
        if st.cfg.car_type != setup.car_type {
            st.cfg.car_type = setup.car_type;
            st.circuits = crate::models::load_circuits(&sys.data, setup.car_type);
        }
    }
    // Everyone decorates the circuits alike (see net::Setup::decoration).
    st.net_decoration = Some(setup.decoration);
    if ok {
        sys.net = Some(net);
    } else {
        net.bye();
    }
    ok
}

/// The idle-menu attract sequence: demo race, credits, record tables.
async fn attract(sys: &mut Sys, st: &mut State) -> Result<()> {
    let mut cfg = st.cfg.clone();
    cfg.background_detail = 50;
    st.sel_circuit = 0;
    sys.screen = race_screen(sys, &st.cfg);
    show_entry_screen(sys, st.cfg.car_type).await;
    let setup = RaceSetup { cfg: &cfg, mode: race::MODE_DEMO, circuit: 0, cars: vec![], net_node: None, net_keys: vec![], seed: st.seed, circuits: &st.circuits, pal: &st.game_pal };
    race::do_demo(sys, &setup, &st.fonts).await?;
    sys.screen = Screen::new(320, 200);
    sys.done_music();
    sys.init_music(Some("final.s3m")).await;
    if crate::intro::credits(sys, &st.game_pal).await && records::show_best(sys, &st.fonts, &st.rec, car_idx(&st.cfg)).await {
        records::show_circ(sys, &st.fonts, &st.rec, car_idx(&st.cfg)).await;
    }
    sys.done_music();
    sys.init_music(Some("menus.s3m")).await;
    sys.pal = st.game_pal.clone();
    Ok(())
}

fn car_idx(cfg: &Config) -> usize {
    cfg.car_type.clamp(0, 1) as usize
}

fn race_screen(sys: &Sys, cfg: &Config) -> Screen {
    if cfg.svga_on {
        let (w, h) = sys.hires_size();
        Screen::new(w, h)
    } else {
        Screen::new(320, 200)
    }
}

/// ShowEntryScreen: the loading picture and the engine starting.
async fn show_entry_screen(sys: &mut Sys, car_type: i32) {
    sys.screen.clear(0);
    sys.update();
    let d = sys.data.clone();
    if let Ok(p) = Palette::load(&d, &format!("rload{car_type}0.pal")) {
        sys.pal = p;
    }
    if let Some(pix) = d.get(&format!("rload{car_type}0.pix")) {
        let s = &mut sys.screen;
        let sx = s.w / 320;
        let y0 = if s.h == 480 { 40 } else { 0 };
        for y in 0..200 * sx {
            for x in 0..320 * sx {
                s.pix[(y + y0) * s.w + x] = pix[(y / sx) * 320 + x / sx];
            }
        }
    }
    sys.update();
    let eff = sys.audio.lock().unwrap().load_effect(&d, "startech.raw", 11025, false);
    if eff.is_some() {
        sys.audio.lock().unwrap().eff_start(0, 256, 450, 0, eff);
        sys.wait_ticks(3 * 70).await;
    }
    let mut a = sys.audio.lock().unwrap();
    a.eff_stop_all();
    a.unload_all();
}

/// ShowRaceResults: championship points. Returns true if abandoned.
async fn show_race_results(sys: &mut Sys, st: &State, racemode: i32, results: &[RaceResult], scores: &mut [i32], last: bool) -> bool {
    const POINTS: [i32; 12] = [25, 18, 15, 12, 9, 7, 6, 5, 4, 3, 2, 1];
    for (s, r) in scores.iter_mut().zip(results) {
        if let Some(p) = POINTS.get(r.pos as usize) {
            *s += p;
        }
    }
    sys.fade_out(3).await;
    if let Some(b) = sys.data.get("mbg_resu.pix") {
        let b = b.to_vec();
        sys.screen.blit_pix(&b);
    }
    for i in 0..results.len() as i32 {
        for j in 0..23i32 {
            let y = (j + 70 + 30 * i - 2) as usize;
            for x in 10..310 {
                let p = &mut sys.screen.pix[y * 320 + x];
                *p = st.clr[((16 - 11 + (j - 11).abs() * 15 / 11) as usize) * 256 + *p as usize];
            }
        }
    }
    let back = sys.screen.pix.clone();
    sys.update();
    sys.start_fade(Some(st.game_pal.clone()), 3);
    sys.vsync(0).await;
    sys.last_scan = 0;
    let f = &st.fonts;
    let mut l = 0;
    let abandon = loop {
        sys.screen.pix.copy_from_slice(&back);
        for (i, r) in results.iter().enumerate() {
            // In network games your own line blinks.
            if sys.net.is_some() && l & 0x10 == 0 && r.flags & race::RACERF_ISCONSOLE != 0 {
                continue;
            }
            let y = 70 + 30 * i as i32;
            let th = ["st", "nd", "rd"].get(r.pos as usize).unwrap_or(&"th");
            f.white.write(&mut sys.screen, 10, y, &format!("Player {}", i + 1), 0);
            f.red.write(&mut sys.screen, 120, y, &format!("Finished {}{}", r.pos + 1, th), 0);
            f.red.write(&mut sys.screen, 120, y + 12, &format!("Total score is {}", scores[i]), 0);
        }
        l += sys.vsync(1).await;
        sys.update();
        if sys.net.as_mut().is_some_and(crate::net::someone_left) {
            break true;
        }
        let k = sys.last_scan;
        if k == 0 {
            continue;
        }
        sys.last_scan = 0;
        if k == keys::ESC && racemode == race::MODE_CHAMPIONSHIP && !last {
            let x = 0;
            let y = 0;
            let s = &mut sys.screen;
            s.rectangle(x + 20, y + 70, 280, 60, 15, -1);
            s.rectangle(x + 21, y + 71, 278, 58, 7, -1);
            s.rectangle(x + 22, y + 72, 276, 56, 8, -1);
            s.rectangle(x + 23, y + 73, 274, 54, 7, 8);
            f.yellow.write(s, x + 35, y + 80, "Abandon Championship", 15);
            f.red.write(s, x + 25, y + 105, "Press  ESC  to confirm", 15);
            sys.update();
            let k = sys.wait_key().await;
            sys.last_scan = 0;
            if k == keys::ESC {
                break true;
            }
            continue;
        }
        break k == keys::ESC;
    };
    sys.fade.pos = 0;
    sys.pal = st.game_pal.clone();
    sys.vsync(0).await;
    sys.vsync(1).await;
    abandon
}

/// RunRace: one race, or the whole championship.
async fn run_race(sys: &mut Sys, st: &mut State, racemode: i32) -> Result<()> {
    let mut scores = vec![0; sys.net.as_ref().map_or(2, |n| n.nodes.len())];
    let mut lastm = -1;
    st.menus.end();
    loop {
        // Pick a song (not the last one if possible).
        let mut ns;
        loop {
            ns = (sys.rng.next() & 63) as i32;
            if !(st.cfg.songs.len() > 1 && ns == lastm) {
                break;
            }
        }
        lastm = ns;
        sys.done_music();
        if st.cfg.songs.is_empty() {
            sys.init_music(None).await;
        } else {
            let k = ns as usize % st.cfg.songs.len();
            sys.init_music(Some(&format!("haste{}.s3m", st.cfg.songs[k] + 1))).await;
        }
        sys.screen = race_screen(sys, &st.cfg);
        show_entry_screen(sys, st.cfg.car_type).await;
        let mut cfg = st.cfg.clone();
        let circuits = crate::models::load_circuits(&sys.data, cfg.car_type);
        let mut cars = st.sel_car[..st.menus.console_players].to_vec();
        let mut net_node = None;
        let mut net_keys = Vec::new();
        if let Some(n) = &sys.net {
            cars = n.cars.clone();
            net_keys = n.keys.clone();
            net_node = Some(n.node());
            cfg.decoration_detail = st.net_decoration.unwrap_or(cfg.decoration_detail);
        }
        let setup = RaceSetup { cfg: &cfg, mode: racemode, circuit: st.sel_circuit, cars, net_node, net_keys, seed: st.seed, circuits: &circuits, pal: &st.game_pal };
        let results = race::do_race(sys, &setup, &st.fonts).await?;
        st.seed = sys.rng.next();
        sys.screen = Screen::new(320, 200);
        sys.pal = st.game_pal.clone();
        sys.done_music();
        sys.init_music(Some("final.s3m")).await;
        if racemode < 2 {
            let ncar = car_idx(&st.cfg);
            for (j, r) in results.iter().enumerate().filter(|(_, r)| r.flags & race::RACERF_ISCONSOLE != 0) {
                let circ = st.sel_circuit as i32 + 1;
                records::input_best(sys, &st.fonts, &mut st.rec, j, circ, r.nlaps, r.time, ncar).await;
                records::input_circ(sys, &st.fonts, &mut st.rec, j, circ, r.best, ncar).await;
            }
        }
        let last = st.sel_circuit + 1 >= st.circuits.len();
        let lost = sys.net.as_mut().is_some_and(crate::net::someone_left);
        if racemode == race::MODE_PRACTICE || lost || show_race_results(sys, st, racemode, &results, &mut scores, last).await {
            end_net(sys);
            break;
        }
        if racemode == race::MODE_CHAMPIONSHIP {
            st.sel_circuit += 1;
        }
        if !(racemode == race::MODE_CHAMPIONSHIP && st.sel_circuit < st.circuits.len()) {
            break;
        }
    }
    let ncar = car_idx(&st.cfg);
    if records::show_best(sys, &st.fonts, &st.rec, ncar).await {
        records::show_circ(sys, &st.fonts, &st.rec, ncar).await;
    }
    Ok(())
}
