//! Audio: a replacement for the VTAL sound library (whose core was not
//! released). Mirrors the `GAME_EFF_*` / `GAME_MUS_*` interface from
//! wcgsl/gamevtal.c on top of a small software mixer.
//!
//! The platform owns the output device and calls [`Audio::render`] from its
//! audio callback; the game issues commands through the same (mutex-shared)
//! object.

use std::sync::Arc;

use crate::data::Data;
use crate::s3m::S3mPlayer;

/// Number of effect channels (SPEED.CFG FXChannels).
pub const EFF_CHANNELS: usize = 5;

pub type EffectId = usize;

struct Effect {
    pcm: Arc<Vec<i8>>,
    rate: u32,
    looped: bool,
}

/// GAME_TEffChan + the mixer voice playing it.
#[derive(Default, Clone)]
struct EffChan {
    pcm: Option<Arc<Vec<i8>>>,
    looped: bool,
    /// Playback position in samples, 32.32 fixed point.
    pos: u64,
    /// Natural rate of the sample (Hz) and 8.8 rate multiplier.
    eff_rate: u32,
    rate: u32,
    /// 0..512, 256 = standard.
    vol: i32,
    /// Angle-like panning: 0 front, -64 left, 64 right, ±128 rear.
    pan: i32,
    locked: bool,
    launched: u32,
}

pub struct Audio {
    out_rate: u32,
    effects: Vec<Option<Effect>>,
    chans: [EffChan; EFF_CHANNELS],
    time: u32,
    /// Global effects volume (GAME_EFF_ChangeVolume), 256 = standard.
    eff_volume: i32,
    /// Configured volumes (SPEED.CFG MusicVolume / FXVolume), 0..256.
    pub cfg_music_volume: i32,
    pub cfg_fx_volume: i32,
    music: Option<S3mPlayer>,
    /// Music fade: current volume and per-sample delta, 0..1.
    mus_fade_vol: f32,
    mus_fade_delta: f32,
    /// Counts 70 Hz "effect ticks" worth of samples (EffTickCount).
    pub eff_tick_count: u32,
    tick_frac: u32,
}

impl Audio {
    pub fn new(out_rate: u32) -> Audio {
        Audio {
            out_rate,
            effects: Vec::new(),
            chans: Default::default(),
            time: 0,
            eff_volume: 256,
            cfg_music_volume: 256,
            cfg_fx_volume: 256,
            music: None,
            mus_fade_vol: 1.0,
            mus_fade_delta: 0.0,
            eff_tick_count: 0,
            tick_frac: 0,
        }
    }

    pub fn out_rate(&self) -> u32 {
        self.out_rate
    }

    // ---- Effects --------------------------------------------------------

    /// GL_LoadEffect: 8-bit signed raw PCM at `rate` Hz.
    pub fn load_effect(&mut self, data: &Data, name: &str, rate: u32, looped: bool) -> Option<EffectId> {
        let b = data.get(name)?;
        let pcm = Arc::new(b.iter().map(|&v| v as i8).collect());
        let e = Effect { pcm, rate, looped };
        if let Some(i) = self.effects.iter().position(Option::is_none) {
            self.effects[i] = Some(e);
            Some(i)
        } else {
            self.effects.push(Some(e));
            Some(self.effects.len() - 1)
        }
    }

    pub fn unload_effect(&mut self, id: Option<EffectId>) {
        if let Some(id) = id
            && let Some(slot) = self.effects.get_mut(id)
        {
            *slot = None;
        }
    }

    pub fn unload_all(&mut self) {
        self.effects.clear();
    }

    /// GAME_EFF_Start. `chan` -1 picks a free (or the oldest) unlocked
    /// channel. `rate` is an 8.8 multiplier of the sample's own rate, `vol`
    /// 256 = standard. Returns the channel used or -1.
    pub fn eff_start(&mut self, chan: i32, rate: u32, vol: i32, pan: i32, eff: Option<EffectId>) -> i32 {
        let Some(e) = eff.and_then(|id| self.effects.get(id)).and_then(Option::as_ref) else { return -1 };
        if chan < -1 || chan >= EFF_CHANNELS as i32 {
            return -1;
        }
        let chan = if chan == -1 {
            match self.chans.iter().position(|c| !c.locked && c.pcm.is_none()) {
                Some(c) => c,
                None => {
                    let mut best = 0;
                    let mut l = self.time;
                    for (i, c) in self.chans.iter().enumerate() {
                        if !c.locked && c.launched < l {
                            best = i;
                            l = c.launched;
                        }
                    }
                    best
                }
            }
        } else {
            chan as usize
        };
        let c = &mut self.chans[chan];
        if c.locked {
            return -1;
        }
        *c = EffChan {
            pcm: Some(e.pcm.clone()),
            looped: e.looped,
            pos: 0,
            eff_rate: e.rate,
            rate,
            vol,
            pan,
            locked: false,
            launched: self.time,
        };
        self.time += 1;
        chan as i32
    }

    pub fn eff_stop(&mut self, chan: i32) {
        if let Some(c) = self.chans.get_mut(chan as usize) {
            c.pcm = None;
        }
    }

    pub fn eff_stop_all(&mut self) {
        for c in &mut self.chans {
            c.pcm = None;
        }
    }

    pub fn eff_lock(&mut self, chan: i32) {
        if let Some(c) = self.chans.get_mut(chan as usize) {
            c.locked = true;
        }
    }

    pub fn eff_unlock(&mut self, chan: i32) {
        if let Some(c) = self.chans.get_mut(chan as usize) {
            c.locked = false;
        }
    }

    /// GAME_EFF_ChangeVolume: global effects volume (0 mutes, e.g. on pause).
    pub fn eff_change_volume(&mut self, vol: i32) {
        self.eff_volume = vol;
    }

    pub fn eff_set_freq(&mut self, chan: i32, rate: u32) {
        if let Some(c) = self.chans.get_mut(chan as usize) {
            c.rate = rate;
        }
    }

    pub fn eff_set_volume(&mut self, chan: i32, vol: i32) {
        if let Some(c) = self.chans.get_mut(chan as usize) {
            c.vol = vol;
        }
    }

    pub fn eff_set_panning(&mut self, chan: i32, pan: i32) {
        if let Some(c) = self.chans.get_mut(chan as usize) {
            c.pan = pan;
        }
    }

    // ---- Music ----------------------------------------------------------

    /// Load and start a song (GAME_MUS_Load + GAME_MUS_Start).
    pub fn mus_play(&mut self, data: &Data, name: &str) -> bool {
        self.music = data.get(name).and_then(|b| S3mPlayer::new(b, self.out_rate));
        self.mus_fade_vol = 1.0;
        self.mus_fade_delta = 0.0;
        self.music.is_some()
    }

    pub fn mus_stop(&mut self) {
        self.music = None;
    }

    /// True while a fade-out is in progress (or finished).
    pub fn mus_fading_out(&self) -> bool {
        self.mus_fade_delta < 0.0 && self.mus_fade_vol > 0.0
    }

    pub fn mus_playing(&self) -> bool {
        self.music.is_some() && self.mus_fade_vol > 0.0
    }

    /// GAME_MUS_SetFading: fade out (or in) over `ticks` 70 Hz ticks.
    pub fn mus_set_fading(&mut self, fade_in: bool, ticks: u32) {
        if fade_in && ticks == 0 {
            self.mus_fade_vol = 1.0;
            self.mus_fade_delta = 0.0;
            return;
        }
        let samples = (ticks.max(1) as f32) * self.out_rate as f32 / 70.0;
        self.mus_fade_delta = if fade_in { 1.0 / samples } else { -1.0 / samples };
        if fade_in {
            self.mus_fade_vol = 0.0;
        }
    }

    // ---- Mixing ---------------------------------------------------------

    /// Render interleaved stereo samples.
    pub fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let frames = out.len() / 2;
        if let Some(m) = self.music.as_mut() {
            let mv = self.cfg_music_volume as f32 / 256.0;
            m.render(out, mv);
            if self.mus_fade_delta != 0.0 {
                for f in out.chunks_mut(2) {
                    self.mus_fade_vol = (self.mus_fade_vol + self.mus_fade_delta).clamp(0.0, 1.0);
                    f[0] *= self.mus_fade_vol;
                    f[1] *= self.mus_fade_vol;
                }
                if self.mus_fade_vol >= 1.0 {
                    self.mus_fade_delta = 0.0;
                } else if self.mus_fade_vol <= 0.0 {
                    // Faded out: the song is over.
                    self.music = None;
                    self.mus_fade_delta = 0.0;
                }
            }
        }
        let master = self.eff_volume as f32 / 256.0 * self.cfg_fx_volume as f32 / 256.0;
        for c in &mut self.chans {
            let Some(pcm) = c.pcm.clone() else { continue };
            if pcm.is_empty() {
                c.pcm = None;
                continue;
            }
            let step = ((c.eff_rate as u64 * c.rate as u64) << 24) / self.out_rate as u64;
            let gain = master * (c.vol.clamp(0, 512) as f32 / 256.0) * 0.35;
            let p = (c.pan as f32) * std::f32::consts::PI / 128.0;
            let side = p.sin();
            let (gl, gr) = (gain * (1.0 - side.max(0.0)), gain * (1.0 + side.min(0.0)));
            let len = (pcm.len() as u64) << 32;
            for f in out.chunks_mut(2) {
                if c.pos >= len {
                    if c.looped {
                        c.pos %= len;
                    } else {
                        c.pcm = None;
                        break;
                    }
                }
                let i = (c.pos >> 32) as usize;
                let fr = (c.pos & 0xffff_ffff) as f32 / 4294967296.0;
                let a = pcm[i] as f32;
                let b = match pcm.get(i + 1) {
                    Some(&v) => v as f32,
                    None if c.looped => pcm[0] as f32,
                    None => 0.0,
                };
                let v = (a + (b - a) * fr) / 128.0;
                f[0] += v * gl;
                f[1] += v * gr;
                c.pos += step;
            }
        }
        for v in out.iter_mut() {
            *v = v.clamp(-1.0, 1.0);
        }
        // EffTickCount advances at 70 Hz of rendered audio.
        self.tick_frac += frames as u32 * 70;
        self.eff_tick_count += self.tick_frac / self.out_rate;
        self.tick_frac %= self.out_rate;
    }
}
