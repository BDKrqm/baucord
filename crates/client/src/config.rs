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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeybindEntry {
    pub action: BindableAction,
    pub bind: String,
}

impl KeybindEntry {
    pub fn parse(&self) -> Option<ParsedKeybind> {
        parse_bind(&self.bind)
    }

    pub fn is_empty(&self) -> bool {
        self.bind.is_empty()
    }

    pub fn display(&self) -> &str {
        if self.bind.is_empty() { "Not set" } else { &self.bind }
    }
}

pub struct ParsedKeybind {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub code: String,
}

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

    Some(ParsedKeybind { ctrl, alt, shift, code: code? })
}

pub fn build_bind_string(ctrl: bool, alt: bool, shift: bool, code: &str) -> String {
    let mut parts = Vec::new();
    if ctrl { parts.push("Ctrl"); }
    if alt { parts.push("Alt"); }
    if shift { parts.push("Shift"); }
    parts.push(code);
    parts.join("+")
}

// ── Settings sub-sections ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSettings {
    pub nickname: String,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self { nickname: String::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSettings {
    pub address: String,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self { address: "64.176.16.18:5000".to_string() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioInputSettings {
    pub device: Option<String>,
    pub gain: f32,
}

impl Default for AudioInputSettings {
    fn default() -> Self {
        Self { device: None, gain: 1.0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioOutputSettings {
    pub device: Option<String>,
    pub gain: f32,
}

impl Default for AudioOutputSettings {
    fn default() -> Self {
        Self { device: None, gain: 1.0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioSettings {
    pub input: AudioInputSettings,
    pub output: AudioOutputSettings,
    pub gate_threshold: f32,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            input: AudioInputSettings::default(),
            output: AudioOutputSettings::default(),
            gate_threshold: 0.005,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowSettings {
    pub width: i32,
    pub height: i32,
    pub x: Option<i32>,
    pub y: Option<i32>,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self { width: 1000, height: 700, x: None, y: None }
    }
}

fn default_keybinds() -> Vec<KeybindEntry> {
    vec![
        KeybindEntry { action: BindableAction::ToggleMute, bind: "Ctrl+Shift+KeyM".to_string() },
        KeybindEntry { action: BindableAction::ToggleDeafen, bind: "Ctrl+Shift+KeyD".to_string() },
    ]
}

// ── Root settings ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub user: UserSettings,
    #[serde(default)]
    pub server: ServerSettings,
    #[serde(default)]
    pub audio: AudioSettings,
    #[serde(default)]
    pub window: WindowSettings,
    #[serde(default = "default_keybinds")]
    pub keybinds: Vec<KeybindEntry>,
    #[serde(default)]
    pub peer_settings: HashMap<String, PeerAudioSettings>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            user: UserSettings::default(),
            server: ServerSettings::default(),
            audio: AudioSettings::default(),
            window: WindowSettings::default(),
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

    /// Compute centered position for a child window of given size,
    /// relative to the saved main window geometry (fallback: screen center).
    pub fn center_child(&self, child_w: i32, child_h: i32) -> (i32, i32) {
        if let (Some(wx), Some(wy)) = (self.window.x, self.window.y) {
            (
                wx + (self.window.width - child_w) / 2,
                wy + (self.window.height - child_h) / 2,
            )
        } else {
            let (sw, sh) = fltk::app::screen_size();
            ((sw as i32 - child_w) / 2, (sh as i32 - child_h) / 2)
        }
    }
}