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
            .with_size(450, 750) // Increased height for device selection
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

        // === Audio Devices Section ===
        let mut devices_section = frame::Frame::default()
            .with_label("Audio Devices")
            .with_align(enums::Align::Left | enums::Align::Inside);
        devices_section.set_label_color(DarkTheme::TEXT_SECONDARY);
        devices_section.set_label_size(14);
        col.fixed(&devices_section, 25);

        // Input device selection
        let mut input_label = frame::Frame::default()
            .with_label("Input Device (Microphone)")
            .with_align(enums::Align::Left | enums::Align::Inside);
        input_label.set_label_color(DarkTheme::TEXT_SECONDARY);
        input_label.set_label_size(12);
        col.fixed(&input_label, 20);

        let mut input_choice = menu::Choice::default();
        input_choice.set_color(DarkTheme::BG_INPUT);
        input_choice.set_text_color(DarkTheme::TEXT_PRIMARY);
        input_choice.set_frame(enums::FrameType::BorderBox);
        col.fixed(&input_choice, 30);

        // Output device selection
        let mut output_label = frame::Frame::default()
            .with_label("Output Device (Speakers/Headphones)")
            .with_align(enums::Align::Left | enums::Align::Inside);
        output_label.set_label_color(DarkTheme::TEXT_SECONDARY);
        output_label.set_label_size(12);
        col.fixed(&output_label, 20);

        let mut output_choice = menu::Choice::default();
        output_choice.set_color(DarkTheme::BG_INPUT);
        output_choice.set_text_color(DarkTheme::TEXT_PRIMARY);
        output_choice.set_frame(enums::FrameType::BorderBox);
        col.fixed(&output_choice, 30);

        // Refresh devices button
        let mut refresh_btn = button::Button::default().with_label("🔄 Refresh Devices");
        refresh_btn.set_color(DarkTheme::BG_TERTIARY);
        refresh_btn.set_label_color(DarkTheme::TEXT_PRIMARY);
        refresh_btn.set_frame(enums::FrameType::FlatBox);
        col.fixed(&refresh_btn, 30);

        // Device status indicator
        let mut device_status = frame::Frame::default()
            .with_label("⚠ Devices will be applied on next connection")
            .with_align(enums::Align::Center);
        device_status.set_label_color(DarkTheme::WARNING);
        device_status.set_label_size(11);
        col.fixed(&device_status, 20);

        // === Audio Configuration Section ===
        let mut sep1_5 = frame::Frame::default();
        sep1_5.set_frame(enums::FrameType::EngravedBox);
        sep1_5.set_color(DarkTheme::BG_TERTIARY);
        col.fixed(&sep1_5, 2);

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
                    input_choice.set_value(0); // System Default
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
                    output_choice.set_value(0); // System Default
                }
            }
        };

        // Initial device population
        populate_devices();

        // === Callbacks ===

        // Refresh devices button
        let mut populate_devices_clone = populate_devices.clone();
        refresh_btn.set_callback(move |_| {
            populate_devices_clone();
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
                        device_status_input.set_label("✓ Input device changed successfully");
                        device_status_input.set_label_color(DarkTheme::SUCCESS);
                    } else {
                        device_status_input
                            .set_label("⚠ Input device will be applied on reconnect");
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
                        device_status_output.set_label("✓ Output device changed successfully");
                        device_status_output.set_label_color(DarkTheme::SUCCESS);
                    } else {
                        device_status_output
                            .set_label("⚠ Output device will be applied on reconnect");
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

        // VAD slider
        let voice_chat_vad = self.voice_chat.clone();
        let settings_vad = self.settings.clone();
        vad_slider.set_callback(move |s| {
            let value = s.value() as f32;
            vad_value.set_label(&format!("{:.2} (lower = more sensitive)", value));

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
            input_choice_clone.set_value(0); // System Default
            output_choice_clone.set_value(0); // System Default

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

                    // Show current devices
                    if let Some(input) = vc.get_current_input_device() {
                        stats_text.push_str(&format!("📥 Input: {}\n", input));
                    }
                    if let Some(output) = vc.get_current_output_device() {
                        stats_text.push_str(&format!("📤 Output: {}\n", output));
                    }

                    stats_text.push_str(&format!("\n{}", vc.get_stats()));
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
