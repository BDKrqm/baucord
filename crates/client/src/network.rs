use bincode::Options;
use renet::{ChannelConfig, ClientId, ConnectionConfig, RenetClient, SendType};
use renet_netcode::{ClientAuthentication, NETCODE_USER_DATA_BYTES, NetcodeClientTransport};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant, SystemTime};

const PROTOCOL_ID: u64 = 27;

const VOICE_PACKET_BATCH_THRESHOLD: usize = 2;
const RELIABLE_CHANNEL: u8 = 2;
const VOICE_CHANNEL: u8 = 3;
const VOICE_PROTOCOL_VERSION: u8 = 1;
const VOICE_MTU_BUDGET: usize = 800; // Reduced from 1000
const MAX_FRAMES_PER_PACKET: usize = 2; // Already 2, keep it

#[derive(Debug, Clone)]
pub enum NetworkMessage {
    Connected,
    Disconnected(String),
    InitClient {
        users: HashMap<ClientId, String>,
        server_name: String,
        greeting: String,
    },
    UserJoined {
        client_id: ClientId,
        username: String,
    },
    UserLeft {
        client_id: ClientId,
        username: String,
    },
    TextMessage {
        client_id: ClientId,
        username: String,
        text: String,
    },
    VoiceData {
        client_id: ClientId,
        seq: u32,
        timestamp: u32,
        data: Vec<u8>,
    },
    UserSpeaking {
        client_id: ClientId,
        is_speaking: bool,
    },
    UiUpdate, // For periodic UI refreshes
}

#[repr(C)]
#[derive(Copy, Clone)]
struct VoiceHeader {
    version: u8,        // Protocol version (1)
    _reserved: [u8; 3], // Padding for alignment
    client_id: u64,     // sender id
    seq: u32,           // monotonic frame counter
    ts_48k: u32,        // 48kHz sample clock
    frame_count: u8,    // Number of frames (1-2)
    _padding: [u8; 3],  // Alignment padding
}

impl VoiceHeader {
    const SIZE: usize = 24; // 1+3+8+4+4+1+3
}

struct VoiceBundler {
    buf: Vec<u8>,
    frames: u8,
    base_seq: u32,
    base_ts: u32,
}

impl VoiceBundler {
    fn new() -> Self {
        Self {
            buf: Vec::with_capacity(VOICE_MTU_BUDGET),
            frames: 0,
            base_seq: 0,
            base_ts: 0,
        }
    }

    fn begin(&mut self, client_id: u64, seq: u32, ts_48k: u32) {
        self.buf.clear();
        self.frames = 0;
        self.base_seq = seq;
        self.base_ts = ts_48k;

        // Write header with version byte
        self.buf.push(VOICE_PROTOCOL_VERSION); // Version 1
        self.buf.extend_from_slice(&[0, 0, 0]); // Reserved
        self.buf.extend_from_slice(&client_id.to_le_bytes());
        self.buf.extend_from_slice(&seq.to_le_bytes());
        self.buf.extend_from_slice(&ts_48k.to_le_bytes());
        self.buf.push(0); // Placeholder for frame_count
        self.buf.extend_from_slice(&[0, 0, 0]); // Padding
    }

    fn try_push_frame(&mut self, opus_frame: &[u8]) -> bool {
        if self.frames as usize >= MAX_FRAMES_PER_PACKET {
            return false;
        }

        // Validate frame size
        if opus_frame.len() > 512 {
            return false; // Opus frames should never be this large
        }

        let need = 2 + opus_frame.len();
        if self.buf.len() + need > VOICE_MTU_BUDGET {
            return false;
        }

        let len_le = (opus_frame.len() as u16).to_le_bytes();
        self.buf.extend_from_slice(&len_le);
        self.buf.extend_from_slice(opus_frame);
        self.frames += 1;
        true
    }

    fn is_empty(&self) -> bool {
        self.frames == 0
    }

    fn finish_into_vec(&mut self) -> Option<Vec<u8>> {
        if self.frames == 0 {
            return None;
        }
        self.buf[20] = self.frames; // frame_count is at byte 20, NOT 16!
        Some(self.buf.clone())
    }
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

pub struct NetworkClient {
    client: RenetClient,
    transport: NetcodeClientTransport,
    usernames: HashMap<ClientId, String>,
    message_queue: Vec<NetworkMessage>,
    last_updated: Instant,
    voice_bundler: VoiceBundler,

    // Monotonic 48kHz counters
    voice_next_seq: u32,
    voice_ts_48k: u32, // Increments by 480 per 10ms frame

    // Reordering buffer for received packets
    reorder_buffer: HashMap<(u64, u32), Vec<u8>>, // (client_id, seq) -> packet
    expected_seqs: HashMap<u64, u32>,             // client_id -> next expected seq
    last_speaking_sent: bool,
    last_speaking_instant: Instant,
    speaking_cache: HashMap<ClientId, (bool, Instant)>,
}

impl NetworkClient {
    pub fn connect(server_addr: SocketAddr, username: String) -> Result<Self, String> {
        let mut connection_config = ConnectionConfig::default();

        // MUST CLEAR THE DEFAULT CHANNELS (0, 1, 2)
        connection_config.client_channels_config.clear();
        connection_config.server_channels_config.clear();

        // Now add YOUR channels (2 and 3) without conflict
        connection_config
            .client_channels_config
            .push(ChannelConfig {
                channel_id: RELIABLE_CHANNEL, // 2 - now won't conflict
                max_memory_usage_bytes: 1024 * 1024,
                send_type: SendType::ReliableOrdered {
                    resend_time: Duration::from_millis(200),
                },
            });

        connection_config
            .server_channels_config
            .push(ChannelConfig {
                channel_id: RELIABLE_CHANNEL, // 2
                max_memory_usage_bytes: 1024 * 1024,
                send_type: SendType::ReliableOrdered {
                    resend_time: Duration::from_millis(200),
                },
            });

        connection_config
            .client_channels_config
            .push(ChannelConfig {
                channel_id: VOICE_CHANNEL, // 3
                max_memory_usage_bytes: 5 * 1024 * 1024,
                send_type: SendType::Unreliable,
            });

        connection_config
            .server_channels_config
            .push(ChannelConfig {
                channel_id: VOICE_CHANNEL, // 3
                max_memory_usage_bytes: 5 * 1024 * 1024,
                send_type: SendType::Unreliable,
            });

        let client = RenetClient::new(connection_config);
        let socket =
            UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("Failed to bind socket: {}", e))?;

        let current_time = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();
        let client_id = current_time.as_millis() as u64;

        let authentication = ClientAuthentication::Unsecure {
            server_addr,
            client_id,
            user_data: Some(Username(username).to_netcode_user_data()),
            protocol_id: PROTOCOL_ID,
        };

        let transport = NetcodeClientTransport::new(current_time, authentication, socket)
            .map_err(|e| format!("Failed to create transport: {}", e))?;

        Ok(NetworkClient {
            client,
            transport,
            usernames: HashMap::new(),
            message_queue: vec![NetworkMessage::Connected],
            last_updated: Instant::now(),
            voice_bundler: VoiceBundler::new(),
            voice_next_seq: 0,
            voice_ts_48k: 0,
            reorder_buffer: HashMap::new(),
            expected_seqs: HashMap::new(),
            last_speaking_sent: false,
last_speaking_instant: Instant::now()
    .checked_sub(Duration::from_secs(5))
    .unwrap(),
speaking_cache: HashMap::new(),
        })
    }

    pub fn update(&mut self) {
        let now = Instant::now();
        let duration = now - self.last_updated;
        self.last_updated = now;

        self.client.update(duration);

        if let Err(e) = self.transport.update(duration, &mut self.client) {
            self.message_queue
                .push(NetworkMessage::Disconnected(e.to_string()));
            return;
        }

        if let Some(reason) = self.client.disconnect_reason() {
            self.message_queue
                .push(NetworkMessage::Disconnected(reason.to_string()));
            return;
        }

        // Process reliable channel messages
        while let Some(message) = self.client.receive_message(RELIABLE_CHANNEL) {
            if let Ok(msg) = bincode::options().deserialize::<ServerMessage>(&message) {
                match msg {
                    ServerMessage::InitClient {
                        usernames,
                        server_name,
                        greeting,
                    } => {
                        // This is the first message we receive after connecting
                        self.usernames = usernames.clone();
                        self.message_queue.push(NetworkMessage::InitClient {
                            users: usernames,
                            server_name,
                            greeting,
                        });
                    }
                    ServerMessage::ClientConnected {
                        client_id,
                        username,
                    } => {
                        self.usernames.insert(client_id, username.clone());
                        self.message_queue.push(NetworkMessage::UserJoined {
                            client_id,
                            username,
                        });
                    }
                    ServerMessage::ClientDisconnected { client_id } => {
                        let username = self
                            .usernames
                            .remove(&client_id)
                            .unwrap_or_else(|| format!("User{}", client_id));

                        self.message_queue.push(NetworkMessage::UserLeft {
                            client_id,
                            username,
                        });
                    }
                    ServerMessage::ClientMessage(msg) => {
                        let username = self
                            .usernames
                            .get(&msg.client_id)
                            .cloned()
                            .unwrap_or_else(|| format!("User{}", msg.client_id));

                        self.message_queue.push(NetworkMessage::TextMessage {
                            client_id: msg.client_id,
                            username,
                            text: msg.text,
                        });
                    }
                    ServerMessage::Speaking {
                        client_id,
                        is_speaking,
                    } => {
                        self.message_queue.push(NetworkMessage::UserSpeaking {
                            client_id,
                            is_speaking,
                        });
                    }
                    ServerMessage::VoiceData { .. } => {
                        // Shouldn't happen on reliable channel
                    }
                }
            }
        }

        // Process voice channel messages with validation
        while let Some(message) = self.client.receive_message(VOICE_CHANNEL) {
            if !self.process_voice_packet(message.to_vec()) {
                // Invalid packet, already logged in process_voice_packet
                continue;
            }
        }

        // Clean up old reorder buffer entries
        self.cleanup_reorder_buffer();

        if let Err(e) = self.transport.send_packets(&mut self.client) {
            self.message_queue
                .push(NetworkMessage::Disconnected(format!(
                    "Failed to send packets: {}",
                    e
                )));
        }
    }

    fn process_voice_packet(&mut self, message: Vec<u8>) -> bool {
        const OPUS_10MS_48K: u32 = 480;

        // Cheap structural checks up front
        if message.len() < VoiceHeader::SIZE + 3 {
            return false;
        }
        if message[0] != VOICE_PROTOCOL_VERSION {
            return false;
        }

        let mut off = 0;

        // version + 3 reserved
        let _version = message[off];
        off += 4;

        // header fields (LE v1)
        if off + 8 + 4 + 4 + 1 + 3 > message.len() {
            return false;
        }
        let client_id = u64::from_le_bytes(message[off..off + 8].try_into().unwrap());
        off += 8;
        let base_seq = u32::from_le_bytes(message[off..off + 4].try_into().unwrap());
        off += 4;
        let base_ts = u32::from_le_bytes(message[off..off + 4].try_into().unwrap());
        off += 4;
        let frame_count = message[off] as usize;
        off += 4; // 1 byte count + 3 pad

        // Validate frame_count against your negotiated cap
        if frame_count == 0 || frame_count > MAX_FRAMES_PER_PACKET {
            return false;
        }

        // Compute signed distance to know if this packet moves us forward
        let expected = self
            .expected_seqs
            .get(&client_id)
            .copied()
            .unwrap_or(base_seq);
        let seq_diff = base_seq.wrapping_sub(expected) as i32;

        // Always unbundle frames and assign per-frame seq/ts (even if slightly reordered)
        let mut seq_frame = base_seq;
        let mut ts_frame = base_ts;

        for _ in 0..frame_count {
            if off + 2 > message.len() {
                return false;
            }
            let len = u16::from_le_bytes(message[off..off + 2].try_into().unwrap()) as usize;
            off += 2;

            if len == 0 || len > 512 || off + len > message.len() {
                return false;
            }

            let data = message[off..off + len].to_vec();
            off += len;

            self.message_queue.push(NetworkMessage::VoiceData {
                client_id,
                seq: seq_frame,
                timestamp: ts_frame,
                data,
            });

            seq_frame = seq_frame.wrapping_add(1);
            ts_frame = ts_frame.wrapping_add(OPUS_10MS_48K);
        }

        // Advance expected only if this packet is new or ahead (don’t move it backward on reorders)
        if seq_diff >= 0 {
            self.expected_seqs
                .insert(client_id, base_seq.wrapping_add(frame_count as u32));
        }

        true
    }

    fn cleanup_reorder_buffer(&mut self) {
        // Remove packets older than 50ms
        let now_keys: Vec<_> = self.reorder_buffer.keys().cloned().collect();
        for (client_id, seq) in now_keys {
            if let Some(expected) = self.expected_seqs.get(&client_id) {
                let age = expected.wrapping_sub(seq);
                if age > 5 {
                    // More than 5 frames old
                    self.reorder_buffer.remove(&(client_id, seq));
                }
            }
        }
    }
    /// Send one 10ms Opus frame
    pub fn send_voice_frame_raw(
        &mut self,
        our_client_id: ClientId,
        _ts_48k_ignored: u32,
        opus_frame: &[u8],
    ) {
        // Start a new packet if needed
        if self.voice_bundler.is_empty() {
            self.voice_bundler
                .begin(our_client_id, self.voice_next_seq, self.voice_ts_48k);
        }

        // If it doesn't fit, flush and begin a new one
        if !self.voice_bundler.try_push_frame(opus_frame) {
            self.flush_voice_packet();
            self.voice_bundler
                .begin(our_client_id, self.voice_next_seq, self.voice_ts_48k);

            if !self.voice_bundler.try_push_frame(opus_frame) {
                // Frame too large, drop it
                return;
            }
        }

        // Increment counters for next frame
        self.voice_next_seq = self.voice_next_seq.wrapping_add(1);
        self.voice_ts_48k = self.voice_ts_48k.wrapping_add(480);

        // FLUSH AT THRESHOLD (not just at max)
        if self.voice_bundler.frames >= VOICE_PACKET_BATCH_THRESHOLD as u8 {
            self.flush_voice_packet();
        }
    }

    /// Flush the current voice bundle
    pub fn flush_voice_packet(&mut self) {
        if let Some(pkt) = self.voice_bundler.finish_into_vec()
            && pkt.len() <= VOICE_MTU_BUDGET {
                self.client.send_message(VOICE_CHANNEL, pkt);
            }
    }

    pub fn send_message(&mut self, text: String) {
        let message = bincode::options()
            .serialize(&ClientMessage::Text(text))
            .expect("Failed to serialize message");
        self.client.send_message(RELIABLE_CHANNEL, message);
    }

   pub fn send_speaking_status(&mut self, is_speaking: bool) {
    use std::time::{Duration, Instant};
    const SPEAKING_THROTTLE_MS: u64 = 50;

    let now = Instant::now();
    let due = now
        .duration_since(self.last_speaking_instant)
        >= Duration::from_millis(SPEAKING_THROTTLE_MS);
    let edge = is_speaking != self.last_speaking_sent;

    // Send only on edge or when the throttle window elapsed
    if edge || due {
        let message = bincode::options()
            .serialize(&ClientMessage::Speaking(is_speaking))
            .expect("Failed to serialize speaking status");
        self.client.send_message(RELIABLE_CHANNEL, message);

        self.last_speaking_sent = is_speaking;
        self.last_speaking_instant = now;
    }
}
 

    pub fn poll_messages(&mut self) -> Vec<NetworkMessage> {
        self.message_queue.drain(..).collect()
    }

    pub fn is_connected(&self) -> bool {
        self.client.is_connected()
    }

    pub fn is_connecting(&self) -> bool {
        self.client.is_connecting()
    }

    pub fn disconnect(&mut self) {
        self.client.disconnect();
    }

    pub fn get_users(&self) -> &HashMap<ClientId, String> {
        &self.usernames
    }
}

struct Username(String);

impl Username {
    pub fn to_netcode_user_data(&self) -> [u8; NETCODE_USER_DATA_BYTES] {
        let mut user_data = [0u8; NETCODE_USER_DATA_BYTES];
        if self.0.len() > NETCODE_USER_DATA_BYTES - 8 {
            panic!("Username is too big");
        }
        user_data[0] = self.0.len() as u8;
        user_data[1..self.0.len() + 1].copy_from_slice(self.0.as_bytes());
        user_data
    }
}
