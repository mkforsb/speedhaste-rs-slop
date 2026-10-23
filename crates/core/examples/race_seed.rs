//! Dev tool: find the startup seed (the original's BIOS_Clock) that makes
//! our race match a frame captured from the original in DOSBox.
//! usage: race_seed SPEEDH.JCL TARGET.ppm FROM TO MAXTICKS
//! Runs -nologo -seed N into a single race (first circuit and car) and, at
//! one frame per game tick, reports the best-matching frame for each seed.
use speedhaste_core::audio::Audio;
use speedhaste_core::data::{Data, Jcl};
use speedhaste_core::keys;
use speedhaste_core::sys::{Game, NoStorage};
use std::sync::{Arc, Mutex};

fn ppm(path: &str) -> Vec<u8> {
    let b = std::fs::read(path).unwrap();
    // "P6\nW H\n255\n" header
    let mut nl = 0;
    let start = b.iter().position(|&c| {
        nl += (c == b'\n') as i32;
        nl == 3
    });
    b[start.unwrap() + 1..].to_vec()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let jcl = std::fs::read(&a[1]).unwrap();
    let target = ppm(&a[2]);
    let (from, to, max_ticks): (u32, u32, u32) = (a[3].parse().unwrap(), a[4].parse().unwrap(), a[5].parse().unwrap());
    for seed in from..=to {
        let audio = Arc::new(Mutex::new(Audio::new(11025)));
        let args = ["-nologo", "-seed", &seed.to_string()].map(String::from).to_vec();
        let mut game = Game::new(Data::new(Jcl::parse(jcl.clone()).unwrap()), audio.clone(), Box::new(NoStorage), args, speedhaste_core::game::run);
        let mut t = 0.0;
        // Music fades only progress as audio is rendered.
        let mut buf = vec![0f32; 2 * 158];
        let mut step = |g: &mut Game, n: u32| {
            for _ in 0..n {
                t += 1000.0 / 70.0;
                audio.lock().unwrap().render(&mut buf);
                g.frame(t);
            }
        };
        let tap = |g: &mut Game, k: u8, step: &mut dyn FnMut(&mut Game, u32)| {
            g.key(k, true);
            step(g, 1);
            g.key(k, false);
            step(g, 1);
        };
        step(&mut game, 700);
        tap(&mut game, keys::ENTER, &mut step);
        step(&mut game, 70);
        tap(&mut game, keys::DARROW, &mut step);
        tap(&mut game, keys::ENTER, &mut step);
        step(&mut game, 350);
        tap(&mut game, keys::ENTER, &mut step);
        step(&mut game, 400);
        tap(&mut game, keys::ENTER, &mut step);
        let mut best = (u64::MAX, 0);
        for tick in 0..max_ticks {
            step(&mut game, 1);
            if let Some(f) = game.front()
                && f.w == 320
            {
                let lut = f.pal.to_rgba_table();
                let diff: u64 = f.pix.iter().enumerate().map(|(i, &c)| (lut[c as usize][..3] != target[i * 3..i * 3 + 3]) as u64).sum();
                if diff < best.0 && std::env::var_os("RACE_SEED_DUMP").is_some() {
                    let mut out = format!("P6\n{} {}\n255\n", f.w, f.h).into_bytes();
                    out.extend(f.pix.iter().flat_map(|&c| lut[c as usize][..3].to_vec()));
                    std::fs::write(format!("/tmp/race_seed_{seed}.ppm"), out).unwrap();
                }
                if let Some(t) = std::env::var("RACE_SEED_SAVE_TICK").ok().and_then(|v| v.parse::<u32>().ok())
                    && t == tick
                {
                    let mut out = format!("P6\n{} {}\n255\n", f.w, f.h).into_bytes();
                    out.extend(f.pix.iter().flat_map(|&c| lut[c as usize][..3].to_vec()));
                    std::fs::write(format!("/tmp/race_seed_{seed}_t{tick}.ppm"), out).unwrap();
                }
                if std::env::var_os("RACE_SEED_TRACE").is_some() && diff < 5000 {
                    println!("  tick {tick}: {diff}");
                }
                best = best.min((diff, tick));
            }
        }
        println!("seed {seed}: best {} differing pixels at tick {}", best.0, best.1);
    }
}
