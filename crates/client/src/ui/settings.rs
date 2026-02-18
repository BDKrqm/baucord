// ui/settings.rs — Audio settings + live diagnostics panel.
//
// Controls send UiMessages; levels come from shared atomics written by
// the voice thread.  Meters update at 8 Hz only while the window is visible.

use crate::config::Settings;
use crate::ui::messages::UiMessage;
use fltk::{prelude::*, *};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use ui::{gate_bar, helpers, theme, vu_meter};

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
            .with_size(560, 710)
            .with_label("Settings");
        wind.make_resizable(false);

        let mut scroll = group::Scroll::default_fill();
        scroll.set_scrollbar_size(8);

        let mut col = group::Flex::default()
            .with_size(530, 690)
            .with_pos(10, 10)
            .column();
        col.set_spacing(4);

        // ════════════════════════════════════════════════════════════════
        //  DEVICES
        // ════════════════════════════════════════════════════════════════

        helpers::section_header(&mut col, "\u{1f3a7}", "DEVICES");

        helpers::field_label(&mut col, "Input Device (Microphone)");
        let mut input_choice = helpers::choice(&mut col);
        helpers::populate_device_choice_with_current(
            &mut input_choice,
            &input_devices,
            &settings.input_device,
        );

        helpers::field_label(&mut col, "Output Device (Speakers/Headphones)");
        let mut output_choice = helpers::choice(&mut col);
        helpers::populate_device_choice_with_current(
            &mut output_choice,
            &output_devices,
            &settings.output_device,
        );

        helpers::hint_label(
            &mut col,
            &format!(
                "{} input, {} output devices",
                input_devices.len(),
                output_devices.len()
            ),
        );

        helpers::separator(&mut col);

        // ════════════════════════════════════════════════════════════════
        //  INPUT
        // ════════════════════════════════════════════════════════════════

        helpers::section_header(&mut col, "\u{1f3a4}", "INPUT");

        helpers::field_label(&mut col, "Capture Level");
        let vu_input = vu_meter::make(&mut col, input_level.clone(), 22, theme::SUCCESS);

        helpers::field_label(&mut col, "Input Gain");
        let mut input_gain_slider = helpers::slider(
            &mut col,
            0.0,
            2.0,
            settings.input_gain as f64,
            0.05,
            theme::ACCENT,
        );
        let input_gain_lbl = helpers::hint_label(
            &mut col,
            &format!("Gain: {}%", (settings.input_gain * 100.0) as i32),
        );

        // Test Microphone button
        let mut test_mic_btn = helpers::button("\u{1f3a4}  Test Microphone");
        col.fixed(&test_mic_btn, 30);

        let test_mic_info = helpers::hint_label(
            &mut col,
            "Listen to your own mic (others won't hear you)",
        );

        helpers::separator(&mut col);

        // ════════════════════════════════════════════════════════════════
        //  OUTPUT
        // ════════════════════════════════════════════════════════════════

        helpers::section_header(&mut col, "\u{1f50a}", "OUTPUT");

        let vu_output = vu_meter::make(&mut col, output_level.clone(), 22, theme::CYAN);

        helpers::field_label(&mut col, "Master Volume");
        let mut output_gain_slider = helpers::slider(
            &mut col,
            0.0,
            2.0,
            settings.output_gain as f64,
            0.05,
            theme::CYAN,
        );
        let output_gain_lbl = helpers::hint_label(
            &mut col,
            &format!("Volume: {}%", (settings.output_gain * 100.0) as i32),
        );

        helpers::separator(&mut col);

        // ════════════════════════════════════════════════════════════════
        //  VOICE GATE
        // ════════════════════════════════════════════════════════════════

        helpers::section_header(&mut col, "\u{26a1}", "VOICE GATE");

        helpers::field_label(
            &mut col,
            "Audio below the threshold is silenced before transmission",
        );

        // VAD meter with threshold line
        let threshold_rc = std::rc::Rc::new(std::cell::RefCell::new(settings.gate_threshold));
        let vu_vad = gate_bar::make(
            &mut col,
            denoised_level.clone(),
            threshold_rc.clone(),
            28,
        );

        helpers::field_label(&mut col, "Gate Threshold");
        let mut vad_slider = helpers::slider(
            &mut col,
            0.005,
            0.10,
            settings.gate_threshold as f64,
            0.001,
            theme::ACCENT,
        );
        let vad_sens_lbl = helpers::hint_label(
            &mut col,
            &helpers::format_sensitivity(settings.gate_threshold as f64),
        );

        let mute_on_start_toggle = helpers::check(&mut col, "Start muted", settings.mute_on_start);

        helpers::separator(&mut col);

        // ════════════════════════════════════════════════════════════════
        //  BUTTONS
        // ════════════════════════════════════════════════════════════════

        let mut button_row = group::Flex::default().row();
        col.fixed(&button_row, 35);
        button_row.set_spacing(10);

        let mut reset_btn = helpers::button("Reset Defaults");
        let mut save_btn = helpers::accent_button("Save and Close", theme::SUCCESS);

        button_row.end();
        col.end();
        scroll.end();
        wind.end();
        wind.show();

        // ════════════════════════════════════════════════════════════════
        //  CALLBACKS
        // ════════════════════════════════════════════════════════════════

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
                info_lbl.set_label("Listening to your mic... others can't hear you");
                info_lbl.set_label_color(theme::WARNING);
            } else {
                btn.set_label("\u{1f3a4}  Test Microphone");
                btn.set_color(theme::BG_TERTIARY);
                info_lbl.set_label("Listen to your own mic (others won't hear you)");
                info_lbl.set_label_color(theme::TEXT_MUTED);
            }

            s.send(UiMessage::TestMicrophone(is_testing));
        });

        // VAD slider — clone vu_vad before the closure captures it
        let s = sender;
        let threshold_update = threshold_rc.clone();
        let mut vu_vad_reset = vu_vad.clone();
        let mut sens_lbl = vad_sens_lbl.clone();
        let mut vu_vad_cb = vu_vad.clone();
        vad_slider.set_callback(move |sl| {
            let val = sl.value() as f32;
            *threshold_update.borrow_mut() = val;
            sens_lbl.set_label(&helpers::format_sensitivity(sl.value()));
            vu_vad_cb.redraw();
            s.send(UiMessage::SetGateThreshold(val));
        });

        // Window close — stop test mic if active
        let s = sender;
        let testing_close = testing.clone();
        wind.set_callback(move |w| {
            if app::event() == enums::Event::Close {
                // Stop test mic if it was active
                if *testing_close.borrow() {
                    s.send(UiMessage::TestMicrophone(false));
                }
                s.send(UiMessage::SettingsWindowClosed);
                w.hide();
            }
        });

        // Save & Close — stop test mic if active
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
        let ms = mute_on_start_toggle.clone();
        let threshold_reset = threshold_rc.clone();
        reset_btn.set_callback(move |_| {
            let d = Settings::default();
            ig.set_value(d.input_gain as f64);
            ig.do_callback();
            og.set_value(d.output_gain as f64);
            og.do_callback();
            vs.set_value(d.gate_threshold as f64);
            vs.do_callback();
            ms.set_checked(d.mute_on_start);
            *threshold_reset.borrow_mut() = d.gate_threshold;
            vu_vad_reset.redraw();
            s.send(UiMessage::ResetSettings);
        });

        // ════════════════════════════════════════════════════════════════
        //  TIMER: 8 Hz — update VU meters + speaking indicator
        // ════════════════════════════════════════════════════════════════

        let mut vu_in_t = vu_input.clone();
        let mut vu_out_t = vu_output.clone();
        let mut vu_vad_t = vu_vad.clone();
        let speaking_timer = speaking.clone();
        let prev_speak = std::rc::Rc::new(std::cell::RefCell::new(false));

        app::add_timeout3(0.125, move |handle| {
            vu_in_t.redraw();
            vu_out_t.redraw();
            vu_vad_t.redraw();

            // Speaking indicator (edge-triggered)
            let is_speaking = speaking_timer.load(Ordering::Relaxed);
            if is_speaking != *prev_speak.borrow() {
                *prev_speak.borrow_mut() = is_speaking;
            }

            app::repeat_timeout3(0.125, handle);
        });
    }
}