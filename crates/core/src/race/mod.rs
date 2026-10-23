//! A race (game/race.c): setup, the 70 Hz simulation loop, cameras and
//! rendering of the views.

pub mod control;
pub mod hud;
pub mod map;
pub mod render;
pub mod robot;
pub mod world;

pub use render::scene::{OV_DARK, OV_DARK_LEVEL, OV_NONE, OV_OPAQUE, Scene, SceneIndex, SceneStatic, SceneTexture, SceneVertex, SceneView, TEXEL_CLEAR};

use std::collections::HashMap;
use std::sync::Arc;

use control::{InputConfig, MAX_USERCONTROLS, PLYF_ISCONSOLE, PLYF_SLIDING, Player, Status, UserControl};
use map::{MapThing, StartPos, StaticCamera};
use render::{CAMF_STATIC, CamData, Camera, Geometry, Renderer};
use world::*;

use crate::config::Config;
use crate::data::Result;
use crate::fx::*;
use crate::gfx::Screen;
use crate::is2::Sprite;
use crate::keys;
use crate::models::{CarModel, Circuit};
use crate::sys::Sys;

/// Race modes as passed by the menus.
pub const MODE_CHAMPIONSHIP: i32 = 0;
pub const MODE_SINGLE: i32 = 1;
pub const MODE_PRACTICE: i32 = 2;
pub const MODE_DEMO: i32 = 3;

pub const RACERF_ISCONSOLE: u32 = 0x0001;
pub const RACERF_NOTPRESENT: u32 = 0x8000;

#[derive(Clone, Copy, Debug, Default)]
pub struct RaceResult {
    pub nlaps: i32,
    pub time: u32,
    pub best: u32,
    pub flags: u32,
    pub pos: i32,
}

// ---------------------------------------------------------------------------
// Standard cameras (game/3dfloor.c)

const fn cam(radius: i32, h: i32, focus: i32, horizon: i32, flags: u16) -> CamData {
    CamData { radius, h, focus, horizon, flags }
}

pub const STDCAM_NORMAL: usize = 0;
pub const STDCAM_LOW: usize = 1;
pub const STDCAM_HIGH: usize = 2;
pub const STDCAM_STATIC: usize = 3;

const CAMS_1P: [CamData; 4] = [
    cam(20165824, 1848, 3136, 1, 0),
    cam(0, 1100, 0x0BE0, 1, 0),
    cam(40137344, 2976, 3200, 1, 0),
    cam(0, 13920, 0xA80, 8, CAMF_STATIC),
];
const CAMS_2P: [CamData; 4] = [
    cam(28000000, 1720, 0xA00, 1, 0),
    cam(0, 1050, 0x0B00, 1, 0),
    cam(45000000, 2770, 0xA00, 1, 0),
    cam(0, 5568, 0xA00, 4, CAMF_STATIC),
];

/// GL_TViewInfo: a camera following a thing, with smooth transitions.
#[derive(Clone, Copy, Default)]
struct View {
    c: Camera,
    from: CamData,
    to: CamData,
    change_pos: i32,
    thn: ThingId,
}

// ---------------------------------------------------------------------------
// Thing creation and update

/// THN_AddThing: create a thing of `ty` with its type-specific data.
pub fn add_thing(w: &mut World, x: u32, y: u32, z: u32, ty: u32, angle: u16, flags: u16) -> Option<ThingId> {
    let id = w.new_thing(x, y, z, ty, angle);
    match ty & 0xFF00 {
        THNT_PLAYER | THNT_CAR => {
            let name = format!("car{}n{}", w.set.car_type, (ty & 0xFF) >> 4);
            // Sprites were preloaded by the race setup.
            let spr = *w.sprite_names.get(&name.to_ascii_uppercase())?;
            if ty & 0xFF00 == THNT_PLAYER {
                control::new_player(w, id, flags, spr);
            } else {
                robot::new_car(w, id, spr);
            }
            w.make_thinker(id);
        }
        THNT_SPARKS => {
            let (clock, spr) = (w.game_clock, w.sparks[0]);
            let t = w.thing_mut(id);
            t.kind = Kind::Sparks(Sparks { clock, dx: 0, dy: 0 });
            t.spr = Some(spr);
            w.make_thinker(id);
        }
        THNT_SMOKE => {
            let clock = w.game_clock;
            let spr = w.smoke[6 * (ty & 3) as usize];
            let t = w.thing_mut(id);
            t.kind = Kind::Smoke(Smoke { clock, dx: 0, dy: 0, dz: 0 });
            t.spr = Some(spr);
            w.make_thinker(id);
        }
        THNT_RACEPOS | THNT_HUMANPOS => {
            let name = if ty & 0xFF00 == THNT_RACEPOS { format!("racepos{}", ty & 7) } else { format!("humansp{}", (ty & 7) + 1) };
            w.thing_mut(id).spr = w.sprite_names.get(&name.to_ascii_uppercase()).copied();
        }
        _ => {
            let t = ty.wrapping_sub(THNT_OBSTACLE);
            let t = ((t & 0xFF00) >> 4) + (t & 0xF);
            w.thing_mut(id).spr = w.sprite_names.get(&format!("XPR{t:03X}")).copied();
        }
    }
    Some(id)
}

/// THN_RunThings: run every thinker once per tick.
pub fn run_things(w: &mut World) {
    w.magic = w.magic.wrapping_add(1);
    let magic = w.magic;
    for id in w.thinker_list.snapshot() {
        if !w.alive(id) || w.thing(id).magic == magic {
            continue;
        }
        run_thing(w, id, magic);
    }
}

fn run_thing(w: &mut World, id: ThingId, magic: u32) {
    let t = w.thing_mut(id);
    t.magic = magic;
    let mut kind = std::mem::replace(&mut t.kind, Kind::None);
    let keep = match &mut kind {
        Kind::Player(p) => {
            control::run(w, id, p, |w, id| w.order.first().is_some_and(|&r| w.racers[r].thn == id));
            true
        }
        Kind::Car(c) => {
            robot::run(w, id, c);
            true
        }
        Kind::Sparks(s) => run_particle(w, id, s.clock, &mut s.dx, &mut s.dy, 0, |w, c| w.sparks[c * 6 / 50]),
        Kind::Smoke(s) => {
            let base = 6 * (w.thing(id).ty & 3) as usize;
            run_particle(w, id, s.clock, &mut s.dx, &mut s.dy, s.dz, |w, c| w.smoke[base + c * 18 / 3 / 50])
        }
        Kind::None => true,
    };
    if w.alive(id) {
        w.thing_mut(id).kind = kind;
        if !keep {
            w.delete_thing(id);
        }
    }
}

/// THN_SparksFunc / THN_SmokeFunc: drift, slow down, animate, die after 50.
fn run_particle(w: &mut World, id: ThingId, clock: u32, dx: &mut i32, dy: &mut i32, dz: i32, frame: impl Fn(&World, usize) -> SpriteId) -> bool {
    if w.paused {
        return true;
    }
    let age = w.game_clock.wrapping_sub(clock) as usize;
    let (x, y) = (w.thing(id).x, w.thing(id).y);
    w.thing_mut(id).z = w.thing(id).z.wrapping_add(dz as u32);
    w.move_thing(id, x.wrapping_add(*dx as u32), y.wrapping_add(*dy as u32));
    *dx -= *dx / 16;
    *dy -= *dy / 16;
    if age < 50 {
        let f = frame(w, age);
        w.thing_mut(id).spr = Some(f);
        true
    } else {
        false
    }
}

fn player(w: &World, id: ThingId) -> &Player {
    match &w.thing(id).kind {
        Kind::Player(p) => p,
        _ => panic!("not a player"),
    }
}

fn player_view(w: &World, id: ThingId) -> hud::PlayerView<'_> {
    let pos = w.order.iter().position(|&r| w.racers[r].thn == id).unwrap_or(0) as i32 + 1;
    hud::PlayerView { p: player(w, id), pos, total: w.order.len() as i32 }
}

fn player_mut(w: &mut World, id: ThingId) -> &mut Player {
    match &mut w.thing_mut(id).kind {
        Kind::Player(p) => p,
        _ => panic!("not a player"),
    }
}

// ---------------------------------------------------------------------------

/// Everything a race needs besides the [`Sys`].
pub struct RaceSetup<'a> {
    pub cfg: &'a Config,
    pub mode: i32,
    pub circuit: usize,
    /// Car model per console player (1 or 2 players), or per node in a
    /// network game.
    pub cars: Vec<usize>,
    /// Network game: our node (whose player is the console player).
    pub net_node: Option<usize>,
    /// Network game: every node's keys (up, down, left, right, gear).
    pub net_keys: Vec<[u8; 5]>,
    pub seed: u32,
    pub circuits: &'a [Circuit],
    /// GamePal, set when the race starts.
    pub pal: &'a crate::gfx::Palette,
}

struct Race {
    w: World,
    r: Renderer,
    hud: hud::Hud,
    clr: Vec<u8>,
    g: Geometry,
    mode: i32,
    humans: Vec<ThingId>,
    console1: Option<ThingId>,
    console2: Option<ThingId>,
    robots: Vec<ThingId>,
    race_pos: Vec<ThingId>,
    human_pos: Vec<ThingId>,
    views: [View; 2],
    cams: &'static [CamData; 4],
    split: bool,
    view_hud: [bool; 2],
    console_input: [u8; MAX_USERCONTROLS],
    console_clock: u32,
    quitting: i32,
    tst_angle: u16,
    view_car: i32,
    advance_target: i32,
    close_car: [usize; 2],
    pos_dots: Vec<Sprite>,
    /// Pad buttons held at the previous input tick (for presses).
    pad_prev: [u32; 2],
    /// Camera (F1-F4 / F5-F8 index) of each view, for the pad's cycling.
    cam_sel: [usize; 2],
    /// GPU scene data, built on the first GPU-drawn frame.
    scene: Option<(Arc<SceneStatic>, SceneIndex)>,
    /// Network game state.
    net: Option<NetRace>,
}

/// RACE_HandleComms state: human `n` is node `n`'s player.
struct NetRace {
    me: usize,
    /// Our player's controls sampled but not sent yet.
    outbox: Vec<UserControl>,
    sent: u32,
    /// Per node: controls received but not yet in the player's ring (it
    /// holds MAX_USERCONTROLS ticks), and the number of ticks received.
    pending: Vec<std::collections::VecDeque<UserControl>>,
    received: Vec<u32>,
    /// Our state hashes by tick; the others' that came first.
    mine: HashMap<u32, u32>,
    theirs: Vec<(u32, u32)>,
    desync: bool,
    lost: bool,
    /// Frames the simulation has been waiting for remote players.
    stalled: u32,
}

/// Ticks between desync checks.
const NET_CHECK: u32 = 35;

/// A hash of the simulation state, for desync checks.
fn state_hash(w: &World) -> u32 {
    let mut h = 0x811C_9DC5u32;
    let mut mix = |x: u32| h = (h ^ x).wrapping_mul(0x0100_0193);
    for s in w.rng.state() {
        mix(s);
    }
    for r in &w.racers {
        let t = w.thing(r.thn);
        for x in [t.x, t.y, t.z, t.angle as u32, r.npoint as u32, r.nlap as u32] {
            mix(x);
        }
    }
    h
}

impl Race {
    /// InitData: load the circuit and create every racer.
    fn new(sys: &mut Sys, st: &RaceSetup) -> Result<Race> {
        let d = sys.data.clone();
        let cfg = st.cfg;
        let car_type = cfg.car_type;
        let models: Vec<CarModel> = crate::models::load_cars(&d, car_type);
        let circ = &st.circuits[st.circuit];
        let record = circ.record;
        let race_time = [(record + 15) * 70, record * 70, (record - 2) * 70, (record - 4) * 70, (record - 5) * 70];

        let sec = map::SectorMap::load(&d, &format!("map{:02}.sec", st.circuit))?;
        let (floor, bg, things) = map::load_map(&d, &format!("map{:02}.dat", st.circuit), circ.nback)?;
        let nsec = sec.secs.len();
        let audio = sys.audio.clone();
        let mut w = World {
            set: Settings {
                car_type,
                timed_race: cfg.timed_race,
                race_laps: cfg.num_laps,
                race_time,
                decoration_detail: cfg.decoration_detail,
                floor_detail: cfg.floor_detail != 0,
                walls_detail: cfg.walls_detail != 0,
                polygon_detail: cfg.polygon_detail != 0,
                background_detail: cfg.background_detail,
                no_collide: false,
            },
            rng: std::mem::take(&mut sys.rng),
            audio,
            sfx: Sfx::default(),
            floor,
            bg,
            sec,
            path: Vec::new(),
            start_pos: Vec::new(),
            static_cams: Vec::new(),
            car_models: models.clone(),
            things: Vec::new(),
            thing_list: List::default(),
            thinker_list: List::default(),
            blocks: vec![List::default(); 64 * 64],
            sec_lists: vec![List::default(); nsec],
            sprites: Vec::new(),
            sprite_names: HashMap::new(),
            sparks: [0; 6],
            smoke: [0; 18],
            racers: Vec::new(),
            order: Vec::new(),
            magic: 0,
            game_clock: 0,
            global_clock: 0,
            race_started: false,
            paused: false,
            rumble: Vec::new(),
        };
        // Preload the sprites that things may create (FS3_New on demand in
        // the original): cars, position markers, particles.
        for m in 0..6 {
            w.sprite(&d, &format!("car{car_type}n{m}"))?;
        }
        for i in 0..5 {
            w.sprite(&d, &format!("racepos{i}"))?;
        }
        for i in 1..=8 {
            let _ = w.sprite(&d, &format!("humansp{i}"));
        }
        for i in 0..6 {
            w.sparks[i] = w.sprite(&d, &format!("sprk{:02}aa", i + 1))?;
        }
        for i in 0..18 {
            w.smoke[i] = w.sprite(&d, &format!("gnd{}{:02}aa", i / 6, i % 6 + 1))?;
        }

        // MAP_LoadThings.
        load_things(&mut w, &d, &things)?;
        let hud = hud::Hud::load(&d, car_type)?;
        w.path = map::load_path(&d, &format!("map{:02}.pth", st.circuit))?;

        let clr = d.load("grafs.clr")?.to_vec();
        let mut race = Race {
            w,
            r: Renderer::default(),
            hud,
            clr,
            g: Geometry::new(sys.screen.w as i32, sys.screen.h as i32),
            mode: st.mode,
            humans: Vec::new(),
            console1: None,
            console2: None,
            robots: Vec::new(),
            race_pos: Vec::new(),
            human_pos: Vec::new(),
            views: [View::default(); 2],
            cams: &CAMS_1P,
            split: false,
            view_hud: [true, true],
            console_input: [0; MAX_USERCONTROLS],
            console_clock: 0,
            quitting: 0,
            tst_angle: 0,
            view_car: 0,
            advance_target: 0,
            close_car: [0, 0],
            pos_dots: (0..8).map(|i| Sprite::load(&d, &format!("carpos{i}.is2"))).collect::<Result<_>>()?,
            pad_prev: [0; 2],
            cam_sel: [STDCAM_NORMAL; 2],
            scene: None,
            net: None,
        };

        // Human players.
        let w = &mut race.w;
        let keysets = [cfg.p1_keys, cfg.p2_keys];
        let controls = [cfg.p1_control, cfg.p2_control];
        if st.mode != MODE_DEMO {
            let n = if st.net_node.is_some() { crate::net::MAX_NODES } else { 2 };
            for (i, &model) in st.cars.iter().enumerate().take(n) {
                // In network games, the other nodes' players are remote.
                let console = st.net_node.is_none_or(|me| me == i);
                let (x, y, a) = match use_start_pos(w) {
                    Some(sp) => (sp.x, sp.y, sp.angle),
                    None => (1 << 30, 1 << 30, 0x6000),
                };
                let flags = if console { PLYF_ISCONSOLE } else { 0 };
                let id = add_thing(w, x, y, 0x40000, THNT_PLAYER | (((model % 6) as u32) << 4), a, flags).expect("player");
                let p = player_mut(w, id);
                if console {
                    // Network games have one console player, with player 1's keys.
                    let c = if st.net_node.is_some() { 0 } else { i };
                    let k = keysets[c];
                    let np = if st.net_node.is_some() { 1 } else { st.cars.len().min(2) };
                    p.cfg = InputConfig { pad: pad_for(&controls, np, c), kup: k[0], kdn: k[1], kleft: k[2], kright: k[3], kgear: k[4] };
                } else if let Some(k) = st.net_keys.get(i) {
                    // Remote: gear changes compare scancodes with the keys.
                    p.cfg = InputConfig { pad: None, kup: k[0], kdn: k[1], kleft: k[2], kright: k[3], kgear: k[4] };
                }
                p.maxspeed = models[model].max_speed;
                p.automatic = !models[model].automatic;
                p.cartype = car_type;
                race.humans.push(id);
                if !console {
                } else if race.console1.is_none() {
                    race.console1 = Some(id);
                } else {
                    race.console2 = Some(id);
                }
            }
        }
        if let Some(me) = st.net_node {
            let n = race.humans.len();
            race.net = Some(NetRace {
                me,
                outbox: Vec::new(),
                sent: 0,
                pending: vec![Default::default(); n],
                received: vec![0; n],
                mine: HashMap::new(),
                theirs: Vec::new(),
                desync: false,
                lost: false,
                stalled: 0,
            });
        }
        let w = &mut race.w;
        // Race position markers.
        if st.mode < 2 {
            for i in 0..5 {
                race.race_pos.push(add_thing(w, 0, 0, 0x800000, THNT_RACEPOS + i, 0, 0).expect("marker"));
            }
        }
        for i in 0..race.humans.len() {
            race.human_pos.push(add_thing(w, 0, 0, 0x800000, THNT_HUMANPOS + i as u32, 0, 0).expect("marker"));
        }
        // Fill the rest of the grid with robots.
        let ncars = if st.mode == MODE_PRACTICE { 0 } else { w.start_pos.len() - race.humans.len() };
        let nstart = w.start_pos.len();
        for _ in 0..ncars {
            let Some(i) = use_start_pos_index(w) else { break };
            let sp = w.start_pos[i];
            let model = w.rng.next() % 6;
            let id = add_thing(w, sp.x, sp.y, 0x40000, THNT_CAR | (model << 4), sp.angle, 0).expect("car");
            if let Kind::Car(c) = &mut w.thing_mut(id).kind {
                c.car_speed = (nstart - i) as i32;
                c.v = 0;
            }
            race.robots.push(id);
        }
        w.order = (0..w.racers.len()).collect();

        // Sound effects.
        {
            let mut a = w.audio.lock().unwrap();
            for c in 0..4 {
                a.eff_unlock(c);
            }
            a.eff_stop_all();
            a.unload_all();
            let mut sfx = Sfx {
                motor: a.load_effect(&d, &format!("motor{car_type}.raw"), 11025, true),
                crash: a.load_effect(&d, "crash.raw", 13000, false),
                skid: a.load_effect(&d, "derrape.raw", 11000, true),
                ..Default::default()
            };
            if cfg.timed_race {
                sfx.extended = a.load_effect(&d, "exttime.raw", 13000, false);
                sfx.timeout = a.load_effect(&d, "timeover.raw", 13000, false);
            } else {
                sfx.lap_record = a.load_effect(&d, "laprec.raw", 13000, false);
            }
            if st.mode != MODE_PRACTICE {
                sfx.you_win = a.load_effect(&d, "unbeli.raw", 13000, false);
            }
            sfx.game_over = a.load_effect(&d, "gameover.raw", 13000, false);
            w.sfx = sfx;
        }
        Ok(race)
    }

    fn set_screen(&mut self, s: &Screen) {
        self.g = Geometry::new(s.w as i32, s.h as i32);
    }

    /// Switch the race screen size (the F12 SVGA toggle), resetting the
    /// cameras for it.
    fn resize(&mut self, sys: &mut Sys, w: usize, h: usize) {
        sys.screen = Screen::new(w, h);
        self.set_screen(&sys.screen);
        for v in 0..2 {
            let view = &mut self.views[v];
            view.c.data = view.to;
            view.change_pos = -1;
            view.c.tilt = 0;
            let mut d = view.c.data;
            self.scale(&mut d);
            self.views[v].c.data = d;
        }
    }

    fn build_scene(&mut self) {
        if self.scene.is_none() {
            let (stat, idx) = SceneStatic::build(&self.w, &self.clr);
            self.scene = Some((Arc::new(stat), idx));
        }
    }

    /// GPU frames are composed on a 320x200 screen.
    fn gpu_screen(&mut self, sys: &mut Sys) {
        if sys.gpu && (sys.screen.w, sys.screen.h) != (320, 200) {
            self.resize(sys, 320, 200);
        }
    }

    /// Hand a GPU frame to the platform: `views` plus the overlays `draw`
    /// puts over them (drawn twice to tell what they cover, see
    /// [`Scene::overlay_from`]).
    fn update_scene(&mut self, sys: &mut Sys, views: Vec<SceneView>, mut draw: impl FnMut(&mut Race, &mut Screen)) {
        let mut ab = [Screen::new(320, 200), Screen::new(320, 200)];
        ab[1].clear(255);
        for s in &mut ab {
            draw(self, s);
        }
        let (overlay, overlay_kind) = Scene::overlay_from(&ab[0].pix, &ab[1].pix, &self.clr);
        let stat = self.scene.as_ref().expect("scene data").0.clone();
        sys.update_scene(Scene { stat, views, overlay, overlay_kind });
    }

    // ---- Cameras ---------------------------------------------------------

    fn init_view(&mut self, v: usize, thn: ThingId) {
        let t = self.w.thing(thn);
        let view = &mut self.views[v];
        view.thn = thn;
        view.c.data = self.cams[STDCAM_HIGH];
        view.to = self.cams[STDCAM_NORMAL];
        view.change_pos = 0;
        view.c.tilt = 0;
        view.c.angle = t.angle;
        view.c.dx = t.x;
        view.c.dy = t.y;
        view.c.sec = t.sec;
    }

    /// Apply the screen-size scaling to freshly set camera data.
    fn scale(&self, d: &mut CamData) {
        d.focus *= self.g.xratio;
        d.horizon *= self.g.yratio;
    }

    /// HandleView: camera transitions and following the target.
    fn handle_view(&mut self, v: usize, h: i32) {
        let (xr, yr) = (self.g.xratio, self.g.yratio);
        let thn = self.views[v].thn;
        if let Kind::Player(p) = &self.w.thing(thn).kind
            && p.status != Status::Racing
        {
            self.views[v].change_pos = 0;
            self.views[v].to = self.cams[STDCAM_STATIC];
        }
        let view = &mut self.views[v];
        if view.change_pos >= 0 {
            if ((view.c.data.flags & CAMF_STATIC != 0 || view.to.flags & CAMF_STATIC != 0) && view.change_pos == 0) || view.change_pos > 16384 {
                view.c.data = view.to;
                view.change_pos = -1;
                view.c.tilt = 0;
                view.c.data.focus *= xr;
                view.c.data.horizon *= yr;
            } else if view.change_pos == 0 {
                view.from = view.c.data;
                view.from.focus /= xr;
                view.from.horizon /= yr;
                view.change_pos += 1;
            } else {
                let s = sin(view.change_pos as u16);
                let sin2 = fp_mult(s, s);
                let lerp = |a: i32, b: i32| a.wrapping_add(fp_mult(sin2, b.wrapping_sub(a)));
                view.c.data.horizon = lerp(view.from.horizon, view.to.horizon) & 0xFFFF;
                view.c.data.h = lerp(view.from.h, view.to.h);
                view.c.data.focus = lerp(view.from.focus, view.to.focus);
                view.c.data.radius = lerp(view.from.radius, view.to.radius);
                view.change_pos += 111;
                view.c.data.focus *= xr;
                view.c.data.horizon *= yr;
            }
        }
        let t = self.w.thing(thn);
        let (tx, ty, tz, ta) = (t.x, t.y, t.z, t.angle);
        let movangle = match &t.kind {
            Kind::Player(p) => Some(p.movangle as u16),
            _ => None,
        };
        let view = &mut self.views[v];
        if view.c.data.flags & CAMF_STATIC != 0 {
            if let Some(pc) = map::find_static_camera(&self.w.static_cams, tx, ty) {
                let dx = ((tx >> 12) as i32).wrapping_sub((pc.x >> 12) as i32);
                let dy = ((ty >> 12) as i32).wrapping_sub((pc.y >> 12) as i32);
                view.c.angle = get_angle(dx, -dy);
                view.c.dx = pc.x;
                view.c.dy = pc.y;
                view.c.data.h = pc.z as i32;
                view.c.data.horizon = pc.z as i32 / 2000 + 1;
                let r = sqrt(pow2(dx >> 8).wrapping_add(pow2(dy >> 8)) as u32) as i32;
                view.c.data.focus = 0xA00 + fp_mult_div(0x1000, r, 64 * 4);
                if r > 32 {
                    let y = fp_mult_div(view.c.data.h << 2, view.c.data.focus, r << 14) + view.c.data.horizon;
                    view.c.tilt = if y > h / 3 { y - h / 3 } else { 0 };
                }
                view.c.data.horizon *= yr;
                view.c.data.focus *= xr;
            }
        } else {
            let angle = movangle.unwrap_or(ta);
            if view.c.data.radius == 0 {
                view.c.angle = ta;
            } else if (view.c.angle.wrapping_sub(angle) as i16) / 16 == 0 {
                view.c.angle = angle;
            } else {
                let k = (angle.wrapping_sub(view.c.angle) as i16) / 16;
                view.c.angle = view.c.angle.wrapping_add(k as u16);
            }
            view.c.dx = tx;
            view.c.dy = ty;
            if view.c.data.radius == 0 && view.change_pos <= 0 {
                view.c.data.h = view.to.h + ((tz.wrapping_sub(0x20000) as i32) >> 12);
            }
        }
        if view.c.data.horizon < 1 {
            view.c.data.horizon = 1;
        }
    }

    // ---- Input -----------------------------------------------------------

    /// TimerUserControlFunc: sample input for one 70 Hz tick.
    fn timer_tick(&mut self, sys: &mut Sys) {
        let gc = self.w.global_clock;
        let mut ok = self.console_clock < gc + MAX_USERCONTROLS as u32;
        for &h in &self.humans {
            if player(&self.w, h).clock >= gc + MAX_USERCONTROLS as u32 {
                ok = false;
            }
        }
        // Network: stay within NET_LEAD ticks of the slowest remote player.
        if let Some(n) = &self.net {
            let local = player(&self.w, self.humans[n.me]).clock;
            let slowest = n.received.iter().enumerate().filter(|&(i, _)| i != n.me).map(|(_, &r)| r).min().unwrap_or(u32::MAX);
            if local >= slowest.saturating_add(crate::net::NET_LEAD) {
                ok = false;
            }
        }
        if !ok {
            return;
        }
        let pressed: [u32; 2] = std::array::from_fn(|i| sys.pads[i].buttons & !self.pad_prev[i]);
        self.pad_prev = sys.pads.map(|p| p.buttons);
        let mut console_scan = sys.last_scan;
        for (v, c) in [self.console1, self.console2].into_iter().enumerate() {
            if let Some(id) = c {
                let p = player_mut(&mut self.w, id);
                let pad = p.cfg.pad.map(|i| (&sys.pads[i], pressed[i]));
                let k = p.clock as usize % MAX_USERCONTROLS;
                p.ctl[k] = control::get_user_control(&sys.keys, sys.last_scan, &p.cfg, pad);
                p.clock += 1;
                let c = p.ctl[k];
                if let Some(n) = self.net.as_mut() {
                    n.outbox.push(c);
                }
                // The pad's Y cycles this view's camera, as F1-F4 / F5-F8 would.
                if pad.is_some_and(|(_, pr)| pr & crate::pad::NORTH != 0) && console_scan == 0 {
                    let base = if v == 0 { keys::F1 } else { keys::F5 };
                    console_scan = base + ((self.cam_sel[v] + 1) % 4) as u8;
                }
            }
        }
        self.console_input[self.console_clock as usize % MAX_USERCONTROLS] = console_scan;
        self.console_clock += 1;
        sys.last_scan = 0;
    }

    /// RACE_HandleComms: send our controls, take in the others'.
    fn net_pump(&mut self, sys: &mut Sys) {
        let (Some(n), Some(net)) = (self.net.as_mut(), sys.net.as_mut()) else { return };
        if !n.outbox.is_empty() {
            let ctl = std::mem::take(&mut n.outbox);
            net.send_ctl(n.sent, ctl.clone());
            n.sent += ctl.len() as u32;
        }
        n.received[n.me] = n.sent;
        for e in net.race_events() {
            match e {
                crate::net::RaceEvent::Lost => n.lost = true,
                crate::net::RaceEvent::Ctl { node, tick, ctl } if node < n.received.len() && node != n.me => {
                    // The relay keeps order: anything else is a duplicate.
                    if tick == n.received[node] {
                        n.received[node] += ctl.len() as u32;
                        n.pending[node].extend(ctl);
                    }
                }
                crate::net::RaceEvent::Check { tick, hash, .. } => match n.mine.get(&tick) {
                    Some(&h) => {
                        n.desync |= h != hash;
                        sys.report_check(h == hash);
                    }
                    None => n.theirs.push((tick, hash)),
                },
                _ => {}
            }
        }
        self.net_feed();
    }

    /// Move received controls into the remote players' input rings.
    fn net_feed(&mut self) {
        let Some(n) = self.net.as_mut() else { return };
        let gc = self.w.global_clock;
        for (i, q) in n.pending.iter_mut().enumerate() {
            let Some(&id) = self.humans.get(i) else { continue };
            let p = player_mut(&mut self.w, id);
            while p.clock < gc + MAX_USERCONTROLS as u32
                && let Some(c) = q.pop_front()
            {
                p.ctl[p.clock as usize % MAX_USERCONTROLS] = c;
                p.clock += 1;
            }
        }
    }

    /// After a simulated tick: every NET_CHECK ticks, swap state hashes.
    fn net_check(&mut self, sys: &mut Sys) {
        let gc = self.w.global_clock;
        let Some(n) = self.net.as_mut() else { return };
        if !gc.is_multiple_of(NET_CHECK) {
            return;
        }
        let h = state_hash(&self.w);
        n.mine.insert(gc, h);
        n.mine.retain(|&t, _| t + 70 * 60 > gc);
        if let Some(net) = sys.net.as_mut() {
            net.send_check(gc, h);
        }
        for (_, hash) in n.theirs.extract_if(.., |(t, _)| *t == gc) {
            n.desync |= hash != h;
            sys.report_check(hash == h);
        }
    }

    /// Pass this frame's rumble requests to the players' gamepads.
    fn send_rumble(&mut self, sys: &mut Sys) {
        let mut want: [Option<(f32, f32, u32)>; 2] = [None; 2];
        for (id, strong, weak, ms) in self.w.rumble.drain(..) {
            if let Some(pad) = self.w.things.get(id).and_then(Option::as_ref).and_then(|t| match &t.kind {
                Kind::Player(p) => p.cfg.pad,
                _ => None,
            }) {
                let w = want[pad].get_or_insert((0.0, 0.0, 0));
                *w = (w.0.max(strong), w.1.max(weak), w.2.max(ms));
            }
        }
        for (pad, w) in want.into_iter().enumerate() {
            if let Some((strong, weak, ms)) = w {
                sys.rumble(pad, strong, weak, ms);
            }
        }
    }

    /// ConsoleRoutine: race-wide keys (pause, cameras, abort...).
    fn console_routine(&mut self, sys: &mut Sys, clock: u32) {
        let k = clock as usize % MAX_USERCONTROLS;
        let ci = self.console_input[k];
        let humans: Vec<UserControl> = self.humans.iter().map(|&h| player(&self.w, h).ctl[k]).collect();
        let any = |f: &dyn Fn(u8) -> bool| f(ci) || humans.iter().any(|c| f(c.scancode));
        if any(&|s| s == keys::PAUSE) {
            let vol = if self.w.paused { 256 } else { 0 };
            let _ = self.w.audio.lock().map(|mut a| a.eff_change_volume(vol));
            self.w.paused = !self.w.paused;
        }
        if self.quitting == 1 && any(&|s| s != 0 && s != keys::ESC) {
            self.quitting = 0;
        }
        if any(&|s| s == keys::ESC) {
            self.quitting += 1;
        }
        let cam = |c: u8, base: u8| -> Option<usize> { (base..base + 4).position(|k| k == c) };
        if let Some(c) = cam(ci, keys::F1) {
            self.cam_sel[0] = c;
            self.views[0].to = self.cams[c];
            self.views[0].change_pos = 0;
        }
        if let Some(c) = cam(ci, keys::F5) {
            self.cam_sel[1] = c;
            self.views[1].to = self.cams[c];
            self.views[1].change_pos = 0;
        }
        if ci == keys::F9 {
            self.view_hud[0] = !self.view_hud[0];
        }
        if ci == keys::F10 {
            self.view_hud[1] = !self.view_hud[1];
        }
        if ci == keys::F11 {
            self.w.set.floor_detail = !self.w.set.floor_detail;
        }
        if ci == keys::F12 && !sys.gpu {
            // The original's SVGA toggle: 640x400 with everything scaled.
            let (w, h) = if sys.screen.w == 320 { sys.hires_size() } else { (320, 200) };
            self.resize(sys, w, h);
        }
    }

    // ---- Rendering -------------------------------------------------------

    /// Hide the position marker hovering over the viewed car.
    fn hide_markers(&mut self, v: usize) -> Vec<(ThingId, Option<SpriteId>)> {
        let mut saved = Vec::new();
        let thn = self.views[v].thn;
        let n = self.w.order.len();
        for i in 0..n.min(self.race_pos.len().max(self.humans.len())) {
            if i < self.race_pos.len() && self.w.racers[self.w.order[i]].thn == thn {
                let m = self.race_pos[i];
                saved.push((m, self.w.thing(m).spr));
                self.w.thing_mut(m).spr = None;
            }
            if i < self.humans.len() && self.humans[i] == thn {
                let m = self.human_pos[i];
                saved.push((m, self.w.thing(m).spr));
                self.w.thing_mut(m).spr = None;
            }
        }
        saved
    }

    fn draw_map(&mut self, s: &mut Screen, who: ThingId, x: i32, y: i32, size: i32, focus: i32) {
        let t = self.w.thing(who);
        let center = (t.x, t.y, t.angle);
        let dots: Vec<(u32, u32, usize)> = (0..self.w.order.len())
            .rev()
            .map(|i| {
                let th = self.w.thing(self.w.racers[self.w.order[i]].thn);
                let nsp = if i > 4 || self.mode == MODE_PRACTICE {
                    if th.ty & 0xFF00 == THNT_PLAYER { 7 } else { 6 }
                } else {
                    i
                };
                (th.x, th.y, nsp)
            })
            .collect();
        self.r.draw_map(s, &self.w, &self.clr, center, x, y, size, size, focus, &dots, &self.pos_dots);
    }


    /// The 3D views (RenderView for each player), drawn into `s` or, given
    /// `scene`, extracted for the GPU.
    fn render_world(&mut self, s: &mut Screen, mut scene: Option<&mut Vec<SceneView>>) {
        if scene.is_some() {
            self.build_scene();
        }
        let g = self.g;
        let tst = self.tst_angle;
        for (i, &c) in [self.console1, self.console2].iter().enumerate() {
            if let Some(c) = c
                && player(&self.w, c).status != Status::Racing
            {
                self.view_hud[i] = true;
            }
        }
        let views = if self.split { 2 } else { 1 };
        for v in 0..views {
            let (y, h) = if self.split { (g.min_y + v as i32 * g.h / 2, g.h / 2) } else { (g.min_y, g.h) };
            s.clip = crate::gfx::Rect { min_x: g.min_x, min_y: y, max_x: g.max_x, max_y: y + h };
            let is_static = self.views[v].c.data.flags & CAMF_STATIC != 0;
            let saved = if is_static { Vec::new() } else { self.hide_markers(v) };
            let mut cam = self.views[v].c;
            if self.split {
                cam.tilt += 20;
            }
            match (scene.as_deref_mut(), &self.scene) {
                (Some(out), Some((_, idx))) => out.push(self.r.scene_view(&self.w, idx, &mut cam, tst, [g.min_x, y, g.w, h])),
                _ => self.r.render_view(s, &self.w, &self.clr, &g, &mut cam, tst, y, h),
            }
            if self.split {
                cam.tilt -= 20;
            }
            self.views[v].c = cam;
            for (m, spr) in saved {
                self.w.thing_mut(m).spr = spr;
            }
        }
        s.clip = crate::gfx::Rect { min_x: g.min_x, min_y: g.min_y, max_x: g.max_x, max_y: g.max_y };
    }

    /// The HUD over the rendered views, laid out for `g` (see `Overlay`).
    fn render_hud(&mut self, s: &mut Screen, g: &Geometry) {
        let g = *g;
        s.clip = crate::gfx::Rect { min_x: g.min_x, min_y: g.min_y, max_x: g.max_x, max_y: g.max_y };
        let leader = if self.mode != MODE_PRACTICE { self.w.order.first().map(|&r| self.w.racers[r].thn) } else { None };
        let paused = self.w.paused;
        if !self.split {
            if let Some(c1) = self.console1 {
                if self.view_hud[0] {
                    let salp = self.views[0].c.data.flags & CAMF_STATIC == 0 && self.views[0].c.data.radius == 0;
                    if salp {
                        let maxy = s.clip.max_y;
                        self.hud.draw_salp(s, &g, maxy);
                    }
                    self.hud.draw_player(s, &self.w, &self.clr, &g, &player_view(&self.w, c1), self.mode, salp);
                    self.draw_map(s, c1, s.w as i32 - 76, g.min_y + 4, 68, 0x1CD0000);
                }
                let p = player(&self.w, c1);
                self.hud.draw_logos(s, &self.w, p, paused, leader == Some(c1), g.cx, g.cy - g.h / 4, g.cy - 40);
            }
        } else {
            for (i, c) in [self.console1, self.console2].into_iter().enumerate() {
                let Some(c) = c else { continue };
                let y = g.min_y + i as i32 * g.h / 2;
                if self.view_hud[i] {
                    s.clip = crate::gfx::Rect { min_x: g.min_x, min_y: y, max_x: g.max_x, max_y: y + g.h / 2 };
                    self.hud.draw_2player(s, &self.w, &self.clr, &g, &player_view(&self.w, c), self.mode, y);
                    self.draw_map(s, c, s.w as i32 - 92, y + 2, 36, 0x1CD0000 * 3 / 2);
                }
                let p = player(&self.w, c);
                self.hud.draw_logos(s, &self.w, p, paused, leader == Some(c), g.cx, y + g.h / 6, y + g.h / 4 - 10);
            }
            s.clip = crate::gfx::Rect { min_x: g.min_x, min_y: g.min_y, max_x: g.max_x, max_y: g.max_y };
        }
    }

    // ---- Sound -----------------------------------------------------------

    /// Motor sounds: own engine, the two nearest robots (panned), skids.
    fn engine_sounds(&mut self, skidding: &mut bool) {
        const MOTORMIN: u32 = 100;
        const MOTORRAT: i32 = 700;
        let slide = |c: Option<ThingId>| {
            c.is_some_and(|c| {
                let p = player(&self.w, c);
                p.flags & PLYF_SLIDING != 0 && abs32(self.w.thing(c).angle.wrapping_sub(p.ma) as i16 as i32) > 0x600
            })
        };
        let sliding = slide(self.console1) || slide(self.console2);
        let mut a = self.w.audio.lock().unwrap();
        if sliding {
            if !*skidding {
                a.eff_start(3, 256, 100, 0, self.w.sfx.skid);
                *skidding = true;
            }
        } else if *skidding {
            *skidding = false;
            a.eff_stop(3);
        }
        let freq = |v: i32| (((v >> 16) * MOTORRAT) >> 6) as u32 + MOTORMIN;
        let Some(c1) = self.console1 else { return };
        a.eff_set_freq(0, freq(player(&self.w, c1).revo));
        if self.console2.is_none() && !self.robots.is_empty() {
            let (vx, vy) = (self.views[0].c.dx, self.views[0].c.dy);
            let (mut p0, mut p1, mut d0, mut d1) = (0usize, 0usize, i32::MAX, i32::MAX);
            for (i, &r) in self.robots.iter().enumerate() {
                let t = self.w.thing(r);
                let dx = ((t.x >> 20) as i32).wrapping_sub((vx >> 20) as i32);
                let dy = ((t.y >> 20) as i32).wrapping_sub((vy >> 20) as i32);
                let d = dx * dx + dy * dy;
                if d < d1 {
                    if d < d0 {
                        d1 = d0;
                        p1 = p0;
                        d0 = d;
                        p0 = i;
                    } else {
                        d1 = d;
                        p1 = i;
                    }
                }
            }
            let (c0, c1ch) = if p0 == self.close_car[0] || p0 == self.close_car[1] {
                if p0 == self.close_car[0] { (1, 2) } else { (2, 1) }
            } else if p1 == self.close_car[1] {
                (1, 2)
            } else {
                (2, 1)
            };
            self.close_car = [p0, p1];
            let me = self.w.thing(c1);
            for (ch, p, d) in [(c0, p0, d0), (c1ch, p1, d1)] {
                let t = self.w.thing(self.robots[p]);
                let v = match &t.kind {
                    Kind::Car(c) => c.v,
                    _ => 0,
                };
                let ang = self.views[0].c.angle.wrapping_sub(get_angle(
                    (t.x.wrapping_sub(me.x) as i32) >> 8,
                    -((t.y.wrapping_sub(me.y) as i32) >> 8),
                ));
                let e = (4 + 6) * 2;
                let mut vol = ((1 << (e + 8)) / 15) / (((1 << e) / 15) + d) / 4;
                vol = vol.min(64);
                if self.w.set.car_type == 1 {
                    vol *= 2;
                }
                a.eff_set_freq(ch, freq(v));
                a.eff_set_volume(ch, vol);
                a.eff_set_panning(ch, (ang >> 8) as u8 as i8 as i32);
            }
        } else if let Some(c2) = self.console2 {
            a.eff_set_freq(1, freq(player(&self.w, c2).revo));
        }
    }
}

fn use_start_pos_index(w: &mut World) -> Option<usize> {
    if w.start_pos.iter().all(|s| s.used) {
        return None;
    }
    loop {
        let i = (w.rng.next() % w.start_pos.len() as u32) as usize;
        if !w.start_pos[i].used {
            w.start_pos[i].used = true;
            return Some(i);
        }
    }
}

/// MAP_UseStartPos: a random free grid slot.
fn use_start_pos(w: &mut World) -> Option<StartPos> {
    use_start_pos_index(w).map(|i| w.start_pos[i])
}

/// MAP_LoadThings: decorations, start positions and static cameras.
fn load_things(w: &mut World, d: &crate::data::Data, things: &[MapThing]) -> Result<()> {
    for t in things {
        let tx = (t.x << 19).wrapping_add(1 << 30);
        let ty = (t.y << 19).wrapping_add(1 << 30);
        if t.ty >= 240 << 8 {
            match t.ty >> 8 {
                241 if t.ty & 0xF == 0 && w.start_pos.len() < 40 => w.start_pos.push(StartPos { x: tx, y: ty, angle: t.angle, used: false }),
                242 if w.static_cams.len() < 40 => w.static_cams.push(StaticCamera { x: tx, y: ty, z: 900 + (t.ty as u32 & 0xFF) * 500 }),
                _ => {}
            }
        } else if (w.rng.next() & 15) < w.set.decoration_detail as u32 {
            let name = format!("XPR{:03X}", ((t.ty as u32 & 0xFF00) >> 4) + (t.ty as u32 & 0xF));
            w.sprite(d, &name)?;
            add_thing(w, tx, ty, 0, THNT_OBSTACLE + t.ty as u32, t.angle, 0);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------

/// Overlays (HUD, countdown, messages) above the original's 2x: its 320 and
/// 640 layouts don't fit, so they are drawn with the 320x200 layout on a
/// downsampled copy of the frame, and the pixels they change are blown up
/// onto the full-size frame (translucent bits like the map darken the
/// low-res copy, so they come out blocky but right).
struct Overlay {
    lo: Option<(Screen, Vec<u8>, usize)>,
}

impl Overlay {
    fn new(s: &Screen, g: &Geometry) -> Overlay {
        let n = g.xratio.min(g.yratio) as usize;
        if n <= 2 || s.w != 320 * n || s.h != 200 * n {
            return Overlay { lo: None };
        }
        let mut lo = Screen::new(320, 200);
        for y in 0..200 {
            for x in 0..320 {
                lo.pix[y * 320 + x] = s.pix[y * n * s.w + x * n];
            }
        }
        let before = lo.pix.clone();
        Overlay { lo: Some((lo, before, n)) }
    }

    /// Where to draw and with which layout.
    fn target<'a>(&'a mut self, s: &'a mut Screen, g: &Geometry) -> (&'a mut Screen, Geometry) {
        match self.lo.as_mut() {
            Some((lo, _, _)) => (lo, Geometry::new(320, 200)),
            None => (s, *g),
        }
    }

    fn finish(self, s: &mut Screen) {
        let Some((lo, before, n)) = self.lo else { return };
        for (i, (&c, &b)) in lo.pix.iter().zip(&before).enumerate() {
            if c != b {
                let (x, y) = (i % 320 * n, i / 320 * n);
                for r in 0..n {
                    s.pix[(y + r) * s.w + x..][..n].fill(c);
                }
            }
        }
    }
}

/// Which gamepad drives human player `i` of `n`: the one picked in the
/// controls menu (1 = gamepad 1, 2 = gamepad 2), else by default pad `i`
/// unless another player picked it.
fn pad_for(controls: &[u8; 2], n: usize, i: usize) -> Option<usize> {
    match controls[i] {
        c @ 1..=2 => Some(c as usize - 1),
        _ => (!controls[..n].contains(&(i as u8 + 1))).then_some(i),
    }
}

/// RACE_DoRace. Returns one result per human player.
pub async fn do_race(sys: &mut Sys, st: &RaceSetup<'_>, fonts: &crate::Fonts) -> Result<Vec<RaceResult>> {
    sys.rng.randomize(st.seed);
    let mut race = Race::new(sys, st)?;
    let d = sys.data.clone();
    let countdown: Vec<Sprite> = (0..6).filter_map(|i| Sprite::load(&d, &format!("race_{i}.is2")).ok()).collect();
    let gameover = Sprite::load(&d, "gameover.is2")?;
    let countsmp: Vec<Option<usize>> = {
        let mut a = sys.audio.lock().unwrap();
        ["go.raw", "one.raw", "two.raw", "three.raw"].iter().map(|n| a.load_effect(&d, n, 13000, false)).collect()
    };

    sys.pads_in_race = true;
    race.split = race.console2.is_some();
    race.cams = if race.split { &CAMS_2P } else { &CAMS_1P };
    let first = race.console1.expect("no players");
    race.init_view(0, first);
    race.views[0].c.data = race.cams[STDCAM_NORMAL];
    if st.mode < 2 {
        race.views[0].c.data.radius = 32 << 23;
    }
    if let Some(c2) = race.console2 {
        race.init_view(1, c2);
        race.views[1].c.data = race.cams[STDCAM_NORMAL];
        if st.mode < 2 {
            race.views[1].c.data.radius = 32 << 23;
        }
    }
    race.w.game_clock = 0;
    race.w.race_started = false;
    race.w.paused = false;
    let mut start_delay: u32 = if st.mode == MODE_PRACTICE { 0 } else { 3 * 70 + 69 };
    race.handle_view(0, 200);
    if race.split {
        race.handle_view(1, 200);
    }
    race.w.global_clock = 0;
    race.console_clock = 0;

    sys.screen.clear(0);
    sys.update();
    sys.pal = st.pal.clone();
    // NET_SyncStart: everyone starts together.
    if let Some(mut net) = sys.net.take() {
        let ok = crate::net::sync_start(sys, fonts, &mut net).await;
        sys.net = Some(net);
        if let Some(n) = race.net.as_mut() {
            n.lost |= !ok;
        }
        sys.screen.clear(0);
        sys.update();
    }
    {
        let mut a = sys.audio.lock().unwrap();
        let motor = race.w.sfx.motor;
        a.eff_start(0, 100, if st.cfg.car_type == 1 { 3 * 64 } else { 64 }, 0, motor);
        if race.console2.is_none() && !race.robots.is_empty() {
            a.eff_start(1, 100, 0, 0, motor);
            a.eff_start(2, 100, 0, 0, motor);
        } else if race.console2.is_some() {
            a.eff_start(1, 100, if st.cfg.car_type == 1 { 2 * 64 } else { 64 }, 0, motor);
        }
    }

    let mut countgover: i32 = -1;
    let mut countsmpsec: i32 = -1;
    let mut countsmpch: i32 = -1;
    let mut skidding = false;
    let mut last_clock = sys.clock();
    loop {
        sys.frame().await;
        // The timer interrupt sampled input at 70 Hz.
        while last_clock != sys.clock() {
            last_clock = last_clock.wrapping_add(1);
            race.timer_tick(sys);
        }
        race.net_pump(sys);

        let mut nsims = 0;
        loop {
            race.net_feed();
            let gc = race.w.global_clock;
            if race.humans.iter().any(|&h| player(&race.w, h).clock <= gc) {
                break;
            }
            run_things(&mut race.w);
            race.console_routine(sys, gc);
            race.w.global_clock += 1;
            race.net_check(sys);
            if !race.split {
                race.handle_view(0, 200);
            } else {
                race.handle_view(0, 100);
                race.handle_view(1, 100);
            }
            if !race.w.paused {
                start_delay = start_delay.saturating_sub(1);
                if start_delay < 70 {
                    race.w.race_started = true;
                    race.w.game_clock += 1;
                }
            }
            // Keep the race order (bubble sort, finished racers stay put).
            if countgover < 0 {
                let w = &mut race.w;
                loop {
                    let mut changed = false;
                    let dist = |r: &Racer| r.nlap * 10000 + r.npoint;
                    let mut d = dist(&w.racers[w.order[0]]);
                    for i in 1..w.order.len() {
                        let d1 = dist(&w.racers[w.order[i]]);
                        if !w.racers[w.order[i - 1]].finished && d1 > d {
                            w.order.swap(i - 1, i);
                            changed = true;
                        }
                        d = d1;
                    }
                    if !changed {
                        break;
                    }
                }
            }
            if countgover > 0 {
                countgover -= 1;
            } else if countgover < 0 && race.humans.iter().all(|&h| player(&race.w, h).status != Status::Racing) {
                race.w.eff_start(4, 256, 400, 0, race.w.sfx.game_over);
                countgover = 70 * 3;
            }
            nsims += 1;
        }

        // Network: show when we wait for the others.
        let waiting = match race.net.as_mut() {
            Some(n) => {
                n.stalled = if nsims > 0 { 0 } else { n.stalled + 1 };
                n.stalled > 35
            }
            None => false,
        };
        let desync = race.net.as_ref().is_some_and(|n| n.desync);
        // Redraw when the race moved on, or to show the waiting message.
        let fresh = nsims > 0 || waiting;
        race.gpu_screen(sys);
        let mut views = sys.gpu.then(Vec::new);
        if fresh {
            // Position markers follow the leaders and the humans.
            for i in 0..race.race_pos.len().min(race.w.order.len()) {
                let t = race.w.racers[race.w.order[i]].thn;
                let (x, y) = (race.w.thing(t).x, race.w.thing(t).y);
                race.w.move_thing(race.race_pos[i], x, y);
            }
            for i in 0..race.humans.len() {
                let t = race.humans[i];
                let (x, y) = (race.w.thing(t).x, race.w.thing(t).y);
                race.w.move_thing(race.human_pos[i], x, y);
            }
            race.render_world(&mut sys.screen, views.as_mut());
        }
        if nsims > 0 {
            race.engine_sounds(&mut skidding);
            race.send_rumble(sys);
        }
        // The countdown voice.
        if nsims > 0 && start_delay > 0 {
            let sec = (start_delay / 70) as i32;
            if sec != countsmpsec && (sec as usize) < countsmp.len() {
                let mut a = sys.audio.lock().unwrap();
                a.eff_unlock(countsmpch);
                countsmpch = a.eff_start(3, 256, 400, 0, countsmp[sec as usize]);
                countsmpsec = sec;
                a.eff_lock(countsmpch);
            }
        } else if nsims > 0 && countsmpch != -1 {
            sys.audio.lock().unwrap().eff_unlock(countsmpch);
            countsmpch = -1;
        }
        let draw_overlays = |race: &mut Race, s: &mut Screen, g: &Geometry| {
            if fresh {
                race.render_hud(s, g);
            }
            for (on, f, t, y) in [(waiting, &fonts.yellow, "Waiting for players", s.h as i32 / 2), (desync, &fonts.red, "Out of sync", 2)] {
                if on {
                    f.write(s, (s.w as i32 - f.text_width(t)) / 2, y, t, 15);
                }
            }
            let xlat = &race.clr[256 * 16..256 * 17];
            if nsims > 0 && start_delay > 0 {
                let sec = (start_delay / 70) as usize;
                if let Some(sp) = countdown.get(sec) {
                    let k = (start_delay % 70 + 5) as i32;
                    sp.draw(s, g.cx, g.cy, g.xratio * sp.w * 30 / k, g.yratio * sp.h * 30 / k, xlat);
                }
            }
            if nsims > 0 && countgover >= 0 {
                let k = countgover + 30;
                let (cx, cy) = (s.w as i32 / 2, s.h as i32 / 2);
                gameover.draw(s, cx, cy, gameover.w * 30 / k, gameover.h * 30 / k, xlat);
            }
            if race.quitting == 1 {
                let x = s.w as i32 / 2 - 160;
                let y = s.h as i32 / 2 - 100;
                s.rectangle(x + 20, y + 70, 280, 60, 15, -1);
                s.rectangle(x + 21, y + 71, 278, 58, 7, -1);
                s.rectangle(x + 22, y + 72, 276, 56, 8, -1);
                s.rectangle(x + 23, y + 73, 274, 54, 7, 8);
                fonts.yellow.write(s, x + 35, y + 80, "Abort race", 15);
                fonts.red.write(s, x + 25, y + 105, "Press  ESC  to confirm.", 15);
            }
        };
        match views {
            // Nothing new to show: keep the previous scene.
            Some(_) if !fresh => {}
            Some(views) => {
                let g = race.g;
                race.update_scene(sys, views, |race, s| draw_overlays(race, s, &g));
            }
            None => {
                let mut overlay = Overlay::new(&sys.screen, &race.g);
                let (s, g) = overlay.target(&mut sys.screen, &race.g);
                draw_overlays(&mut race, s, &g);
                overlay.finish(&mut sys.screen);
                sys.update();
            }
        }
        let ctrl = sys.keys[keys::LEFTCTRL as usize] || sys.keys[keys::RIGHTCTRL as usize];
        if sys.keys[keys::ESC as usize] && ctrl {
            // Leaving on our own: the others can't go on without us.
            if let Some(n) = sys.net.as_mut() {
                n.bye();
            }
            break;
        }
        if countgover == 0 || race.quitting >= 2 || race.net.as_ref().is_some_and(|n| n.lost) {
            break;
        }
    }

    // Results for the human players.
    let mut results = Vec::new();
    for &h in &race.humans {
        let p = player(&race.w, h);
        let r = &race.w.racers[p.racer];
        let pos = race.w.order.iter().position(|&o| race.w.racers[o].thn == h).unwrap_or(race.w.order.len() - 1);
        results.push(RaceResult {
            nlaps: r.nlap + (p.status == Status::Won) as i32,
            time: r.totaltime,
            best: r.bestlaptime,
            flags: if p.flags & PLYF_ISCONSOLE != 0 { RACERF_ISCONSOLE } else { 0 },
            pos: pos as i32,
        });
    }
    {
        let mut a = sys.audio.lock().unwrap();
        a.eff_stop_all();
        for c in 0..5 {
            a.eff_unlock(c);
        }
        a.unload_all();
    }
    sys.rng = std::mem::take(&mut race.w.rng);
    sys.screen.reset_clip();
    sys.pads_in_race = false;
    Ok(results)
}

/// RACE_DoDemo: robots racing with random cameras, until a key or 60 s.
/// Returns true if the user pressed a key.
pub async fn do_demo(sys: &mut Sys, st: &RaceSetup<'_>, fonts: &crate::Fonts) -> Result<bool> {
    let mut race = Race::new(sys, st)?;
    let d = sys.data.clone();
    let logo = Sprite::load(&d, "demologo.is2")?;
    race.split = false;
    race.cams = &CAMS_1P;
    if race.robots.is_empty() {
        return Ok(false);
    }
    let first = race.robots[(race.w.rng.next() % 10) as usize % race.robots.len()];
    race.init_view(0, first);
    let mut lastcam = STDCAM_STATIC;
    race.views[0].to = race.cams[lastcam];
    race.views[0].change_pos = 0;
    race.w.global_clock = 0;
    race.console_clock = 0;
    race.w.game_clock = 0;
    race.w.race_started = true;
    race.w.paused = false;
    race.handle_view(0, 200);
    for _ in 0..2000 {
        run_things(&mut race.w);
    }
    sys.screen.clear(0);
    sys.update();
    sys.pal = st.pal.clone();
    let mut leave = false;
    let mut last_clock = sys.clock();
    let (xr, yr) = (race.g.xratio, race.g.yratio);
    loop {
        sys.frame().await;
        while last_clock != sys.clock() {
            last_clock = last_clock.wrapping_add(1);
            race.timer_tick(sys);
        }
        while race.console_clock > race.w.global_clock {
            let k = race.w.global_clock as usize % MAX_USERCONTROLS;
            let ci = race.console_input[k];
            leave |= ci != 0 && ci != keys::SYSREQ && ci != keys::PRTSC;
            run_things(&mut race.w);
            race.w.global_clock += 1;
            race.handle_view(0, 200);
            race.w.game_clock += 1;
            let gc = race.w.game_clock;
            let v = &mut race.views[0];
            // Randomly change camera, or the car followed (and the camera).
            let change_cam = gc & 0xFF == 0x7F && race.w.rng.next() % 4 == 0;
            let change_car = !change_cam && gc & 0xFF == 0x40 && race.w.rng.next() % 4 == 0;
            if change_cam || change_car {
                let c = loop {
                    let c = (race.w.rng.next() % 6) as usize;
                    if c != lastcam {
                        break c;
                    }
                };
                if change_car {
                    race.advance_target = 1;
                }
                if c < 4 {
                    v.to = race.cams[c];
                    v.c.tilt = 0;
                    race.tst_angle = 0;
                    if change_car || lastcam >= 4 {
                        v.c.data = v.to;
                        v.change_pos = -1;
                        v.c.data.focus *= xr;
                        v.c.data.horizon *= yr;
                    } else {
                        v.change_pos = 0;
                    }
                } else {
                    v.to = race.cams[0];
                    v.c.data = v.to;
                    v.c.data.h = 200 + 300 * (race.w.rng.next() % 10) as i32;
                    v.c.tilt = (v.c.data.h - 200) * 80 / 3000;
                    v.c.data.radius = 20 << 20;
                    v.c.data.focus = 0x1000 * xr;
                    v.c.data.horizon *= yr;
                    race.tst_angle = if change_car { 0x6000 } else { 0xA000 };
                    v.change_pos = -1;
                }
                if change_car {
                    v.change_pos = -1;
                }
                lastcam = c;
            }
            if lastcam == 4 {
                race.tst_angle = race.tst_angle.wrapping_add(128);
            } else if lastcam == 5 {
                race.tst_angle = race.tst_angle.wrapping_sub(128);
            }
        }
        if race.advance_target != 0 {
            let n = race.robots.len() as i32;
            race.view_car = (race.view_car + race.advance_target).rem_euclid(n);
            race.advance_target = 0;
            let t = race.robots[race.view_car as usize];
            race.views[0].thn = t;
            let th = race.w.thing(t);
            let (a, x, y) = (th.angle, th.x, th.y);
            let c = &mut race.views[0].c;
            c.angle = a;
            c.dx = x;
            c.dy = y;
        }
        race.gpu_screen(sys);
        let g = race.g;
        sys.screen.clip = crate::gfx::Rect { min_x: g.min_x, min_y: g.min_y, max_x: g.max_x, max_y: g.max_y };
        let mut cam = race.views[0].c;
        let draw_overlays = |race: &mut Race, s: &mut Screen| {
            logo.draw_horizontal(s, 10, 10);
            if race.w.global_clock & 0x40 == 0 {
                let t = "Press Any Key";
                let w = fonts.yellow.text_width(t);
                fonts.yellow.write(s, g.cx - w / 2, g.max_y - 30, t, 15);
            }
        };
        if sys.gpu {
            race.build_scene();
            let idx = &race.scene.as_ref().expect("scene data").1;
            let view = race.r.scene_view(&race.w, idx, &mut cam, race.tst_angle, [g.min_x, g.min_y, g.w, g.h]);
            race.views[0].c = cam;
            race.update_scene(sys, vec![view], draw_overlays);
        } else {
            race.r.render_view(&mut sys.screen, &race.w, &race.clr, &g, &mut cam, race.tst_angle, g.min_y, g.h);
            race.views[0].c = cam;
            draw_overlays(&mut race, &mut sys.screen);
            sys.update();
        }
        let ctrl = sys.keys[keys::LEFTCTRL as usize] || sys.keys[keys::RIGHTCTRL as usize];
        if leave || race.w.game_clock > 70 * 60 || (sys.keys[keys::ESC as usize] && ctrl) {
            break;
        }
    }
    {
        let mut a = sys.audio.lock().unwrap();
        a.eff_stop_all();
        a.unload_all();
    }
    sys.rng = std::mem::take(&mut race.w.rng);
    sys.screen.reset_clip();
    Ok(leave)
}
