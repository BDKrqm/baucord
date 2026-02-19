use log::info;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

/// Stable hash key for per-user audio settings.
/// Prevents JSON injection from malicious usernames.
pub fn user_key(username: &str) -> String {
    let mut h = DefaultHasher::new();
    username.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Persisted application settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub nickname: String,
    pub last_server: String,

    // Voice
    pub gate_threshold: f32,
    pub input_gain: f32,
    pub output_gain: f32,

    // Window
    pub window_width: i32,
    pub window_height: i32,
    pub window_x: Option<i32>,
    pub window_y: Option<i32>,

    // Devices
    pub input_device: Option<String>,
    pub output_device: Option<String>,

    // Per-user audio (keyed by hashed username)
    pub user_audio_settings: HashMap<String, UserAudioSettings>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            nickname: String::new(),
            last_server: "64.176.16.18:5000".to_string(),
            gate_threshold: 0.005,
            input_gain: 1.0,
            output_gain: 1.0,
            window_width: 1000,
            window_height: 700,
            window_x: None,
            window_y: None,
            input_device: None,
            output_device: None,
            user_audio_settings: HashMap::new(),
        }
    }
}

impl Settings {
    fn get_settings_path() -> PathBuf {
        let mut path = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        path.push("settings.json");
        path
    }

    pub fn load() -> Self {
        let path = Self::get_settings_path();
        if path.exists() {
            if let Ok(contents) = fs::read_to_string(&path) {
                if let Ok(settings) = serde_json::from_str(&contents) {
                    info!("Loaded settings from {:?}", path);
                    return settings;
                }
            }
        }
        info!("Using default settings");
        Settings::default()
    }

    pub fn save(&self) {
        let path = Self::get_settings_path();
        match serde_json::to_string_pretty(self) {
            Ok(contents) => {
                if let Err(e) = fs::write(&path, contents) {
                    info!("Failed to save settings: {}", e);
                } else {
                    info!("Settings saved to {:?}", path);
                }
            }
            Err(e) => info!("Failed to serialize settings: {}", e),
        }
    }

    /// Reset all settings to defaults and persist to disk.
    pub fn reset(&mut self) {
        *self = Settings::default();
        self.save();
    }
}

/// Per-user audio settings (persisted by username).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserAudioSettings {
    pub volume: f32,
    pub is_muted: bool,
}

impl Default for UserAudioSettings {
    fn default() -> Self {
        UserAudioSettings {
            volume: 1.0,
            is_muted: false,
        }
    }
}