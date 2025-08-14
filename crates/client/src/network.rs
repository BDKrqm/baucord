use bincode::Options;
use log::info;
use renet::{ChannelConfig, ClientId, ConnectionConfig, RenetClient, SendType};
use renet_netcode::{ClientAuthentication, NETCODE_USER_DATA_BYTES, NetcodeClientTransport};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant, SystemTime};

const PROTOCOL_ID: u64 = 27;
const RELIABLE_CHANNEL: u8 = 2;
const VOICE_CHANNEL: u8 = 3;

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
        data: Vec<u8>,
    },
    UserSpeaking {
        client_id: ClientId,
        is_speaking: bool,
    },
    UiUpdate, // For periodic UI refreshes
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
}

impl NetworkClient {
    pub fn connect(server_addr: SocketAddr, username: String) -> Result<Self, String> {
        let mut connection_config = ConnectionConfig::default();

        connection_config
            .client_channels_config
            .push(ChannelConfig {
                channel_id: VOICE_CHANNEL,
                max_memory_usage_bytes: 5 * 1024 * 1024,
                send_type: SendType::Unreliable,
            });

        connection_config
            .server_channels_config
            .push(ChannelConfig {
                channel_id: VOICE_CHANNEL,
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

        // Process voice channel messages
        while let Some(message) = self.client.receive_message(VOICE_CHANNEL) {
            if let Ok(ServerMessage::VoiceData { client_id, data }) =
                bincode::options().deserialize::<ServerMessage>(&message)
            {
                self.message_queue
                    .push(NetworkMessage::VoiceData { client_id, data });
            }
        }

        if let Err(e) = self.transport.send_packets(&mut self.client) {
            self.message_queue
                .push(NetworkMessage::Disconnected(format!(
                    "Failed to send packets: {}",
                    e
                )));
        }
    }

    pub fn send_message(&mut self, text: String) {
        let message = bincode::options()
            .serialize(&ClientMessage::Text(text))
            .expect("Failed to serialize message");
        self.client.send_message(RELIABLE_CHANNEL, message);
    }

    pub fn send_voice_data(&mut self, data: Vec<u8>) {
        let message = bincode::options()
            .serialize(&ClientMessage::VoiceData(data))
            .expect("Failed to serialize voice data");
        self.client.send_message(VOICE_CHANNEL, message);
    }

    pub fn send_speaking_status(&mut self, is_speaking: bool) {
        let message = bincode::options()
            .serialize(&ClientMessage::Speaking(is_speaking))
            .expect("Failed to serialize speaking status");
        self.client.send_message(RELIABLE_CHANNEL, message);
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
