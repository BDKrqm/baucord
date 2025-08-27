// ui/messages.rs
use renet::ClientId;
use std::collections::HashMap;
use crate::settings::UserAudioSettings;

#[derive(Debug, Clone)]
pub enum UiMessage {
    // Connection flow
    Connect { nickname: String, server: String },
    ConnectionEstablished,
    ConnectionFailed(String),
    Disconnect,
    
    // Network events from server
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
    
    // Chat messages
    SendChatMessage(String),
    ReceiveChatMessage {
        username: String,
        text: String,
    },
    
    // Voice/Audio controls
    ToggleMute,
    SetLoopback(bool),
    SetVadThreshold(f32),
    SetVadEnabled(bool),
    
    // User audio settings
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
    
    // Settings window
    OpenSettings,
    SaveSettings,
    ResetSettings,
    SettingsWindowClosed,  // Track window closure
    
    // Device management
    RefreshDevices,
    SetInputDevice(Option<String>),
    SetOutputDevice(Option<String>),
    DevicesRefreshed,
    
    // Diagnostics
    OpenDiagnostics,
    RunDiagnosticCommand(String),
    UpdateDiagnostics(String),
    DiagnosticsWindowClosed,  // Track window closure
    
    // UI updates
    UpdateUserList,
    UpdateStats,
    ShowError(String),
    ClearError,
    
    // Application lifecycle
    Shutdown,
}