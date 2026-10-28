//! A bot linked in as a library: the gateway's session, network loop
//! and voice pacer behind one handle, no WebSocket in between. The
//! Rust shape of the review document's `baucord-bot` — a bot is a
//! binary of its own that `Bot::connect`s, reads [`Outbound`] frames
//! from [`Bot::events`], answers with [`Bot::act`], and plays audio
//! with [`Bot::play`]. `examples/music.rs` is one.
//!
//! The same frames the WebSocket carries, as values instead of JSON:
//! `Outbound::Dispatch { t: "MESSAGE_CREATE", d }` and so on, with
//! README.md §2 as the reference.

use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender, TrySendError};
use log::warn;
use serde_json::Value;

use crate::net::{self, Command, Config, Link, Sink};
use crate::voice::{self, Feeder};
use crate::wire::{Action, Outbound};

/// Frames queued for the bot before the oldest are dropped: a bot
/// that stops reading loses events, not the process its memory.
const EVENT_CAPACITY: usize = 4096;

struct Events(Sender<Outbound>);

impl Sink for Events {
    fn deliver(&self, out: &Outbound) {
        // Disconnected: the bot dropped its receiver, nothing wants these.
        if let Err(TrySendError::Full(_)) = self.0.try_send(out.clone()) {
            warn!("the bot is not reading its events; dropping one");
        }
    }

    fn snapshot(&self, _ready: Option<Value>) {}
}

/// The acting half of a [`Bot`], for another thread: a player that
/// announces what it plays, say.
#[derive(Clone)]
pub struct Actor(Link);

impl Actor {
    /// As [`Bot::act`].
    pub fn act(&self, action: Action) {
        self.0.send(Command::Act(action));
    }
}

/// One bot's session with a server, in this process.
pub struct Bot {
    events: Receiver<Outbound>,
    link: Link,
    feeder: Feeder,
    net: Option<JoinHandle<bool>>,
}

impl Bot {
    /// Open the session on its own threads (the network loop, the
    /// voice pacer) and return at once; `events` says when it is
    /// `READY`. `voice_kbps` is the Opus bitrate of what the bot plays.
    ///
    /// A bot that answers `!ping` in whatever channel it is in — the
    /// whole of one, the key made on the spot (a real bot keeps its key
    /// file, `ClientKey::load_or_create`, so the server knows it again):
    ///
    /// ```no_run
    /// use std::sync::Arc;
    /// use gateway::{bot::Bot, net::Config, wire::{Action, Outbound}};
    /// use transport::ClientKey;
    ///
    /// let config = Config {
    ///     server: "127.0.0.1:51000".parse().unwrap(),
    ///     pin: None,
    ///     pin_file: None,
    ///     name: "PingBot".into(),
    ///     invite: Some("ABCDEFGHJKMN".into()),
    ///     key: Arc::new(ClientKey::generate()),
    /// };
    /// let bot = Bot::connect(config, 64).unwrap();
    /// for event in bot.events() {
    ///     if let Outbound::Dispatch { t: "MESSAGE_CREATE", d } = &event
    ///         && d["text"] == "!ping"
    ///     {
    ///         bot.act(Action::CreateMessage { text: "pong".into() });
    ///     }
    /// }
    /// ```
    ///
    /// # Errors
    /// The doorbell socket or a thread could not be made.
    pub fn connect(config: Config, voice_kbps: u32) -> std::io::Result<Self> {
        let (link, commands, waker) = net::link()?;
        let (events_tx, events) = crossbeam_channel::bounded(EVENT_CAPACITY);
        let (feeder, pacer) = Feeder::pair();
        {
            let link = link.clone();
            thread::Builder::new()
                .name("voice".to_string())
                .spawn(move || voice::run(&pacer, &link, voice_kbps))?;
        }
        let net = thread::Builder::new().name("network".to_string()).spawn({
            let waker: Arc<net::Waker> = waker;
            move || {
                let mut config = config;
                net::run(&mut config, &commands, &waker, &Events(events_tx))
            }
        })?;
        Ok(Self {
            events,
            link,
            feeder,
            net: Some(net),
        })
    }

    /// What the server says, oldest first: the `status` frames and
    /// the `dispatch`es of README.md §2. The channel closes when the
    /// session ended for good (refused, removed) — `recv` then errs.
    #[must_use]
    pub fn events(&self) -> &Receiver<Outbound> {
        &self.events
    }

    /// Ask the server for something. Before `READY`, or when the
    /// server refuses with words, an `Outbound::Error` comes back
    /// through `events`.
    pub fn act(&self, action: Action) {
        self.link.send(Command::Act(action));
    }

    /// The acting half, for another thread.
    #[must_use]
    pub fn actor(&self) -> Actor {
        Actor(self.link.clone())
    }

    /// A handle on the voice sink for another thread (a decoder's):
    /// [`Feeder::feed`] is [`Self::play`], [`Feeder::stop`] is
    /// [`Self::stop_playing`], and every handle shares the one queue.
    #[must_use]
    pub fn voice(&self) -> Feeder {
        self.feeder.another()
    }

    /// Play samples (f32, 48 kHz, mono, in `[-1, 1]`) in the voice
    /// channel the bot is in. Any length; the sink cuts them into 20 ms
    /// frames and sends one every 20 ms. Blocks while ten seconds are
    /// queued ahead — a decoder calling this runs at the room's pace.
    /// `false` once the session is gone.
    pub fn play(&mut self, samples: &[f32]) -> bool {
        self.feeder.feed(samples)
    }

    /// Drop what is queued to play; the room goes quiet within a frame.
    pub fn stop_playing(&mut self) {
        self.feeder.stop();
    }

    /// The session ended for good and the network thread returned.
    #[must_use]
    pub fn is_over(&self) -> bool {
        self.net.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Close the session (the server sees a goodbye) and wait for the
    /// network thread. `true` when it was this shutdown that ended it.
    #[must_use]
    pub fn shutdown(mut self) -> bool {
        self.link.send(Command::Shutdown);
        self.net
            .take()
            .and_then(|net| net.join().ok())
            .unwrap_or(false)
    }
}
