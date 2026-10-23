//! Dev tool: two (or more) games in one process playing a network race
//! through an in-memory relay, with scripted keys. Reports the desync
//! checks (state hashes compared between nodes) and saves frames.
//!
//! usage: net_test SPEEDH.JCL OUTDIR FRAMES SCRIPT_A SCRIPT_B [...]
//! Each SCRIPT is `N:KEY,N:+KEY,N:-KEY,N:shot:NAME,...` (tap / press /
//! release a key at frame N, 60 fps). Env NET_DELAY=frames delays every
//! relay message (default 0), NET_ARGS adds game options (default -nologo
//! is not set: the intro is skipped with ESC in the scripts).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::Write;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use speedhaste_core::audio::Audio;
use speedhaste_core::data::{Data, Jcl};
use speedhaste_core::keys;
use speedhaste_core::net::{Pipe, relay};
use speedhaste_core::sys::{Game, NoStorage};

#[derive(Default)]
struct Hub {
    now: u64,
    delay: u64,
    /// Per client: (deliver at frame, frame).
    queues: Vec<VecDeque<(u64, Vec<u8>)>>,
}

struct MemPipe {
    hub: Rc<RefCell<Hub>>,
    id: usize,
}

impl Pipe for MemPipe {
    fn status(&self) -> Result<bool, String> {
        Ok(true)
    }
    fn send(&mut self, msg: Vec<u8>) {
        let mut h = self.hub.borrow_mut();
        let at = h.now + h.delay;
        for (i, q) in h.queues.iter_mut().enumerate() {
            if i != self.id {
                q.push_back((at, relay::frame(relay::DATA, self.id as u32, &msg)));
            }
        }
    }
    fn recv(&mut self) -> Option<Vec<u8>> {
        let mut h = self.hub.borrow_mut();
        let now = h.now;
        let q = &mut h.queues[self.id];
        if q.front().is_some_and(|(at, _)| *at <= now) { q.pop_front().map(|(_, f)| f) } else { None }
    }
}

fn key_code(name: &str) -> u8 {
    (0..=255u8).find(|&k| keys::name(k) == Some(name)).unwrap_or_else(|| panic!("unknown key {name}"))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&args[1]).unwrap();
    let out = args[2].clone();
    let frames: u64 = args[3].parse().unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let hub = Rc::new(RefCell::new(Hub { delay: std::env::var("NET_DELAY").ok().and_then(|d| d.parse().ok()).unwrap_or(0), ..Default::default() }));
    let extra: Vec<String> = std::env::var("NET_ARGS").unwrap_or_default().split_whitespace().map(String::from).collect();

    struct Node {
        game: Game,
        audio: Arc<Mutex<Audio>>,
        script: Vec<(u64, String)>,
    }
    let mut nodes: Vec<Node> = args[4..]
        .iter()
        .map(|script| {
            let data = Data::new(Jcl::parse(bytes.clone()).unwrap());
            let audio = Arc::new(Mutex::new(Audio::new(44100)));
            let mut g = Game::new(data, audio.clone(), Box::new(NoStorage), [&["-seed".to_string(), "7".into()][..], &extra].concat(), speedhaste_core::game::run);
            let h = hub.clone();
            g.set_connector(Box::new(move || {
                let mut hb = h.borrow_mut();
                let id = hb.queues.len();
                let now = hb.now;
                hb.queues.push(VecDeque::from([(now, relay::frame(relay::WELCOME, id as u32, &[]))]));
                Box::new(MemPipe { hub: h.clone(), id }) as Box<dyn Pipe>
            }));
            let script = script
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| {
                    let (n, what) = s.split_once(':').unwrap();
                    (n.parse().unwrap(), what.to_string())
                })
                .collect();
            Node { game: g, audio, script }
        })
        .collect();

    let mut buf = vec![0f32; 2 * 735];
    for f in 1..=frames {
        hub.borrow_mut().now = f;
        for (i, n) in nodes.iter_mut().enumerate() {
            for (at, what) in &n.script {
                if *at == f {
                    if let Some(name) = what.strip_prefix("shot:") {
                        let fr = n.game.front().unwrap();
                        let path = format!("{out}/{}_{name}.ppm", (b'a' + i as u8) as char);
                        let mut w = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
                        write!(w, "P6\n{} {}\n255\n", fr.w, fr.h).unwrap();
                        for &c in &fr.pix {
                            w.write_all(&fr.pal.rgba(c)[..3]).unwrap();
                        }
                    } else if let Some(k) = what.strip_prefix('+') {
                        n.game.key(key_code(k), true);
                    } else if let Some(k) = what.strip_prefix('-') {
                        n.game.key(key_code(k), false);
                    } else {
                        n.game.key(key_code(what), true);
                    }
                } else if *at + 1 == f && !what.starts_with(['+', '-']) && !what.starts_with("shot:") {
                    n.game.key(key_code(what), false);
                }
            }
            n.audio.lock().unwrap().render(&mut buf);
            n.game.frame(f as f64 * 1000.0 / 60.0);
        }
        if f % 600 == 0 {
            let c: Vec<_> = nodes.iter().map(|n| n.game.net_checks()).collect();
            eprintln!("frame {f}: checks (done, failed) per node {c:?}");
        }
    }
    let c: Vec<_> = nodes.iter().map(|n| n.game.net_checks()).collect();
    println!("checks (done, failed) per node: {c:?}");
}
