// settings.rs - Audio settings with persistence and dark theme UI

use crate::ui::theme::DarkTheme;
use crate::voice::VoiceChat;
use fltk::{prelude::*, *};
use log::info;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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

pub struct AudioSettings {
    voice_chat: Arc<Mutex<Option<VoiceChat>>>,
    settings: Arc<Mutex<Settings>>,
}

impl AudioSettings {
    pub fn new(voice_chat: Arc<Mutex<Option<VoiceChat>>>, settings: Arc<Mutex<Settings>>) -> Self {
        AudioSettings {
            voice_chat,
            settings,
        }
    }

    pub fn show_window(&self) {
        let mut wind = window::Window::default()
            .with_size(450, 600)
            .with_label("Audio Settings");
        wind.make_resizable(false);
        wind.set_color(DarkTheme::BG_PRIMARY);

        let mut col = group::Flex::default_fill().column();
        col.set_margins(15, 15, 15, 15);
        col.set_spacing(15);
        col.set_color(DarkTheme::BG_PRIMARY);

        // Title
        let mut title = frame::Frame::default()
            .with_label("⚙  Audio & Settings")
            .with_align(enums::Align::Center);
        title.set_label_size(18);
        title.set_label_color(DarkTheme::TEXT_PRIMARY);
        col.fixed(&title, 35);

        // Separator
        let mut sep = frame::Frame::default();
        sep.set_frame(enums::FrameType::EngravedBox);
        sep.set_color(DarkTheme::BG_TERTIARY);
        col.fixed(&sep, 2);

        // === Audio Section ===
        let mut audio_section = frame::Frame::default()
            .with_label("Audio Configuration")
            .with_align(enums::Align::Left | enums::Align::Inside);
        audio_section.set_label_color(DarkTheme::TEXT_SECONDARY);
        audio_section.set_label_size(14);
        col.fixed(&audio_section, 25);

        // Loopback toggle
        let mut loopback_btn =
            button::CheckButton::default().with_label("Enable Audio Loopback (hear yourself)");
        loopback_btn.set_checked(self.settings.lock().unwrap().loopback_enabled);
        loopback_btn.set_color(DarkTheme::BG_PRIMARY);
        loopback_btn.set_label_color(DarkTheme::TEXT_PRIMARY);
        loopback_btn.set_selection_color(DarkTheme::ACCENT);
        col.fixed(&loopback_btn, 30);

        // VAD Sensitivity
        let mut vad_frame = frame::Frame::default()
            .with_label("Voice Detection Sensitivity")
            .with_align(enums::Align::Left | enums::Align::Inside);
        vad_frame.set_label_color(DarkTheme::TEXT_SECONDARY);
        vad_frame.set_label_size(12);
        col.fixed(&vad_frame, 20);

        let mut vad_slider = valuator::HorSlider::default();
        vad_slider.set_minimum(0.1);
        vad_slider.set_maximum(0.9);
        vad_slider.set_value(self.settings.lock().unwrap().vad_threshold as f64);
        vad_slider.set_step(0.05, 1);
        vad_slider.set_color(DarkTheme::BG_TERTIARY);
        vad_slider.set_selection_color(DarkTheme::ACCENT);
        col.fixed(&vad_slider, 35);

        let mut vad_value = frame::Frame::default()
            .with_label(&format!(
                "{:.2} (lower = more sensitive)",
                vad_slider.value()
            ))
            .with_align(enums::Align::Center);
        vad_value.set_label_color(DarkTheme::TEXT_MUTED);
        vad_value.set_label_size(11);
        col.fixed(&vad_value, 20);

        // Mute on start
        let mut mute_on_start = button::CheckButton::default().with_label("Start muted by default");
        mute_on_start.set_checked(self.settings.lock().unwrap().mute_on_start);
        mute_on_start.set_color(DarkTheme::BG_PRIMARY);
        mute_on_start.set_label_color(DarkTheme::TEXT_PRIMARY);
        mute_on_start.set_selection_color(DarkTheme::ACCENT);
        col.fixed(&mute_on_start, 30);

        // === Debug Section ===
        let mut sep2 = frame::Frame::default();
        sep2.set_frame(enums::FrameType::EngravedBox);
        sep2.set_color(DarkTheme::BG_TERTIARY);
        col.fixed(&sep2, 2);

        let mut debug_section = frame::Frame::default()
            .with_label("Debug Options")
            .with_align(enums::Align::Left | enums::Align::Inside);
        debug_section.set_label_color(DarkTheme::TEXT_SECONDARY);
        debug_section.set_label_size(14);
        col.fixed(&debug_section, 25);

        // Verbose logging
        let mut debug_verbose = button::CheckButton::default().with_label("Enable verbose logging");
        debug_verbose.set_checked(self.settings.lock().unwrap().verbose_logging);
        debug_verbose.set_color(DarkTheme::BG_PRIMARY);
        debug_verbose.set_label_color(DarkTheme::TEXT_PRIMARY);
        debug_verbose.set_selection_color(DarkTheme::ACCENT);
        col.fixed(&debug_verbose, 30);

        // === Statistics Section ===
        let mut sep3 = frame::Frame::default();
        sep3.set_frame(enums::FrameType::EngravedBox);
        sep3.set_color(DarkTheme::BG_TERTIARY);
        col.fixed(&sep3, 2);

        let mut stats_section = frame::Frame::default()
            .with_label("Audio Statistics")
            .with_align(enums::Align::Left | enums::Align::Inside);
        stats_section.set_label_color(DarkTheme::TEXT_SECONDARY);
        stats_section.set_label_size(14);
        col.fixed(&stats_section, 25);

        let mut stats_display = text::TextDisplay::default();
        let stats_buffer = text::TextBuffer::default();
        stats_display.set_buffer(stats_buffer.clone());
        stats_display.set_text_size(11);
        stats_display.set_color(DarkTheme::BG_INPUT);
        stats_display.set_text_color(DarkTheme::TEXT_PRIMARY);
        stats_display.set_frame(enums::FrameType::BorderBox);
        col.fixed(&stats_display, 100);

        // === Buttons ===
        let mut button_row = group::Flex::default().row();
        col.fixed(&button_row, 40);
        button_row.set_spacing(10);

        let mut reset_btn = button::Button::default().with_label("Reset to Defaults");
        reset_btn.set_color(DarkTheme::BG_TERTIARY);
        reset_btn.set_label_color(DarkTheme::TEXT_PRIMARY);
        reset_btn.set_frame(enums::FrameType::FlatBox);

        let mut save_btn = button::Button::default().with_label("Save");
        save_btn.set_color(DarkTheme::SUCCESS);
        save_btn.set_label_color(enums::Color::White);
        save_btn.set_frame(enums::FrameType::FlatBox);

        let mut close_btn = button::Button::default().with_label("Close");
        close_btn.set_color(DarkTheme::BG_TERTIARY);
        close_btn.set_label_color(DarkTheme::TEXT_PRIMARY);
        close_btn.set_frame(enums::FrameType::FlatBox);

        button_row.end();

        col.end();
        wind.end();
        wind.show();

        // === Callbacks ===

        // Loopback toggle
        let voice_chat = self.voice_chat.clone();
        let settings = self.settings.clone();
        loopback_btn.set_callback(move |b| {
            let enabled = b.is_checked();
            settings.lock().unwrap().loopback_enabled = enabled;

            if let Ok(mut vc_guard) = voice_chat.lock() {
                if let Some(vc) = vc_guard.as_mut() {
                    vc.set_loopback(enabled);
                }
            }
        });

        // VAD slider
        let voice_chat_vad = self.voice_chat.clone();
        let settings_vad = self.settings.clone();
        vad_slider.set_callback(move |s| {
            let value = s.value() as f32;
            vad_value.set_label(&format!("{:.2} (lower = more sensitive)", value));

            settings_vad.lock().unwrap().vad_threshold = value;

            if let Ok(mut vc_guard) = voice_chat_vad.lock() {
                if let Some(_vc) = vc_guard.as_mut() {
                    //  vc.set_vad_threshold(value);
                }
            }
        });

        // Mute on start
        let settings_mute = self.settings.clone();
        mute_on_start.set_callback(move |b| {
            settings_mute.lock().unwrap().mute_on_start = b.is_checked();
        });

        // Verbose logging
        let settings_verbose = self.settings.clone();
        debug_verbose.set_callback(move |b| {
            settings_verbose.lock().unwrap().verbose_logging = b.is_checked();
        });

        // Reset button
        let settings_reset = self.settings.clone();
        let mut vad_slider_clone = vad_slider.clone();
        let loopback_btn_clone = loopback_btn.clone();
        let mute_on_start_clone = mute_on_start.clone();
        let debug_verbose_clone = debug_verbose.clone();
        reset_btn.set_callback(move |_| {
            let defaults = Settings::default();
            vad_slider_clone.set_value(defaults.vad_threshold as f64);
            loopback_btn_clone.set_checked(defaults.loopback_enabled);
            mute_on_start_clone.set_checked(defaults.mute_on_start);
            debug_verbose_clone.set_checked(defaults.verbose_logging);

            let mut settings = settings_reset.lock().unwrap();
            settings.vad_threshold = defaults.vad_threshold;
            settings.loopback_enabled = defaults.loopback_enabled;
            settings.mute_on_start = defaults.mute_on_start;
            settings.verbose_logging = defaults.verbose_logging;
        });

        // Save button
        let settings_save = self.settings.clone();
        save_btn.set_callback(move |_| {
            settings_save.lock().unwrap().save();
            info!("[Settings] Saved to disk");
        });

        // Close button
        close_btn.set_callback(move |_| {
            wind.hide();
        });

        // Update stats periodically
        let voice_chat_stats = self.voice_chat.clone();
        let mut stats_buffer_clone = stats_buffer.clone();
        app::add_timeout3(0.5, move |handle| {
            let mut stats_text = String::new();

            if let Ok(vc_guard) = voice_chat_stats.lock() {
                if let Some(vc) = vc_guard.as_ref() {
                    stats_text.push_str(&format!("🎤 Status: Connected\n"));
                    stats_text.push_str(&format!(
                        "🔊 Speaking: {}\n",
                        if vc.is_speaking() { "Yes" } else { "No" }
                    ));
                    stats_text.push_str(&format!("\n{}", vc.get_stats()));

                    //let threshold = vc.get_vad_threshold();
                    //stats_text.push_str(&format!("\n\nVAD Threshold: {:.2}", threshold));
                } else {
                    stats_text.push_str("Status: Not initialized\n");
                    stats_text.push_str(
                        "\nVoice chat will be initialized\nwhen you connect to a server.",
                    );
                }
            }

            stats_buffer_clone.set_text(&stats_text);
            app::repeat_timeout3(0.5, handle);
        });
    }
}
