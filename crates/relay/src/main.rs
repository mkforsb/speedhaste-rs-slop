//! Speed Haste network relay: a WebSocket server for network races (the
//! port's stand-in for the original's IPX/serial/modem links).
//!
//! usage: speedhaste-relay [ADDRESS:PORT]   (default 0.0.0.0:8790)
//!
//! Clients connecting to the same URL path (`ws://host:8790/myroom`) share
//! a room; each binary message is passed to the room's other clients
//! tagged with the sender's id (framing in `speedhaste_core::net::relay`).
//! It knows nothing about the game.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use speedhaste_core::net::relay;
use tungstenite::{Error, Message};

/// Rooms by path: the clients' ids and outgoing queues.
type Rooms = Arc<Mutex<HashMap<String, Vec<(u32, Sender<Vec<u8>>)>>>>;

fn main() {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "0.0.0.0:8790".into());
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("can't listen on {addr}: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("speedhaste-relay listening on {addr}");
    let rooms: Rooms = Arc::default();
    let next_id = Arc::new(AtomicU32::new(1));
    for stream in listener.incoming().flatten() {
        let (rooms, id) = (rooms.clone(), next_id.fetch_add(1, Ordering::Relaxed));
        std::thread::spawn(move || {
            if let Err(e) = client(stream, &rooms, id) {
                eprintln!("client {id}: {e}");
            }
        });
    }
}

fn broadcast(rooms: &Rooms, room: &str, from: u32, frame: &[u8]) {
    if let Some(clients) = rooms.lock().unwrap().get(room) {
        for (id, tx) in clients {
            if *id != from {
                let _ = tx.send(frame.to_vec());
            }
        }
    }
}

// The handshake callback's error type is tungstenite's.
#[allow(clippy::result_large_err)]
fn client(stream: TcpStream, rooms: &Rooms, id: u32) -> Result<(), Error> {
    let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
    stream.set_nodelay(true)?;
    let mut room = String::new();
    let mut ws = tungstenite::accept_hdr(stream, |req: &tungstenite::handshake::server::Request, resp| {
        room = req.uri().path().to_string();
        Ok(resp)
    })
    .map_err(|e| match e {
        tungstenite::HandshakeError::Failure(e) => e,
        tungstenite::HandshakeError::Interrupted(_) => Error::ConnectionClosed,
    })?;
    // Poll for messages and the outgoing queue in turn.
    ws.get_ref().set_read_timeout(Some(Duration::from_millis(2)))?;
    let (tx, rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = channel();
    {
        let mut r = rooms.lock().unwrap();
        let clients = r.entry(room.clone()).or_default();
        for (_, other) in clients.iter() {
            let _ = other.send(relay::frame(relay::JOINED, id, &[]));
        }
        clients.push((id, tx));
        eprintln!("client {id} ({peer}) joined room {room:?}: {} clients", clients.len());
    }
    ws.send(Message::Binary(relay::frame(relay::WELCOME, id, &[]).into()))?;
    let result = (|| loop {
        match ws.read() {
            Ok(Message::Binary(b)) => broadcast(rooms, &room, id, &relay::frame(relay::DATA, id, &b)),
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => {}
            Err(Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(Error::ConnectionClosed) => return Ok(()),
            Err(e) => return Err(e),
        }
        let mut wrote = false;
        while let Ok(m) = rx.try_recv() {
            ws.write(Message::Binary(m.into()))?;
            wrote = true;
        }
        if wrote {
            ws.flush()?;
        }
    })();
    {
        let mut r = rooms.lock().unwrap();
        if let Some(clients) = r.get_mut(&room) {
            clients.retain(|(c, _)| *c != id);
            eprintln!("client {id} left room {room:?}: {} clients", clients.len());
            if clients.is_empty() {
                r.remove(&room);
            }
        }
    }
    broadcast(rooms, &room, id, &relay::frame(relay::LEFT, id, &[]));
    result
}
