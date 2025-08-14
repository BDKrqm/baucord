use crate::settings::Settings;
use crate::ui::theme::DarkTheme;
use crate::voice::VoiceChat;
use fltk::{prelude::*, *};
use log::info;
use std::sync::{Arc, Mutex};

pub struct AudioSettingsWindow {
    voice_chat: Arc<Mutex<Option<VoiceChat>>>,
    settings: Arc<Mutex<Settings>>,
}

impl AudioSettingsWindow {
    pub fn new(voice_chat: Arc<Mutex<Option<VoiceChat>>>, settings: Arc<Mutex<Settings>>) -> Self {
        AudioSettingsWindow {
            voice_chat,
            settings,
        }
    }

    pub fn show_window(&self) {
        let mut wind = window::Window::default()
            .with_size(500, 700)
            .with_label("Audio Settings");
        wind.make_resizable(false);
        wind.set_color(DarkTheme::BG_PRIMARY);

        // Main container with padding
        let mut main_col = group::Flex::default_fill().column();
        main_col.set_margins(15, 15, 15, 15);
        main_col.set_spacing(10);

        // Title
        let mut title = frame::Frame::default()
            .with_label("⚙  Audio & Settings")
            .with_align(enums::Align::Center);
        title.set_label_size(20);
        title.set_label_color(DarkTheme::TEXT_PRIMARY);
        main_col.fixed(&title, 40);

        // Tabs container
        let mut tabs = group::Tabs::default();
        tabs.set_color(DarkTheme::BG_PRIMARY);
        tabs.set_selection_color(DarkTheme::BG_SECONDARY);
        tabs.set_label_color(DarkTheme::TEXT_PRIMARY);

        // === Audio Tab ===
        let mut audio_grp = group::Flex::default().with_label("  Audio  \t").column();
        audio_grp.set_margins(10, 10, 10, 10);
        audio_grp.set_spacing(15);

        // Device Selection Section
        let mut device_title = frame::Frame::default()
            .with_label("🎧  Audio Devices")
            .with_align(enums::Align::Left | enums::Align::Inside);
        device_title.set_label_color(DarkTheme::TEXT_PRIMARY);
        device_title.set_label_size(14);
        audio_grp.fixed(&device_title, 25);

        // Input device
        let mut input_label = frame::Frame::default()
            .with_label("Microphone")
            .with_align(enums::Align::Left | enums::Align::Inside);
        input_label.set_label_color(DarkTheme::TEXT_SECONDARY);
        input_label.set_label_size(12);
        audio_grp.fixed(&input_label, 20);

        let mut input_choice = menu::Choice::default();
        input_choice.set_color(DarkTheme::BG_INPUT);
        input_choice.set_text_color(DarkTheme::TEXT_PRIMARY);
        input_choice.set_frame(enums::FrameType::BorderBox);
        audio_grp.fixed(&input_choice, 30);

        // Output device
        let mut output_label = frame::Frame::default()
            .with_label("Speakers/Headphones")
            .with_align(enums::Align::Left | enums::Align::Inside);
        output_label.set_label_color(DarkTheme::TEXT_SECONDARY);
        output_label.set_label_size(12);
        audio_grp.fixed(&output_label, 20);

        let mut output_choice = menu::Choice::default();
        output_choice.set_color(DarkTheme::BG_INPUT);
        output_choice.set_text_color(DarkTheme::TEXT_PRIMARY);
        output_choice.set_frame(enums::FrameType::BorderBox);
        audio_grp.fixed(&output_choice, 30);

        // Refresh button row
        let mut refresh_row = group::Flex::default().row();
        audio_grp.fixed(&refresh_row, 30);
        refresh_row.set_spacing(10);

        let mut refresh_btn = button::Button::default().with_label("🔄 Refresh Devices");
        refresh_btn.set_color(DarkTheme::BG_TERTIARY);
        refresh_btn.set_label_color(DarkTheme::TEXT_PRIMARY);
        refresh_btn.set_frame(enums::FrameType::FlatBox);
        refresh_btn.set_label_size(11);

        let mut device_status = frame::Frame::default().with_align(enums::Align::Center);
        device_status.set_label_color(DarkTheme::TEXT_MUTED);
        device_status.set_label_size(11);

        refresh_row.end();

        // Separator
        let mut sep1 = frame::Frame::default();
        sep1.set_frame(enums::FrameType::FlatBox);
        sep1.set_color(DarkTheme::BG_TERTIARY);
        audio_grp.fixed(&sep1, 1);

        // Voice Settings Section
        let mut voice_title = frame::Frame::default()
            .with_label("🎤  Voice Settings")
            .with_align(enums::Align::Left | enums::Align::Inside);
        voice_title.set_label_color(DarkTheme::TEXT_PRIMARY);
        voice_title.set_label_size(14);
        audio_grp.fixed(&voice_title, 25);

        // VAD threshold (user-friendly)
        let mut sensitivity_label = frame::Frame::default()
            .with_label("Microphone Activation Sensitivity")
            .with_align(enums::Align::Left | enums::Align::Inside);
        sensitivity_label.set_label_color(DarkTheme::TEXT_SECONDARY);
        sensitivity_label.set_label_size(12);
        audio_grp.fixed(&sensitivity_label, 20);

        let mut sensitivity_help = frame::Frame::default()
            .with_label("How sensitive the mic is to picking up your voice")
            .with_align(enums::Align::Left | enums::Align::Inside);
        sensitivity_help.set_label_color(DarkTheme::TEXT_MUTED);
        sensitivity_help.set_label_size(10);
        audio_grp.fixed(&sensitivity_help, 15);

        let mut vad_slider = valuator::HorSlider::default();
        vad_slider.set_minimum(0.1);
        vad_slider.set_maximum(0.9);
        vad_slider.set_value(self.settings.lock().unwrap().vad_threshold as f64);
        vad_slider.set_step(0.05, 1);
        vad_slider.set_color(DarkTheme::BG_TERTIARY);
        vad_slider.set_selection_color(DarkTheme::ACCENT);
        audio_grp.fixed(&vad_slider, 35);

        let mut vad_value = frame::Frame::default()
            .with_label(&format!(
                "Current: {} ({})",
                match vad_slider.value() {
                    v if v < 0.3 => "High",
                    v if v < 0.6 => "Medium",
                    _ => "Low",
                },
                if vad_slider.value() < 0.5 {
                    "picks up more sounds"
                } else {
                    "requires louder voice"
                }
            ))
            .with_align(enums::Align::Center);
        vad_value.set_label_color(DarkTheme::TEXT_MUTED);
        vad_value.set_label_size(11);
        audio_grp.fixed(&vad_value, 20);

        // Checkboxes
        let mut loopback_btn =
            button::CheckButton::default().with_label("Hear myself (audio loopback for testing)");
        loopback_btn.set_checked(self.settings.lock().unwrap().loopback_enabled);
        loopback_btn.set_color(DarkTheme::BG_PRIMARY);
        loopback_btn.set_label_color(DarkTheme::TEXT_PRIMARY);
        loopback_btn.set_selection_color(DarkTheme::ACCENT);
        loopback_btn.set_label_size(12);
        audio_grp.fixed(&loopback_btn, 25);

        let mut mute_on_start =
            button::CheckButton::default().with_label("Start with microphone muted");
        mute_on_start.set_checked(self.settings.lock().unwrap().mute_on_start);
        mute_on_start.set_color(DarkTheme::BG_PRIMARY);
        mute_on_start.set_label_color(DarkTheme::TEXT_PRIMARY);
        mute_on_start.set_selection_color(DarkTheme::ACCENT);
        mute_on_start.set_label_size(12);
        audio_grp.fixed(&mute_on_start, 25);

        audio_grp.end();

        // === Diagnostics Tab ===
        let mut diag_grp = group::Flex::default()
            .with_label("  Diagnostics  \t")
            .column();
        diag_grp.set_margins(10, 10, 10, 10);
        diag_grp.set_spacing(15);

        // Audio Interface Info Section
        let mut interface_title = frame::Frame::default()
            .with_label("🔌  Audio Interface")
            .with_align(enums::Align::Left | enums::Align::Inside);
        interface_title.set_label_color(DarkTheme::TEXT_PRIMARY);
        interface_title.set_label_size(14);
        diag_grp.fixed(&interface_title, 25);

        let mut interface_display = text::TextDisplay::default();
        let interface_buffer = text::TextBuffer::default();
        interface_display.set_buffer(interface_buffer.clone());
        interface_display.set_text_size(11);
        interface_display.set_color(DarkTheme::BG_INPUT);
        interface_display.set_text_color(DarkTheme::TEXT_PRIMARY);
        interface_display.set_frame(enums::FrameType::BorderBox);
        diag_grp.fixed(&interface_display, 80);

        let mut copy_interface_btn = button::Button::default().with_label("📋 Copy Interface Info");
        copy_interface_btn.set_color(DarkTheme::BG_TERTIARY);
        copy_interface_btn.set_label_color(DarkTheme::TEXT_PRIMARY);
        copy_interface_btn.set_frame(enums::FrameType::FlatBox);
        copy_interface_btn.set_label_size(11);
        diag_grp.fixed(&copy_interface_btn, 30);

        // Separator
        let mut sep2 = frame::Frame::default();
        sep2.set_frame(enums::FrameType::FlatBox);
        sep2.set_color(DarkTheme::BG_TERTIARY);
        diag_grp.fixed(&sep2, 1);

        // Live Statistics Section
        let mut stats_title = frame::Frame::default()
            .with_label("📊  Live Statistics")
            .with_align(enums::Align::Left | enums::Align::Inside);
        stats_title.set_label_color(DarkTheme::TEXT_PRIMARY);
        stats_title.set_label_size(14);
        diag_grp.fixed(&stats_title, 25);

        let mut stats_display = text::TextDisplay::default();
        let stats_buffer = text::TextBuffer::default();
        stats_display.set_buffer(stats_buffer.clone());
        stats_display.set_text_size(11);
        stats_display.set_color(DarkTheme::BG_INPUT);
        stats_display.set_text_color(DarkTheme::TEXT_PRIMARY);
        stats_display.set_frame(enums::FrameType::BorderBox);
        diag_grp.fixed(&stats_display, 120);

        let mut copy_stats_btn = button::Button::default().with_label("📋 Copy Statistics");
        copy_stats_btn.set_color(DarkTheme::BG_TERTIARY);
        copy_stats_btn.set_label_color(DarkTheme::TEXT_PRIMARY);
        copy_stats_btn.set_frame(enums::FrameType::FlatBox);
        copy_stats_btn.set_label_size(11);
        diag_grp.fixed(&copy_stats_btn, 30);

        // Separator
        let mut sep3 = frame::Frame::default();
        sep3.set_frame(enums::FrameType::FlatBox);
        sep3.set_color(DarkTheme::BG_TERTIARY);
        diag_grp.fixed(&sep3, 1);

        // Debug Options Section
        let mut debug_title = frame::Frame::default()
            .with_label("🐛  Debug Options")
            .with_align(enums::Align::Left | enums::Align::Inside);
        debug_title.set_label_color(DarkTheme::TEXT_PRIMARY);
        debug_title.set_label_size(14);
        diag_grp.fixed(&debug_title, 25);

        let mut debug_verbose = button::CheckButton::default()
            .with_label("Enable verbose logging (for troubleshooting)");
        debug_verbose.set_checked(self.settings.lock().unwrap().verbose_logging);
        debug_verbose.set_color(DarkTheme::BG_PRIMARY);
        debug_verbose.set_label_color(DarkTheme::TEXT_PRIMARY);
        debug_verbose.set_selection_color(DarkTheme::ACCENT);
        debug_verbose.set_label_size(12);
        diag_grp.fixed(&debug_verbose, 25);

        diag_grp.end();

        tabs.end();
        tabs.auto_layout();

        // Bottom button row
        let mut button_row = group::Flex::default().row();
        main_col.fixed(&button_row, 40);
        button_row.set_spacing(10);

        let mut reset_btn = button::Button::default().with_label("Reset to Defaults");
        reset_btn.set_color(DarkTheme::BG_TERTIARY);
        reset_btn.set_label_color(DarkTheme::TEXT_PRIMARY);
        reset_btn.set_frame(enums::FrameType::FlatBox);

        let mut save_btn = button::Button::default().with_label("Save Settings");
        save_btn.set_color(DarkTheme::SUCCESS);
        save_btn.set_label_color(enums::Color::White);
        save_btn.set_frame(enums::FrameType::FlatBox);

        let mut close_btn = button::Button::default().with_label("Close");
        close_btn.set_color(DarkTheme::BG_TERTIARY);
        close_btn.set_label_color(DarkTheme::TEXT_PRIMARY);
        close_btn.set_frame(enums::FrameType::FlatBox);

        button_row.end();
        main_col.end();

        wind.end();
        wind.show();

        // === Load and populate audio devices ===
        let mut populate_devices = {
            let voice_chat = self.voice_chat.clone();
            let settings = self.settings.clone();
            let mut input_choice = input_choice.clone();
            let mut output_choice = output_choice.clone();

            move || {
                // Clear existing choices
                input_choice.clear();
                output_choice.clear();

                // Add default option
                input_choice.add_choice("System Default");
                output_choice.add_choice("System Default");

                // Get devices from VoiceChat if available
                if let Ok(vc_guard) = voice_chat.lock() {
                    if let Some(vc) = vc_guard.as_ref() {
                        // Get input devices
                        if let Ok(input_devices) = vc.get_input_devices() {
                            for device in input_devices {
                                input_choice.add_choice(&device);
                            }
                        }

                        // Get output devices
                        if let Ok(output_devices) = vc.get_output_devices() {
                            for device in output_devices {
                                output_choice.add_choice(&device);
                            }
                        }
                    }
                }

                // Set current selection
                let settings = settings.lock().unwrap();
                if let Some(ref input_dev) = settings.input_device {
                    for i in 0..input_choice.size() {
                        if input_choice
                            .text(i)
                            .map(|t| t == *input_dev)
                            .unwrap_or(false)
                        {
                            input_choice.set_value(i);
                            break;
                        }
                    }
                } else {
                    input_choice.set_value(0);
                }

                if let Some(ref output_dev) = settings.output_device {
                    for i in 0..output_choice.size() {
                        if output_choice
                            .text(i)
                            .map(|t| t == *output_dev)
                            .unwrap_or(false)
                        {
                            output_choice.set_value(i);
                            break;
                        }
                    }
                } else {
                    output_choice.set_value(0);
                }
            }
        };

        // Initial device population
        populate_devices();

        // === Callbacks ===

        // Refresh devices button
        let mut populate_devices_clone = populate_devices.clone();
        let mut device_status_clone = device_status.clone();
        refresh_btn.set_callback(move |_| {
            populate_devices_clone();
            device_status_clone.set_label("✓ Refreshed");
            device_status_clone.set_label_color(DarkTheme::SUCCESS);
            info!("[Settings] Refreshed audio devices");
        });

        // Input device selection
        let settings_input = self.settings.clone();
        let voice_chat_input = self.voice_chat.clone();
        let mut device_status_input = device_status.clone();
        input_choice.set_callback(move |c| {
            let selected = c.choice().unwrap_or_else(|| "System Default".to_string());

            let mut settings = settings_input.lock().unwrap();
            if selected == "System Default" {
                settings.input_device = None;
            } else {
                settings.input_device = Some(selected.clone());
            }

            // Try to apply immediately if connected
            if let Ok(mut vc_guard) = voice_chat_input.lock() {
                if let Some(vc) = vc_guard.as_mut() {
                    if vc
                        .set_input_device(settings.input_device.as_deref())
                        .is_ok()
                    {
                        device_status_input.set_label("✓ Applied");
                        device_status_input.set_label_color(DarkTheme::SUCCESS);
                    } else {
                        device_status_input.set_label("⚠ Reconnect needed");
                        device_status_input.set_label_color(DarkTheme::WARNING);
                    }
                }
            }
        });

        // Output device selection
        let settings_output = self.settings.clone();
        let voice_chat_output = self.voice_chat.clone();
        let mut device_status_output = device_status.clone();
        output_choice.set_callback(move |c| {
            let selected = c.choice().unwrap_or_else(|| "System Default".to_string());

            let mut settings = settings_output.lock().unwrap();
            if selected == "System Default" {
                settings.output_device = None;
            } else {
                settings.output_device = Some(selected.clone());
            }

            // Try to apply immediately if connected
            if let Ok(mut vc_guard) = voice_chat_output.lock() {
                if let Some(vc) = vc_guard.as_mut() {
                    if vc
                        .set_output_device(settings.output_device.as_deref())
                        .is_ok()
                    {
                        device_status_output.set_label("✓ Applied");
                        device_status_output.set_label_color(DarkTheme::SUCCESS);
                    } else {
                        device_status_output.set_label("⚠ Reconnect needed");
                        device_status_output.set_label_color(DarkTheme::WARNING);
                    }
                }
            }
        });

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

        // VAD slider with user-friendly labels
        let voice_chat_vad = self.voice_chat.clone();
        let settings_vad = self.settings.clone();
        vad_slider.set_callback(move |s| {
            let value = s.value() as f32;
            let sensitivity = match value {
                v if v < 0.3 => "High",
                v if v < 0.6 => "Medium",
                _ => "Low",
            };
            let description = if value < 0.5 {
                "picks up more sounds"
            } else {
                "requires louder voice"
            };

            vad_value.set_label(&format!("Current: {} ({})", sensitivity, description));

            settings_vad.lock().unwrap().vad_threshold = value;

            if let Ok(mut vc_guard) = voice_chat_vad.lock() {
                if let Some(vc) = vc_guard.as_mut() {
                    // vc.set_vad_threshold(value);
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

        // Copy interface info button
        let interface_buffer_copy = interface_buffer.clone();
        copy_interface_btn.set_callback(move |_| {
            let text = interface_buffer_copy.text();
            app::copy(&text);
            info!("[Settings] Copied interface info to clipboard");
        });

        // Copy stats button
        let stats_buffer_copy = stats_buffer.clone();
        copy_stats_btn.set_callback(move |_| {
            let text = stats_buffer_copy.text();
            app::copy(&text);
            info!("[Settings] Copied statistics to clipboard");
        });

        // Reset button
        let settings_reset = self.settings.clone();
        let mut vad_slider_clone = vad_slider.clone();
        let mut loopback_btn_clone = loopback_btn.clone();
        let mut mute_on_start_clone = mute_on_start.clone();
        let mut debug_verbose_clone = debug_verbose.clone();
        let mut input_choice_clone = input_choice.clone();
        let mut output_choice_clone = output_choice.clone();
        reset_btn.set_callback(move |_| {
            let defaults = Settings::default();
            vad_slider_clone.set_value(defaults.vad_threshold as f64);
            loopback_btn_clone.set_checked(defaults.loopback_enabled);
            mute_on_start_clone.set_checked(defaults.mute_on_start);
            debug_verbose_clone.set_checked(defaults.verbose_logging);
            input_choice_clone.set_value(0);
            output_choice_clone.set_value(0);

            let mut settings = settings_reset.lock().unwrap();
            settings.vad_threshold = defaults.vad_threshold;
            settings.loopback_enabled = defaults.loopback_enabled;
            settings.mute_on_start = defaults.mute_on_start;
            settings.verbose_logging = defaults.verbose_logging;
            settings.input_device = None;
            settings.output_device = None;
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

        // Update interface info periodically (slower update)
        let voice_chat_interface = self.voice_chat.clone();
        let mut interface_buffer_clone = interface_buffer.clone();
        app::add_timeout3(2.0, move |handle| {
            let mut interface_text = String::new();

            if let Ok(vc_guard) = voice_chat_interface.lock() {
                if let Some(vc) = vc_guard.as_ref() {
                    // Get current devices and sample rate info
                    if let Some(input) = vc.get_current_input_device() {
                        interface_text.push_str(&format!("Input: {}\n", input));
                    }
                    if let Some(output) = vc.get_current_output_device() {
                        interface_text.push_str(&format!("Output: {}\n", output));
                    }
                    // Add sample rate if available
                    interface_text.push_str("Sample Rate: 48000 Hz\n");
                } else {
                    interface_text.push_str("Not connected\n");
                }
            }

            interface_buffer_clone.set_text(&interface_text);
            app::repeat_timeout3(2.0, handle);
        });

        // Update stats periodically (faster update)
        let voice_chat_stats = self.voice_chat.clone();
        let mut stats_buffer_clone = stats_buffer.clone();
        app::add_timeout3(0.5, move |handle| {
            let mut stats_text = String::new();

            if let Ok(vc_guard) = voice_chat_stats.lock() {
                if let Some(vc) = vc_guard.as_ref() {
                    stats_text.push_str(&format!("Status: Connected\n"));
                    stats_text.push_str(&format!(
                        "Speaking: {}\n",
                        if vc.is_speaking() { "Yes" } else { "No" }
                    ));
                    stats_text.push_str(&format!("{}", vc.get_stats()));
                } else {
                    stats_text.push_str("Status: Not initialized\n");
                    stats_text.push_str("\nConnect to a server to\nview live statistics");
                }
            }

            stats_buffer_clone.set_text(&stats_text);
            app::repeat_timeout3(0.5, handle);
        });
    }
}

