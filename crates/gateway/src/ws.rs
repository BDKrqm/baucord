//! The bot's side: a WebSocket listener on the loopback (or wherever
//! `--listen` says), one thread per bot connection, and the hub that
//! fans the session's frames out to every connected bot.
//!
//! A bot connection is a plain blocking socket read with a short
//! timeout: between reads the thread hands over whatever the session
//! has for the bot. Text frames are actions, binary frames are voice
//! (README.md §2.4), and a bot that falls more than a few thousand
//! frames behind is dropped rather than buffered without bound.

use std::io::ErrorKind;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crossbeam_channel::{bounded, Sender, TrySendError};
use log::{debug, info, warn};
use serde_json::{json, Value};
use tungstenite::handshake::server::{Callback, ErrorResponse, Request, Response};
use tungstenite::http::StatusCode;
use tungstenite::{Message, WebSocket};

use crate::net::{Command, Link, Sink};
use crate::voice::Feeder;
use crate::wire::{Action, Inbound, Outbound};

/// How long a bot connection's read waits before the thread looks for
/// frames to send: the latency a server event may see on its way to
/// the bot, and the thread's wake rate while idle (40 a second).
const READ_SLICE: Duration = Duration::from_millis(25);
/// Frames queued for one bot before it is dropped as not reading.
const OUTBOUND_CAPACITY: usize = 4096;

struct HubInner {
    clients: Vec<Sender<String>>,
    /// The `READY` data of the live session, for a bot that connects
    /// to the gateway after the fact.
    ready: Option<Value>,
}

/// Where the session's frames go: every connected bot.
#[derive(Clone)]
pub struct Hub {
    inner: Arc<Mutex<HubInner>>,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HubInner {
                clients: Vec::new(),
                ready: None,
            })),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HubInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// One frame to every bot; a bot whose queue is full is dropped.
    pub fn broadcast(&self, out: &Outbound) {
        let text = out.to_json();
        let mut inner = self.lock();
        inner.clients.retain(|tx| match tx.try_send(text.clone()) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                warn!("a bot is not reading its frames; dropping it");
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        });
    }

    /// Bots connected right now.
    #[cfg(test)]
    #[must_use]
    pub fn client_count(&self) -> usize {
        self.lock().clients.len()
    }

    fn subscribe(&self, tx: Sender<String>) -> String {
        let mut inner = self.lock();
        inner.clients.push(tx);
        Outbound::Hello {
            d: json!({
                "gateway": concat!("baucord-gateway ", env!("CARGO_PKG_VERSION")),
                "connected": inner.ready.is_some(),
                "ready": inner.ready,
            }),
        }
        .to_json()
    }
}

impl Sink for Hub {
    fn deliver(&self, out: &Outbound) {
        self.broadcast(out);
    }

    fn snapshot(&self, ready: Option<Value>) {
        self.lock().ready = ready;
    }
}

/// What a bot connection's thread needs.
pub struct Context {
    pub hub: Hub,
    pub link: Link,
    /// The voice sink; each connection feeds through one of its own.
    pub feeder: Feeder,
    /// Required in `Authorization: Bearer …` or `?token=…` when set.
    pub token: Option<String>,
}

impl Clone for Context {
    fn clone(&self) -> Self {
        Self {
            hub: self.hub.clone(),
            link: self.link.clone(),
            feeder: self.feeder.another(),
            token: self.token.clone(),
        }
    }
}

/// Accept bot connections for ever. Returns only if the listener
/// fails.
///
/// # Errors
/// The address could not be bound.
pub fn serve(listen: SocketAddr, context: &Context) -> std::io::Result<()> {
    let listener = TcpListener::bind(listen)?;
    info!("bots connect at ws://{}/", listener.local_addr()?);
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let context = context.clone();
                let peer = stream
                    .peer_addr()
                    .map_or_else(|_| "?".to_string(), |a| a.to_string());
                let spawned = std::thread::Builder::new()
                    .name(format!("bot {peer}"))
                    .spawn(move || serve_one(stream, &context, &peer));
                if let Err(e) = spawned {
                    warn!("could not start a bot thread: {e}");
                }
            }
            Err(e) => warn!("accept failed: {e}"),
        }
    }
    Ok(())
}

fn serve_one(stream: TcpStream, context: &Context, peer: &str) {
    if let Err(e) = stream.set_read_timeout(Some(READ_SLICE)) {
        warn!("bot {peer}: could not set the read timeout: {e}");
        return;
    }
    let gate = TokenGate(context.token.clone());
    let mut ws = match tungstenite::accept_hdr(stream, gate) {
        Ok(ws) => ws,
        Err(e) => {
            info!("bot {peer}: handshake failed: {e}");
            return;
        }
    };
    let (tx, rx) = bounded::<String>(OUTBOUND_CAPACITY);
    let hello = context.hub.subscribe(tx);
    if let Err(e) = ws.send(Message::text(hello)) {
        info!("bot {peer}: could not greet: {e}");
        return;
    }
    info!("bot {peer} connected");
    let mut feeder = context.feeder.another();
    loop {
        match ws.read() {
            Ok(Message::Text(text)) => handle_text(&mut ws, context, &mut feeder, text.as_str()),
            Ok(Message::Binary(bytes)) => {
                if !handle_pcm(&mut feeder, &bytes) {
                    send_error(&mut ws, "voice frames are f32 little-endian PCM at 48 kHz, mono: a multiple of 4 bytes", Some("voice"));
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                break;
            }
            Err(e) => {
                info!("bot {peer}: {e}");
                break;
            }
        }
        let mut failed = false;
        while let Ok(text) = rx.try_recv() {
            if let Err(e) = ws.send(Message::text(text)) {
                info!("bot {peer}: send failed: {e}");
                failed = true;
                break;
            }
        }
        if failed {
            break;
        }
        // A pong queued by a read goes out now.
        if let Err(e) = ws.flush()
            && !matches!(&e, tungstenite::Error::Io(io) if io.kind() == ErrorKind::WouldBlock)
        {
            debug!("bot {peer}: flush: {e}");
            break;
        }
    }
    // Whatever was queued to play stops with the bot that queued it.
    feeder.stop();
    info!("bot {peer} disconnected");
}

/// The handshake's door: with a token set, a request without it is
/// answered 401 and never becomes a WebSocket.
struct TokenGate(Option<String>);

impl Callback for TokenGate {
    // The error type is tungstenite's (a whole HTTP response).
    #[allow(clippy::result_large_err)]
    fn on_request(self, request: &Request, response: Response) -> Result<Response, ErrorResponse> {
        if authorized(request, self.0.as_deref()) {
            Ok(response)
        } else {
            let mut refused = ErrorResponse::new(Some("a token is required".to_string()));
            *refused.status_mut() = StatusCode::UNAUTHORIZED;
            Err(refused)
        }
    }
}

/// `Authorization: Bearer <token>` or `?token=<token>` matches, or no
/// token is required.
fn authorized(request: &Request, token: Option<&str>) -> bool {
    let Some(token) = token else {
        return true;
    };
    let header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|v| v.trim() == token);
    let query = request
        .uri()
        .query()
        .unwrap_or("")
        .split('&')
        .any(|pair| pair.strip_prefix("token=") == Some(token));
    header || query
}

fn send_error(ws: &mut WebSocket<TcpStream>, message: &str, action: Option<&str>) {
    let _ = ws.send(Message::text(Outbound::error(message, action).to_json()));
}

fn handle_text(ws: &mut WebSocket<TcpStream>, context: &Context, feeder: &mut Feeder, text: &str) {
    let inbound: Inbound = match serde_json::from_str(text) {
        Ok(inbound) => inbound,
        Err(e) => {
            send_error(ws, &format!("not a frame the gateway knows: {e}"), None);
            return;
        }
    };
    match inbound {
        Inbound::Action { t, d } => match Action::parse(&t, d) {
            Ok(action) => context.link.send(Command::Act(action)),
            Err(e) => send_error(ws, &e, Some(&t)),
        },
        Inbound::Voice { t } => match t.as_str() {
            "STOP" => feeder.stop(),
            other => send_error(ws, &format!("unknown voice op {other:?}"), Some("voice")),
        },
    }
}

/// Binary frame → samples for the feeder, which cuts them into 20 ms
/// frames and waits when the queue is full — the back-pressure a bot
/// sending faster than the room plays sees on its socket. `false` for
/// bytes that are not whole samples.
fn handle_pcm(feeder: &mut Feeder, bytes: &[u8]) -> bool {
    if !bytes.len().is_multiple_of(4) {
        return false;
    }
    let samples: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    feeder.feed(&samples);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(uri: &str, auth: Option<&str>) -> Request {
        let mut builder = Request::builder().uri(uri);
        if let Some(auth) = auth {
            builder = builder.header("authorization", auth);
        }
        builder.body(()).expect("a request")
    }

    #[test]
    fn the_token_is_checked_in_the_header_or_the_query() {
        assert!(authorized(&request("/", None), None));
        assert!(!authorized(&request("/", None), Some("s3cret")));
        assert!(authorized(
            &request("/", Some("Bearer s3cret")),
            Some("s3cret")
        ));
        assert!(!authorized(
            &request("/", Some("Bearer wrong")),
            Some("s3cret")
        ));
        assert!(authorized(&request("/?token=s3cret", None), Some("s3cret")));
        assert!(authorized(
            &request("/?a=1&token=s3cret", None),
            Some("s3cret")
        ));
        assert!(!authorized(
            &request("/?token=s3cret2", None),
            Some("s3cret")
        ));
    }

    #[test]
    fn pcm_frames_must_be_whole_samples() {
        let (mut feeder, pacer) = Feeder::pair();
        let samples: Vec<u8> = (0..1000u32)
            .flat_map(|i| (i as f32).to_le_bytes())
            .collect();
        assert!(handle_pcm(&mut feeder, &samples));
        assert!(!handle_pcm(&mut feeder, &[1, 2, 3]));
        // One whole frame went to the pacer; the odd 40 samples wait.
        assert_eq!(pacer_len(&pacer), 1);
    }

    fn pacer_len(pacer: &crate::voice::Pacer) -> usize {
        pacer.queued()
    }

    #[test]
    fn a_full_queue_drops_the_bot() {
        let hub = Hub::new();
        let (tx, rx) = bounded::<String>(1);
        let hello = hub.subscribe(tx);
        assert!(hello.contains("\"connected\":false"));
        assert_eq!(hub.client_count(), 1);
        hub.broadcast(&Outbound::status("CONNECTED", json!({})));
        hub.broadcast(&Outbound::status("CONNECTED", json!({})));
        assert_eq!(hub.client_count(), 0);
        assert_eq!(rx.len(), 1);
    }
}
