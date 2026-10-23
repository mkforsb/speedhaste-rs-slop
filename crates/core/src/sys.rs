//! The "hardware" the game runs on: screen, palette/VBL fades, keyboard,
//! the 70 Hz timer, plus the async plumbing that lets the original's blocking
//! loops run inside a browser's frame callback.
//!
//! The game is one `async` task that owns a [`Sys`]. Wherever the original
//! busy-waited (on the timer, VBL or a key) we `await` [`Sys::frame`], which
//! yields to the platform. The platform calls [`Game::frame`] once per display
//! frame: it delivers input/time, polls the task once, then presents
//! [`Game::front`]. Only that small handoff state is shared.

use std::cell::{Ref, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use crate::audio::Audio;
use crate::data::Data;
use crate::fx::Rng;
use crate::gfx::{Palette, Screen};
use crate::pad::Pad;

/// Ticks per second of the original's timer (VGA mode 13h refresh rate).
pub const TICK_HZ: u32 = 70;

/// What the platform shows: the last `update()`d screen and the current DAC.
#[derive(Clone)]
pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub pix: Vec<u8>,
    pub pal: Palette,
    /// A race frame for the GPU renderer instead of `pix` (see
    /// [`Game::set_gpu`]).
    pub scene: Option<Arc<crate::race::Scene>>,
}

#[derive(Default)]
struct Shared {
    now_ms: f64,
    events: Vec<(u8, bool)>,
    pads: [Pad; 2],
    rumble: [Option<Rumble>; 2],
    gpu: bool,
    front: Option<Frame>,
    connector: Option<crate::net::Connector>,
    net_checks: (u32, u32),
    in_race: bool,
}

/// A gamepad rumble request: motor strengths 0..1 for `ms` milliseconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rumble {
    pub strong: f32,
    pub weak: f32,
    pub ms: u32,
}

/// Persistent key/value storage for config and records (files on native,
/// localStorage on the web).
pub trait Storage {
    fn load(&self, key: &str) -> Option<String>;
    fn save(&mut self, key: &str, value: &str);
}

/// Storage that forgets everything (tests, headless runs).
#[derive(Default)]
pub struct NoStorage;

impl Storage for NoStorage {
    fn load(&self, _: &str) -> Option<String> {
        None
    }
    fn save(&mut self, _: &str, _: &str) {}
}

/// VBL palette fade state (VBL_FadeHandler in wcgsl/vblc.c).
#[derive(Default)]
pub struct Fade {
    /// 0 = idle; set to 1 to start, runs to 64.
    pub pos: i32,
    /// Steps per tick.
    pub speed: i32,
    /// Fade towards this palette, or towards `rgb` if `None`.
    pub dest: Option<Palette>,
    pub rgb: (u8, u8, u8),
    /// VBL_FADEFULL: clamp the distance to the target instead of stepping.
    pub full: bool,
}

pub struct Sys {
    pub data: Rc<Data>,
    pub audio: Arc<Mutex<Audio>>,
    pub screen: Screen,
    /// Current DAC palette (VBL_Palette).
    pub pal: Palette,
    pub fade: Fade,
    /// LLK_Keys: pressed state by scancode (extended keys have bit 7 set).
    pub keys: [bool; 256],
    /// LLK_LastScan: scancode of the last key pressed, cleared by the game.
    pub last_scan: u8,
    pub rng: Rng,
    /// Command-line style options (`-nologo`, `-car 3`...).
    pub args: Vec<String>,
    pub storage: Box<dyn Storage>,
    /// LLK_Autorepeat: held keys repeat into `last_scan` (DOS typematic).
    pub autorepeat: bool,
    repeat: Option<(u8, u32)>,
    /// Gamepads 1 and 2.
    pub pads: [Pad; 2],
    /// The platform draws races on the GPU: races then produce scenes.
    pub gpu: bool,
    /// Set during races: pads then only map Start/Back to keys.
    pub pads_in_race: bool,
    /// The network game being played, if any.
    pub net: Option<crate::net::Net>,
    pad_keys: Vec<u8>,
    /// MENU_SongName: the current song.
    pub song: String,
    clock: u32,
    vsync_old: u32,
    shared: Rc<RefCell<Shared>>,
}

impl Sys {
    /// TIMER_Clock / VBL counter: 70 Hz ticks since start.
    pub fn clock(&self) -> u32 {
        self.clock
    }

    /// Yield to the platform until the next display frame.
    pub async fn frame(&mut self) {
        {
            let mut sh = self.shared.borrow_mut();
            if let Some(f) = sh.front.as_mut() {
                f.pal = self.pal.clone();
            }
        }
        YieldOnce(false).await;
        let (now, mut events) = {
            let mut sh = self.shared.borrow_mut();
            self.pads = sh.pads;
            self.gpu = sh.gpu;
            sh.in_race = self.pads_in_race;
            (sh.now_ms, std::mem::take(&mut sh.events))
        };
        // Pad buttons become key presses/releases like the keyboard's.
        let mut held = Vec::new();
        for p in &self.pads {
            p.keys(self.pads_in_race, &mut held);
        }
        held.sort_unstable();
        held.dedup();
        for &k in &self.pad_keys {
            if !held.contains(&k) {
                events.push((k, false));
            }
        }
        for &k in &held {
            if !self.pad_keys.contains(&k) {
                events.push((k, true));
            }
        }
        self.pad_keys = held;
        for (sc, down) in events {
            self.keys[sc as usize] = down;
            if down {
                self.last_scan = sc;
                self.repeat = Some((sc, self.clock + 35));
            } else if self.repeat.is_some_and(|r| r.0 == sc) {
                self.repeat = None;
            }
        }
        let new_clock = (now * TICK_HZ as f64 / 1000.0) as u32;
        while self.clock < new_clock {
            self.clock += 1;
            self.fade_tick();
            // Typematic repeat: 500 ms delay, then ~11 per second.
            if let Some((sc, at)) = self.repeat
                && self.clock >= at
            {
                if self.autorepeat {
                    self.last_scan = sc;
                }
                self.repeat = Some((sc, at + 6));
            }
        }
    }

    /// LLS_Update: make the current screen contents visible.
    pub fn update(&mut self) {
        let mut sh = self.shared.borrow_mut();
        match sh.front.as_mut() {
            Some(f) if f.w == self.screen.w && f.h == self.screen.h => {
                f.pix.copy_from_slice(&self.screen.pix);
                f.scene = None;
            }
            _ => {
                sh.front = Some(Frame {
                    w: self.screen.w,
                    h: self.screen.h,
                    pix: self.screen.pix.clone(),
                    pal: self.pal.clone(),
                    scene: None,
                })
            }
        }
    }

    /// Like [`Sys::update`] for a race frame drawn by the GPU.
    pub fn update_scene(&mut self, scene: crate::race::Scene) {
        self.update();
        if let Some(f) = self.shared.borrow_mut().front.as_mut() {
            f.scene = Some(Arc::new(scene));
        }
    }

    /// VBL_VSync(n): wait until at least `n` ticks passed since the previous
    /// call; returns the number of ticks that actually passed.
    pub async fn vsync(&mut self, n: u32) -> u32 {
        while self.clock.wrapping_sub(self.vsync_old) < n {
            self.frame().await;
        }
        let e = self.clock.wrapping_sub(self.vsync_old);
        self.vsync_old = self.clock;
        e
    }

    /// Wait for `n` ticks to pass (busy waits on TIMER_Clock).
    pub async fn wait_ticks(&mut self, n: u32) {
        let start = self.clock;
        while self.clock.wrapping_sub(start) < n {
            self.frame().await;
        }
    }

    /// `while (LLK_LastScan == 0);` — wait for a key press, returning it.
    pub async fn wait_key(&mut self) -> u8 {
        self.last_scan = 0;
        while self.last_scan == 0 {
            self.frame().await;
        }
        self.last_scan
    }

    /// Start a VBL fade towards a palette (or black when `None`).
    pub fn start_fade(&mut self, dest: Option<Palette>, speed: i32) {
        self.fade.dest = dest;
        self.fade.rgb = (0, 0, 0);
        self.fade.speed = speed;
        self.fade.full = false;
        self.fade.pos = 1;
    }

    /// Start a VBL_FADEFULL fade.
    pub fn start_full_fade(&mut self, dest: Option<Palette>, speed: i32) {
        self.start_fade(dest, speed);
        self.fade.full = true;
    }

    /// Fade to black then wait, abortable by a key (the common
    /// "start VBL fading; while (FadePos > 0 && LastScan == 0);" idiom).
    pub async fn fade_out(&mut self, speed: i32) {
        self.start_fade(None, speed);
        self.last_scan = 0;
        self.wait_fade(true).await;
        self.fade.pos = 0;
        self.pal = Palette::default();
        self.last_scan = 0;
    }

    /// MENU_InitMusic: switch to `name` (waiting for the previous song's
    /// fade-out) or stop music with `None`.
    pub async fn init_music(&mut self, name: Option<&str>) {
        let name = name.map(str::to_ascii_lowercase);
        if name.as_deref() != Some(self.song.as_str()) {
            self.song = name.clone().unwrap_or_default();
            while self.audio.lock().map(|a| a.mus_playing() && a.mus_fading_out()).unwrap_or(false) && !self.keys[crate::keys::ESC as usize] {
                self.frame().await;
            }
            let data = self.data.clone();
            let mut a = self.audio.lock().unwrap();
            match &name {
                None => a.mus_stop(),
                Some(n) => {
                    a.mus_play(&data, n);
                }
            }
        }
        let mut a = self.audio.lock().unwrap();
        a.mus_set_fading(true, 0);
    }

    /// MENU_DoneMusic: fade the song out over 30 ticks.
    pub fn done_music(&mut self) {
        self.audio.lock().unwrap().mus_set_fading(false, 30);
    }

    /// Wait until the current song has faded out completely.
    pub async fn wait_music_fade(&mut self) {
        while self.audio.lock().map(|a| a.mus_playing() && a.mus_fading_out()).unwrap_or(false) && !self.keys[crate::keys::ESC as usize] {
            self.frame().await;
        }
    }

    /// Open a connection to the platform's relay server, if it has one.
    pub fn connect(&mut self) -> Option<Box<dyn crate::net::Pipe>> {
        self.shared.borrow_mut().connector.as_mut().map(|c| c())
    }

    pub fn can_connect(&self) -> bool {
        self.shared.borrow().connector.is_some()
    }

    /// Count a network desync check (for [`Game::net_checks`]).
    pub fn report_check(&mut self, ok: bool) {
        let c = &mut self.shared.borrow_mut().net_checks;
        c.0 += 1;
        c.1 += !ok as u32;
    }

    /// Ask the platform to rumble gamepad `pad` (0 or 1).
    pub fn rumble(&mut self, pad: usize, strong: f32, weak: f32, ms: u32) {
        if let Some(r) = self.shared.borrow_mut().rumble.get_mut(pad) {
            *r = Some(Rumble { strong, weak, ms });
        }
    }

    /// Wait for the running fade to end, or a key press if `abortable`.
    pub async fn wait_fade(&mut self, abortable: bool) {
        while self.fade.pos > 0 && !(abortable && self.last_scan != 0) {
            self.frame().await;
        }
    }

    fn fade_tick(&mut self) {
        let f = &mut self.fade;
        if f.pos <= 0 || f.speed <= 0 {
            return;
        }
        for _ in 0..f.speed {
            f.pos = f.pos.min(64);
            // VGA_FullFadePalette limits to 65 - pos (so it ends one short
            // of a target palette), VGA_FullFadeOutPalette to 64 - pos.
            let k = ((if f.dest.is_some() { 65 } else { 64 }) - f.pos.min(64)) as u8;
            for i in 0..768 {
                let target = match &f.dest {
                    Some(p) => p.0[i],
                    None => [f.rgb.0, f.rgb.1, f.rgb.2][i % 3],
                };
                let c = &mut self.pal.0[i];
                if f.full {
                    if *c > target && *c - target >= k {
                        *c = target + k;
                    } else if *c < target && target - *c >= k {
                        *c = target - k;
                    }
                } else if *c < target {
                    *c += 1;
                } else if *c > target {
                    *c -= 1;
                }
            }
            f.pos += 1;
            if f.pos > 64 {
                f.pos = 0;
                break;
            }
        }
    }

    /// The hi-res race screen: the original's SVGA 640x400 (scale 2), or
    /// `-scale N` times 320x200 (a port addition).
    pub fn hires_size(&self) -> (usize, usize) {
        let n = self.arg_value("scale").and_then(|v| v.parse::<usize>().ok()).unwrap_or(2).clamp(2, 8);
        (320 * n, 200 * n)
    }

    /// BASE_CheckArg: index just past the matching `-name` argument.
    pub fn arg(&self, name: &str) -> Option<usize> {
        self.args
            .iter()
            .position(|a| (a.starts_with('-') || a.starts_with('/')) && a[1..].eq_ignore_ascii_case(name))
            .map(|i| i + 1)
    }

    pub fn arg_value(&self, name: &str) -> Option<&str> {
        self.arg(name).and_then(|i| self.args.get(i)).map(String::as_str)
    }
}

struct YieldOnce(bool);

impl Future for YieldOnce {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            Poll::Pending
        }
    }
}

/// The running game, as seen by a platform frontend.
pub struct Game {
    task: Option<Pin<Box<dyn Future<Output = ()>>>>,
    shared: Rc<RefCell<Shared>>,
}

impl Game {
    /// Create the game task. `main` receives the [`Sys`] and runs the whole
    /// program flow; when it returns, the game has quit.
    pub fn new<F, Fut>(data: Data, audio: Arc<Mutex<Audio>>, storage: Box<dyn Storage>, args: Vec<String>, main: F) -> Game
    where
        F: FnOnce(Sys) -> Fut,
        Fut: Future<Output = ()> + 'static,
    {
        let shared = Rc::new(RefCell::new(Shared::default()));
        let sys = Sys {
            data: Rc::new(data),
            audio,
            screen: Screen::new(320, 200),
            pal: Palette::default(),
            fade: Fade::default(),
            keys: [false; 256],
            last_scan: 0,
            rng: Rng::default(),
            args,
            storage,
            autorepeat: true,
            repeat: None,
            pads: [Pad::default(); 2],
            gpu: false,
            pads_in_race: false,
            net: None,
            pad_keys: Vec::new(),
            song: String::new(),
            clock: 0,
            vsync_old: 0,
            shared: shared.clone(),
        };
        Game { task: Some(Box::pin(main(sys))), shared }
    }

    /// Feed a key event (DOS set-1 scancode, extended keys | 0x80).
    pub fn key(&mut self, scancode: u8, down: bool) {
        self.shared.borrow_mut().events.push((scancode, down));
    }

    /// The rumble requested for gamepad `i` since the last call, if any.
    pub fn take_rumble(&mut self, i: usize) -> Option<Rumble> {
        self.shared.borrow_mut().rumble.get_mut(i).and_then(Option::take)
    }

    /// Have races produce [`crate::race::Scene`]s for a GPU renderer.
    pub fn set_gpu(&mut self, on: bool) {
        self.shared.borrow_mut().gpu = on;
    }

    /// Enable network play through this relay connector.
    pub fn set_connector(&mut self, c: crate::net::Connector) {
        self.shared.borrow_mut().connector = Some(c);
    }

    /// Network desync checks done so far, and how many failed.
    pub fn net_checks(&self) -> (u32, u32) {
        self.shared.borrow().net_checks
    }

    /// Set gamepad `i`'s (0 or 1) current state.
    pub fn pad(&mut self, i: usize, pad: Pad) {
        if let Some(p) = self.shared.borrow_mut().pads.get_mut(i) {
            *p = pad;
        }
    }

    /// Whether a race is running (else menus and other screens), e.g. for
    /// on-screen touch controls.
    pub fn in_race(&self) -> bool {
        self.shared.borrow().in_race
    }

    /// Advance the game to `now_ms` (monotonic milliseconds). Returns false
    /// once the game has finished.
    pub fn frame(&mut self, now_ms: f64) -> bool {
        self.shared.borrow_mut().now_ms = now_ms;
        if let Some(task) = self.task.as_mut() {
            let mut cx = Context::from_waker(Waker::noop());
            if task.as_mut().poll(&mut cx).is_ready() {
                self.task = None;
            }
        }
        self.task.is_some()
    }

    /// The frame to show, if anything has been drawn yet.
    pub fn front(&self) -> Option<Ref<'_, Frame>> {
        Ref::filter_map(self.shared.borrow(), |s| s.front.as_ref()).ok()
    }
}
