//! The gateway's session against a real `ChatServer` on the in-process
//! network (the server's own e2e harness, cut to what a bot does):
//! admission by invite, the READY snapshot, chat both ways, a voice
//! frame reaching a listener, and a refusal.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use gateway::session::Session;
use gateway::wire::{Action, Outbound};
use protocol::permissions::ROLE_MEMBER;
use protocol::{features, Auth, ClientKeyProof, ClientMessage, Lane, ServerMessage};
use serde_json::Value;
use server::config::{ChannelSettings, Config};
use server::server::ChatServer;
use transport::identity::{CLIENT_AUTH_EXPORTER_BYTES, CLIENT_AUTH_EXPORTER_LABEL};
use transport::sim::{LinkSpec, NodeId, SimNet};
use transport::{Budget, ClientKey, Driver, PeerId, ServerIdentity};

const TICK: Duration = Duration::from_millis(2);
const SERVER_ADDR: &str = "10.0.0.1:5000";
const WAIT: Duration = Duration::from_secs(5);

struct World {
    net: SimNet,
    server: NodeId,
    fingerprint: transport::Fingerprint,
    member_code: String,
    next_addr: u8,
}

/// A plain client beside the bot: a driver, a key, an inbox.
struct Person {
    node: NodeId,
    peer: PeerId,
    key: ClientKey,
    inbox: Vec<ServerMessage>,
    voice: Vec<Vec<u8>>,
}

impl World {
    fn new() -> Self {
        let _ = env_logger::builder().is_test(true).try_init();
        let mut config = Config {
            channels: vec![
                ChannelSettings {
                    name: "General".to_string(),
                    default: true,
                    password: None,
                    user_limit: None,
                },
                ChannelSettings {
                    name: "Music".to_string(),
                    default: false,
                    password: None,
                    user_limit: None,
                },
            ],
            ..Config::default()
        };
        config.logging.link_stats_interval_secs = 0;
        config.persistence.enabled = false;
        config.limits.chat.burst = 1000;
        let identity = ServerIdentity::generate();
        let fingerprint = identity.fingerprint();
        let mut server = ChatServer::new(config, &identity).expect("server start");
        let (member_code, _) = server
            .mint_invite(None, ROLE_MEMBER, None, None)
            .expect("member invite");
        let mut net = SimNet::new(0xB07);
        let server = net.add_node(server, SERVER_ADDR.parse().expect("addr"));
        World {
            net,
            server,
            fingerprint,
            member_code,
            next_addr: 2,
        }
    }

    fn addr(&mut self) -> SocketAddr {
        let addr = format!("10.0.0.{}:6000", self.next_addr)
            .parse()
            .expect("addr");
        self.next_addr += 1;
        addr
    }

    fn connected_driver(&mut self) -> (Driver, PeerId) {
        let mut driver =
            Driver::new_client(Some(self.fingerprint), Budget::client()).expect("client");
        let now = self.net.now();
        let peer = driver
            .connect(now, SERVER_ADDR.parse().expect("addr"))
            .expect("connect");
        (driver, peer)
    }

    /// A gateway session, connecting with `invite` (or by key).
    fn bot(&mut self, name: &str, invite: Option<&str>, key: Arc<ClientKey>) -> NodeId {
        let addr = self.addr();
        let (driver, peer) = self.connected_driver();
        let session = Session::new(driver, peer, name, invite, key);
        let node = self.net.add_node(session, addr);
        self.net.set_link(
            self.server,
            node,
            LinkSpec::clean(Duration::from_millis(10)),
        );
        node
    }

    fn person(&mut self, name: &str) -> Person {
        let addr = self.addr();
        let (driver, peer) = self.connected_driver();
        let node = self.net.add_node(driver, addr);
        self.net.set_link(
            self.server,
            node,
            LinkSpec::clean(Duration::from_millis(10)),
        );
        let mut person = Person {
            node,
            peer,
            key: ClientKey::generate(),
            inbox: Vec::new(),
            voice: Vec::new(),
        };
        let deadline = self.net.now() + WAIT;
        while !self.net.driver(person.node).is_connected(person.peer) {
            assert!(self.net.now() < deadline, "handshake timed out");
            self.pump(&mut [&mut person], &mut []);
        }
        let mut exported = [0u8; CLIENT_AUTH_EXPORTER_BYTES];
        self.net
            .driver(person.node)
            .export_keying_material(person.peer, CLIENT_AUTH_EXPORTER_LABEL, b"", &mut exported)
            .expect("exporter");
        let proof = ClientKeyProof {
            public_key: person.key.public_key(),
            signature: person.key.sign(&exported),
        };
        let code = self.member_code.clone();
        self.send(
            &person,
            &ClientMessage::Hello {
                username: name.to_string(),
                auth: Auth::Invite { code },
                client_key: Some(proof),
                features: features::CLIENT_1_1,
                client_version: protocol::CLIENT_VERSION.to_string(),
            },
        );
        self.wait_person(&mut person, |m| matches!(m, ServerMessage::Init { .. }));
        self.send(
            &person,
            &ClientMessage::JoinChannel {
                channel_id: 0,
                password: None,
            },
        );
        self.wait_person(&mut person, |m| {
            matches!(m, ServerMessage::UserMovedChannel { channel_id: 0, .. })
        });
        person
    }

    fn send(&mut self, person: &Person, msg: &ClientMessage) {
        let bytes = protocol::encode_client_message(msg).expect("encode");
        let now = self.net.now();
        self.net
            .driver(person.node)
            .send(now, person.peer, msg.lane(), &bytes)
            .expect("send");
    }

    /// One tick: the wire moves, the server runs, every bot session
    /// ticks and its frames are collected, every person's inbox fills.
    fn pump(&mut self, people: &mut [&mut Person], bots: &mut [(NodeId, &mut Vec<Outbound>)]) {
        self.net.step(TICK);
        let now = self.net.now();
        self.net
            .host_mut::<ChatServer>(self.server)
            .expect("server")
            .update(now);
        for (node, out) in bots.iter_mut() {
            let session = self.net.host_mut::<Session>(*node).expect("bot");
            session.tick(now);
            out.extend(session.take_outbox());
        }
        for p in people.iter_mut() {
            let driver = self.net.driver(p.node);
            let _ = driver.drain_events().count();
            for lane in [Lane::Control, Lane::Signal, Lane::Media] {
                while let Some(raw) = driver.recv(p.peer, lane) {
                    if let Some(msg) = protocol::decode_server_message(&raw) {
                        p.inbox.push(msg);
                    }
                }
            }
            while let Some(raw) = driver.recv(p.peer, Lane::Voice) {
                p.voice.push(raw.to_vec());
            }
        }
    }

    fn wait_person(&mut self, person: &mut Person, pred: impl Fn(&ServerMessage) -> bool) {
        let deadline = self.net.now() + WAIT;
        while !person.inbox.iter().any(&pred) {
            assert!(
                self.net.now() < deadline,
                "waited in vain: {:?}",
                person.inbox
            );
            self.pump(&mut [person], &mut []);
        }
    }

    /// Pump until the bot emitted a frame `pred` accepts; all frames so
    /// far are returned.
    fn wait_bot(
        &mut self,
        bot: NodeId,
        out: &mut Vec<Outbound>,
        people: &mut [&mut Person],
        pred: impl Fn(&Outbound) -> bool,
    ) {
        let deadline = self.net.now() + WAIT;
        while !out.iter().any(&pred) {
            assert!(self.net.now() < deadline, "the bot never got it: {out:?}");
            self.pump(people, &mut [(bot, out)]);
        }
    }

    fn settle(
        &mut self,
        people: &mut [&mut Person],
        bots: &mut [(NodeId, &mut Vec<Outbound>)],
        d: Duration,
    ) {
        let until = self.net.now() + d;
        while self.net.now() < until {
            self.pump(people, bots);
        }
    }

    fn act(&mut self, bot: NodeId, action: Action) {
        let now = self.net.now();
        self.net
            .host_mut::<Session>(bot)
            .expect("bot")
            .act(now, action)
            .expect("the bot is ready");
    }
}

fn dispatch<'a>(out: &'a [Outbound], t: &str) -> Option<&'a Value> {
    out.iter().find_map(|o| match o {
        Outbound::Dispatch { t: kind, d } if *kind == t => Some(d),
        _ => None,
    })
}

fn dispatches<'a>(out: &'a [Outbound], t: &str) -> Vec<&'a Value> {
    out.iter()
        .filter_map(|o| match o {
            Outbound::Dispatch { t: kind, d } if *kind == t => Some(d),
            _ => None,
        })
        .collect()
}

fn status<'a>(out: &'a [Outbound], t: &str) -> Option<&'a Value> {
    out.iter().find_map(|o| match o {
        Outbound::Status { t: kind, d } if *kind == t => Some(d),
        _ => None,
    })
}

#[test]
fn a_bot_is_admitted_by_invite_and_gets_the_room_as_ready() {
    let mut world = World::new();
    let mut ana = world.person("ana");
    let key = Arc::new(ClientKey::generate());
    let code = world.member_code.clone();
    let bot = world.bot("MusicBot", Some(&code), key);
    let mut out = Vec::new();
    world.wait_bot(bot, &mut out, &mut [&mut ana], |o| {
        matches!(o, Outbound::Dispatch { t: "READY", .. })
    });
    let connected = status(&out, "CONNECTED").expect("CONNECTED first");
    assert!(connected["fingerprint"]
        .as_str()
        .unwrap()
        .starts_with("SHA256:"));
    let ready = dispatch(&out, "READY").unwrap();
    let channels = ready["channels"].as_array().unwrap();
    assert!(
        channels
            .iter()
            .any(|c| c["name"] == "Music" && c["id"] == 1),
        "the configured channels: {channels:?}"
    );
    let members = ready["members"].as_array().unwrap();
    assert!(
        members
            .iter()
            .any(|m| m["name"] == "ana" && m["channel_id"] == 0),
        "ana is in General: {members:?}"
    );
    assert!(ready["client_id"].is_string(), "ids travel as strings");
    world.wait_bot(bot, &mut out, &mut [&mut ana], |o| {
        matches!(o, Outbound::Dispatch { t: "SESSION", .. })
    });
    let session = dispatch(&out, "SESSION").unwrap();
    assert_eq!(session["user_id"].as_str().unwrap().len(), 64);
    assert!(session["permissions"].as_u64().unwrap() > 0);
}

#[test]
fn chat_flows_both_ways_with_names() {
    let mut world = World::new();
    let mut ana = world.person("ana");
    let key = Arc::new(ClientKey::generate());
    let code = world.member_code.clone();
    let bot = world.bot("MusicBot", Some(&code), key);
    let mut out = Vec::new();
    world.wait_bot(bot, &mut out, &mut [&mut ana], |o| {
        matches!(o, Outbound::Dispatch { t: "READY", .. })
    });
    // The bot joins the room, like a client out of the lobby.
    world.act(
        bot,
        Action::JoinChannel {
            channel_id: 0,
            password: None,
        },
    );
    world.wait_bot(bot, &mut out, &mut [&mut ana], |o| {
        matches!(
            o,
            Outbound::Dispatch {
                t: "VOICE_STATE",
                ..
            }
        )
    });

    world.send(&ana, &ClientMessage::Chat("hello bot".to_string()));
    world.wait_bot(
        bot,
        &mut out,
        &mut [&mut ana],
        |o| matches!(o, Outbound::Dispatch { t: "MESSAGE_CREATE", d } if d["text"] == "hello bot"),
    );
    let msg = dispatches(&out, "MESSAGE_CREATE")
        .into_iter()
        .find(|d| d["text"] == "hello bot")
        .unwrap();
    assert_eq!(msg["name"], "ana");
    assert_eq!(msg["conv"], "general");
    assert_eq!(msg["own"], false);
    assert!(msg["client_id"].is_string());

    world.act(
        bot,
        Action::CreateMessage {
            text: "hi ana".to_string(),
        },
    );
    // A 1.1 client gets lines as `ChatFrom` (the author's user id
    // along); a 1.0 one as `Chat`.
    world.wait_person(&mut ana, |m| match m {
        ServerMessage::Chat(c) | ServerMessage::ChatFrom { msg: c, .. } => c.text == "hi ana",
        _ => false,
    });
    // The bot's own line comes back marked as its own.
    world.wait_bot(
        bot,
        &mut out,
        &mut [&mut ana],
        |o| matches!(o, Outbound::Dispatch { t: "MESSAGE_CREATE", d } if d["text"] == "hi ana"),
    );
    let own = dispatches(&out, "MESSAGE_CREATE")
        .into_iter()
        .find(|d| d["text"] == "hi ana")
        .unwrap();
    assert_eq!(own["own"], true);
    assert_eq!(own["name"], "MusicBot");

    // A nickname change is a MEMBER_UPDATE for everyone, the bot too.
    world.act(
        bot,
        Action::SetNickname {
            name: "DJ".to_string(),
        },
    );
    world.wait_bot(
        bot,
        &mut out,
        &mut [&mut ana],
        |o| matches!(o, Outbound::Dispatch { t: "MEMBER_UPDATE", d } if d["name"] == "DJ"),
    );
}

#[test]
fn voice_frames_reach_a_listener_in_the_room() {
    let mut world = World::new();
    let mut ana = world.person("ana");
    let key = Arc::new(ClientKey::generate());
    let code = world.member_code.clone();
    let bot = world.bot("MusicBot", Some(&code), key);
    let mut out = Vec::new();
    world.wait_bot(bot, &mut out, &mut [&mut ana], |o| {
        matches!(o, Outbound::Dispatch { t: "READY", .. })
    });
    world.act(
        bot,
        Action::JoinChannel {
            channel_id: 0,
            password: None,
        },
    );
    world.wait_bot(bot, &mut out, &mut [&mut ana], |o| {
        matches!(
            o,
            Outbound::Dispatch {
                t: "VOICE_STATE",
                ..
            }
        )
    });
    // Twenty "Opus" frames (the relay checks the header, not the
    // codec), one per tick, with the speaking light on.
    for i in 0..20u8 {
        let now = world.net.now();
        let session = world.net.host_mut::<Session>(bot).expect("bot");
        session.set_speaking(now, true);
        session.send_voice(now, &[0xfc, i, 1, 2, 3]);
        world.pump(&mut [&mut ana], &mut [(bot, &mut out)]);
    }
    world.settle(
        &mut [&mut ana],
        &mut [(bot, &mut out)],
        Duration::from_millis(200),
    );
    assert!(
        ana.voice.len() >= 10,
        "ana heard {} of 20 frames",
        ana.voice.len()
    );
    // Each relayed datagram carries the bot's id, then our packet.
    let (sender, packet) = protocol::split_sender(&ana.voice[0]).expect("a relayed packet");
    let bot_id: u64 = dispatch(&out, "READY").unwrap()["client_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(sender, bot_id);
    let (header, payload) = protocol::parse_voice(packet).expect("a voice packet");
    assert_eq!(payload[0], 0xfc);
    assert_eq!(header.timestamp % 960, 0);
    assert!(ana.inbox.iter().any(|m| matches!(
        m,
        ServerMessage::Speaking {
            is_speaking: true,
            ..
        }
    )));
}

#[test]
fn a_bad_invite_is_a_refusal_the_bot_hears_about() {
    let mut world = World::new();
    let key = Arc::new(ClientKey::generate());
    let bot = world.bot("MusicBot", Some("not-a-code"), key);
    let mut out = Vec::new();
    world.wait_bot(bot, &mut out, &mut [], |o| {
        matches!(o, Outbound::Status { t: "REFUSED", .. })
    });
    let refused = status(&out, "REFUSED").unwrap();
    assert!(
        refused["reason"].as_str().unwrap().contains("invite"),
        "{refused}"
    );
    let session = world.net.host_mut::<Session>(bot).expect("bot");
    let end = session.ended().expect("ended");
    assert!(!end.transient, "a bad code is not a dropped link");
    assert!(session.ready_snapshot().is_none());
}

#[test]
fn an_action_before_ready_is_refused_with_words() {
    let mut world = World::new();
    let key = Arc::new(ClientKey::generate());
    let code = world.member_code.clone();
    let bot = world.bot("MusicBot", Some(&code), key);
    let now = world.net.now();
    let session = world.net.host_mut::<Session>(bot).expect("bot");
    let err = session
        .act(
            now,
            Action::CreateMessage {
                text: "too soon".to_string(),
            },
        )
        .unwrap_err();
    assert!(err.contains("not connected"));
}
