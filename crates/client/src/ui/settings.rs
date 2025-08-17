// ui/settings.rs - Refactored to use message passing
use crate::settings::Settings;
use crate::ui::messages::UiMessage;
use crate::ui::theme::DarkTheme;
use fltk::{prelude::*, *};
use log::info;

pub struct AudioSettingsWindow;

impl AudioSettingsWindow {
    pub fn show(sender: app::Sender<UiMessage>, current_settings: Settings) {
        let mut wind = window::Window::default()
            .with_size(500, 950)
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

        // VAD threshold
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
        vad_slider.set_value(current_settings.vad_threshold as f64);
        vad_slider.set_step(0.05, 1);
        vad_slider.set_color(DarkTheme::BG_TERTIARY);
        vad_slider.set_selection_color(DarkTheme::ACCENT);
        audio_grp.fixed(&vad_slider, 35);

        let mut vad_value = frame::Frame::default()
            .with_label(&Self::format_vad_label(vad_slider.value()))
            .with_align(enums::Align::Center);
        vad_value.set_label_color(DarkTheme::TEXT_MUTED);
        vad_value.set_label_size(11);
        audio_grp.fixed(&vad_value, 20);

        // Checkboxes
        let mut loopback_btn =
            button::CheckButton::default().with_label("Hear myself (audio loopback for testing)");
        loopback_btn.set_checked(current_settings.loopback_enabled);
        loopback_btn.set_color(DarkTheme::BG_PRIMARY);
        loopback_btn.set_label_color(DarkTheme::TEXT_PRIMARY);
        loopback_btn.set_selection_color(DarkTheme::ACCENT);
        loopback_btn.set_label_size(12);
        audio_grp.fixed(&loopback_btn, 25);

        let mut mute_on_start =
            button::CheckButton::default().with_label("Start with microphone muted");
        mute_on_start.set_checked(current_settings.mute_on_start);
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

        // Device Capabilities Section
        let mut capabilities_title = frame::Frame::default()
            .with_label("🎛️  Device Capabilities & Configuration")
            .with_align(enums::Align::Left | enums::Align::Inside);
        capabilities_title.set_label_color(DarkTheme::TEXT_PRIMARY);
        capabilities_title.set_label_size(14);
        diag_grp.fixed(&capabilities_title, 25);

        let mut capabilities_display = text::TextDisplay::default();
        let capabilities_buffer = text::TextBuffer::default();
        capabilities_display.set_buffer(capabilities_buffer.clone());
        capabilities_display.set_text_size(10);
        capabilities_display.set_color(DarkTheme::BG_INPUT);
        capabilities_display.set_text_color(DarkTheme::TEXT_PRIMARY);
        capabilities_display.set_frame(enums::FrameType::BorderBox);
        diag_grp.fixed(&capabilities_display, 150);

        let mut copy_capabilities_btn = button::Button::default().with_label("📋 Copy Device Info");
        copy_capabilities_btn.set_color(DarkTheme::BG_TERTIARY);
        copy_capabilities_btn.set_label_color(DarkTheme::TEXT_PRIMARY);
        copy_capabilities_btn.set_frame(enums::FrameType::FlatBox);
        copy_capabilities_btn.set_label_size(11);
        diag_grp.fixed(&copy_capabilities_btn, 30);

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
        diag_grp.fixed(&stats_display, 100);

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
        debug_verbose.set_checked(current_settings.verbose_logging);
        debug_verbose.set_color(DarkTheme::BG_PRIMARY);
        debug_verbose.set_label_color(DarkTheme::TEXT_PRIMARY);
        debug_verbose.set_selection_color(DarkTheme::ACCENT);
        debug_verbose.set_label_size(12);
        diag_grp.fixed(&debug_verbose, 25);

        // Advanced Diagnostics Section
        let mut sep4 = frame::Frame::default();
        sep4.set_frame(enums::FrameType::FlatBox);
        sep4.set_color(DarkTheme::BG_TERTIARY);
        diag_grp.fixed(&sep4, 1);

        let mut advanced_title = frame::Frame::default()
            .with_label("🖥️  Advanced Diagnostics")
            .with_align(enums::Align::Left | enums::Align::Inside);
        advanced_title.set_label_color(DarkTheme::TEXT_PRIMARY);
        advanced_title.set_label_size(14);
        diag_grp.fixed(&advanced_title, 25);

        let mut terminal_desc = frame::Frame::default()
            .with_label(
                "Open the diagnostics terminal for advanced testing and real-time monitoring",
            )
            .with_align(enums::Align::Left | enums::Align::Inside);
        terminal_desc.set_label_color(DarkTheme::TEXT_MUTED);
        terminal_desc.set_label_size(11);
        diag_grp.fixed(&terminal_desc, 20);

        let mut open_terminal_btn =
            button::Button::default().with_label("🖥️  Open Diagnostics Terminal");
        open_terminal_btn.set_color(DarkTheme::ACCENT);
        open_terminal_btn.set_label_color(enums::Color::White);
        open_terminal_btn.set_frame(enums::FrameType::FlatBox);
        open_terminal_btn.set_label_size(13);
        diag_grp.fixed(&open_terminal_btn, 40);

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
        let s_close = sender.clone();
        wind.set_callback(move |w| {
            if app::event() == enums::Event::Close {
                s_close.send(UiMessage::SettingsWindowClosed);
                w.hide();
            }
        });
        // Close button
let s = sender.clone();
close_btn.set_callback(move |_| {
    s.send(UiMessage::SettingsWindowClosed);
    wind.hide();
});

        // Populate initial device choices
        input_choice.add_choice("System Default");
        output_choice.add_choice("System Default");

        // Set current selections
        if let Some(ref input_dev) = current_settings.input_device {
            input_choice.add_choice(input_dev);
            input_choice.set_value(1);
        } else {
            input_choice.set_value(0);
        }

        if let Some(ref output_dev) = current_settings.output_device {
            output_choice.add_choice(output_dev);
            output_choice.set_value(1);
        } else {
            output_choice.set_value(0);
        }

        // Store settings values for tracking changes
        let mut settings_tracker = SettingsTracker {
            vad_threshold: current_settings.vad_threshold,
            loopback_enabled: current_settings.loopback_enabled,
            mute_on_start: current_settings.mute_on_start,
            verbose_logging: current_settings.verbose_logging,
            input_device: current_settings.input_device.clone(),
            output_device: current_settings.output_device.clone(),
        };

        // === Callbacks using message passing ===

        // Refresh devices
        let s = sender.clone();
        let mut status = device_status.clone();
        refresh_btn.set_callback(move |_| {
            s.send(UiMessage::RefreshDevices);
            status.set_label("✓ Refreshed");
            status.set_label_color(DarkTheme::SUCCESS);
        });

        // Input device selection
        let s = sender.clone();
        input_choice.set_callback(move |c| {
            let selected = c.choice().unwrap_or_else(|| "System Default".to_string());
            let clean_name = selected.replace(" ⚠️", "");

            let device = if clean_name == "System Default" {
                None
            } else {
                Some(clean_name)
            };

            s.send(UiMessage::SetInputDevice(device));
        });

        // Output device selection
        let s = sender.clone();
        output_choice.set_callback(move |c| {
            let selected = c.choice().unwrap_or_else(|| "System Default".to_string());
            let clean_name = selected.replace(" ⚠️", "");

            let device = if clean_name == "System Default" {
                None
            } else {
                Some(clean_name)
            };

            s.send(UiMessage::SetOutputDevice(device));
        });

        // Loopback toggle
        let s = sender.clone();
        loopback_btn.set_callback(move |b| {
            s.send(UiMessage::SetLoopback(b.is_checked()));
            settings_tracker.loopback_enabled = b.is_checked();
        });

        // VAD slider
        let s = sender.clone();
        vad_slider.set_callback(move |slider| {
            let value = slider.value() as f32;
            vad_value.set_label(&Self::format_vad_label(slider.value()));
            s.send(UiMessage::SetVadThreshold(value));
            settings_tracker.vad_threshold = value;
        });

        // Mute on start
        mute_on_start.set_callback(move |b| {
            settings_tracker.mute_on_start = b.is_checked();
        });

        // Verbose logging
        debug_verbose.set_callback(move |b| {
            settings_tracker.verbose_logging = b.is_checked();
        });

        // Copy capabilities
        let buffer = capabilities_buffer.clone();
        copy_capabilities_btn.set_callback(move |_| {
            app::copy(&buffer.text());
            info!("Copied device capabilities to clipboard");
        });

        // Copy stats
        let buffer = stats_buffer.clone();
        copy_stats_btn.set_callback(move |_| {
            app::copy(&buffer.text());
            info!("Copied statistics to clipboard");
        });

        // Open diagnostics terminal
        let s = sender.clone();
        open_terminal_btn.set_callback(move |_| {
            s.send(UiMessage::OpenDiagnostics);
        });

        // Reset button
        let s = sender.clone();
        let mut slider = vad_slider.clone();
        let mut loopback = loopback_btn.clone();
        let mut mute = mute_on_start.clone();
        let mut verbose = debug_verbose.clone();
        let mut input = input_choice.clone();
        let mut output = output_choice.clone();
        reset_btn.set_callback(move |_| {
            let defaults = Settings::default();
            slider.set_value(defaults.vad_threshold as f64);
            slider.do_callback();
            loopback.set_checked(defaults.loopback_enabled);
            mute.set_checked(defaults.mute_on_start);
            verbose.set_checked(defaults.verbose_logging);
            input.set_value(0);
            output.set_value(0);
            s.send(UiMessage::ResetSettings);
        });

        // Save button
        let s = sender.clone();
        save_btn.set_callback(move |_| {
            // Collect all current settings and send save message
            s.send(UiMessage::SaveSettings);
            info!("Settings saved");
        });

        

        // Update displays periodically
        let s = sender.clone();
        app::add_timeout3(0.5, move |handle| {
            s.send(UiMessage::UpdateStats);
            app::repeat_timeout3(0.5, handle);
        });
    }

    fn format_vad_label(value: f64) -> String {
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
        format!("Current: {} ({})", sensitivity, description)
    }
}

// Helper struct to track settings changes
#[derive(Clone)]
struct SettingsTracker {
    vad_threshold: f32,
    loopback_enabled: bool,
    mute_on_start: bool,
    verbose_logging: bool,
    input_device: Option<String>,
    output_device: Option<String>,
}
