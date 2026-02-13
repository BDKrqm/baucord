use bincode::Options;
use log::{info, warn};
use renet::{ChannelConfig, ClientId, ConnectionConfig, RenetServer, SendType, ServerEvent};
use renet_netcode::{
    NETCODE_USER_DATA_BYTES, NetcodeServerTransport, ServerAuthentication, ServerConfig,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub server: ServerSettings,
    pub network: NetworkSettings,
    pub channels: ChannelSettings,
    pub logging: LoggingSettings,
    pub authentication: AuthSettings,
}

#[derive(Debug, Deserialize)]
pub struct ServerSettings {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub greeting_message: String,
}

#[derive(Debug, Deserialize)]
pub struct NetworkSettings {
    pub max_clients: usize,
    pub protocol_id: u64,
}

#[derive(Debug, Deserialize)]
pub struct ChannelSettings {
    pub reliable_channel_id: u8,
    pub voice_channel_id: u8,
    pub voice_buffer_size_mb: usize,
}

#[derive(Debug, Deserialize)]
pub struct LoggingSettings {
    pub level: String,
    pub stats_interval_secs: u64,
}

#[derive(Debug, Deserialize)]
pub struct AuthSettings {
    pub enabled: bool,
}

impl Config {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = fs::read_to_string(path)?;
        Ok(serde_yaml::from_str(&contents)?)
    }

    pub fn socket_addr(&self) -> String {
        format!("{}:{}", self.server.host, self.server.port)
    }
}
const VOICE_PROTOCOL_VERSION: u8 = 1;
const VOICE_MTU_BUDGET: usize = 1000;
const MAX_FRAMES_PER_PACKET: usize = 2;

#[repr(C)]
#[derive(Copy, Clone)]
struct VoiceHeader {
    version: u8,        // NEW: Protocol version byte
    _reserved: [u8; 3], // NEW: Padding for alignment
    client_id: u64,     // sender id
    seq: u32,           // frame counter
    ts_48k: u32,        // 48kHz timestamp
    frame_count: u8,    // NEW: Number of frames in packet
    _padding: [u8; 3],  // NEW: Alignment padding
}

impl VoiceHeader {
    const SIZE: usize = 24; // CHANGED: from 16 to 24 bytes
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
}

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

#[derive(Default)]
struct ServerStats {
    messages_count: u64,
    voice_packets_count: u64,
    bytes_sent: u64,
    bytes_received: u64,
}

impl ChatServer {
    pub fn new(config: Config) -> Result<Self, Box<dyn std::error::Error>> {
        // [Keep existing initialization code]
        let socket = UdpSocket::bind(config.socket_addr())?;
        let public_addr = socket.local_addr()?;

        info!("{} starting on {}", config.server.name, public_addr);

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
            voice_stats: VoiceStats::default(), // CHANGED
        })
    }

    fn create_connection_config(channels: &ChannelSettings) -> ConnectionConfig {
        let mut config = ConnectionConfig::default();
        config.client_channels_config.clear();
        config.server_channels_config.clear();

        // ADD THIS - RELIABLE CHANNEL CONFIG WITH RESEND TIME
        let reliable_channel = ChannelConfig {
            channel_id: channels.reliable_channel_id,
            max_memory_usage_bytes: 1024 * 1024, // 1MB for chat/control
            send_type: SendType::ReliableOrdered {
                resend_time: Duration::from_millis(200), // Resend after 200ms
            },
        };

        config.client_channels_config.push(reliable_channel.clone());
        config.server_channels_config.push(reliable_channel);

        // EXISTING VOICE CHANNEL
        let voice_channel = ChannelConfig {
            channel_id: channels.voice_channel_id,
            max_memory_usage_bytes: channels.voice_buffer_size_mb * 1024 * 1024,
            send_type: SendType::Unreliable,
        };

        config.client_channels_config.push(voice_channel.clone());
        config.server_channels_config.push(voice_channel);

        config
    }

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
        let username = self
            .transport
            .user_data(client_id)
            .map(|user_data: [u8; 256]| {
                <std::string::String as Username>::from_user_data(&user_data)
            })
            .unwrap_or_else(|| format!("User{}", client_id));

        info!("{} ({}) connected", username, client_id);
        self.usernames.insert(client_id, username.clone());

        // Send init message with current users list and greeting to the new client
        let init_message = ServerMessage::InitClient {
            usernames: self.usernames.clone(),
            server_name: self.config.server.name.clone(),
            greeting: self.config.server.greeting_message.clone(),
        };

        let encoded = bincode::options()
            .serialize(&init_message)
            .expect("Failed to serialize init message");

        self.server
            .send_message(client_id, self.config.channels.reliable_channel_id, encoded);

        // Notify all OTHER clients about the new connection
        let connected_msg = ServerMessage::ClientConnected {
            client_id,
            username: username.clone(),
        };

        let encoded = bincode::options()
            .serialize(&connected_msg)
            .expect("Failed to serialize connected message");

        self.server.broadcast_message_except(
            client_id,
            self.config.channels.reliable_channel_id,
            encoded,
        );
    }

    fn handle_client_disconnected(&mut self, client_id: ClientId, reason: renet::DisconnectReason) {
        if let Some(username) = self.usernames.remove(&client_id) {
            info!("{} ({}) disconnected: {}", username, client_id, reason);
        }

        let message = ServerMessage::ClientDisconnected { client_id };
        let encoded = bincode::options()
            .serialize(&message)
            .expect("Failed to serialize disconnect message");

        self.server
            .broadcast_message(self.config.channels.reliable_channel_id, encoded);
    }

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
                info!("[{}]: {}", username, text);
                self.stats.messages_count += 1;

                let msg = ServerMessage::ClientMessage(Message { client_id, text });
                let encoded = bincode::options()
                    .serialize(&msg)
                    .expect("Failed to serialize text message");

                self.stats.bytes_sent +=
                    encoded.len() as u64 * self.server.connected_clients() as u64;
                self.server
                    .broadcast_message(self.config.channels.reliable_channel_id, encoded);
            }
            ClientMessage::Speaking(is_speaking) => {
                let msg = ServerMessage::Speaking {
                    client_id,
                    is_speaking,
                };
                let encoded = bincode::options()
                    .serialize(&msg)
                    .expect("Failed to serialize speaking message");

                self.server
                    .broadcast_message(self.config.channels.reliable_channel_id, encoded);
            }
            ClientMessage::VoiceData(_) => {
                warn!("Voice data received on reliable channel from {}", client_id);
            }
        }
    }

    fn process_voice_messages(&mut self) {
        let channel_id = self.config.channels.voice_channel_id;
        let clients: Vec<ClientId> = self.server.clients_id();

        for client_id in &clients {
            while let Some(message) = self.server.receive_message(*client_id, channel_id) {
                self.stats.bytes_received += message.len() as u64;
                self.voice_stats.packets_received += 1;

                // STEP 1: Validate packet size
                if message.len() < VoiceHeader::SIZE + 3 {
                    // header + min frame
                    self.voice_stats.packets_dropped_malformed += 1;
                    continue;
                }

                if message.len() > VOICE_MTU_BUDGET {
                    self.voice_stats.packets_dropped_oversize += 1;
                    warn!(
                        "Dropping oversize packet: {} bytes from {}",
                        message.len(),
                        client_id
                    );
                    continue;
                }

                // STEP 2: Validate protocol version
                if message[0] != VOICE_PROTOCOL_VERSION {
                    self.voice_stats.packets_dropped_version += 1;
                    warn!("Invalid protocol version {} from {}", message[0], client_id);
                    continue;
                }

                // STEP 3: Parse and validate header
                let mut off = 0;
                let version = message[off];
                off += 4; // version + reserved

                let hdr_client_id = u64::from_le_bytes(message[off..off + 8].try_into().unwrap());
                off += 8;
                let seq = u32::from_le_bytes(message[off..off + 4].try_into().unwrap());
                off += 4;
                let ts_48k = u32::from_le_bytes(message[off..off + 4].try_into().unwrap());
                off += 4;
                let frame_count = message[off] as usize;
                off += 4; // frame_count + padding

                // STEP 4: Validate frame count
                if frame_count == 0 || frame_count > MAX_FRAMES_PER_PACKET {
                    self.voice_stats.packets_dropped_malformed += 1;
                    warn!("Invalid frame count {} from {}", frame_count, client_id);
                    continue;
                }

                // Optional: Verify client_id matches sender
                if hdr_client_id != *client_id {
                    // Could drop for anti-spoofing, but for now just warn
                    warn!(
                        "Client ID mismatch: header {} != sender {}",
                        hdr_client_id, client_id
                    );
                }

                // STEP 5: Validate all frame lengths
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

                    // Validate individual frame size
                    if len == 0 || len > 512 {
                        warn!("Invalid frame {} size: {} bytes", i, len);
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

                // Should have consumed exactly the entire packet
                if validation_offset != message.len() {
                    self.voice_stats.packets_dropped_malformed += 1;
                    warn!(
                        "Packet size mismatch: expected {}, got {}",
                        validation_offset,
                        message.len()
                    );
                    continue;
                }

                // STEP 6: Update statistics
                self.voice_stats.total_frames += frame_count as u64;
                let packet_count = self.voice_stats.packets_forwarded as f32 + 1.0;
                self.voice_stats.avg_frames_per_packet = (self.voice_stats.avg_frames_per_packet
                    * (packet_count - 1.0)
                    + frame_count as f32)
                    / packet_count;

                // STEP 7: PASSTHROUGH - Forward raw bytes immediately
                // No modification, no re-encoding, no rebundling
                self.server.broadcast_message_except(
                    *client_id,
                    channel_id,
                    message.clone(), // Forward exactly as received
                );

                self.voice_stats.packets_forwarded += 1;
                self.stats.voice_packets_count += 1;

                // Update bytes sent stat
                let recipients = clients.len().saturating_sub(1);
                self.stats.bytes_sent += message.len() as u64 * recipients as u64;
            }
        }
    }

    fn get_username(&self, client_id: ClientId) -> String {
        self.usernames
            .get(&client_id)
            .cloned()
            .unwrap_or_else(|| format!("User{}", client_id))
    }

    fn print_stats(&self) {
        info!("=== {} Statistics ===", self.config.server.name);
        info!("Connected clients: {}", self.server.connected_clients());
        info!("Text messages: {}", self.stats.messages_count);

        // NEW: Detailed voice statistics
        info!(
            "Voice packets - RX: {}, FWD: {}",
            self.voice_stats.packets_received, self.voice_stats.packets_forwarded
        );

        if self.voice_stats.packets_received > 0 {
            let drop_rate = (self.voice_stats.packets_dropped_malformed
                + self.voice_stats.packets_dropped_oversize
                + self.voice_stats.packets_dropped_version) as f32
                / self.voice_stats.packets_received as f32
                * 100.0;

            info!(
                "Voice drops: {} malformed, {} oversize, {} bad version ({:.1}% total)",
                self.voice_stats.packets_dropped_malformed,
                self.voice_stats.packets_dropped_oversize,
                self.voice_stats.packets_dropped_version,
                drop_rate
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

trait Username {
    fn from_user_data(user_data: &[u8; NETCODE_USER_DATA_BYTES]) -> String;
}

impl Username for String {
    fn from_user_data(user_data: &[u8; NETCODE_USER_DATA_BYTES]) -> String {
        let len = (user_data[0] as usize).min(NETCODE_USER_DATA_BYTES - 1);
        let data = &user_data[1..=len];

        String::from_utf8_lossy(data).into_owned()
    }
}
