use log::info;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

// Settings struct that will be serialized/deserialized
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub nickname: String,
    pub last_server: String,
    pub vad_threshold: f32,
    pub loopback_enabled: bool,
    pub verbose_logging: bool,
    pub mute_on_start: bool,
    pub window_width: i32,
    pub window_height: i32,
    pub window_x: Option<i32>,
    pub window_y: Option<i32>,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub user_audio_settings: HashMap<String, UserAudioSettings>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            nickname: String::new(),
            last_server: "127.0.0.1:5000".to_string(),
            vad_threshold: 0.3,
            loopback_enabled: false,
            verbose_logging: false,
            mute_on_start: false,
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
    // Get the path to the settings file
    fn get_settings_path() -> PathBuf {
        let mut path = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));

        // Just add -dev suffix in debug mode
        if cfg!(debug_assertions) {
            path.push("baucord-dev");
        } else {
            path.push("baucord");
        }

        fs::create_dir_all(&path).ok();
        path.push("settings.json");
        path
    }

    // Load settings from disk
    pub fn load() -> Self {
        let path = Self::get_settings_path();

        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(contents) => match serde_json::from_str(&contents) {
                    Ok(settings) => {
                        info!("Loaded settings from {:?}", path);
                        return settings;
                    }
                    Err(e) => {
                        info!("Failed to parse settings: {}", e);
                    }
                },
                Err(e) => {
                    info!("Failed to read settings file: {}", e);
                }
            }
        }

        info!("Using default settings");
        Settings::default()
    }

    // Save settings to disk
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
            Err(e) => {
                info!("Failed to serialize settings: {}", e);
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserAudioSettings {
    pub volume: f32, // 0.0 to 2.0 (0% to 200%)
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
