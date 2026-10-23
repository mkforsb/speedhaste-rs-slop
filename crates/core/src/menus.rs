//! The menu system (game/menus.c): a stack of menu screens run by
//! [`Menus::run`], each a loop around the polled control menu.

use crate::config::Config;
use crate::fx::*;
use crate::game::Fonts;
use crate::gfx::{Palette, Rect};
use crate::is2::Sprite;
use crate::keys;
use crate::mesh::Mesh;
use crate::models::{self, CarModel};
use crate::race::render::Renderer;
use crate::records::Records;
use crate::sys::Sys;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Menu {
    Main,
    Sound,
    Detail,
    ChooseCirc,
    ChooseCar,
    Config,
    RaceType,
    Controls,
    TimeOrLaps,
    Jukebox,
    CarType,
    Shareware,
    NetGame,
}

/// MENU_Run results besides a positive race mode / circuit / car.
pub const MENU_QUIT: i32 = -1;
pub const MENU_IDLE: i32 = -2;
pub const MENU_REINIT: i32 = -3;
pub const MENU_GREETS: i32 = -4;

/// Logo letter positions. The source (Circuit Racer) had 12 letters; the
/// shipped Speed Haste table at 0x68394 has these 10 (found with REA).
const LOGO_POS: [(i32, i32); 10] = [(59, 31), (98, 31), (132, 31), (164, 31), (196, 31), (88, 60), (117, 65), (157, 60), (194, 60), (227, 60)];

pub const SONG_NAMES: [&str; 8] = ["Speed'em", "Machine rage", "Soul road", "Sakara bru", "Rock'n'roll", "Green brain", "Que mas da", "Over again"];

/// What the menus read and change.
pub struct Ctx<'a> {
    pub fonts: &'a Fonts,
    pub cfg: &'a mut Config,
    pub rec: &'a Records,
    pub game_pal: &'a Palette,
    pub clr: &'a [u8],
    pub registered: bool,
}

pub struct Menus {
    back: Option<Vec<u8>>,
    logos: Vec<Sprite>,
    helmet: Sprite,
    credits: Sprite,
    tm: Sprite,
    stack: Vec<Menu>,
    opts: [usize; 16],
    sel_circ: usize,
    sel_car: usize,
    jukebox_cursor: (usize, usize),
    /// GL_ConsolePlayers.
    pub console_players: usize,
    /// MENU_SelCarNumPlayer: which player is choosing a car.
    pub car_player: usize,
    /// GL_NoCollide cheat (hold C when starting a race).
    pub no_collide: bool,
    renderer: Renderer,
}

impl Menus {
    pub fn new(sys: &Sys) -> crate::data::Result<Menus> {
        let d = &sys.data;
        Ok(Menus {
            back: None,
            logos: (0..LOGO_POS.len()).map(|i| Sprite::load(d, &format!("sphl{i}.is2"))).collect::<crate::data::Result<_>>()?,
            helmet: Sprite::load(d, "mmhelmet.is2")?,
            credits: Sprite::load(d, "mmcred.is2")?,
            tm: Sprite::load(d, "mmtm.is2")?,
            stack: Vec::new(),
            opts: [0; 16],
            sel_circ: 0,
            sel_car: 0,
            jukebox_cursor: (0, 0),
            console_players: 1,
            car_player: 0,
            no_collide: false,
            renderer: Renderer::default(),
        })
    }

    pub fn enter(&mut self, m: Menu) {
        self.stack.push(m);
    }

    fn ret(&mut self) {
        self.stack.pop();
    }

    /// MENU_End: forget the background (menus will recompose it).
    pub fn end(&mut self) {
        self.back = None;
    }

    /// MENU_Init with a .PIX background.
    fn init_back(&mut self, sys: &Sys, name: &str) {
        let n = sys.screen.w * sys.screen.h;
        let mut b = vec![127u8; n];
        if let Some(p) = sys.data.get(name).filter(|p| p.len() >= n) {
            b.copy_from_slice(&p[..n]);
        }
        self.back = Some(b);
    }

    fn blit_back(&self, sys: &mut Sys) {
        if let Some(b) = &self.back {
            sys.screen.pix.copy_from_slice(b);
        }
    }

    /// SetBackgroundScreen: the main menu picture with helmet and logo.
    fn set_background(&mut self, sys: &mut Sys, ctx: &Ctx) {
        self.init_back(sys, "mbg_prin.pix");
        self.blit_back(sys);
        let x = &ctx.clr[15 * 256..16 * 256];
        let s = &mut sys.screen;
        self.helmet.draw(s, 200, 20, self.helmet.w, self.helmet.h, x);
        self.credits.draw(s, 160, 200 - 2 - self.credits.h, self.credits.w, self.credits.h, x);
        for (p, &(lx, ly)) in self.logos.iter().zip(LOGO_POS.iter()) {
            p.draw(s, lx, ly, p.w, p.h, x);
        }
        self.tm.draw(s, 255, 45, self.tm.w, self.tm.h, x);
        self.back = Some(sys.screen.pix.clone());
    }

    fn ensure_background(&mut self, sys: &mut Sys, ctx: &Ctx) {
        if self.back.is_none() {
            self.set_background(sys, ctx);
        }
    }

    /// MENU_DoControlMenu: draw the options, let one frame pass, and handle
    /// cursor movement. Returns the key pressed (0 if none).
    async fn control_menu(&mut self, sys: &mut Sys, ctx: &Ctx<'_>, items: &[String], opt: &mut usize) -> u8 {
        self.blit_back(sys);
        let n = items.len();
        let base = match n {
            3 => 105,
            4 => 97,
            _ => 90,
        };
        for (i, t) in items.iter().enumerate() {
            let w = ctx.fonts.white.text_width(t);
            let f = if i == *opt { &ctx.fonts.white } else { &ctx.fonts.yellow };
            f.write(&mut sys.screen, (320 - w) / 2, 12 * i as i32 + base, t, 11);
        }
        sys.update();
        sys.frame().await;
        let s = sys.last_scan;
        sys.last_scan = 0;
        match s {
            keys::UARROW | keys::KEYPAD8 => *opt = if *opt == 0 { n - 1 } else { *opt - 1 },
            keys::DARROW | keys::KEYPAD2 => *opt = if *opt + 1 >= n { 0 } else { *opt + 1 },
            keys::PGUP => *opt = opt.saturating_sub(4),
            keys::HOME => *opt = 0,
            keys::PGDN => *opt = (*opt + 4).min(n - 1),
            keys::END => *opt = n - 1,
            _ => {}
        }
        s
    }

    fn save_cfg(sys: &mut Sys, cfg: &Config) {
        sys.storage.save("speed.cfg", &cfg.to_text());
    }

    /// MENU_Run: run menus until one leaves the stack empty.
    pub async fn run(&mut self, sys: &mut Sys, ctx: &mut Ctx<'_>) -> i32 {
        let mut rez = 0;
        while let Some(&m) = self.stack.last() {
            sys.last_scan = 0;
            sys.autorepeat = true;
            rez = match m {
                Menu::Main => self.main_menu(sys, ctx).await,
                Menu::Sound => self.sound_menu(sys, ctx).await,
                Menu::Detail => self.detail_menu(sys, ctx).await,
                Menu::Controls => self.controls_menu(sys, ctx).await,
                Menu::ChooseCirc => self.choose_circ(sys, ctx).await,
                Menu::ChooseCar => self.choose_car(sys, ctx).await,
                Menu::Config => self.config_menu(sys, ctx).await,
                Menu::RaceType => self.race_type_menu(sys, ctx).await,
                Menu::TimeOrLaps => self.time_or_laps_menu(sys, ctx).await,
                Menu::Jukebox => self.jukebox(sys, ctx).await,
                Menu::CarType => self.car_type_menu(sys, ctx).await,
                Menu::Shareware => self.shareware(sys, ctx).await,
                Menu::NetGame => self.net_game_menu(sys, ctx).await,
            };
            sys.autorepeat = false;
        }
        rez
    }

    // ---- Main menu -------------------------------------------------------

    /// The first-time logo animation of MainMenu.
    async fn logo_intro(&mut self, sys: &mut Sys, ctx: &Ctx<'_>) {
        self.init_back(sys, "mbg_prin.pix");
        let mut pal = ctx.game_pal.clone();
        for _ in 0..15 {
            for c in pal.0.iter_mut() {
                if *c < 63 {
                    *c += 1;
                }
            }
        }
        sys.vsync(0).await;
        let x = &ctx.clr[15 * 256..16 * 256];
        let mut back = self.back.clone().unwrap();
        let (mut lm, mut ls, mut bv, mut cv) = (0usize, 20 << 8, 0, 0);
        sys.last_scan = 0;
        while lm < self.logos.len() {
            sys.screen.pix.copy_from_slice(&back);
            if ls > 0 {
                let p = &self.logos[lm];
                let w = fpn_mult(ls + (1 << 8), p.w, 8);
                let h = fpn_mult(ls + (1 << 8), p.h, 8);
                p.draw(&mut sys.screen, LOGO_POS[lm].0, LOGO_POS[lm].1, w, h, x);
            }
            sys.update();
            if sys.last_scan != 0 {
                break;
            }
            let nf = sys.vsync(1).await;
            for _ in 0..nf {
                if lm > 2 && bv < 0x4000 {
                    bv += 100;
                }
                if lm > 5 && cv < 0x4000 {
                    cv += 100;
                }
                if lm < self.logos.len() {
                    ls -= 1 << 9;
                    if ls < 0 {
                        sys.pal = pal.clone();
                        sys.start_fade(Some(ctx.game_pal.clone()), 5);
                        lm += 1;
                        if lm < self.logos.len() {
                            ls = 20 << 8;
                            self.blit_back(sys);
                            for (p, &(lx, ly)) in self.logos.iter().zip(LOGO_POS.iter()).take(lm) {
                                p.draw(&mut sys.screen, lx, ly, p.w, p.h, x);
                            }
                            back.copy_from_slice(&sys.screen.pix);
                            sys.vsync(0).await;
                        }
                    }
                }
            }
        }
        sys.last_scan = 0;
        sys.update();
        self.set_background(sys, ctx);
        sys.pal = Palette([63; 768]);
        sys.start_fade(Some(ctx.game_pal.clone()), 2);
    }

    async fn main_menu(&mut self, sys: &mut Sys, ctx: &mut Ctx<'_>) -> i32 {
        self.no_collide = false;
        // Back here from a network game's race type menu: leave it.
        if let Some(mut n) = sys.net.take() {
            n.bye();
        }
        if self.back.is_none() {
            self.logo_intro(sys, ctx).await;
        }
        let items: Vec<String> = ["New Race", "Multiplayer Race", "Setup", "Quit"].map(String::from).to_vec();
        let mut opt = self.opts[0];
        let mut clock = 0;
        sys.vsync(0).await;
        let noquit = sys.arg("noquit").is_some();
        let ret = loop {
            let key = self.control_menu(sys, ctx, &items, &mut opt).await;
            let can_quit = !noquit || (sys.keys[keys::RIGHTCTRL as usize] && sys.keys[keys::KEYPADMINUS as usize] && sys.keys[keys::F5 as usize]);
            match key {
                keys::ENTER | keys::SPACE | keys::KEYPADENTER => match opt {
                    0 => {
                        if sys.keys[keys::C as usize] {
                            self.no_collide = true;
                        }
                        self.enter(Menu::RaceType);
                        break 0;
                    }
                    1 => {
                        self.enter(Menu::NetGame);
                        break 0;
                    }
                    2 => {
                        self.enter(Menu::Config);
                        break 0;
                    }
                    _ => {
                        if can_quit {
                            self.ret();
                            break MENU_QUIT;
                        }
                    }
                },
                keys::ESC => {
                    if opt == items.len() - 1 && can_quit {
                        self.ret();
                        break MENU_QUIT;
                    }
                    opt = items.len() - 1;
                }
                _ => {}
            }
            if key != 0 {
                clock = 0;
            }
            if sys.keys[keys::I as usize] && sys.keys[keys::G as usize] && sys.keys[keys::N as usize] {
                self.ret();
                break MENU_GREETS;
            }
            clock += sys.vsync(0).await;
            if clock >= 20 * 70 {
                self.ret();
                break MENU_IDLE;
            }
        };
        self.opts[0] = opt;
        ret
    }

    async fn race_type_menu(&mut self, sys: &mut Sys, ctx: &mut Ctx<'_>) -> i32 {
        self.ensure_background(sys, ctx);
        let mut opt = self.opts[1];
        let ret = loop {
            let np = format!("{} Player{}", self.console_players, if self.console_players == 1 { "" } else { "s" });
            let items: Vec<String> = ["Championship", "Single Race", "Practice", &np, "Select race mode...", "Select car model...", "Exit"]
                .map(String::from)
                .to_vec();
            match self.control_menu(sys, ctx, &items, &mut opt).await {
                keys::ENTER | keys::SPACE | keys::KEYPADENTER => match opt {
                    0..=2 => {
                        if opt == 0 && !ctx.registered {
                            self.enter(Menu::Shareware);
                            break 0;
                        }
                        self.ret();
                        self.ret();
                        break opt as i32;
                    }
                    3 => self.console_players = 3 - self.console_players,
                    4 => {
                        self.enter(Menu::TimeOrLaps);
                        break 0;
                    }
                    5 => {
                        self.enter(Menu::CarType);
                        break 0;
                    }
                    _ => {
                        self.ret();
                        break MENU_QUIT;
                    }
                },
                keys::LARROW | keys::KEYPAD4 if opt == 3 => self.console_players = 1,
                keys::RARROW | keys::KEYPAD6 if opt == 3 => self.console_players = 2,
                keys::ESC => {
                    self.ret();
                    break MENU_QUIT;
                }
                _ => {}
            }
        };
        self.opts[1] = opt;
        ret
    }

    async fn time_or_laps_menu(&mut self, sys: &mut Sys, ctx: &mut Ctx<'_>) -> i32 {
        self.ensure_background(sys, ctx);
        let mut nlaps = ctx.cfg.num_laps;
        let mut opt = if ctx.cfg.timed_race { 1 } else { 2 };
        let ret = loop {
            let items = vec![
                format!("Race for {nlaps:2} laps"),
                if ctx.cfg.timed_race { "Race Against Time" } else { "No Time Limit" }.to_string(),
                "Exit".to_string(),
            ];
            match self.control_menu(sys, ctx, &items, &mut opt).await {
                keys::ENTER | keys::SPACE | keys::KEYPADENTER => match opt {
                    1 => ctx.cfg.timed_race = !ctx.cfg.timed_race,
                    2 => break MENU_QUIT,
                    _ => {}
                },
                keys::LARROW | keys::KEYPAD4 | keys::KEYPADMINUS => match opt {
                    0 => nlaps = (nlaps - 1).max(1),
                    1 => ctx.cfg.timed_race = !ctx.cfg.timed_race,
                    _ => {}
                },
                keys::RARROW | keys::KEYPAD6 | keys::KEYPADPLUS => match opt {
                    0 => nlaps = (nlaps + 1).min(20),
                    1 => ctx.cfg.timed_race = !ctx.cfg.timed_race,
                    _ => {}
                },
                keys::ESC => break MENU_QUIT,
                _ => {}
            }
        };
        ctx.cfg.num_laps = nlaps;
        Self::save_cfg(sys, ctx.cfg);
        self.ret();
        ret
    }

    async fn car_type_menu(&mut self, sys: &mut Sys, ctx: &mut Ctx<'_>) -> i32 {
        self.ensure_background(sys, ctx);
        let items: Vec<String> = ["Formula 1", "Stock", "Exit"].map(String::from).to_vec();
        let mut opt = ctx.cfg.car_type.clamp(0, 1) as usize;
        loop {
            match self.control_menu(sys, ctx, &items, &mut opt).await {
                keys::ENTER | keys::SPACE | keys::KEYPADENTER => {
                    if opt < 2 {
                        ctx.cfg.car_type = opt as i32;
                        Self::save_cfg(sys, ctx.cfg);
                    }
                    self.ret();
                    return MENU_QUIT;
                }
                keys::ESC => {
                    self.ret();
                    return MENU_QUIT;
                }
                _ => {}
            }
        }
    }

    async fn shareware(&mut self, sys: &mut Sys, ctx: &mut Ctx<'_>) -> i32 {
        self.ensure_background(sys, ctx);
        sys.last_scan = 0;
        let mut f = 0;
        sys.vsync(0).await;
        while sys.last_scan == 0 {
            self.blit_back(sys);
            if f & 0x10 != 0 {
                let t = "REGISTERED VERSION ONLY";
                let w = ctx.fonts.white.text_width(t);
                ctx.fonts.white.write(&mut sys.screen, (320 - w) / 2, 120, t, 11);
            }
            f += sys.vsync(1).await;
            sys.update();
        }
        self.ret();
        0
    }

    /// NET_GameMenu + StartOrJoinMenu. The original offered IPX, serial
    /// and modem links; the port plays through a relay server (`-relay
    /// URL`). Starting leads on to the race type menu; joining leaves the
    /// menus (the master's choices arrive in NET_ContactPlayers).
    async fn net_game_menu(&mut self, sys: &mut Sys, ctx: &mut Ctx<'_>) -> i32 {
        self.ensure_background(sys, ctx);
        if !sys.can_connect() {
            self.warning(sys, ctx, "No server: use -relay URL").await;
            self.ret();
            return 0;
        }
        let forced = if sys.arg("master").is_some() {
            Some(0)
        } else if sys.arg("slave").is_some() {
            Some(1)
        } else {
            None
        };
        let items: Vec<String> = ["Start Multiplayer Game", "Join Multiplayer Game", "Exit"].map(String::from).to_vec();
        let mut opt = self.opts[15];
        loop {
            let choice = match forced {
                Some(c) => c,
                None => match self.control_menu(sys, ctx, &items, &mut opt).await {
                    keys::ENTER | keys::SPACE | keys::KEYPADENTER if opt < 2 => opt,
                    keys::ENTER | keys::SPACE | keys::KEYPADENTER | keys::ESC => {
                        self.opts[15] = opt;
                        self.ret();
                        return 0;
                    }
                    _ => continue,
                },
            };
            self.opts[15] = opt;
            let Some(pipe) = sys.connect() else { continue };
            let mut net = crate::net::Net::new(pipe, choice == 1);
            if !crate::net::connect(sys, ctx.fonts, &mut net).await {
                if forced.is_some() {
                    self.ret();
                    return 0;
                }
                self.blit_back(sys);
                continue;
            }
            sys.net = Some(net);
            self.ret();
            return if choice == 0 {
                self.enter(Menu::RaceType);
                0
            } else {
                // Quit the main menu: the race type comes from the master.
                self.ret();
                1
            };
        }
    }

    // ---- Setup menus -----------------------------------------------------

    async fn config_menu(&mut self, sys: &mut Sys, ctx: &mut Ctx<'_>) -> i32 {
        self.ensure_background(sys, ctx);
        let items: Vec<String> = ["Sound Options", "Graphics Options", "Set Controls", "Multiplayer...", "Exit"].map(String::from).to_vec();
        let mut opt = self.opts[2];
        let ret = loop {
            match self.control_menu(sys, ctx, &items, &mut opt).await {
                keys::ENTER | keys::SPACE | keys::KEYPADENTER => match opt {
                    0 => {
                        self.enter(Menu::Sound);
                        break 0;
                    }
                    1 => {
                        self.enter(Menu::Detail);
                        break 0;
                    }
                    2 => {
                        self.enter(Menu::Controls);
                        break 0;
                    }
                    3 => {
                        // NET_EditPhones in the original (modem numbers);
                        // the port's network setting is the relay server.
                        let text = match sys.arg_value("relay") {
                            Some(url) if sys.can_connect() => format!("Server {}", url.trim_start_matches("ws://").trim_start_matches("wss://")),
                            _ => "No server: use -relay URL".to_string(),
                        };
                        let text: String = text.chars().take(26).collect();
                        self.warning(sys, ctx, &text).await;
                        break 0;
                    }
                    _ => {
                        self.ret();
                        break MENU_QUIT;
                    }
                },
                keys::ESC => {
                    self.ret();
                    break MENU_QUIT;
                }
                _ => {}
            }
        };
        self.opts[2] = opt;
        ret
    }

    async fn sound_menu(&mut self, sys: &mut Sys, ctx: &mut Ctx<'_>) -> i32 {
        self.ensure_background(sys, ctx);
        let mut opt = self.opts[3];
        let ret = loop {
            let items = vec![
                format!("Music volume:   {}%", ctx.cfg.music_volume * 100 / 511),
                format!("Effects volume: {}%", ctx.cfg.fx_volume * 100 / 511),
                "Jukebox".into(),
                "Exit".into(),
            ];
            let key = self.control_menu(sys, ctx, &items, &mut opt).await;
            let delta = match key {
                keys::LARROW | keys::KEYPAD4 | keys::KEYPADMINUS => -10,
                keys::RARROW | keys::KEYPAD6 | keys::KEYPADPLUS => 10,
                _ => 0,
            };
            if delta != 0 {
                let v = if opt == 0 { &mut ctx.cfg.music_volume } else { &mut ctx.cfg.fx_volume };
                if opt < 2 {
                    *v = if delta < 0 { (*v - 10).max(0) } else if *v < 501 { *v + 10 } else { 511 };
                    let mut a = sys.audio.lock().unwrap();
                    a.cfg_music_volume = ctx.cfg.music_volume;
                    a.cfg_fx_volume = ctx.cfg.fx_volume;
                }
            }
            match key {
                keys::ENTER | keys::SPACE | keys::KEYPADENTER if opt == 2 => {
                    self.enter(Menu::Jukebox);
                    break MENU_QUIT;
                }
                keys::ENTER | keys::SPACE | keys::KEYPADENTER if opt == 3 => {
                    self.ret();
                    break MENU_QUIT;
                }
                keys::ESC => {
                    self.ret();
                    break MENU_QUIT;
                }
                _ => {}
            }
        };
        self.opts[3] = opt;
        Self::save_cfg(sys, ctx.cfg);
        ret
    }

    async fn detail_menu(&mut self, sys: &mut Sys, ctx: &mut Ctx<'_>) -> i32 {
        self.ensure_background(sys, ctx);
        let mut opt = self.opts[4];
        let ret = loop {
            let c = &*ctx.cfg;
            let items = vec![
                format!("Decoration density: {}%", c.decoration_detail * 100 / 16),
                if c.polygon_detail != 0 { "Shaded Polygons" } else { "Flat Polygons" }.into(),
                if c.floor_detail != 0 { "Detailed floor" } else { "Half detail floor" }.into(),
                if c.walls_detail != 0 { "Textured walls" } else { "Flat walls" }.into(),
                format!("Background Depth: {}%", c.background_detail * 10),
                if c.svga_on { "SVGA 640x480" } else { "VGA 320x200" }.into(),
                "Exit".into(),
            ];
            let key = self.control_menu(sys, ctx, &items, &mut opt).await;
            let c = &mut *ctx.cfg;
            let toggle = |v: &mut i32| *v = (*v == 0) as i32;
            match key {
                keys::LARROW | keys::KEYPAD4 | keys::KEYPADMINUS | keys::RARROW | keys::KEYPAD6 | keys::KEYPADPLUS => {
                    let up = matches!(key, keys::RARROW | keys::KEYPAD6 | keys::KEYPADPLUS);
                    match opt {
                        0 => c.decoration_detail = if up { (c.decoration_detail + 1).min(16) } else { (c.decoration_detail - 1).max(0) },
                        1 => toggle(&mut c.polygon_detail),
                        2 => toggle(&mut c.floor_detail),
                        3 => toggle(&mut c.walls_detail),
                        4 => c.background_detail = if up { (c.background_detail + 1).min(10) } else { (c.background_detail - 1).max(0) },
                        5 => c.svga_on = !c.svga_on,
                        _ => {}
                    }
                }
                keys::ENTER | keys::SPACE | keys::KEYPADENTER => match opt {
                    1 => toggle(&mut c.polygon_detail),
                    2 => toggle(&mut c.floor_detail),
                    3 => toggle(&mut c.walls_detail),
                    5 => c.svga_on = !c.svga_on,
                    6 => {
                        self.ret();
                        break MENU_QUIT;
                    }
                    _ => {}
                },
                keys::ESC => {
                    self.ret();
                    break MENU_QUIT;
                }
                _ => {}
            }
        };
        self.opts[4] = opt;
        Self::save_cfg(sys, ctx.cfg);
        ret
    }

    /// ShowWarning: a framed message; returns true if ESC (ignore) was hit.
    async fn warning(&mut self, sys: &mut Sys, ctx: &Ctx<'_>, msg: &str) -> bool {
        let x = sys.screen.w as i32 / 2 - 160;
        let y = sys.screen.h as i32 / 2 - 100;
        let s = &mut sys.screen;
        s.rectangle(x + 20, y + 70, 280, 60, 15, -1);
        s.rectangle(x + 21, y + 71, 278, 58, 7, -1);
        s.rectangle(x + 22, y + 72, 276, 56, 8, -1);
        s.rectangle(x + 23, y + 73, 274, 54, 7, 8);
        ctx.fonts.yellow.write(s, x + 25, y + 75, "Attention", 15);
        ctx.fonts.yellow.write(s, x + 40, y + 115, "ESC to ignore", 15);
        ctx.fonts.white.write(s, x + 26, y + 95, msg, 15);
        sys.update();
        let k = sys.wait_key().await;
        sys.last_scan = 0;
        k == keys::ESC
    }

    /// DefineKeys: ask for the five keys of player `n`.
    async fn define_keys(&mut self, sys: &mut Sys, ctx: &mut Ctx<'_>, n: usize) {
        const TEXTS: [&str; 5] = ["Accelerate", "Brake", "Turn left", "Turn right", "Change Gear"];
        let mut pk = if n == 0 { ctx.cfg.p1_keys } else { ctx.cfg.p2_keys };
        sys.autorepeat = false;
        for o in 0..5 {
            self.blit_back(sys);
            let f = ctx.fonts;
            let t = format!("Define keys - Player {}", n + 1);
            let w = f.red.text_width("Define keys - Player 1");
            f.red.write(&mut sys.screen, (320 - w) / 2, 90, &t, 11);
            let w = f.red.text_width("ESC to Exit");
            f.red.write(&mut sys.screen, (320 - w) / 2, 160, "ESC to Exit", 11);
            for (i, label) in TEXTS.iter().enumerate() {
                let kn = key_label(pk[i]);
                let font = if i == o { &f.white } else { &f.yellow };
                let w = f.white.text_width(&kn);
                font.write(&mut sys.screen, 40, 103 + 10 * i as i32, label, 11);
                font.write(&mut sys.screen, 280 - w, 103 + 10 * i as i32, &kn, 11);
            }
            sys.update();
            let k = sys.wait_key().await;
            if k == keys::ESC {
                sys.autorepeat = true;
                sys.last_scan = 0;
                return;
            }
            pk[o] = k;
        }
        if n == 0 {
            ctx.cfg.p1_keys = pk;
        } else {
            ctx.cfg.p2_keys = pk;
        }
        sys.autorepeat = true;
        sys.last_scan = 0;
    }

    async fn controls_menu(&mut self, sys: &mut Sys, ctx: &mut Ctx<'_>) -> i32 {
        // Saved as the original's Joystick A/B; see pad.rs for the defaults.
        const CTLS: [&str; 3] = ["Keyboard", "Gamepad 1", "Gamepad 2"];
        self.ensure_background(sys, ctx);
        let mut opt = self.opts[5];
        let ret = loop {
            let items = vec![
                format!("Player 1 - {}", CTLS[ctx.cfg.p1_control.min(2) as usize]),
                format!("Player 2 - {}", CTLS[ctx.cfg.p2_control.min(2) as usize]),
                "Player 1 - Define Keys".into(),
                "Player 2 - Define Keys".into(),
                // The original's "Calibrate Joystick A/B" entries are gone:
                // gamepads need no calibration.
                "Exit".into(),
            ];
            let key = self.control_menu(sys, ctx, &items, &mut opt).await;
            let c = &mut *ctx.cfg;
            match key {
                keys::LARROW | keys::KEYPAD4 | keys::KEYPADMINUS => match opt {
                    0 => c.p1_control = c.p1_control.saturating_sub(1),
                    1 => c.p2_control = c.p2_control.saturating_sub(1),
                    _ => {}
                },
                keys::RARROW | keys::KEYPAD6 | keys::KEYPADPLUS => match opt {
                    0 => c.p1_control = (c.p1_control + 1).min(2),
                    1 => c.p2_control = (c.p2_control + 1).min(2),
                    _ => {}
                },
                keys::ENTER | keys::SPACE | keys::KEYPADENTER => match opt {
                    2 | 3 => self.define_keys(sys, ctx, opt - 2).await,
                    4 => {
                        if self.check_controls(sys, ctx).await {
                            self.ret();
                            break MENU_QUIT;
                        }
                    }
                    _ => {}
                },
                keys::ESC => {
                    if self.check_controls(sys, ctx).await {
                        self.ret();
                        break MENU_QUIT;
                    }
                }
                _ => {}
            }
        };
        self.opts[5] = opt;
        Self::save_cfg(sys, ctx.cfg);
        ret
    }

    /// CheckControls: warn about duplicated keys.
    async fn check_controls(&mut self, sys: &mut Sys, ctx: &Ctx<'_>) -> bool {
        let all: Vec<u8> = ctx.cfg.p1_keys.iter().chain(ctx.cfg.p2_keys.iter()).copied().collect();
        let dup = (0..all.len()).any(|i| all[i + 1..].contains(&all[i]));
        if dup {
            return self.warning(sys, ctx, "Some keys are duplicated").await;
        }
        true
    }

    // ---- Jukebox ---------------------------------------------------------

    async fn jukebox(&mut self, sys: &mut Sys, ctx: &mut Ctx<'_>) -> i32 {
        let mut songs: Vec<i32> = ctx.cfg.songs.clone();
        sys.fade.rgb = (63, 63, 63);
        sys.start_fade(None, 3);
        sys.fade.rgb = (63, 63, 63);
        sys.last_scan = 0;
        sys.wait_fade(true).await;
        sys.fade.pos = 0;
        sys.last_scan = 0;
        self.init_back(sys, "mbg_juke.pix");
        self.blit_back(sys);
        sys.update();
        sys.start_full_fade(Some(ctx.game_pal.clone()), 2);
        let (mut cx, mut cy) = self.jukebox_cursor;
        let mut l = 0;
        sys.vsync(0).await;
        sys.last_scan = 0;
        loop {
            sys.last_scan = 0;
            self.blit_back(sys);
            let y = &ctx.fonts.yellow;
            for (i, name) in SONG_NAMES.iter().enumerate() {
                if cx == 1 || cy != i || l & 0x10 != 0 {
                    y.write(&mut sys.screen, 40, 84 + 13 * i as i32, name, 0);
                }
            }
            for (i, &s) in songs.iter().enumerate() {
                let p = if s >= 0 { SONG_NAMES[s as usize % 8] } else { "-------" };
                if cx == 0 || cy != i || l & 0x10 != 0 {
                    y.write(&mut sys.screen, 184, 84 + 13 * i as i32, p, 0);
                }
            }
            let help = if cx == 0 { "Enter to add - arrows to move" } else { "Enter to delete - arrows to move" };
            let w = ctx.fonts.white.text_width(help);
            ctx.fonts.white.write(&mut sys.screen, (320 - w) / 2, 188, help, 1);
            sys.update();
            l += sys.vsync(1).await;
            let n = songs.len();
            match sys.last_scan {
                keys::UARROW | keys::KEYPAD8 => cy = cy.saturating_sub(1),
                keys::DARROW | keys::KEYPAD2 => {
                    if cy < 7 && (cx == 0 || cy + 1 < n) {
                        cy += 1;
                    }
                }
                keys::LARROW | keys::KEYPAD4 => cx = cx.saturating_sub(1),
                keys::RARROW | keys::KEYPAD6 => {
                    if cx < 1 && n > 0 {
                        cy = cy.min(n - 1);
                        cx += 1;
                    }
                }
                keys::ENTER | keys::SPACE | keys::KEYPADENTER => {
                    if cx == 1 {
                        if cy < n {
                            songs.remove(cy);
                        }
                        if cy >= songs.len() {
                            if cy > 0 {
                                cy -= 1;
                            } else {
                                cx = 0;
                            }
                        }
                    } else {
                        sys.done_music();
                        sys.init_music(Some(&format!("haste{}.s3m", cy + 1))).await;
                        if !songs.contains(&(cy as i32)) {
                            songs.push(cy as i32);
                        }
                    }
                }
                keys::ESC => break,
                _ => {}
            }
        }
        self.jukebox_cursor = (cx, cy);
        sys.fade.pos = 0;
        sys.pal = ctx.game_pal.clone();
        sys.vsync(0).await;
        sys.vsync(1).await;
        sys.last_scan = 0;
        for _ in 0..4 {
            self.ret();
        }
        self.end();
        ctx.cfg.songs = songs;
        Self::save_cfg(sys, ctx.cfg);
        MENU_REINIT
    }

    // ---- Circuit and car selection ----------------------------------------

    /// ChooseCircMenu: returns the circuit's map number or -1.
    async fn choose_circ(&mut self, sys: &mut Sys, ctx: &mut Ctx<'_>) -> i32 {
        let circuits = models::load_circuits(&sys.data, ctx.cfg.car_type);
        if let Some(i) = sys.arg_value("track").and_then(|v| v.parse::<usize>().ok()) {
            let i = if !ctx.registered && i > 2 { 1 } else { i };
            if (1..=circuits.len()).contains(&i) {
                self.ret();
                return circuits[i - 1].nmap;
            }
        }
        let sprs: Vec<Sprite> = circuits.iter().filter_map(|c| Sprite::load(&sys.data, &format!("mapspr{:02}.is2", c.nmap)).ok()).collect();
        if sprs.len() != circuits.len() || circuits.is_empty() {
            self.ret();
            return -1;
        }
        sys.fade_out(2).await;
        let voice = sys.audio.lock().unwrap().load_effect(&sys.data, "circuit.raw", 13000, false);
        self.init_back(sys, "mbg_circ.pix");
        self.blit_back(sys);
        for i in 48..152 {
            for j in 180..300 {
                let p = &mut sys.screen.pix[i * 320 + j];
                *p = ctx.clr[22 * 256 + *p as usize];
            }
        }
        if let Some(bar) = sys.data.get("mscbar.pix") {
            for (i, &c) in bar.iter().enumerate() {
                if c != 0 && 3200 + i < sys.screen.pix.len() {
                    sys.screen.pix[3200 + i] = c;
                }
            }
        }
        let f = ctx.fonts;
        for (t, y) in [("Arrow Keys to select", 170), ("Enter to confirm", 185)] {
            let w = f.yellow.text_width(t);
            f.yellow.write(&mut sys.screen, (320 - w) / 2, y, t, 1);
        }
        self.back = Some(sys.screen.pix.clone());
        sys.update();
        let level: Vec<Sprite> = ["msfbeg.is2", "msfama.is2", "msfpro.is2"].iter().filter_map(|n| Sprite::load(&sys.data, n).ok()).collect();
        sys.start_full_fade(Some(ctx.game_pal.clone()), 4);
        sys.audio.lock().unwrap().eff_start(-1, 256, 350, 0, voice);
        sys.vsync(0).await;
        let step = sprs[0].h + 20;
        let mut sel = self.sel_circ.min(circuits.len() - 1);
        let mut cpos = step * sel as i32;
        let mut vel = 0;
        let mut frames = 0;
        sys.last_scan = 0;
        let key = loop {
            sys.last_scan = 0;
            self.blit_back(sys);
            let dest = step * sel as i32;
            let clip = sys.screen.clip;
            sys.screen.clip = Rect { min_y: 48, max_y: 152, ..clip };
            let ident: Vec<u8> = (0..=255).collect();
            for (i, sp) in sprs.iter().enumerate() {
                sp.draw(&mut sys.screen, 240 - sp.w / 2, 60 - cpos + step * i as i32, sp.w, sp.h, &ident);
            }
            sys.screen.clip = clip;
            let c = &circuits[sel];
            let w = f.white.text_width(&c.name);
            let x = if (180 - w) / 2 < 0 { 3 } else { (180 - w) / 2 };
            f.yellow.write(&mut sys.screen, x, 48, &c.name, 4);
            f.white.write(&mut sys.screen, 20, 107, "Length", 15);
            f.yellow.write(&mut sys.screen, 100, 107, &format!("{}.{:03}", c.length / 1000, c.length % 1000), 15);
            f.white.write(&mut sys.screen, 20, 120, "Best", 15);
            let t = |t: u32| format!("{}'{:02}\"{:02}", t / 70 / 60, (t / 70) % 60, t % 70 * 100 / 70);
            f.yellow.write(&mut sys.screen, 100, 120, &t(c.best as u32), 15);
            f.white.write(&mut sys.screen, 20, 133, "Record", 15);
            let rt = ctx.rec.circ.get(sel).map_or(0, |r| r.time[ctx.cfg.car_type.clamp(0, 1) as usize]);
            f.yellow.write(&mut sys.screen, 100, 133, &t(rt), 15);
            if let Some(lv) = level.get(c.level as usize) {
                lv.draw_horizontal(&mut sys.screen, 42, 80);
            }
            if sel >= 2 && !ctx.registered && frames & 0x20 != 0 {
                f.white.write(&mut sys.screen, 10, 150, "Registered Version", 15);
            }
            sys.update();
            let nf = sys.vsync(1).await;
            frames += nf;
            for _ in 0..nf {
                if cpos < dest {
                    vel = if vel < 0 { 0 } else { (vel + 1).min(7) };
                } else if cpos > dest {
                    vel = if vel > 0 { 0 } else { (vel - 1).max(-7) };
                } else {
                    vel = 0;
                }
                cpos += vel;
            }
            match sys.last_scan {
                keys::LARROW | keys::KEYPAD4 | keys::UARROW | keys::KEYPAD8 => sel = sel.saturating_sub(1),
                keys::RARROW | keys::KEYPAD6 | keys::DARROW | keys::KEYPAD2 => sel = (sel + 1).min(circuits.len() - 1),
                keys::ENTER | keys::SPACE | keys::KEYPADENTER if sel < 2 || ctx.registered => break keys::ENTER,
                keys::ESC => break keys::ESC,
                _ => {}
            }
        };
        {
            let mut a = sys.audio.lock().unwrap();
            a.eff_stop_all();
            a.unload_effect(voice);
        }
        self.sel_circ = sel;
        sys.fade.pos = 0;
        sys.vsync(0).await;
        sys.vsync(1).await;
        sys.last_scan = 0;
        self.ret();
        if key == keys::ESC { -1 } else { circuits[sel].nmap }
    }

    /// ChooseCarMenu: returns the chosen model's mesh number or -1.
    async fn choose_car(&mut self, sys: &mut Sys, ctx: &mut Ctx<'_>) -> i32 {
        let ct = ctx.cfg.car_type;
        let cars: Vec<CarModel> = models::load_cars(&sys.data, ct);
        if let Some(i) = sys.arg_value("car").and_then(|v| v.parse::<usize>().ok()) {
            let i = if !ctx.registered { 1 } else { i };
            if (1..=6).contains(&i) && i <= cars.len() {
                self.ret();
                return cars[i - 1].n3d;
            }
        }
        // MDL_LoadCars: meshes scaled by 1/32 and centred on their bounds.
        let mut meshes = Vec::new();
        for c in &cars {
            let m = Mesh::load(&sys.data, &format!("car{ct}n{}.i3d", c.n3d)).or_else(|_| Mesh::load(&sys.data, &format!("car{ct}n{}a.i3d", c.n3d)));
            let Ok(mut m) = m else {
                self.ret();
                return -1;
            };
            for s in m.sc.iter_mut() {
                if *s == 0 {
                    *s = 1 << 19;
                }
                *s >>= 5;
            }
            for k in 0..3 {
                let (lo, hi) = m.verts.iter().fold((i32::MAX, i32::MIN), |(a, b), v| (a.min(v[k]), b.max(v[k])));
                m.dc[k] = (hi + lo) / 2;
            }
            for v in m.verts.iter_mut() {
                for k in 0..3 {
                    v[k] = fpn_mult(v[k] - m.dc[k], m.sc[k], 16);
                }
            }
            meshes.push(m);
        }
        sys.fade_out(2).await;
        let (voice, vauto, vmanu) = {
            let mut a = sys.audio.lock().unwrap();
            (
                a.load_effect(&sys.data, "vihecul.raw", 13000, false),
                a.load_effect(&sys.data, "automati.raw", 13000, false),
                a.load_effect(&sys.data, "manual.raw", 13000, false),
            )
        };
        self.init_back(sys, "mbg_car.pix");
        self.blit_back(sys);
        if let Some(bar) = sys.data.get("msfbar.pix") {
            for (i, &c) in bar.iter().enumerate() {
                if c != 0 && 3200 + i < sys.screen.pix.len() {
                    sys.screen.pix[3200 + i] = c;
                }
            }
        }
        let f = ctx.fonts;
        for (t, y) in [("Arrow Keys to select", 170), ("Enter to confirm", 185)] {
            let w = f.yellow.text_width(t);
            f.yellow.write(&mut sys.screen, (320 - w) / 2, y, t, 1);
        }
        f.white.write(&mut sys.screen, 15, 6, &format!("Player {}", self.car_player + 1), 0);
        let ld = |n: &str| Sprite::load(&sys.data, n).ok();
        let level: Vec<Sprite> = ["msfbeg.is2", "msfama.is2", "msfpro.is2"].iter().filter_map(|n| ld(n)).collect();
        let manual: Vec<Sprite> = ["msfaut.is2", "msfman.is2"].iter().filter_map(|n| ld(n)).collect();
        let maxspeed = ld("msfmax.is2");
        self.back = Some(sys.screen.pix.clone());
        sys.update();
        sys.start_full_fade(Some(ctx.game_pal.clone()), 2);
        sys.audio.lock().unwrap().eff_start(-1, 256, 350, 0, voice);
        let n = cars.len() as i32;
        let lx = -1000 * (n - 1) / 2;
        let (focus, center) = ((256, 256 * 5 / 6), (sys.screen.w as i32 / 2, 30));
        sys.vsync(0).await;
        let mut selpos = 0;
        let mut sel = self.sel_car.min(cars.len() - 1);
        let mut selrot = sys.rng.next() as u16;
        let mut first = true;
        let mut frames = 0;
        let mut vozch = -1;
        let mut back = Vec::new();
        sys.autorepeat = false;
        sys.last_scan = 0;
        let key = loop {
            sys.last_scan = 0;
            if selpos == 0 {
                self.blit_back(sys);
                let c = &cars[sel];
                let w = f.red.text_width(&c.name);
                f.red.write(&mut sys.screen, (320 - w) / 2, 38, &c.name, 4);
                if let Some(lv) = level.get(c.level as usize) {
                    lv.draw_horizontal(&mut sys.screen, 208, 93);
                }
                let lh = level.first().map_or(0, |s| s.h);
                let mh = manual.first().map_or(0, |s| s.h);
                if let Some(m) = manual.get(c.automatic as usize) {
                    m.draw_horizontal(&mut sys.screen, 160, 100 + lh);
                }
                if let Some(ms) = &maxspeed {
                    ms.draw_horizontal(&mut sys.screen, 200, 100 + lh + mh + 1);
                }
                f.white.write(&mut sys.screen, 220, 100 + lh + mh + 1 + 11, &c.max_speed.to_string(), 15);
                for (i, m) in meshes.iter().enumerate() {
                    if i != sel {
                        self.renderer.draw_object(&mut sys.screen, ctx.clr, m, 0, [lx + 900 * i as i32, -900, 5000], focus, center);
                    }
                }
                back = sys.screen.pix.clone();
                sys.vsync(0).await;
            } else {
                sys.screen.pix.copy_from_slice(&back);
            }
            selpos = selpos.min(40);
            let interp = |base: i32, dest: i32| base + fp_mult_div(dest - base, 40 - selpos, 40);
            let pos = [interp(-550, lx + 900 * sel as i32), interp(-800, -900), interp(2000, 5000)];
            let rot = interp(selrot as i16 as i32, 0) as u16;
            self.renderer.draw_object(&mut sys.screen, ctx.clr, &meshes[sel], rot, pos, focus, center);
            if sel != 0 && !ctx.registered && frames & 0x20 != 0 {
                f.white.write(&mut sys.screen, 10, 145, "Registered Version", 15);
            }
            sys.update();
            let nf = sys.vsync(1).await as i32;
            frames += nf;
            if selpos >= 40 {
                selrot = selrot.wrapping_add((nf * 112) as u16);
            }
            if selpos < 39 && selpos + nf >= 39 {
                if first {
                    first = false;
                } else {
                    let v = if cars[sel].automatic { vmanu } else { vauto };
                    vozch = sys.audio.lock().unwrap().eff_start(vozch, 256, 350, 0, v);
                }
            }
            selpos += nf;
            match sys.last_scan {
                keys::LARROW | keys::KEYPAD4 => {
                    sel = if sel == 0 { cars.len() - 1 } else { sel - 1 };
                    first = false;
                    selpos = 0;
                    selrot = sys.rng.next() as u16;
                }
                keys::RARROW | keys::KEYPAD6 => {
                    sel = if sel + 1 >= cars.len() { 0 } else { sel + 1 };
                    first = false;
                    selpos = 0;
                    selrot = sys.rng.next() as u16;
                }
                keys::ENTER | keys::SPACE | keys::KEYPADENTER if sel == 0 || ctx.registered => break keys::ENTER,
                keys::ESC => break keys::ESC,
                _ => {}
            }
        };
        sys.autorepeat = true;
        {
            let mut a = sys.audio.lock().unwrap();
            a.eff_stop_all();
            for v in [voice, vauto, vmanu] {
                a.unload_effect(v);
            }
        }
        self.sel_car = sel;
        sys.fade.pos = 0;
        sys.pal = ctx.game_pal.clone();
        sys.vsync(0).await;
        sys.vsync(1).await;
        sys.last_scan = 0;
        self.ret();
        if key == keys::ESC { -1 } else { cars[sel].n3d }
    }
}

/// KEYN_FindKey-style display name for a scancode.
pub fn key_label(sc: u8) -> String {
    match sc {
        keys::ENTER => "Return".into(),
        keys::SPACE => "Space".into(),
        keys::UARROW => "Up".into(),
        keys::DARROW => "Down".into(),
        keys::LARROW => "Left".into(),
        keys::RARROW => "Right".into(),
        _ => match crate::records::key_char(sc) {
            Some(c) => c.to_string(),
            None => keys::name(sc).map_or_else(|| format!("{sc:02X}"), |n| n.replace("KEYPAD", "Pad ").replace("LEFT", "Left ").replace("RIGHT", "Right ")),
        },
    }
}
