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
            .with_label("Settings");
        wind.make_resizable(false);
        wind.set_color(DarkTheme::BG_PRIMARY);

        // Main container with padding
        let mut main_col = group::Flex::default_fill().column();
        main_col.set_margins(15, 15, 15, 15);
        main_col.set_spacing(10);

        // Title
        let mut title = frame::Frame::default()
            .with_label("Settings")
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
        let s_close = sender;
        wind.set_callback(move |w| {
            if app::event() == enums::Event::Close {
                s_close.send(UiMessage::SettingsWindowClosed);
                w.hide();
            }
        });
        // Close button
let s = sender;
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
        let s = sender;
        let mut status = device_status.clone();
        refresh_btn.set_callback(move |_| {
            s.send(UiMessage::RefreshDevices);
            status.set_label("✓ Refreshed");
            status.set_label_color(DarkTheme::SUCCESS);
        });

        // Input device selection
        let s = sender;
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
        let s = sender;
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
        let s = sender;
        loopback_btn.set_callback(move |b| {
            s.send(UiMessage::SetLoopback(b.is_checked()));
            settings_tracker.loopback_enabled = b.is_checked();
        });

        // VAD slider
        let s = sender;
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

        // Reset button
        let s = sender;
        let mut slider = vad_slider.clone();
        let loopback = loopback_btn.clone();
        let mute = mute_on_start.clone();
        let mut input = input_choice.clone();
        let mut output = output_choice.clone();
        reset_btn.set_callback(move |_| {
            let defaults = Settings::default();
            slider.set_value(defaults.vad_threshold as f64);
            slider.do_callback();
            loopback.set_checked(defaults.loopback_enabled);
            mute.set_checked(defaults.mute_on_start);
            input.set_value(0);
            output.set_value(0);
            s.send(UiMessage::ResetSettings);
        });

        // Save button
        let s = sender;
        save_btn.set_callback(move |_| {
            // Collect all current settings and send save message
            s.send(UiMessage::SaveSettings);
            info!("Settings saved");
        });

        

        // Update displays periodically
        let s = sender;
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