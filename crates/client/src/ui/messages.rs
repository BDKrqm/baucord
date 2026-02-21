// ui/messages.rs
use renet::ClientId;

/// Result of the background version check.
#[derive(Debug, Clone)]
pub enum VersionCheckResult {
    /// Remote version is newer.
    UpdateAvailable { remote_version: String },
    /// Up to date, or the check failed (don't block the user).
    UpToDate,
}

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
    ToggleDeafen,
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

    // ── Per-user audio popup ────────────────────────────────────────
    OpenUserAudioPopup {
        client_id: ClientId,
        username: String,
        click_x: i32,
        click_y: i32,
    },

    // ── Device management ───────────────────────────────────────────
    SetInputDevice(Option<String>),
    SetOutputDevice(Option<String>),

    // ── Update flow ─────────────────────────────────────────────────
    /// Background version check finished.
    VersionCheckComplete(VersionCheckResult),
    /// User chose to skip the update and proceed to connect.
    SkipUpdate,
    /// User clicked "Update" — start the download.
    StartDownload(String),
    /// Download thread finished successfully. Contains the file path.
    DownloadComplete(String),
    /// Download thread hit an error.
    DownloadFailed(String),
    /// User cancelled an in-progress download.
    DownloadCancelled,
    /// User clicked "Open File" — open the zip and shut down.
    OpenUpdateAndQuit(String),

    // ── UI updates ──────────────────────────────────────────────────
    ShowError(String),
    ClearError,

    // ── Keybinds ────────────────────────────────────────────────────
    SetKeybind {
        action: crate::config::BindableAction,
        bind: String,
    },

    // ── Application lifecycle ───────────────────────────────────────
    Shutdown,
}