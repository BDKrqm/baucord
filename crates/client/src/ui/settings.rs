// ui/settings.rs — Audio settings + live diagnostics panel.
//
// Controls send UiMessages; levels come from shared atomics written by
// the voice thread. Meters update at 8 Hz only while the window is visible.

use crate::settings::Settings;
use crate::ui::messages::UiMessage;
use crate::ui::theme::DarkTheme;
use fltk::{prelude::*, *};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// ── Helpers ─────────────────────────────────────────────────────────────

fn section_header(col: &mut group::Flex, label: &str) {
    let mut lbl = frame::Frame::default()
        .with_label(label)
        .with_align(enums::Align::Left | enums::Align::Inside);
    lbl.set_label_color(DarkTheme::TEXT_PRIMARY);
    lbl.set_label_size(13);
    col.fixed(&lbl, 24);
}

fn field_label(col: &mut group::Flex, text: &str) {
    let mut lbl = frame::Frame::default()
        .with_label(text)
        .with_align(enums::Align::Left | enums::Align::Inside);
    lbl.set_label_color(DarkTheme::TEXT_SECONDARY);
    lbl.set_label_size(11);
    col.fixed(&lbl, 18);
}

fn separator(col: &mut group::Flex) {
    let mut sep = frame::Frame::default();
    sep.set_frame(enums::FrameType::FlatBox);
    sep.set_color(DarkTheme::BG_TERTIARY);
    col.fixed(&sep, 1);
}

fn styled_check(col: &mut group::Flex, label: &str, checked: bool) -> button::CheckButton {
    let mut cb = button::CheckButton::default().with_label(label);
    cb.set_checked(checked);
    cb.set_color(DarkTheme::BG_PRIMARY);
    cb.set_label_color(DarkTheme::TEXT_PRIMARY);
    cb.set_selection_color(DarkTheme::ACCENT);
    cb.set_label_size(11);
    col.fixed(&cb, 22);
    cb
}

fn styled_choice(col: &mut group::Flex) -> menu::Choice {
    let mut c = menu::Choice::default();
    c.set_color(DarkTheme::BG_INPUT);
    c.set_text_color(DarkTheme::TEXT_PRIMARY);
    c.set_frame(enums::FrameType::BorderBox);
    col.fixed(&c, 26);
    c
}

fn populate_device_choice(
    choice: &mut menu::Choice,
    devices: &[(String, bool)],
    current: &Option<String>,
) {
    choice.clear();
    choice.add_choice("System Default");
    let mut selected = 0;
    for (i, (name, _)) in devices.iter().enumerate() {
        choice.add_choice(name);
        if let Some(cur) = current.as_ref() {
            if cur == name {
                selected = i + 1;
            }
        }
    }
    choice.set_value(selected as i32);
}

fn make_slider(
    col: &mut group::Flex,
    min: f64,
    max: f64,
    value: f64,
    step: f64,
    color: enums::Color,
) -> valuator::HorSlider {
    let mut s = valuator::HorSlider::default();
    s.set_minimum(min);
    s.set_maximum(max);
    s.set_value(value);
    s.set_step(step, 1);
    s.set_color(DarkTheme::BG_TERTIARY);
    s.set_selection_color(color);
    col.fixed(&s, 22);
    s
}

fn value_label(col: &mut group::Flex, text: &str) -> frame::Frame {
    let mut lbl = frame::Frame::default()
        .with_label(text)
        .with_align(enums::Align::Left | enums::Align::Inside);
    lbl.set_label_color(DarkTheme::TEXT_MUTED);
    lbl.set_label_size(10);
    col.fixed(&lbl, 14);
    lbl
}

fn make_vu_meter(
    col: &mut group::Flex,
    level_arc: Arc<AtomicU32>,
    height: i32,
    color: enums::Color,
) -> frame::Frame {
    let mut f = frame::Frame::default();
    f.set_frame(enums::FrameType::BorderBox);
    f.set_color(DarkTheme::BG_INPUT);
    col.fixed(&f, height);

    f.draw(move |frm| {
        draw::set_draw_color(DarkTheme::BG_INPUT);
        draw::draw_rectf(frm.x(), frm.y(), frm.w(), frm.h());

        let level = f32::from_bits(level_arc.load(Ordering::Relaxed)).clamp(0.0, 1.0);
        let bar_w = ((frm.w() - 4) as f32 * level) as i32;

        let c = if level > 0.9 {
            DarkTheme::DANGER
        } else if level > 0.7 {
            DarkTheme::WARNING
        } else {
            color
        };

        if bar_w > 0 {
            draw::set_draw_color(c);
            draw::draw_rectf(frm.x() + 2, frm.y() + 3, bar_w, frm.h() - 6);
        }

        draw::set_draw_color(DarkTheme::BORDER);
        draw::draw_rect(frm.x(), frm.y(), frm.w(), frm.h());
    });
    f
}

fn format_sensitivity(value: f64) -> String {
    let name = match value {
        v if v < 0.08 => "Very High",
        v if v < 0.18 => "High",
        v if v < 0.30 => "Medium",
        v if v < 0.42 => "Low",
        _ => "Very Low",
    };
    format!("Sensitivity: {}", name)
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
            .with_size(560, 710)
            .with_label("Settings");
        wind.set_color(DarkTheme::BG_PRIMARY);
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

        section_header(&mut col, "\u{1f3a7}  DEVICES");

        field_label(&mut col, "Input Device (Microphone)");
        let mut input_choice = styled_choice(&mut col);
        populate_device_choice(&mut input_choice, &input_devices, &settings.input_device);

        field_label(&mut col, "Output Device (Speakers/Headphones)");
        let mut output_choice = styled_choice(&mut col);
        populate_device_choice(&mut output_choice, &output_devices, &settings.output_device);

        let mut device_info = frame::Frame::default()
            .with_label(&format!(
                "{} input, {} output devices",
                input_devices.len(),
                output_devices.len(),
            ))
            .with_align(enums::Align::Left | enums::Align::Inside);
        device_info.set_label_color(DarkTheme::TEXT_MUTED);
        device_info.set_label_size(10);
        col.fixed(&device_info, 14);

        separator(&mut col);

        // ════════════════════════════════════════════════════════════════
        //  INPUT
        // ════════════════════════════════════════════════════════════════

        section_header(&mut col, "\u{1f3a4}  INPUT");

        field_label(&mut col, "Capture Level");
        let vu_input = make_vu_meter(&mut col, input_level.clone(), 22, DarkTheme::SUCCESS);

        field_label(&mut col, "Input Gain");
        let mut input_gain_slider = make_slider(
            &mut col,
            0.0,
            2.0,
            settings.input_gain as f64,
            0.05,
            DarkTheme::ACCENT,
        );
        let mut input_gain_lbl = value_label(
            &mut col,
            &format!("Gain: {}%", (settings.input_gain * 100.0) as i32),
        );

        // Test Microphone button
        let mut test_mic_btn = button::Button::default().with_label("\u{1f3a4}  Test Microphone");
        col.fixed(&test_mic_btn, 30);
        test_mic_btn.set_color(DarkTheme::BG_TERTIARY);
        test_mic_btn.set_label_color(DarkTheme::TEXT_PRIMARY);
        test_mic_btn.set_frame(enums::FrameType::FlatBox);
        test_mic_btn.set_label_size(11);

        let mut test_mic_info = frame::Frame::default()
            .with_label("Listen to your own mic (others won't hear you)")
            .with_align(enums::Align::Left | enums::Align::Inside);
        test_mic_info.set_label_color(DarkTheme::TEXT_MUTED);
        test_mic_info.set_label_size(10);
        col.fixed(&test_mic_info, 14);

        separator(&mut col);

        // ════════════════════════════════════════════════════════════════
        //  OUTPUT
        // ════════════════════════════════════════════════════════════════

        section_header(&mut col, "\u{1f50a}  OUTPUT");

        let vu_output = make_vu_meter(
            &mut col,
            output_level.clone(),
            22,
            enums::Color::from_rgb(56, 189, 248),
        );

        field_label(&mut col, "Master Volume");
        let mut output_gain_slider = make_slider(
            &mut col,
            0.0,
            2.0,
            settings.output_gain as f64,
            0.05,
            enums::Color::from_rgb(56, 189, 248),
        );
        let mut output_gain_lbl = value_label(
            &mut col,
            &format!("Volume: {}%", (settings.output_gain * 100.0) as i32),
        );

        separator(&mut col);

        // ════════════════════════════════════════════════════════════════
        //  VOICE ACTIVATION
        // ════════════════════════════════════════════════════════════════

        section_header(&mut col, "\u{26a1}  VOICE ACTIVATION");

        field_label(
            &mut col,
            "Select when to activate the microphone based on noise level",
        );

        // VAD meter with threshold line
        let threshold_rc = std::rc::Rc::new(std::cell::RefCell::new(settings.vad_threshold));

        let threshold_draw = threshold_rc.clone();
        let level_draw = denoised_level.clone();
        let mut vu_vad = frame::Frame::default();
        vu_vad.set_frame(enums::FrameType::BorderBox);
        vu_vad.set_color(DarkTheme::BG_INPUT);
        col.fixed(&vu_vad, 28);

        vu_vad.draw(move |f| {
            draw::set_draw_color(DarkTheme::BG_INPUT);
            draw::draw_rectf(f.x(), f.y(), f.w(), f.h());

            let level = f32::from_bits(level_draw.load(Ordering::Relaxed)).clamp(0.0, 1.0);
            let bar_w = ((f.w() - 4) as f32 * level) as i32;
            let thr = *threshold_draw.borrow();

            let color = if level > thr {
                DarkTheme::SUCCESS
            } else {
                DarkTheme::BG_TERTIARY
            };
            if bar_w > 0 {
                draw::set_draw_color(color);
                draw::draw_rectf(f.x() + 2, f.y() + 4, bar_w, f.h() - 8);
            }

            let thr_x = f.x() + 2 + ((f.w() - 4) as f32 * thr) as i32;
            draw::set_draw_color(DarkTheme::DANGER);
            draw::set_line_style(draw::LineStyle::Solid, 2);
            draw::draw_line(thr_x, f.y() + 2, thr_x, f.y() + f.h() - 2);
            draw::set_line_style(draw::LineStyle::Solid, 1);

            draw::set_draw_color(DarkTheme::BORDER);
            draw::draw_rect(f.x(), f.y(), f.w(), f.h());
        });

        field_label(&mut col, "Activation Sensitivity");
        let mut vad_slider = make_slider(
            &mut col,
            0.01,
            0.50,
            settings.vad_threshold as f64,
            0.01,
            DarkTheme::ACCENT,
        );
        let mut vad_sens_lbl =
            value_label(&mut col, &format_sensitivity(settings.vad_threshold as f64));

        let mute_on_start_toggle = styled_check(&mut col, "Start muted", settings.mute_on_start);

        separator(&mut col);

        // ════════════════════════════════════════════════════════════════
        //  BUTTONS
        // ════════════════════════════════════════════════════════════════

        let mut button_row = group::Flex::default().row();
        col.fixed(&button_row, 35);
        button_row.set_spacing(10);

        let mut reset_btn = button::Button::default().with_label("Reset Defaults");
        reset_btn.set_color(DarkTheme::BG_TERTIARY);
        reset_btn.set_label_color(DarkTheme::TEXT_PRIMARY);
        reset_btn.set_frame(enums::FrameType::FlatBox);

        let mut save_btn = button::Button::default().with_label("Save and Close");
        save_btn.set_color(DarkTheme::SUCCESS);
        save_btn.set_label_color(enums::Color::White);
        save_btn.set_frame(enums::FrameType::FlatBox);

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
                btn.set_color(DarkTheme::DANGER);
                info_lbl.set_label("Listening to your mic... others can't hear you");
                info_lbl.set_label_color(DarkTheme::WARNING);
            } else {
                btn.set_label("\u{1f3a4}  Test Microphone");
                btn.set_color(DarkTheme::BG_TERTIARY);
                info_lbl.set_label("Listen to your own mic (others won't hear you)");
                info_lbl.set_label_color(DarkTheme::TEXT_MUTED);
            }

            s.send(UiMessage::TestMicrophone(is_testing));
        });

        // VAD slider
        let s = sender;
        let threshold_update = threshold_rc.clone();
        let mut vu_redraw = vu_vad.clone();
        let mut sens_lbl = vad_sens_lbl.clone();
        vad_slider.set_callback(move |sl| {
            let val = sl.value() as f32;
            *threshold_update.borrow_mut() = val;
            sens_lbl.set_label(&format_sensitivity(sl.value()));
            vu_redraw.redraw();
            s.send(UiMessage::SetVadThreshold(val));
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
        let mut ms = mute_on_start_toggle.clone();
        let threshold_reset = threshold_rc.clone();
        let mut vu_reset = vu_vad.clone();
        reset_btn.set_callback(move |_| {
            let d = Settings::default();
            ig.set_value(d.input_gain as f64);
            ig.do_callback();
            og.set_value(d.output_gain as f64);
            og.do_callback();
            vs.set_value(d.vad_threshold as f64);
            vs.do_callback();
            ms.set_checked(d.mute_on_start);
            *threshold_reset.borrow_mut() = d.vad_threshold;
            vu_reset.redraw();
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
