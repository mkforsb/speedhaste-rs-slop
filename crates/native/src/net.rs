//! The relay connection for network races: a WebSocket served by a
//! background thread (blocking tungstenite, polled every few ms).

use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use speedhaste_core::net::Pipe;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Error, Message};

pub struct WsPipe {
    tx: Sender<Vec<u8>>,
    rx: Receiver<Vec<u8>>,
    status: Arc<Mutex<Result<bool, String>>>,
}

impl Pipe for WsPipe {
    fn status(&self) -> Result<bool, String> {
        self.status.lock().unwrap().clone()
    }
    fn send(&mut self, msg: Vec<u8>) {
        let _ = self.tx.send(msg);
    }
    fn recv(&mut self) -> Option<Vec<u8>> {
        self.rx.try_recv().ok()
    }
}

fn tcp(s: &MaybeTlsStream<TcpStream>) -> Option<&TcpStream> {
    match s {
        MaybeTlsStream::Plain(s) => Some(s),
        MaybeTlsStream::Rustls(s) => Some(s.get_ref()),
        _ => None,
    }
}

/// Connect to the relay at `url` (ws:// or wss://).
pub fn connect(url: String) -> WsPipe {
    let (tx, out_rx) = channel::<Vec<u8>>();
    let (in_tx, rx) = channel();
    let status = Arc::new(Mutex::new(Ok(false)));
    let st = status.clone();
    std::thread::spawn(move || {
        let r = (|| -> Result<(), Error> {
            let (mut ws, _) = tungstenite::connect(url.as_str())?;
            if let Some(s) = tcp(ws.get_ref()) {
                s.set_nodelay(true)?;
                s.set_read_timeout(Some(Duration::from_millis(2)))?;
            }
            *st.lock().unwrap() = Ok(true);
            loop {
                match ws.read() {
                    Ok(Message::Binary(b)) => {
                        if in_tx.send(b.to_vec()).is_err() {
                            return Ok(()); // The game dropped the pipe.
                        }
                    }
                    Ok(Message::Close(_)) => return Err(Error::ConnectionClosed),
                    Ok(_) => {}
                    Err(Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                    Err(e) => return Err(e),
                }
                let mut wrote = false;
                loop {
                    match out_rx.try_recv() {
                        Ok(m) => {
                            ws.write(Message::Binary(m.into()))?;
                            wrote = true;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            let _ = ws.close(None);
                            let _ = ws.flush();
                            return Ok(());
                        }
                    }
                }
                if wrote {
                    ws.flush()?;
                }
            }
        })();
        *st.lock().unwrap() = Err(match r {
            Ok(()) => "closed".into(),
            Err(e) => e.to_string(),
        });
    });
    WsPipe { tx, rx, status }
}
