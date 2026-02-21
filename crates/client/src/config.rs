use log::info;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use core::PeerAudioSettings;

/// Stable hash key for per-peer audio settings.
pub fn peer_key(username: &str) -> String {
    let mut h = DefaultHasher::new();
    username.hash(&mut h);
    format!("{:016x}", h.finish())
}

// ── Bindable actions ────────────────────────────────────────────────────

/// All actions that can be bound to a global hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindableAction {
    ToggleMute,
    ToggleDeafen,
}

impl BindableAction {
    pub const ALL: &'static [BindableAction] = &[
        BindableAction::ToggleMute,
        BindableAction::ToggleDeafen,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            BindableAction::ToggleMute => "Toggle Mute",
            BindableAction::ToggleDeafen => "Toggle Deafen",
        }
    }
}

// ── Keybind entry ───────────────────────────────────────────────────────

/// A single keybind: pairs an action with a key combination string.
///
/// The `bind` field is human-readable: `"Ctrl+Shift+M"`, `"Alt+F1"`, `"F9"`.
/// Modifiers: `Ctrl`, `Alt`, `Shift`. Separator: `+`.
/// The key name uses `keyboard-types` Code names (KeyM, F1, Space, Digit0…).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeybindEntry {
    pub action: BindableAction,
    pub bind: String,
}

impl KeybindEntry {
    /// Parse the bind string into modifiers + key code.
    pub fn parse(&self) -> Option<ParsedKeybind> {
        parse_bind(&self.bind)
    }

    pub fn is_empty(&self) -> bool {
        self.bind.is_empty()
    }

    pub fn display(&self) -> &str {
        if self.bind.is_empty() {
            "Not set"
        } else {
            &self.bind
        }
    }
}

/// Result of parsing a bind string.
pub struct ParsedKeybind {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Key code string, e.g. "KeyM", "F1", "Space".
    pub code: String,
}

/// Parse a bind string like "Ctrl+Shift+M" into components.
pub fn parse_bind(s: &str) -> Option<ParsedKeybind> {
    if s.is_empty() {
        return None;
    }

    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut code: Option<String> = None;

    for part in s.split('+') {
        let part = part.trim();
        match part.to_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "alt" => alt = true,
            "shift" => shift = true,
            _ => code = Some(part.to_string()),
        }
    }

    Some(ParsedKeybind {
        ctrl,
        alt,
        shift,
        code: code?,
    })
}

/// Build a bind string from components.
pub fn build_bind_string(ctrl: bool, alt: bool, shift: bool, code: &str) -> String {
    let mut parts = Vec::new();
    if ctrl {
        parts.push("Ctrl");
    }
    if alt {
        parts.push("Alt");
    }
    if shift {
        parts.push("Shift");
    }
    parts.push(code);
    parts.join("+")
}

fn default_keybinds() -> Vec<KeybindEntry> {
    vec![
        KeybindEntry {
            action: BindableAction::ToggleMute,
            bind: "Ctrl+Shift+KeyM".to_string(),
        },
        KeybindEntry {
            action: BindableAction::ToggleDeafen,
            bind: "Ctrl+Shift+KeyD".to_string(),
        },
    ]
}

// ── Settings ────────────────────────────────────────────────────────────

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

    // Keybinds
    #[serde(default = "default_keybinds")]
    pub keybinds: Vec<KeybindEntry>,

    // Per-peer audio (keyed by hashed username)
    pub peer_settings: HashMap<String, PeerAudioSettings>,
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
            keybinds: default_keybinds(),
            peer_settings: HashMap::new(),
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

    pub fn reset(&mut self) {
        *self = Settings::default();
        self.save();
    }

    /// Get the bind string for a given action, or empty string if unbound.
    pub fn bind_for(&self, action: BindableAction) -> &str {
        self.keybinds
            .iter()
            .find(|e| e.action == action)
            .map(|e| e.bind.as_str())
            .unwrap_or("")
    }

    pub fn set_bind(&mut self, action: BindableAction, bind: String) {
        if let Some(entry) = self.keybinds.iter_mut().find(|e| e.action == action) {
            entry.bind = bind;
        } else {
            self.keybinds.push(KeybindEntry { action, bind });
        }
    }
}