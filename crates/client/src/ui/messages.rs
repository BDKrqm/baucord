// ui/messages.rs
use renet::ClientId;

#[derive(Debug, Clone)]
pub enum UiMessage {
    // ── Connection flow ─────────────────────────────────────────────
    Connect {
        nickname: String,
        server: String,
    },
    Disconnect,

    // ── Chat ────────────────────────────────────────────────────────
    SendChatMessage(String),

    // ── Voice controls ──────────────────────────────────────────────
    ToggleMute,
    TestMicrophone(bool),
    SetGateThreshold(f32),
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
    ShowError(String),
    ClearError,

    // ── Application lifecycle ───────────────────────────────────────
    Shutdown,
}
