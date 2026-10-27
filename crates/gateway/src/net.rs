//! The network thread: the UDP socket, the QUIC driver and the
//! [`Session`], run until the session ends, then again after a pause
//! when the link merely dropped. The same four-call tick the desktop
//! client runs (`client/src/network.rs`: receive, timeouts, process,
//! send), sleeping in `poll(2)` on the socket between ticks — a
//! command from the bot wakes it with a datagram of our own, the way
//! the client's inbound watcher ends its wait.

use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use log::{error, info, warn};
use serde_json::json;
use transport::{
    Budget, ClientKey, Driver, DriverHost as _, Fingerprint, Observation, Trust, UdpIo,
};

use crate::session::Session;
use crate::wire::{Action, Outbound};

/// Where the session's frames go: the WebSocket hub for the binary, a
/// channel for a bot linked in as a library (`crate::bot`).
pub trait Sink: Send + Sync {
    /// One frame for the bot(s).
    fn deliver(&self, out: &Outbound);
    /// The session came up (`Some`, its `READY` data) or went.
    fn snapshot(&self, ready: Option<serde_json::Value>);
}

/// What the other threads ask of the network thread.
#[derive(Debug)]
pub enum Command {
    /// From a bot's `action` frame.
    Act(Action),
    /// One encoded Opus frame from the voice pacer.
    Voice(Vec<u8>),
    /// The speaking light (the pacer's, when audio starts and stops).
    Speaking(bool),
    /// Ctrl-C: close the session and return.
    Shutdown,
}

/// The network thread's doorbell: a datagram to its own socket ends
/// its `poll`, so a command is picked up at once instead of at the
/// next QUIC timer.
pub struct Waker {
    socket: UdpSocket,
    target: Mutex<Option<SocketAddr>>,
}

impl Waker {
    fn new() -> std::io::Result<Self> {
        Ok(Self {
            socket: UdpSocket::bind("127.0.0.1:0")?,
            target: Mutex::new(None),
        })
    }

    fn aim(&self, local: SocketAddr) {
        // The socket is bound to the wildcard; the loopback address of
        // its family reaches it.
        let target = match local {
            SocketAddr::V4(v4) => SocketAddr::from(([127, 0, 0, 1], v4.port())),
            SocketAddr::V6(v6) => SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, v6.port())),
        };
        *self
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(target);
    }

    fn wake(&self) {
        let target = *self
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(target) = target {
            // A wake that fails is a command picked up a timer later.
            let _ = self.socket.send_to(&[0], target);
        }
    }
}

/// The handle the other threads hold: a command, then the doorbell.
#[derive(Clone)]
pub struct Link {
    tx: Sender<Command>,
    waker: Arc<Waker>,
}

impl Link {
    /// A command for the network thread, which wakes to take it.
    pub fn send(&self, command: Command) {
        if self.tx.send(command).is_ok() {
            self.waker.wake();
        }
    }
}

/// How the session is to be opened.
pub struct Config {
    pub server: SocketAddr,
    pub pin: Option<Fingerprint>,
    /// Where a pin learned on first use is written, for the next run.
    pub pin_file: Option<PathBuf>,
    pub name: String,
    pub invite: Option<String>,
    pub key: Arc<ClientKey>,
}

/// The pause before the next attempt after a dropped link: doubling
/// from one second to this.
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// How long the loop sleeps at most between ticks when the driver
/// names no deadline (it always does while connected).
const IDLE_SLICE: Duration = Duration::from_secs(1);

/// Make the channel and the doorbell the other threads use.
///
/// # Errors
/// The doorbell's loopback socket could not be bound.
pub fn link() -> std::io::Result<(Link, Receiver<Command>, Arc<Waker>)> {
    let (tx, rx) = crossbeam_channel::unbounded();
    let waker = Arc::new(Waker::new()?);
    Ok((
        Link {
            tx,
            waker: Arc::clone(&waker),
        },
        rx,
        waker,
    ))
}

/// Run sessions until one ends for good or a `Shutdown` arrives.
/// `true` when it was the shutdown.
pub fn run(
    config: &mut Config,
    commands: &Receiver<Command>,
    waker: &Waker,
    sink: &dyn Sink,
) -> bool {
    let mut backoff = Duration::from_secs(1);
    loop {
        let outcome = match open(config, waker) {
            Ok((udp, session)) => run_session(config, udp, session, commands, sink),
            Err(e) => {
                error!("{e}");
                Outcome::Dropped
            }
        };
        sink.snapshot(None);
        match outcome {
            Outcome::Shutdown => return true,
            Outcome::Ended => return false,
            Outcome::Dropped => {}
            Outcome::WasUp => backoff = Duration::from_secs(1),
        }
        if matches!(outcome, Outcome::Dropped) {
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
        info!("reconnecting in {backoff:?}");
        // The pause ends early for a shutdown; other commands wait.
        match commands.recv_timeout(backoff) {
            Ok(Command::Shutdown) => return true,
            Ok(other) => {
                sink.deliver(&Outbound::error(
                    "not connected to the server",
                    command_name(&other),
                ));
            }
            Err(_) => {}
        }
    }
}

enum Outcome {
    /// Ctrl-C.
    Shutdown,
    /// The server said no (refused, removed, identity changed).
    Ended,
    /// The link dropped before admission: back off further.
    Dropped,
    /// The link dropped after a working session: a fresh start.
    WasUp,
}

fn command_name(command: &Command) -> Option<&'static str> {
    match command {
        Command::Act(_) => Some("action"),
        Command::Voice(_) | Command::Speaking(_) => Some("voice"),
        Command::Shutdown => None,
    }
}

fn open(config: &Config, waker: &Waker) -> Result<(UdpIo, Session), String> {
    let budget = Budget::client();
    let bind: SocketAddr = if config.server.is_ipv4() {
        SocketAddr::from(([0, 0, 0, 0], 0))
    } else {
        SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, 0))
    };
    let mut udp = UdpIo::bind(bind, budget.socket_buffer_bytes)
        .map_err(|e| format!("could not bind a UDP socket: {e}"))?;
    if budget.mark_voice {
        udp.mark_voice(config.server);
    }
    let local = udp
        .local_addr()
        .map_err(|e| format!("no local address: {e}"))?;
    waker.aim(local);
    let mut driver = Driver::new_client(config.pin, budget)
        .map_err(|e| format!("could not set up the transport: {e}"))?;
    driver.set_max_gso_segments(udp.max_gso_segments());
    let now = Instant::now();
    let peer = driver
        .connect(now, config.server)
        .map_err(|e| format!("could not connect: {e}"))?;
    info!(
        "connecting to {} ({})",
        config.server,
        config.pin.map_or_else(
            || "no pin: trust on first use".to_string(),
            |p| format!("pinned {p}")
        )
    );
    let session = Session::new(
        driver,
        peer,
        &config.name,
        config.invite.as_deref(),
        Arc::clone(&config.key),
    );
    Ok((udp, session))
}

fn run_session(
    config: &mut Config,
    mut udp: UdpIo,
    mut session: Session,
    commands: &Receiver<Command>,
    sink: &dyn Sink,
) -> Outcome {
    let mut was_up = false;
    let mut pinned = config.pin.is_some();
    loop {
        let now = Instant::now();
        let mut shutdown = false;
        while let Ok(command) = commands.try_recv() {
            match command {
                Command::Act(action) => {
                    let name = action_name(&action);
                    if let Err(e) = session.act(now, action) {
                        sink.deliver(&Outbound::error(e, Some(name)));
                    }
                }
                Command::Voice(frame) => session.send_voice(now, &frame),
                Command::Speaking(on) => session.set_speaking(now, on),
                Command::Shutdown => shutdown = true,
            }
        }
        session
            .driver()
            .set_max_gso_segments(udp.max_gso_segments());
        if let Err(e) = udp.recv_batch(|d| session.driver().handle_datagram(now, d)) {
            warn!("UDP receive error: {e}");
        }
        session.driver().handle_timeouts(now);
        session.tick(now);
        for out in session.take_outbox() {
            if let Outbound::Dispatch { t: "READY", .. } = &out {
                sink.snapshot(session.ready_snapshot());
                was_up = true;
                // The invite admitted this key: from now on the key
                // alone logs in, and a stale code would be refused.
                config.invite = None;
            }
            sink.deliver(&out);
        }
        if !pinned
            && was_up
            && let Some(Observation::Accepted { fingerprint, trust }) = session.identity
            && matches!(trust, Trust::FirstUse | Trust::Successor { .. })
        {
            pinned = true;
            config.pin = Some(fingerprint);
            if let Some(path) = &config.pin_file {
                match std::fs::write(path, format!("{fingerprint}\n")) {
                    Ok(()) => info!("pinned {fingerprint} in {}", path.display()),
                    Err(e) => warn!("could not write the pin to {}: {e}", path.display()),
                }
            }
        }
        if shutdown {
            info!("shutting down");
            session.close(now);
            // `close` queues the frame; one flush puts it on the wire.
            while let Some(t) = session.driver().poll_transmit() {
                udp.send(&t);
            }
            sink.deliver(&Outbound::status(
                "DISCONNECTED",
                json!({ "reason": "gateway shutting down", "was_connected": was_up, "reconnecting": false }),
            ));
            return Outcome::Shutdown;
        }
        while let Some(t) = session.driver().poll_transmit() {
            udp.send(&t);
        }
        if let Some(end) = session.ended() {
            return if !end.transient {
                Outcome::Ended
            } else if was_up {
                Outcome::WasUp
            } else {
                Outcome::Dropped
            };
        }
        let wait = session
            .next_deadline()
            .map_or(IDLE_SLICE, |d| d.saturating_duration_since(now))
            .min(IDLE_SLICE);
        if !wait.is_zero()
            && let Err(e) = udp.wait_readable(wait)
        {
            warn!("socket wait failed: {e}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn action_name(action: &Action) -> &'static str {
    match action {
        Action::CreateMessage { .. } => "CREATE_MESSAGE",
        Action::CreateDm { .. } => "CREATE_DM",
        Action::EditMessage { .. } => "EDIT_MESSAGE",
        Action::DeleteMessage { .. } => "DELETE_MESSAGE",
        Action::JoinChannel { .. } => "JOIN_CHANNEL",
        Action::Speaking(_) => "SPEAKING",
        Action::AudioState { .. } => "AUDIO_STATE",
        Action::SetNickname { .. } => "SET_NICKNAME",
        Action::FetchHistory { .. } => "FETCH_HISTORY",
        Action::MarkRead { .. } => "MARK_READ",
    }
}
