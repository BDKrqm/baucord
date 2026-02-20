use bincode::Options;
use log::{debug, info, warn};
use renet::{ChannelConfig, ClientId, ConnectionConfig, RenetServer, SendType, ServerEvent};
use renet_netcode::{
    NETCODE_USER_DATA_BYTES, NetcodeServerTransport, ServerAuthentication, ServerConfig,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant, SystemTime};

use crate::config::{ChannelSettings, Config};

// ── Protocol types ──────────────────────────────────────────────────────

const VOICE_PROTOCOL_VERSION: u8 = 1;
const VOICE_MTU_BUDGET: usize = 1000;
const MAX_FRAMES_PER_PACKET: usize = 2;

#[repr(C)]
#[derive(Copy, Clone)]
struct VoiceHeader {
    version: u8,
    _reserved: [u8; 3],
    client_id: u64,
    seq: u32,
    ts_48k: u32,
    frame_count: u8,
    _padding: [u8; 3],
}

impl VoiceHeader {
    const SIZE: usize = 24;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Message {
    client_id: ClientId,
    text: String,
}

#[derive(Debug, Serialize, Deserialize)]
enum ClientMessage {
    Text(String),
    VoiceData(Vec<u8>),
    Speaking(bool),
    AudioState { muted: bool, deafened: bool },
}

#[derive(Debug, Serialize, Deserialize)]
enum ServerMessage {
    ClientConnected {
        client_id: ClientId,
        username: String,
    },
    ClientDisconnected {
        client_id: ClientId,
    },
    ClientMessage(Message),
    InitClient {
        usernames: HashMap<ClientId, String>,
        server_name: String,
        greeting: String,
    },
    VoiceData {
        client_id: ClientId,
        data: Vec<u8>,
    },
    Speaking {
        client_id: ClientId,
        is_speaking: bool,
    },
    AudioState {
        client_id: ClientId,
        muted: bool,
        deafened: bool,
    },
}

// ── Stats ───────────────────────────────────────────────────────────────

#[derive(Default)]
struct VoiceStats {
    packets_received: u64,
    packets_forwarded: u64,
    packets_dropped_malformed: u64,
    packets_dropped_oversize: u64,
    packets_dropped_version: u64,
    total_frames: u64,
    avg_frames_per_packet: f32,
}

#[derive(Default)]
struct ServerStats {
    messages_count: u64,
    voice_packets_count: u64,
    bytes_sent: u64,
    bytes_received: u64,
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Extract a UTF-8 username from renet's fixed-size user data buffer.
fn username_from_user_data(user_data: &[u8; NETCODE_USER_DATA_BYTES]) -> String {
    let len = (user_data[0] as usize).min(NETCODE_USER_DATA_BYTES - 1);
    String::from_utf8_lossy(&user_data[1..=len]).into_owned()
}

/// Serialize a `ServerMessage` via bincode, logging on failure.
fn encode_message(msg: &ServerMessage) -> Option<Vec<u8>> {
    bincode::options()
        .serialize(msg)
        .map_err(|e| {
            warn!("Failed to serialize message: {e}");
        })
        .ok()
}

// ── ChatServer ──────────────────────────────────────────────────────────

pub struct ChatServer {
    config: Config,
    server: RenetServer,
    transport: NetcodeServerTransport,
    usernames: HashMap<ClientId, String>,
    last_updated: Instant,
    last_stats: Instant,
    stats: ServerStats,
    voice_stats: VoiceStats,
}

impl ChatServer {
    pub fn new(config: Config) -> Result<Self, Box<dyn std::error::Error>> {
        let addr = config.socket_addr()?;
        let socket = UdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;
        let public_addr = socket.local_addr()?;

        info!("{} starting on {public_addr}", config.server.name);

        let server_config = ServerConfig {
            current_time: SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?,
            max_clients: config.network.max_clients,
            protocol_id: config.network.protocol_id,
            public_addresses: vec![public_addr],
            authentication: if config.authentication.enabled {
                ServerAuthentication::Secure {
                    private_key: [0; 32],
                }
            } else {
                ServerAuthentication::Unsecure
            },
        };

        let transport = NetcodeServerTransport::new(server_config, socket)?;
        let connection_config = Self::create_connection_config(&config.channels);
        let server = RenetServer::new(connection_config);

        Ok(Self {
            config,
            server,
            transport,
            usernames: HashMap::new(),
            last_updated: Instant::now(),
            last_stats: Instant::now(),
            stats: ServerStats::default(),
            voice_stats: VoiceStats::default(),
        })
    }

    fn create_connection_config(channels: &ChannelSettings) -> ConnectionConfig {
        let mut config = ConnectionConfig::default();
        config.client_channels_config.clear();
        config.server_channels_config.clear();

        let reliable_channel = ChannelConfig {
            channel_id: channels.reliable_channel_id,
            max_memory_usage_bytes: 1024 * 1024,
            send_type: SendType::ReliableOrdered {
                resend_time: Duration::from_millis(200),
            },
        };

        config.client_channels_config.push(reliable_channel.clone());
        config.server_channels_config.push(reliable_channel);

        let voice_channel = ChannelConfig {
            channel_id: channels.voice_channel_id,
            max_memory_usage_bytes: channels.voice_buffer_size_mb * 1024 * 1024,
            send_type: SendType::Unreliable,
        };

        config.client_channels_config.push(voice_channel.clone());
        config.server_channels_config.push(voice_channel);

        config
    }

    /// Drive one tick of the server: receive, process, send.
    ///
    /// The caller controls the loop cadence. For lowest voice latency,
    /// call this as fast as possible (≤1 ms sleeps or spin-poll).
    pub fn update(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let now = Instant::now();
        let duration = now - self.last_updated;
        self.last_updated = now;

        self.server.update(duration);
        self.transport.update(duration, &mut self.server)?;

        self.process_events();
        self.process_reliable_messages();
        self.process_voice_messages();

        self.transport.send_packets(&mut self.server);

        if now.duration_since(self.last_stats)
            >= Duration::from_secs(self.config.logging.stats_interval_secs)
        {
            self.print_stats();
            self.last_stats = now;
        }

        Ok(())
    }

    // ── Event handling ──────────────────────────────────────────────

    fn process_events(&mut self) {
        while let Some(event) = self.server.get_event() {
            match event {
                ServerEvent::ClientConnected { client_id } => {
                    self.handle_client_connected(client_id);
                }
                ServerEvent::ClientDisconnected { client_id, reason } => {
                    self.handle_client_disconnected(client_id, reason);
                }
            }
        }
    }

    fn handle_client_connected(&mut self, client_id: ClientId) {
        let username = self.transport.user_data(client_id).map_or_else(
            || format!("User{client_id}"),
            |data| username_from_user_data(&data),
        );

        info!("{username} ({client_id}) connected");
        self.usernames.insert(client_id, username.clone());

        // Send init with the full user list to the new client
        let init_msg = ServerMessage::InitClient {
            usernames: self.usernames.clone(),
            server_name: self.config.server.name.clone(),
            greeting: self.config.server.greeting_message.clone(),
        };

        if let Some(encoded) = encode_message(&init_msg) {
            self.server
                .send_message(client_id, self.config.channels.reliable_channel_id, encoded);
        }

        // Notify everyone else
        let connected_msg = ServerMessage::ClientConnected {
            client_id,
            username,
        };

        if let Some(encoded) = encode_message(&connected_msg) {
            self.server.broadcast_message_except(
                client_id,
                self.config.channels.reliable_channel_id,
                encoded,
            );
        }
    }

    fn handle_client_disconnected(&mut self, client_id: ClientId, reason: renet::DisconnectReason) {
        if let Some(username) = self.usernames.remove(&client_id) {
            info!("{username} ({client_id}) disconnected: {reason}");
        }

        let msg = ServerMessage::ClientDisconnected { client_id };
        if let Some(encoded) = encode_message(&msg) {
            self.server
                .broadcast_message(self.config.channels.reliable_channel_id, encoded);
        }
    }

    // ── Reliable messages ───────────────────────────────────────────

    fn process_reliable_messages(&mut self) {
        let channel_id = self.config.channels.reliable_channel_id;

        for client_id in self.server.clients_id() {
            while let Some(message) = self.server.receive_message(client_id, channel_id) {
                self.stats.bytes_received += message.len() as u64;

                if let Ok(msg) = bincode::options().deserialize::<ClientMessage>(&message) {
                    self.handle_client_message(client_id, msg);
                }
            }
        }
    }

    fn handle_client_message(&mut self, client_id: ClientId, message: ClientMessage) {
        match message {
            ClientMessage::Text(text) => {
                let username = self.get_username(client_id);
                info!("[{username}]: {text}");
                self.stats.messages_count += 1;

                let msg = ServerMessage::ClientMessage(Message { client_id, text });
                if let Some(encoded) = encode_message(&msg) {
                    self.stats.bytes_sent +=
                        encoded.len() as u64 * self.server.connected_clients() as u64;
                    self.server
                        .broadcast_message(self.config.channels.reliable_channel_id, encoded);
                }
            }
            ClientMessage::Speaking(is_speaking) => {
                let msg = ServerMessage::Speaking {
                    client_id,
                    is_speaking,
                };
                if let Some(encoded) = encode_message(&msg) {
                    self.server
                        .broadcast_message(self.config.channels.reliable_channel_id, encoded);
                }
            }
            ClientMessage::AudioState { muted, deafened } => {
                let msg = ServerMessage::AudioState {
                    client_id,
                    muted,
                    deafened,
                };
                if let Some(encoded) = encode_message(&msg) {
                    self.server.broadcast_message_except(
                        client_id,
                        self.config.channels.reliable_channel_id,
                        encoded,
                    );
                }
            }
            ClientMessage::VoiceData(_) => {
                warn!("Voice data received on reliable channel from {client_id}");
            }
        }
    }

    // ── Voice relay ─────────────────────────────────────────────────

    fn process_voice_messages(&mut self) {
        let channel_id = self.config.channels.voice_channel_id;
        let clients: Vec<ClientId> = self.server.clients_id();

        for client_id in &clients {
            while let Some(message) = self.server.receive_message(*client_id, channel_id) {
                self.stats.bytes_received += message.len() as u64;
                self.voice_stats.packets_received += 1;

                // Validate packet size
                if message.len() < VoiceHeader::SIZE + 3 {
                    self.voice_stats.packets_dropped_malformed += 1;
                    continue;
                }

                if message.len() > VOICE_MTU_BUDGET {
                    self.voice_stats.packets_dropped_oversize += 1;
                    warn!(
                        "Dropping oversize packet: {} bytes from {client_id}",
                        message.len(),
                    );
                    continue;
                }

                // Validate protocol version
                if message[0] != VOICE_PROTOCOL_VERSION {
                    self.voice_stats.packets_dropped_version += 1;
                    warn!("Invalid protocol version {} from {client_id}", message[0]);
                    continue;
                }

                // Parse header
                let mut off = 0;
                let _version = message[off];
                off += 4; // version + reserved

                let hdr_client_id = u64::from_le_bytes(message[off..off + 8].try_into().unwrap());
                off += 8;
                let _seq = u32::from_le_bytes(message[off..off + 4].try_into().unwrap());
                off += 4;
                let _ts_48k = u32::from_le_bytes(message[off..off + 4].try_into().unwrap());
                off += 4;
                let frame_count = message[off] as usize;
                off += 4; // frame_count + padding

                // Validate frame count
                if frame_count == 0 || frame_count > MAX_FRAMES_PER_PACKET {
                    self.voice_stats.packets_dropped_malformed += 1;
                    warn!("Invalid frame count {frame_count} from {client_id}");
                    continue;
                }

                // Client ID mismatch is expected during fast reconnects —
                // the authenticated transport already guarantees the sender,
                // so this is purely informational.
                if hdr_client_id != *client_id {
                    debug!(
                        "Header client_id {hdr_client_id} != sender {client_id} (stale session?)",
                    );
                }

                // Validate all frame lengths
                let mut valid = true;
                let mut validation_offset = off;

                for i in 0..frame_count {
                    if validation_offset + 2 > message.len() {
                        valid = false;
                        break;
                    }

                    let len = u16::from_le_bytes(
                        message[validation_offset..validation_offset + 2]
                            .try_into()
                            .unwrap(),
                    ) as usize;
                    validation_offset += 2;

                    if len == 0 || len > 512 {
                        debug!("Invalid frame {i} size: {len} bytes from {client_id}");
                        valid = false;
                        break;
                    }

                    if validation_offset + len > message.len() {
                        valid = false;
                        break;
                    }

                    validation_offset += len;
                }

                if !valid {
                    self.voice_stats.packets_dropped_malformed += 1;
                    continue;
                }

                if validation_offset != message.len() {
                    self.voice_stats.packets_dropped_malformed += 1;
                    debug!(
                        "Trailing bytes: expected {validation_offset}, got {} from {client_id}",
                        message.len(),
                    );
                    continue;
                }

                // Stats (EMA, not running mean)
                self.voice_stats.total_frames += frame_count as u64;
                #[allow(clippy::cast_precision_loss)] // frame_count ≤ MAX_FRAMES_PER_PACKET (2)
                {
                    self.voice_stats.avg_frames_per_packet =
                        self.voice_stats.avg_frames_per_packet * 0.95 + frame_count as f32 * 0.05;
                }

                // Forward raw bytes to all other clients.
                // Capture len before move — broadcast_message_except takes ownership.
                let recipients = clients.len().saturating_sub(1);
                let msg_len = message.len();

                self.server.broadcast_message_except(
                    *client_id, channel_id, message, // moved, not cloned
                );

                self.voice_stats.packets_forwarded += 1;
                self.stats.voice_packets_count += 1;
                self.stats.bytes_sent += msg_len as u64 * recipients as u64;
            }
        }
    }

    // ── Utilities ───────────────────────────────────────────────────

    fn get_username(&self, client_id: ClientId) -> String {
        self.usernames
            .get(&client_id)
            .cloned()
            .unwrap_or_else(|| format!("User{client_id}"))
    }

    #[allow(clippy::cast_precision_loss)] // display-only stats, precision irrelevant
    fn print_stats(&self) {
        info!("=== {} Statistics ===", self.config.server.name);
        info!("Connected clients: {}", self.server.connected_clients());
        info!("Text messages: {}", self.stats.messages_count);

        info!(
            "Voice packets - RX: {}, FWD: {}",
            self.voice_stats.packets_received, self.voice_stats.packets_forwarded
        );

        if self.voice_stats.packets_received > 0 {
            let total_dropped = self.voice_stats.packets_dropped_malformed
                + self.voice_stats.packets_dropped_oversize
                + self.voice_stats.packets_dropped_version;
            let drop_rate = total_dropped as f32 / self.voice_stats.packets_received as f32 * 100.0;

            info!(
                "Voice drops: {} malformed, {} oversize, {} bad version ({drop_rate:.1}% total)",
                self.voice_stats.packets_dropped_malformed,
                self.voice_stats.packets_dropped_oversize,
                self.voice_stats.packets_dropped_version,
            );
        }

        info!(
            "Avg frames/packet: {:.2}, Total frames: {}",
            self.voice_stats.avg_frames_per_packet, self.voice_stats.total_frames
        );

        info!(
            "Data sent: {:.2} MB, received: {:.2} MB",
            self.stats.bytes_sent as f64 / 1_048_576.0,
            self.stats.bytes_received as f64 / 1_048_576.0
        );

        info!(
            "Active users: {:?}",
            self.usernames.values().collect::<Vec<_>>()
        );
    }

    pub fn addresses(&self) -> Vec<SocketAddr> {
        self.transport.addresses()
    }
}
