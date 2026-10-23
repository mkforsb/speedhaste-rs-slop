//! Dev tool: run the game without a window, with simulated time and scripted
//! key presses, saving frames as PPM images.
//!
//! usage: headless SPEEDH.JCL SCRIPT OUTDIR [game options...]
//! Env HEADLESS_CFG: SPEED.CFG lines to start from, e.g. "MusicOn No".
//!
//! SCRIPT is a comma-separated list of steps executed in order:
//!   `wait:N`      advance N frames (at 60 fps)
//!   `key:NAME`    tap a key (press, one frame, release), e.g. key:ENTER
//!   `down:NAME` / `up:NAME`  hold / release a key
//!   `shot:NAME`   save the current frame to OUTDIR/NAME.ppm
//!   `raw:NAME`    save the indexed frame and palette (NAME.raw, NAME.pal)
//!   `audio:PATH`  record the audio from here on (raw f32le stereo 44100 Hz)
//!   `pad:SPEC`    set gamepad 1 to SPEC, `/`-separated: `lx=-1/rt=1/SOUTH`
//!                 (axes lx ly lt rt, button names from pad.rs); `pad:` = idle

use speedhaste_core::audio::Audio;
use speedhaste_core::data::{Data, Jcl};
use speedhaste_core::keys;
use speedhaste_core::pad::{self, Pad};
use speedhaste_core::sys::Game;
use std::io::Write;
use std::sync::{Arc, Mutex};

fn key_code(name: &str) -> u8 {
    (0..=255u8).find(|&k| keys::name(k) == Some(name)).unwrap_or_else(|| panic!("unknown key {name}"))
}

/// In-memory storage, optionally seeded with a config.
struct MemStorage(std::collections::HashMap<String, String>);

impl speedhaste_core::sys::Storage for MemStorage {
    fn load(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn save(&mut self, key: &str, value: &str) {
        self.0.insert(key.to_string(), value.to_string());
    }
}

fn pad_state(spec: &str) -> Pad {
    let mut p = Pad { connected: true, ..Default::default() };
    for item in spec.split('/').filter(|s| !s.is_empty()) {
        if let Some((axis, v)) = item.split_once('=') {
            let v: f32 = v.parse().unwrap();
            match axis {
                "lx" => p.lx = v,
                "ly" => p.ly = v,
                "lt" => p.lt = v,
                "rt" => p.rt = v,
                _ => panic!("unknown axis {axis}"),
            }
            continue;
        }
        p.buttons |= match item {
            "SOUTH" => pad::SOUTH, "EAST" => pad::EAST, "WEST" => pad::WEST, "NORTH" => pad::NORTH,
            "LB" => pad::LB, "RB" => pad::RB, "BACK" => pad::BACK, "START" => pad::START,
            "UP" => pad::UP, "DOWN" => pad::DOWN, "LEFT" => pad::LEFT, "RIGHT" => pad::RIGHT,
            _ => panic!("unknown pad button {item}"),
        };
    }
    p
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data = Data::new(Jcl::parse(std::fs::read(&args[1]).unwrap()).unwrap());
    let script = args[2].clone();
    let out = args[3].clone();
    std::fs::create_dir_all(&out).unwrap();
    let audio = Arc::new(Mutex::new(Audio::new(44100)));
    let storage = MemStorage(std::env::var("HEADLESS_CFG").ok().map(|c| ("speed.cfg".to_string(), c)).into_iter().collect());
    let mut game = Game::new(data, audio.clone(), Box::new(storage), args[4..].to_vec(), speedhaste_core::game::run);
    let mut t = 0.0;
    let mut buf = vec![0f32; 2 * 735];
    let mut rec: Option<std::io::BufWriter<std::fs::File>> = None;
    let mut rumbles = Vec::new();
    let mut step = |game: &mut Game, rec: &mut Option<std::io::BufWriter<std::fs::File>>| {
        t += 1000.0 / 60.0;
        audio.lock().unwrap().render(&mut buf);
        if let Some(w) = rec.as_mut() {
            for v in &buf {
                w.write_all(&v.to_le_bytes()).unwrap();
            }
        }
        let alive = game.frame(t);
        if let Some(r) = game.take_rumble(0) {
            rumbles.push(r);
        }
        alive
    };
    for cmd in script.split(',') {
        let (op, arg) = cmd.split_once(':').unwrap_or((cmd, ""));
        match op {
            "wait" => {
                for _ in 0..arg.parse::<u32>().unwrap() {
                    if !step(&mut game, &mut rec) {
                        eprintln!("game ended");
                        return;
                    }
                }
            }
            "key" => {
                game.key(key_code(arg), true);
                step(&mut game, &mut rec);
                game.key(key_code(arg), false);
                step(&mut game, &mut rec);
            }
            "down" => game.key(key_code(arg), true),
            "up" => game.key(key_code(arg), false),
            "pad" => game.pad(0, pad_state(arg)),
            "audio" => rec = Some(std::io::BufWriter::new(std::fs::File::create(arg).unwrap())),
            "raw" => {
                let f = game.front().expect("nothing drawn yet");
                std::fs::write(format!("{out}/{arg}.raw"), &f.pix).unwrap();
                std::fs::write(format!("{out}/{arg}.pal"), f.pal.0).unwrap();
            }
            "shot" => {
                let f = game.front().expect("nothing drawn yet");
                let path = format!("{out}/{arg}.ppm");
                let mut w = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
                write!(w, "P6\n{} {}\n255\n", f.w, f.h).unwrap();
                for &c in &f.pix {
                    w.write_all(&f.pal.rgba(c)[..3]).unwrap();
                }
                eprintln!("saved {path} pal[16..18]={:?}", &f.pal.0[48..54]);
            }
            _ => panic!("bad script op {op}"),
        }
    }
    if !rumbles.is_empty() {
        let strong = rumbles.iter().filter(|r| r.strong > 0.0).count();
        eprintln!("gamepad 1 rumble: {} requests ({} strong)", rumbles.len(), strong);
    }
}
