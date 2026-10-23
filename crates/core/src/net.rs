//! Network play (game/net.c), over a WebSocket relay instead of IPX or a
//! serial/modem link — a port addition in transport, the original's game
//! protocol otherwise.
//!
//! Like the original, the race runs in lockstep: every node simulates the
//! whole race from the same seed and setup, and only the human players'
//! per-tick controls travel. A node simulates tick `t` once it knows every
//! human's controls for `t` (`Player::clock`), and samples its own player
//! at most [`NET_LEAD`] ticks ahead of the slowest remote one.
//!
//! The relay (crates/relay) puts the clients of one URL into a room and
//! forwards each message to the room's other clients; see [`relay`]. The
//! platform provides the connection as a [`Pipe`] of binary messages.
//!
//! Nodes are numbered by the master (who started the game): 0 is the
//! master, the rest in order of arrival. Human player `n` is node `n`'s.

use std::collections::VecDeque;

use crate::keys;
use crate::race::control::UserControl;
use crate::sys::Sys;

/// NET_MAXNODES.
pub const MAX_NODES: usize = 4;
/// How far (ticks) the local player may run ahead of the slowest remote.
pub const NET_LEAD: u32 = 10;

/// A message connection to the relay, provided by the platform.
pub trait Pipe {
    /// `Ok(true)` once open, `Ok(false)` while connecting, `Err` when it
    /// failed or closed.
    fn status(&self) -> Result<bool, String>;
    fn send(&mut self, msg: Vec<u8>);
    fn recv(&mut self) -> Option<Vec<u8>>;
}

/// Opens a connection to the configured relay (set by the platform).
pub type Connector = Box<dyn FnMut() -> Box<dyn Pipe>>;

/// Relay framing. Clients send plain payloads, which go to every other
/// client of the room; the relay sends `[kind, id u32 LE, payload...]`.
pub mod relay {
    /// A payload from client `id`.
    pub const DATA: u8 = 0;
    /// Sent once on connecting: `id` is yours.
    pub const WELCOME: u8 = 1;
    pub const JOINED: u8 = 2;
    pub const LEFT: u8 = 3;

    pub fn frame(kind: u8, id: u32, payload: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(5 + payload.len());
        v.push(kind);
        v.extend_from_slice(&id.to_le_bytes());
        v.extend_from_slice(payload);
        v
    }
}

/// The race setup the master announces (NET_TSyncPacket).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Setup {
    pub racemode: i32,
    pub circuit: usize,
    pub laps: i32,
    pub timed: bool,
    pub car_type: i32,
    pub seed: u32,
    /// Port addition: decorations take thing slots and RNG decisions, so
    /// everyone uses the master's.
    pub decoration: i32,
    pub no_collide: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    /// NETC_SYNC: "I'm here"; the master's carries the setup.
    Sync { master: bool, setup: Setup },
    /// NETC_START: the master's node order (relay ids).
    Start { nodes: Vec<u32> },
    /// NETC_GAMEINFO: my car and keys (the original sent the keys in
    /// NETC_SYNC: gear changes compare scancodes with them), and how many
    /// nodes' cars I know.
    GameInfo { car: u8, keys: [u8; 5], known: u8 },
    /// NETC_START before a race (NET_SyncStart): nodes ready for race `race`.
    Ready { race: u32, ok: u8 },
    /// NETC_INFO: my player's controls from tick `tick` on, in race
    /// number `race` (late ones from a previous race must not count).
    Ctl { race: u32, tick: u32, ctl: Vec<UserControl> },
    /// Port addition: a hash of the race state after tick `tick`.
    Check { race: u32, tick: u32, hash: u32 },
    /// NETC_BYE: leaving (abandoned championship, quit race, menu).
    Bye,
}

const M_SYNC: u8 = 1;
const M_START: u8 = 2;
const M_GAMEINFO: u8 = 3;
const M_READY: u8 = 4;
const M_CTL: u8 = 5;
const M_CHECK: u8 = 6;
const M_BYE: u8 = 7;

impl Msg {
    fn encode(&self) -> Vec<u8> {
        let mut v = Vec::new();
        let u32le = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
        match self {
            Msg::Sync { master, setup: s } => {
                v.extend([M_SYNC, *master as u8, s.racemode as u8, s.circuit as u8, s.laps as u8, s.timed as u8, s.car_type as u8]);
                u32le(&mut v, s.seed);
                v.extend([s.decoration as u8, s.no_collide as u8]);
            }
            Msg::Start { nodes } => {
                v.extend([M_START, nodes.len() as u8]);
                for &n in nodes {
                    u32le(&mut v, n);
                }
            }
            Msg::GameInfo { car, keys, known } => {
                v.extend([M_GAMEINFO, *car, *known]);
                v.extend(keys);
            }
            Msg::Ready { race, ok } => {
                v.push(M_READY);
                u32le(&mut v, *race);
                v.push(*ok);
            }
            Msg::Ctl { race, tick, ctl } => {
                v.push(M_CTL);
                u32le(&mut v, *race);
                u32le(&mut v, *tick);
                for c in ctl {
                    v.extend([c.scancode, c.control, c.analog_x as u8, c.analog_y as u8]);
                }
            }
            Msg::Check { race, tick, hash } => {
                v.push(M_CHECK);
                u32le(&mut v, *race);
                u32le(&mut v, *tick);
                u32le(&mut v, *hash);
            }
            Msg::Bye => v.push(M_BYE),
        }
        v
    }

    fn decode(b: &[u8]) -> Option<Msg> {
        let u32at = |i: usize| b.get(i..i + 4).map(|x| u32::from_le_bytes(x.try_into().unwrap()));
        Some(match *b.first()? {
            M_SYNC if b.len() >= 13 => Msg::Sync {
                master: b[1] != 0,
                setup: Setup {
                    racemode: b[2] as i32,
                    circuit: b[3] as usize,
                    laps: b[4] as i32,
                    timed: b[5] != 0,
                    car_type: b[6] as i32,
                    seed: u32at(7)?,
                    decoration: b[11] as i32,
                    no_collide: b[12] != 0,
                },
            },
            M_START => {
                let n = *b.get(1)? as usize;
                Msg::Start { nodes: (0..n).map(|i| u32at(2 + 4 * i)).collect::<Option<_>>()? }
            }
            M_GAMEINFO if b.len() >= 8 => Msg::GameInfo { car: b[1], known: b[2], keys: b[3..8].try_into().ok()? },
            M_READY if b.len() >= 6 => Msg::Ready { race: u32at(1)?, ok: b[5] },
            M_CTL if b.len() >= 9 => Msg::Ctl {
                race: u32at(1)?,
                tick: u32at(5)?,
                ctl: b[9..]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| UserControl { scancode: c[0], control: c[1], analog_x: c[2] as i8, analog_y: c[3] as i8 })
                    .collect(),
            },
            M_CHECK => Msg::Check { race: u32at(1)?, tick: u32at(5)?, hash: u32at(9)? },
            M_BYE => Msg::Bye,
            _ => return None,
        })
    }
}

/// What the race gets from the network each frame.
pub enum RaceEvent {
    /// Controls of node `node`'s player from tick `tick` on.
    Ctl { node: usize, tick: u32, ctl: Vec<UserControl> },
    Check { node: usize, tick: u32, hash: u32 },
    /// A node left or the connection broke.
    Lost,
}

/// A network game in progress (NET_State != NETS_NONE).
pub struct Net {
    pipe: Box<dyn Pipe>,
    /// Our relay id, once welcomed.
    me: Option<u32>,
    /// NET_Slave.
    pub slave: bool,
    /// Relay ids by node number, once the master started the game.
    pub nodes: Vec<u32>,
    /// Car model and keys (up, down, left, right, gear) per node
    /// (NET_GetCars).
    pub cars: Vec<usize>,
    pub keys: Vec<[u8; 5]>,
    /// Races synchronized so far (NET_SyncStart).
    races: u32,
    /// Received but not yet handled: (relay id, message).
    inbox: VecDeque<(u32, Msg)>,
    /// Relay ids that left the room.
    left: Vec<u32>,
    error: Option<String>,
    /// We said bye.
    gone: bool,
}

impl Net {
    pub fn new(pipe: Box<dyn Pipe>, slave: bool) -> Net {
        Net { pipe, me: None, slave, nodes: Vec::new(), cars: Vec::new(), keys: Vec::new(), races: 0, inbox: VecDeque::new(), left: Vec::new(), error: None, gone: false }
    }

    /// Our node number.
    pub fn node(&self) -> usize {
        self.me.and_then(|m| self.nodes.iter().position(|&n| n == m)).unwrap_or(0)
    }

    fn send(&mut self, m: &Msg) {
        if self.pipe.status() == Ok(true) {
            self.pipe.send(m.encode());
        }
    }

    /// COM_Housekeep + COM_GetPacket: move everything received to the inbox.
    fn pump(&mut self) {
        if let Err(e) = self.pipe.status() {
            self.error.get_or_insert(e);
        }
        while let Some(f) = self.pipe.recv() {
            if f.len() < 5 {
                continue;
            }
            let id = u32::from_le_bytes(f[1..5].try_into().unwrap());
            match f[0] {
                relay::WELCOME => self.me = Some(id),
                relay::LEFT => self.left.push(id),
                relay::DATA => {
                    if let Some(m) = Msg::decode(&f[5..]) {
                        self.inbox.push_back((id, m));
                    }
                }
                _ => {}
            }
        }
    }

    /// Take the first inbox message `f` accepts (and maps).
    fn take<T>(&mut self, mut f: impl FnMut(u32, &Msg) -> Option<T>) -> Option<T> {
        let i = self.inbox.iter().position(|(id, m)| f(*id, m).is_some())?;
        let (id, m) = self.inbox.remove(i)?;
        f(id, &m)
    }

    /// A node of this game left, or we did (or the connection broke).
    fn lost(&self) -> bool {
        self.gone
            || self.error.is_some()
            || self.nodes.iter().any(|n| self.left.contains(n)) || self.inbox.iter().any(|(id, m)| *m == Msg::Bye && self.nodes.contains(id))
    }

    /// Tell the others we leave (NETC_BYE); the connection closes when the
    /// Net is dropped.
    pub fn bye(&mut self) {
        self.send(&Msg::Bye);
        self.gone = true;
    }

    // ---- In the race (RACE_HandleComms) ---------------------------------

    /// Send our player's controls from tick `tick` on.
    pub fn send_ctl(&mut self, tick: u32, ctl: Vec<UserControl>) {
        self.send(&Msg::Ctl { race: self.races, tick, ctl });
    }

    pub fn send_check(&mut self, tick: u32, hash: u32) {
        self.send(&Msg::Check { race: self.races, tick, hash });
    }

    /// Everything that arrived for the race.
    pub fn race_events(&mut self) -> Vec<RaceEvent> {
        self.pump();
        let mut out = Vec::new();
        if self.lost() {
            out.push(RaceEvent::Lost);
        }
        let nodes = self.nodes.clone();
        let node = |id: u32| nodes.iter().position(|&n| n == id);
        // Leftovers of earlier races are dropped (the others can't be
        // ahead: they wait for us in NET_SyncStart).
        let race = self.races;
        self.inbox.retain(|(_, m)| !matches!(m, Msg::Ctl { race: r, .. } | Msg::Check { race: r, .. } if *r != race));
        while let Some(e) = self.take(|id, m| match m {
            Msg::Ctl { tick, ctl, .. } => Some(RaceEvent::Ctl { node: node(id)?, tick: *tick, ctl: ctl.clone() }),
            Msg::Check { tick, hash, .. } => Some(RaceEvent::Check { node: node(id)?, tick: *tick, hash: *hash }),
            _ => None,
        }) {
            out.push(e);
        }
        out
    }
}

// ---- Lobby (NET_ContactPlayers, NET_GetCars, NET_SyncStart) -------------

/// NetShow: the "Contacting players" box.
fn show(sys: &mut Sys, fonts: &crate::Fonts, red: bool, text: &str) {
    let x = sys.screen.w as i32 / 2 - 160;
    let y = sys.screen.h as i32 / 2 - 100;
    let s = &mut sys.screen;
    s.rectangle(x + 20, y + 70, 280, 60, 15, -1);
    s.rectangle(x + 21, y + 71, 278, 58, 7, -1);
    s.rectangle(x + 22, y + 72, 276, 56, 8, -1);
    s.rectangle(x + 23, y + 73, 274, 54, 7, 8);
    fonts.yellow.write(s, x + 25, y + 75, "Contacting players", 15);
    fonts.yellow.write(s, x + 40, y + 115, "ESC to Abort", 15);
    let f = if red { &fonts.red } else { &fonts.white };
    f.write(s, x + 26, y + 95, text, 15);
    sys.update();
}

/// Show an error for a second and fail.
async fn fail(sys: &mut Sys, fonts: &crate::Fonts, text: &str) -> bool {
    show(sys, fonts, true, text);
    sys.wait_ticks(70).await;
    sys.last_scan = 0;
    false
}

/// Wait for the relay connection. False if it failed or ESC was pressed.
pub async fn connect(sys: &mut Sys, fonts: &crate::Fonts, net: &mut Net) -> bool {
    sys.last_scan = 0;
    loop {
        net.pump();
        match net.pipe.status() {
            Ok(true) if net.me.is_some() => return true,
            Err(e) => return fail(sys, fonts, &format!("No connection: {}", short(&e))).await,
            _ => {}
        }
        if sys.last_scan == keys::ESC {
            sys.last_scan = 0;
            return false;
        }
        show(sys, fonts, false, "Connecting to the server");
        sys.vsync(1).await;
    }
}

fn short(e: &str) -> String {
    e.chars().take(22).collect()
}

/// NET_ContactPlayers: find the other nodes. The master announces `setup`
/// and starts the game with Enter; slaves receive it. False if aborted.
pub async fn contact_players(sys: &mut Sys, fonts: &crate::Fonts, net: &mut Net, setup: &mut Setup) -> bool {
    let mut others: Vec<u32> = Vec::new();
    let mut master: Option<u32> = None;
    sys.vsync(0).await;
    sys.last_scan = 0;
    let mut j = 70;
    loop {
        if sys.last_scan == keys::ESC {
            sys.last_scan = 0;
            return false;
        }
        // The master starts the game.
        if !net.slave && sys.last_scan != 0 && !others.is_empty() {
            let me = net.me.unwrap_or(0);
            net.nodes = std::iter::once(me).chain(others.iter().copied()).collect();
            net.send(&Msg::Start { nodes: net.nodes.clone() });
            break;
        }
        sys.last_scan = 0;
        if j >= 30 {
            net.send(&Msg::Sync { master: !net.slave, setup: *setup });
            j = 0;
        }
        j += sys.vsync(1).await;
        let n = others.len() + 1 + (net.slave && master.is_none()) as usize;
        let text = if !net.slave && !others.is_empty() { format!("Press Enter to accept {n} nodes") } else { format!("{n} nodes registered") };
        show(sys, fonts, false, &text);
        net.pump();
        if let Some(e) = net.error.clone() {
            return fail(sys, fonts, &format!("Lost: {}", short(&e))).await;
        }
        others.retain(|o| !net.left.contains(o));
        if master.is_some_and(|m| net.left.contains(&m)) {
            return fail(sys, fonts, "The master left").await;
        }
        while let Some((id, m)) = net.take(|id, m| matches!(m, Msg::Sync { .. } | Msg::Start { .. }).then(|| (id, m.clone()))) {
            match m {
                Msg::Sync { master: true, .. } if !net.slave => return fail(sys, fonts, "Only one master allowed").await,
                Msg::Sync { master: true, setup: s } => {
                    if master.is_some_and(|m| m != id) {
                        return fail(sys, fonts, "Only one master allowed").await;
                    }
                    master = Some(id);
                    *setup = s;
                }
                Msg::Sync { master: false, .. } => {
                    if !others.contains(&id) && others.len() + 1 < MAX_NODES {
                        others.push(id);
                    }
                }
                Msg::Start { nodes } if net.slave && Some(id) == master => {
                    if !net.me.is_some_and(|me| nodes.contains(&me)) {
                        return fail(sys, fonts, "Game started without you").await;
                    }
                    net.nodes = nodes;
                    sys.last_scan = 0;
                    return true;
                }
                _ => {}
            }
        }
    }
    sys.last_scan = 0;
    true
}

/// NET_GetCars: swap car choices (and keys) until every node knows every
/// node's.
pub async fn get_cars(sys: &mut Sys, fonts: &crate::Fonts, net: &mut Net, car: usize, keys: [u8; 5]) -> bool {
    let n = net.nodes.len();
    let me = net.node();
    let mut cars: Vec<Option<(usize, [u8; 5])>> = vec![None; n];
    cars[me] = Some((car, keys));
    let mut done = vec![false; n];
    done[me] = true;
    sys.vsync(0).await;
    sys.last_scan = 0;
    let mut j = 70;
    loop {
        if sys.last_scan == keys::ESC {
            sys.last_scan = 0;
            net.bye();
            return false;
        }
        if j >= 30 {
            let known = cars.iter().filter(|c| c.is_some()).count() as u8;
            net.send(&Msg::GameInfo { car: car as u8, keys, known });
            if done.iter().all(|&d| d) {
                break;
            }
            j = 0;
        }
        j += sys.vsync(1).await;
        show(sys, fonts, false, &format!("{} nodes contacted", done.iter().filter(|&&d| d).count()));
        net.pump();
        if net.lost() {
            return fail(sys, fonts, "Connection lost").await;
        }
        let nodes = net.nodes.clone();
        while let Some((node, c, ks, k)) = net.take(|id, m| match m {
            Msg::GameInfo { car, keys, known } => Some((nodes.iter().position(|&x| x == id)?, *car, *keys, *known)),
            _ => None,
        }) {
            cars[node] = Some((c as usize, ks));
            if k as usize == n {
                done[node] = true;
            }
        }
    }
    net.cars = cars.iter().map(|c| c.map_or(0, |c| c.0)).collect();
    net.keys = cars.iter().map(|c| c.map_or([0; 5], |c| c.1)).collect();
    sys.last_scan = 0;
    true
}

/// NET_SyncStart: wait until every node is about to start the race.
pub async fn sync_start(sys: &mut Sys, fonts: &crate::Fonts, net: &mut Net) -> bool {
    let n = net.nodes.len();
    let me = net.node();
    let race = net.races;
    let mut seen = vec![false; n];
    let mut done = vec![false; n];
    (seen[me], done[me]) = (true, true);
    sys.last_scan = 0;
    let mut j = 70;
    loop {
        if sys.last_scan == keys::ESC {
            sys.last_scan = 0;
            net.bye();
            return false;
        }
        if j >= 70 {
            let ok = seen.iter().filter(|&&s| s).count() as u8;
            net.send(&Msg::Ready { race, ok });
            if done.iter().all(|&d| d) {
                break;
            }
            j = 0;
        }
        j += sys.vsync(1).await;
        show(sys, fonts, false, &format!("{} nodes contacted", seen.iter().filter(|&&s| s).count()));
        net.pump();
        if net.lost() {
            return fail(sys, fonts, "Connection lost").await;
        }
        let nodes = net.nodes.clone();
        while let Some((node, ok)) = net.take(|id, m| match m {
            Msg::Ready { race: r, ok } if *r == race => Some((nodes.iter().position(|&x| x == id)?, *ok)),
            _ => None,
        }) {
            seen[node] = true;
            if ok as usize == n {
                done[node] = true;
            }
        }
    }
    // Leftovers of the lobby won't be needed any more.
    net.inbox.retain(|(_, m)| !matches!(m, Msg::Sync { .. } | Msg::Start { .. } | Msg::GameInfo { .. }) && !matches!(m, Msg::Ready { race: r, .. } if *r <= race));
    net.races += 1;
    sys.last_scan = 0;
    true
}

/// Between races of a championship: has another node abandoned it?
pub fn someone_left(net: &mut Net) -> bool {
    net.pump();
    net.lost()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip() {
        let setup = Setup { racemode: 1, circuit: 3, laps: 5, timed: true, car_type: 1, seed: 0xDEADBEEF, decoration: 12, no_collide: false };
        let ctl = vec![UserControl { scancode: 1, control: 5, analog_x: -16, analog_y: 3 }; 3];
        for m in [
            Msg::Sync { master: true, setup },
            Msg::Start { nodes: vec![7, 9, 12] },
            Msg::GameInfo { car: 4, keys: [1, 2, 3, 4, 5], known: 2 },
            Msg::Ready { race: 3, ok: 2 },
            Msg::Ctl { race: 2, tick: 1234, ctl },
            Msg::Check { race: 2, tick: 70, hash: 0x12345678 },
            Msg::Bye,
        ] {
            assert_eq!(Msg::decode(&m.encode()), Some(m));
        }
    }
}
