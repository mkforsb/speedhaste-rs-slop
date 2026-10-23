//! Scream Tracker 3 module player, standing in for VTAL's player (whose
//! source was not released). Implements the ST3 effect set the game's songs
//! use, with linear-interpolated mixing, and follows what VTAL's S3M loader
//! and player do where they differ from ST3 (read from the binary, see
//! PORTING.md): channels hard-panned by their channel setting in every song,
//! master/global volume ignored, volume slides and tone portamento running
//! on tick 0 too, volume capped at 63, no V/U/I/S2x effects.

use std::sync::Arc;

const PERIODS: [u32; 12] = [1712, 1616, 1524, 1440, 1356, 1280, 1208, 1140, 1076, 1016, 960, 907];
/// ST3 sine table for vibrato/tremolo (0..255 amplitude).
const SINE: [i32; 32] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197, 180, 161,
    141, 120, 97, 74, 49, 24,
];
/// Overall music level (VTAL ignores the songs' own master volumes). Set
/// so music and effects balance as in the original (measured in DOSBox).
const MASTER: f32 = 0.5625;
/// VTAL's music PanWidth setting (fl2.c), 256 = full stereo.
pub const PAN_WIDTH: i32 = 60;

#[derive(Default)]
struct Sample {
    data: Arc<Vec<f32>>,
    loop_start: usize,
    loop_end: usize,
    looped: bool,
    volume: i32,
    c2spd: u32,
}

#[derive(Clone, Copy, Default)]
struct Cell {
    note: u8, // 255 = none, 254 = note off, else hi nibble octave lo nibble note
    ins: u8,
    vol: u8, // 255 = none
    cmd: u8,
    info: u8,
}

#[derive(Default, Clone)]
struct Chan {
    sample: usize, // 1-based, 0 = none
    pos: f64,
    playing: bool,
    period: i32,
    target: i32,
    volume: i32,
    pan: i32, // 0..255
    // effect state
    cmd: u8,
    info: u8,
    last_info: u8,
    porta_speed: u8,
    vib_pos: i32,
    vib_info: u8,
    trem_pos: i32,
    trem_info: u8,
    vib_delta: i32,
    vol_delta: i32,
    arp: [i32; 3],
    retrig: u8,
    note_delay: Option<Cell>,
    cut_tick: i32,
    loop_row: usize,
    loop_count: u8,
    offset_mem: u8,
    /// Volume slide per tick (or once per row if `vslide_fine`), kept per
    /// channel and only set by D/K/L, as in VTAL.
    vslide: i32,
    vslide_fine: bool,
}

pub struct S3mPlayer {
    rate: u32,
    samples: Vec<Sample>,
    orders: Vec<u8>,
    patterns: Vec<Vec<Cell>>,
    nchan: usize,
    chans: Vec<Chan>,
    amiga: bool,
    speed: u32,
    tempo: u32,
    tick: u32,
    row: usize,
    order: usize,
    next_row: Option<(usize, usize)>,
    pattern_delay: u32,
    tick_left: f64,
    /// Stereo width, [`PAN_WIDTH`] by default.
    pub pan_width: i32,
}

impl S3mPlayer {
    pub fn new(b: &[u8], rate: u32) -> Option<S3mPlayer> {
        if b.len() < 0x60 || &b[0x2C..0x30] != b"SCRM" {
            return None;
        }
        let u16a = |o: usize| u16::from_le_bytes([*b.get(o).unwrap_or(&0), *b.get(o + 1).unwrap_or(&0)]) as usize;
        let (ordnum, insnum, patnum, flags, _cwt, ffi) = (u16a(0x20), u16a(0x22), u16a(0x24), u16a(0x26), u16a(0x28), u16a(0x2A));
        // VTAL reads neither the global volume (0x30), the master volume and
        // its stereo bit (0x33), nor the default pan table.
        let (is, it) = (b[0x31], b[0x32]);
        let chset = &b[0x40..0x60];
        let nchan = chset.iter().rposition(|&c| c < 16).map_or(0, |i| i + 1);
        let orders: Vec<u8> = b.get(0x60..0x60 + ordnum)?.iter().copied().filter(|&o| o != 254).collect();
        let pp = 0x60 + ordnum;
        let ins_ptrs: Vec<usize> = (0..insnum).map(|i| u16a(pp + i * 2) * 16).collect();
        let pat_ptrs: Vec<usize> = (0..patnum).map(|i| u16a(pp + insnum * 2 + i * 2) * 16).collect();
        let mut chans = vec![Chan::default(); nchan];
        for (i, c) in chans.iter_mut().enumerate() {
            c.pan = if chset[i] & 8 != 0 { 255 } else { 0 };
        }
        let mut samples = Vec::new();
        for &ip in &ins_ptrs {
            let h = b.get(ip..ip + 0x50)?;
            let mut s = Sample::default();
            if h[0] == 1 {
                let memseg = ((h[0x0D] as usize) << 16) | (h[0x0E] as usize) | ((h[0x0F] as usize) << 8);
                let off = memseg * 16;
                let u32a = |o: usize| u32::from_le_bytes(h[o..o + 4].try_into().unwrap()) as usize;
                let (len, ls, le) = (u32a(0x10), u32a(0x14), u32a(0x18));
                s.volume = h[0x1C].min(63) as i32;
                let sflags = h[0x1F];
                s.looped = sflags & 1 != 0;
                s.c2spd = u32a(0x20) as u32;
                let bits16 = sflags & 4 != 0;
                let bytes = if bits16 { len * 2 } else { len };
                let raw = b.get(off..(off + bytes).min(b.len())).unwrap_or(&[]);
                let data: Vec<f32> = if bits16 {
                    raw.chunks_exact(2)
                        .map(|c| {
                            let v = u16::from_le_bytes([c[0], c[1]]);
                            (if ffi == 2 { (v ^ 0x8000) as i16 } else { v as i16 }) as f32 / 32768.0
                        })
                        .collect()
                } else {
                    raw.iter().map(|&v| (if ffi == 2 { (v ^ 0x80) as i8 } else { v as i8 }) as f32 / 128.0).collect()
                };
                s.loop_start = ls.min(data.len());
                s.loop_end = le.min(data.len());
                if s.loop_end <= s.loop_start {
                    s.looped = false;
                }
                s.data = Arc::new(data);
            }
            samples.push(s);
        }
        let mut patterns = Vec::new();
        for &p in &pat_ptrs {
            let mut cells = vec![Cell { note: 255, ins: 0, vol: 255, cmd: 0, info: 0 }; 64 * nchan.max(1)];
            if p != 0 && p + 2 <= b.len() {
                let mut i = p + 2;
                let mut row = 0;
                while row < 64 && i < b.len() {
                    let what = b[i];
                    i += 1;
                    if what == 0 {
                        row += 1;
                        continue;
                    }
                    let ch = (what & 31) as usize;
                    let mut c = Cell { note: 255, ins: 0, vol: 255, cmd: 0, info: 0 };
                    if what & 32 != 0 {
                        c.note = *b.get(i)?;
                        c.ins = *b.get(i + 1)?;
                        i += 2;
                    }
                    if what & 64 != 0 {
                        c.vol = *b.get(i)?;
                        i += 1;
                    }
                    if what & 128 != 0 {
                        c.cmd = *b.get(i)?;
                        c.info = *b.get(i + 1)?;
                        i += 2;
                    }
                    if ch < nchan {
                        cells[row * nchan + ch] = c;
                    }
                }
            }
            patterns.push(cells);
        }
        let mut p = S3mPlayer {
            rate,
            samples,
            orders,
            patterns,
            nchan,
            chans,
            amiga: flags & 0x10 != 0,
            speed: (is as u32).max(1),
            tempo: (it as u32).max(32),
            tick: 0,
            row: 0,
            order: 0,
            next_row: None,
            pattern_delay: 0,
            tick_left: 0.0,
            pan_width: PAN_WIDTH,
        };
        if p.orders.is_empty() || p.nchan == 0 {
            return None;
        }
        p.tick = p.speed; // Process row 0 on the first tick.
        Some(p)
    }

    fn period_of(&self, note: u8, c2spd: u32) -> i32 {
        let n = (note & 15) as usize % 12;
        let oct = (note >> 4) as u32;
        (8363 * 16 * (PERIODS[n] >> oct) / c2spd.max(1)) as i32
    }

    /// Advance one row when the speed counter runs out.
    fn tick(&mut self) {
        self.tick += 1;
        if self.tick >= self.speed * (1 + self.pattern_delay) {
            self.tick = 0;
            self.pattern_delay = 0;
            if let Some((o, r)) = self.next_row.take() {
                self.order = o;
                self.row = r;
            }
            if self.order >= self.orders.len() {
                self.order = 0;
            }
            let mut pat = self.orders[self.order] as usize;
            while pat == 255 || pat >= self.patterns.len() {
                self.order = (self.order + 1) % self.orders.len();
                pat = self.orders[self.order] as usize;
                if self.order == 0 && (pat == 255 || pat >= self.patterns.len()) {
                    return;
                }
            }
            let row = self.row;
            for ch in 0..self.nchan {
                let cell = self.patterns[pat][row * self.nchan + ch];
                self.row_cell(ch, cell);
                self.tick0_effects(ch);
            }
            if self.next_row.is_none() {
                self.next_row = Some(if row + 1 >= 64 { (self.order + 1, 0) } else { (self.order, row + 1) });
            }
        } else {
            for ch in 0..self.nchan {
                self.tick_effects(ch);
            }
        }
    }

    fn trigger(&mut self, ch: usize, cell: Cell) {
        let c = &mut self.chans[ch];
        if cell.ins > 0 && (cell.ins as usize) <= self.samples.len() {
            c.sample = cell.ins as usize;
            c.volume = self.samples[c.sample - 1].volume;
        }
        if cell.note == 254 {
            self.chans[ch].playing = false;
        } else if cell.note < 254 && self.chans[ch].sample > 0 {
            let c2 = self.samples[self.chans[ch].sample - 1].c2spd;
            let p = self.period_of(cell.note, c2);
            let c = &mut self.chans[ch];
            if cell.cmd == b'G' - 64 || cell.cmd == b'L' - 64 {
                c.target = p;
            } else {
                c.period = p;
                c.target = p;
                c.pos = 0.0;
                c.playing = true;
                c.vib_pos = 0;
                if cell.cmd == b'O' - 64 {
                    if cell.info != 0 {
                        c.offset_mem = cell.info;
                    }
                    c.pos = c.offset_mem as f64 * 256.0;
                }
            }
        }
        if cell.vol != 255 {
            self.chans[ch].volume = cell.vol.min(63) as i32;
        }
    }

    fn row_cell(&mut self, ch: usize, cell: Cell) {
        {
            let c = &mut self.chans[ch];
            c.vib_delta = 0;
            c.vol_delta = 0;
            c.arp = [0; 3];
            c.cut_tick = -1;
            c.cmd = cell.cmd;
            if cell.info != 0 {
                c.last_info = cell.info;
            }
            c.info = if cell.info != 0 { cell.info } else { c.last_info };
        }
        let (cmd, info) = (cell.cmd + 64, self.chans[ch].info);
        if cmd == b'S' && info >> 4 == 0xD && info & 15 > 0 {
            self.chans[ch].note_delay = Some(Cell { info: info & 15, ..cell });
        } else {
            self.trigger(ch, cell);
        }
        let (x, y) = ((info >> 4) as i32, (info & 15) as i32);
        match cmd {
            // VTAL folds A and T into one command: below 0x31 a speed
            // (from A only), else a tempo.
            b'A' if (1..0x31).contains(&info) => self.speed = info as u32,
            b'A' | b'T' if info >= 0x31 => self.tempo = info as u32,
            b'B' => self.next_row = Some((info as usize, 0)),
            b'C' => {
                // Break to a row of the next order (or of a B jump's target).
                let o = match self.next_row {
                    Some((o, _)) if o != self.order => o,
                    _ => self.order + 1,
                };
                self.next_row = Some((o, (x * 10 + y).min(63) as usize));
            }
            b'D' | b'K' | b'L' => {
                let c = &mut self.chans[ch];
                let raw = cell.info as i32;
                if raw != 0 {
                    let (rx, ry) = (raw >> 4, raw & 15);
                    (c.vslide, c.vslide_fine) = if cmd != b'D' || raw < 0xF1 && !(ry == 0xF && raw > 0xF) {
                        (if ry == 0 { rx } else { -ry }, false)
                    } else if raw >= 0xF1 {
                        (-ry, true)
                    } else {
                        (rx, true)
                    };
                }
                // Fine slides happen once on tick 0; VTAL's normal slides
                // run on tick 0 too.
                c.volume = (c.volume + c.vslide).clamp(0, 63);
            }
            b'E' | b'F' => {
                let d = if cmd == b'E' { 1 } else { -1 };
                let c = &mut self.chans[ch];
                if x == 0xF {
                    c.period += d * y * 4;
                } else if x == 0xE {
                    // VTAL turns extra-fine slides into fine ones.
                    c.period += d * ((y + 2) >> 2) * 4;
                }
                c.period = c.period.max(1);
            }
            b'G' if info > 0 => self.chans[ch].porta_speed = info,
            b'H' if info > 0 => self.chans[ch].vib_info = info,
            b'J' => self.chans[ch].arp = [0, x, y],
            b'Q' => self.chans[ch].retrig = info,
            b'R' if info > 0 => self.chans[ch].trem_info = info,
            b'S' => match x {
                0x8 => self.chans[ch].pan = y * 17,
                0xB => {
                    if y == 0 {
                        self.chans[ch].loop_row = self.row;
                    } else {
                        let c = &mut self.chans[ch];
                        if c.loop_count == 0 {
                            c.loop_count = y as u8;
                            self.next_row = Some((self.order, c.loop_row));
                        } else {
                            c.loop_count -= 1;
                            if c.loop_count > 0 {
                                self.next_row = Some((self.order, c.loop_row));
                            }
                        }
                    }
                }
                0xC => self.chans[ch].cut_tick = y,
                0xE => self.pattern_delay = y as u32,
                _ => {}
            },
            b'X' => self.chans[ch].pan = (info as i32 * 2).min(255),
            _ => {}
        }
    }

    /// VTAL runs vibrato and tone portamento on the row's first tick too.
    fn tick0_effects(&mut self, ch: usize) {
        let c = &self.chans[ch];
        if c.note_delay.is_none() && matches!(c.cmd + 64, b'G' | b'H' | b'K' | b'L') {
            let cmd = c.cmd + 64;
            self.chans[ch].cmd = if matches!(cmd, b'G' | b'L') { b'G' - 64 } else { b'H' - 64 };
            self.tick_effects(ch);
            self.chans[ch].cmd = cmd - 64;
        }
    }

    fn tick_effects(&mut self, ch: usize) {
        let tick = (self.tick % self.speed) as i32;
        if let Some(cell) = self.chans[ch].note_delay
            && tick == cell.info as i32
        {
            self.chans[ch].note_delay = None;
            self.trigger(ch, cell);
        }
        let amiga = self.amiga;
        let c = &mut self.chans[ch];
        if c.cut_tick == tick {
            c.volume = 0;
        }
        let (cmd, info) = (c.cmd + 64, c.info);
        let x = (info >> 4) as i32;
        let vol_slide = |c: &mut Chan| {
            if !c.vslide_fine {
                c.volume = (c.volume + c.vslide).clamp(0, 63);
            }
        };
        let porta = |c: &mut Chan| {
            let sp = c.porta_speed as i32 * 4;
            if c.period < c.target {
                c.period = (c.period + sp).min(c.target);
            } else if c.period > c.target {
                c.period = (c.period - sp).max(c.target);
            }
        };
        let vibrato = |c: &mut Chan| {
            let (spd, depth) = ((c.vib_info >> 4) as i32, (c.vib_info & 15) as i32);
            let s = SINE[(c.vib_pos & 31) as usize] * if c.vib_pos & 32 != 0 { -1 } else { 1 };
            c.vib_delta = s * depth / 32;
            c.vib_pos += spd;
        };
        match cmd {
            b'D' => vol_slide(c),
            b'E' if x < 0xE => c.period += info as i32 * 4,
            b'F' if x < 0xE => c.period = (c.period - info as i32 * 4).max(1),
            b'G' => porta(c),
            b'H' => vibrato(c),
            b'K' => {
                vibrato(c);
                vol_slide(c);
            }
            b'L' => {
                porta(c);
                vol_slide(c);
            }
            b'Q' => {
                let (xx, yy) = ((c.retrig >> 4) as i32, (c.retrig & 15) as i32);
                if yy > 0 && tick % yy == 0 {
                    c.pos = 0.0;
                    c.volume = match xx {
                        1..=5 => c.volume - (1 << (xx - 1)),
                        6 => c.volume * 2 / 3,
                        7 => c.volume / 2,
                        9..=13 => c.volume + (1 << (xx - 9)),
                        14 => c.volume * 3 / 2,
                        15 => c.volume * 2,
                        _ => c.volume,
                    }
                    .clamp(0, 64);
                }
            }
            b'R' => {
                let (spd, depth) = ((c.trem_info >> 4) as i32, (c.trem_info & 15) as i32);
                let s = SINE[(c.trem_pos & 31) as usize] * if c.trem_pos & 32 != 0 { -1 } else { 1 };
                c.vol_delta = s * depth / 64;
                c.trem_pos += spd;
            }
            _ => {}
        }
        if amiga {
            c.period = c.period.clamp(907 >> 1, 1712 << 2);
        }
    }

    /// Mix into interleaved stereo `out` (adding), scaled by `vol`.
    pub fn render(&mut self, out: &mut [f32], vol: f32) {
        let mut frame = 0;
        let frames = out.len() / 2;
        while frame < frames {
            if self.tick_left <= 0.0 {
                self.tick();
                self.tick_left += self.rate as f64 * 2.5 / self.tempo as f64;
            }
            let n = (self.tick_left.ceil() as usize).min(frames - frame).max(1);
            let gain = vol * MASTER;
            let tick = (self.tick % self.speed) as usize;
            for c in self.chans.iter_mut() {
                if !c.playing || c.sample == 0 {
                    continue;
                }
                let s = &self.samples[c.sample - 1];
                if s.data.is_empty() {
                    continue;
                }
                let arp = c.arp[tick % 3];
                let mut period = (c.period + c.vib_delta).max(1) as f64;
                if arp != 0 {
                    period /= 2f64.powf(arp as f64 / 12.0);
                }
                let step = 14317056.0 / period / self.rate as f64;
                let v = (c.volume + c.vol_delta).clamp(0, 64) as f32 / 64.0 * gain;
                // VTAL scales the ±128 pan by PanWidth/256 (measured against
                // DOSBox: the songs' hard pans come out at about ±30).
                let pan = 128 + (c.pan - 128) * self.pan_width / 256;
                let (gl, gr) = (v * (255 - pan) as f32 / 255.0, v * pan as f32 / 255.0);
                let data = &s.data;
                let end = if s.looped { s.loop_end } else { data.len() };
                for f in 0..n {
                    let mut i = c.pos as usize;
                    if i >= end {
                        if s.looped {
                            let len = (s.loop_end - s.loop_start) as f64;
                            c.pos = s.loop_start as f64 + (c.pos - s.loop_start as f64) % len;
                            i = c.pos as usize;
                        } else {
                            c.playing = false;
                            break;
                        }
                    }
                    let fr = (c.pos - i as f64) as f32;
                    let a = data[i];
                    let b = if i + 1 < end { data[i + 1] } else if s.looped { data[s.loop_start] } else { a };
                    let smp = a + (b - a) * fr;
                    out[(frame + f) * 2] += smp * gl;
                    out[(frame + f) * 2 + 1] += smp * gr;
                    c.pos += step;
                }
            }
            frame += n;
            self.tick_left -= n as f64;
        }
    }
}
