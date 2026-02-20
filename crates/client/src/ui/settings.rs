// ui/settings.rs — Audio settings + live diagnostics panel.
//
// Controls send UiMessages; levels come from shared atomics written by
// the voice thread.  Meters update at 8 Hz only while the window is visible.

use crate::config::Settings;
use crate::ui::messages::UiMessage;
use fltk::{prelude::*, *};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use ui::theme;
use ui::widgets::{controls, labels, layout, meter};

/// Human-readable sensitivity string from a threshold value.
fn format_sensitivity(value: f64) -> String {
    let name = match value {
        v if v < 0.010 => "Very High",
        v if v < 0.020 => "High",
        v if v < 0.040 => "Medium",
        v if v < 0.065 => "Low",
        _ => "Very Low",
    };
    format!("Sensitivity: {name}  (threshold: {value:.3} RMS)")
}

// ── Settings Window ─────────────────────────────────────────────────────

pub struct SettingsWindow;

impl SettingsWindow {
    pub fn show(
        sender: app::Sender<UiMessage>,
        settings: Settings,
        input_devices: Vec<(String, bool)>,
        output_devices: Vec<(String, bool)>,
        input_level: Arc<AtomicU32>,
        output_level: Arc<AtomicU32>,
        denoised_level: Arc<AtomicU32>,
        speaking: Arc<AtomicBool>,
    ) {
        let mut wind = window::Window::default()
            .with_size(560, 630)
            .with_label("Settings");
        wind.make_resizable(false);

        let mut scroll = group::Scroll::default_fill();
        scroll.set_scrollbar_size(8);

        let mut col = group::Flex::default()
            .with_size(530, 600)
            .with_pos(10, 10)
            .column();
        col.set_spacing(4);

        // ── Devices ─────────────────────────────────────────────────

        labels::section_header(&mut col, "\u{1f3a7}", "DEVICES");

        labels::field_label(&mut col, "Input Device (Microphone)");
        let mut input_choice = controls::dropdown(&mut col);
        let input_names: Vec<&str> = input_devices.iter().map(|(n, _)| n.as_str()).collect();
        controls::populate_dropdown(
            &mut input_choice,
            "System Default",
            &input_names,
            settings.input_device.as_deref(),
        );

        labels::field_label(&mut col, "Output Device (Speakers/Headphones)");
        let mut output_choice = controls::dropdown(&mut col);
        let output_names: Vec<&str> = output_devices.iter().map(|(n, _)| n.as_str()).collect();
        controls::populate_dropdown(
            &mut output_choice,
            "System Default",
            &output_names,
            settings.output_device.as_deref(),
        );

        labels::hint_label(
            &mut col,
            &format!(
                "{} input, {} output devices",
                input_devices.len(),
                output_devices.len()
            ),
        );

        layout::separator(&mut col);

        // ── Input ───────────────────────────────────────────────────

        labels::section_header(&mut col, "\u{1f3a4}", "INPUT");

        // Capture level meter — gate bar shows the threshold line overlaid
        // on the denoised audio level.  Audio below the red line is silenced.
        labels::field_label(&mut col, "Capture Level");
        let threshold_rc = std::rc::Rc::new(std::cell::RefCell::new(settings.gate_threshold));
        let vu_input = meter::gate(
            &mut col,
            denoised_level.clone(),
            threshold_rc.clone(),
            22,
        );

        labels::field_label(&mut col, "Input Gain");
        let mut input_gain_slider = controls::slider(
            &mut col,
            0.0,
            2.0,
            settings.input_gain as f64,
            0.05,
            theme::ACCENT,
        );
        let input_gain_lbl = labels::hint_label(
            &mut col,
            &format!("Gain: {}%", (settings.input_gain * 100.0) as i32),
        );

        labels::field_label(&mut col, "Input Sensitivity");
        let mut vad_slider = controls::slider(
            &mut col,
            0.005,
            0.10,
            settings.gate_threshold as f64,
            0.001,
            theme::ACCENT,
        );
        let vad_sens_lbl = labels::hint_label(
            &mut col,
            &format_sensitivity(settings.gate_threshold as f64),
        );
        labels::hint_label(
            &mut col,
            "Audio below the red line won't be transmitted",
        );

        // Test Microphone button
        let mut test_mic_btn = controls::button("\u{1f3a4}  Test Microphone");
        col.fixed(&test_mic_btn, 30);

        let test_mic_info = labels::hint_label(
            &mut col,
            "Listen to your own mic (others won't hear you)",
        );

        layout::separator(&mut col);

        // ── Output ──────────────────────────────────────────────────

        labels::section_header(&mut col, "\u{1f50a}", "OUTPUT");

        let vu_output = meter::vu(&mut col, output_level.clone(), 22, theme::CYAN);

        labels::field_label(&mut col, "Master Volume");
        let mut output_gain_slider = controls::slider(
            &mut col,
            0.0,
            2.0,
            settings.output_gain as f64,
            0.05,
            theme::CYAN,
        );
        let output_gain_lbl = labels::hint_label(
            &mut col,
            &format!("Volume: {}%", (settings.output_gain * 100.0) as i32),
        );

        layout::separator(&mut col);

        // ── Buttons ─────────────────────────────────────────────────

        let mut button_row = group::Flex::default().row();
        col.fixed(&button_row, 35);
        button_row.set_spacing(10);

        let mut reset_btn = controls::button("Reset Defaults");
        let mut save_btn = controls::accent_button("Save and Close", theme::SUCCESS);

        button_row.end();
        col.end();
        scroll.end();
        wind.end();
        wind.show();

        // ── Callbacks ───────────────────────────────────────────────

        // Device selection
        let s = sender;
        let devs = input_devices.clone();
        input_choice.set_callback(move |c| {
            let idx = c.value() as usize;
            let device = if idx == 0 {
                None
            } else {
                devs.get(idx - 1).map(|(n, _)| n.clone())
            };
            s.send(UiMessage::SetInputDevice(device));
        });

        let s = sender;
        let devs = output_devices.clone();
        output_choice.set_callback(move |c| {
            let idx = c.value() as usize;
            let device = if idx == 0 {
                None
            } else {
                devs.get(idx - 1).map(|(n, _)| n.clone())
            };
            s.send(UiMessage::SetOutputDevice(device));
        });

        // Gain sliders
        let s = sender;
        let mut lbl = input_gain_lbl.clone();
        input_gain_slider.set_callback(move |sl| {
            let val = sl.value() as f32;
            lbl.set_label(&format!("Gain: {}%", (val * 100.0) as i32));
            s.send(UiMessage::SetInputGain(val));
        });

        let s = sender;
        let mut lbl = output_gain_lbl.clone();
        output_gain_slider.set_callback(move |sl| {
            let val = sl.value() as f32;
            lbl.set_label(&format!("Volume: {}%", (val * 100.0) as i32));
            s.send(UiMessage::SetOutputGain(val));
        });

        // Test Microphone toggle
        let s = sender;
        let testing = std::rc::Rc::new(std::cell::RefCell::new(false));
        let testing_cb = testing.clone();
        let mut info_lbl = test_mic_info.clone();
        test_mic_btn.set_callback(move |btn| {
            let is_testing = !*testing_cb.borrow();
            *testing_cb.borrow_mut() = is_testing;

            if is_testing {
                btn.set_label("\u{1f534}  Stop Testing");
                btn.set_color(theme::DANGER);
                btn.set_label_color(enums::Color::White);
                info_lbl.set_label("Listening to your mic... others can't hear you");
                info_lbl.set_label_color(theme::WARNING);
            } else {
                btn.set_label("\u{1f3a4}  Test Microphone");
                btn.set_color(theme::BG_TERTIARY);
                btn.set_label_color(theme::TEXT_PRIMARY);
                info_lbl.set_label("Listen to your own mic (others won't hear you)");
                info_lbl.set_label_color(theme::TEXT_MUTED);
            }

            s.send(UiMessage::TestMicrophone(is_testing));
        });

        // Gate threshold slider
        let s = sender;
        let threshold_update = threshold_rc.clone();
        let mut sens_lbl = vad_sens_lbl.clone();
        let mut vu_input_cb = vu_input.clone();
        vad_slider.set_callback(move |sl| {
            let val = sl.value() as f32;
            *threshold_update.borrow_mut() = val;
            sens_lbl.set_label(&format_sensitivity(sl.value()));
            vu_input_cb.redraw();
            s.send(UiMessage::SetGateThreshold(val));
        });

        // Window close — stop test mic if active
        let s = sender;
        let testing_close = testing.clone();
        wind.set_callback(move |w| {
            if app::event() == enums::Event::Close {
                if *testing_close.borrow() {
                    s.send(UiMessage::TestMicrophone(false));
                }
                s.send(UiMessage::SettingsWindowClosed);
                w.hide();
            }
        });

        // Save & Close
        let s = sender;
        let mut wind_clone = wind.clone();
        let testing_save = testing.clone();
        save_btn.set_callback(move |_| {
            if *testing_save.borrow() {
                s.send(UiMessage::TestMicrophone(false));
            }
            s.send(UiMessage::SaveSettings);
            s.send(UiMessage::SettingsWindowClosed);
            wind_clone.hide();
        });

        // Reset
        let s = sender;
        let mut ig = input_gain_slider.clone();
        let mut og = output_gain_slider.clone();
        let mut vs = vad_slider.clone();
        let threshold_reset = threshold_rc.clone();
        let mut vu_input_reset = vu_input.clone();
        reset_btn.set_callback(move |_| {
            let d = Settings::default();
            ig.set_value(d.input_gain as f64);
            ig.do_callback();
            og.set_value(d.output_gain as f64);
            og.do_callback();
            vs.set_value(d.gate_threshold as f64);
            vs.do_callback();
            *threshold_reset.borrow_mut() = d.gate_threshold;
            vu_input_reset.redraw();
            s.send(UiMessage::ResetSettings);
        });

        // ── Timer: 8 Hz — update VU meters ──────────────────────────

        let mut vu_in_t = vu_input.clone();
        let mut vu_out_t = vu_output.clone();

        app::add_timeout3(0.125, move |handle| {
            vu_in_t.redraw();
            vu_out_t.redraw();
            app::repeat_timeout3(0.125, handle);
        });
    }
}