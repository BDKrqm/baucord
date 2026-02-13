// ui/messages.rs
use renet::ClientId;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum UiMessage {
    // ── Connection flow ─────────────────────────────────────────────
    Connect {
        nickname: String,
        server: String,
    },
    ConnectionEstablished,
    ConnectionFailed(String),
    Disconnect,

    // ── Network events from server ──────────────────────────────────
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
    UserSpeaking {
        client_id: ClientId,
        is_speaking: bool,
    },

    // ── Chat ────────────────────────────────────────────────────────
    SendChatMessage(String),
    ReceiveChatMessage {
        username: String,
        text: String,
    },

    // ── Voice controls ──────────────────────────────────────────────
    ToggleMute,
    SetLoopback(bool),
    SetVadThreshold(f32),
    SetVadEnabled(bool),
    SetNoiseSuppression(bool),
    SetInputGain(f32),
    SetOutputGain(f32),

    // ── Per-user audio ──────────────────────────────────────────────
    MuteUser {
        client_id: ClientId,
        username: String,
        muted: bool,
    },
    SetUserVolume {
        client_id: ClientId,
        username: String,
        volume: f32,
    },
    ResetUserAudio {
        client_id: ClientId,
        username: String,
    },

    // ── Settings window ─────────────────────────────────────────────
    OpenSettings,
    SaveSettings,
    ResetSettings,
    SettingsWindowClosed,

    // ── Device management ───────────────────────────────────────────
    SetInputDevice(Option<String>),
    SetOutputDevice(Option<String>),

    // ── UI updates ──────────────────────────────────────────────────
    UpdateUserList,
    ShowError(String),
    ClearError,

    // ── Speaking state from voice thread ─────────────────────────────
    LocalSpeakingChanged(bool),

    // ── Application lifecycle ───────────────────────────────────────
    Shutdown,
}
