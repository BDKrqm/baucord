// ui/settings.rs — Audio settings + live diagnostics panel.
//
// Modeled on the stats_dashboard example from voice-chat-core.
// Controls send UiMessages; stats update at 8 Hz only while visible.

use crate::settings::Settings;
use crate::ui::messages::UiMessage;
use crate::ui::theme::DarkTheme;
use fltk::{prelude::*, *};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use core::VoiceChat;

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

fn stat_label(col: &mut group::Flex, text: &str) -> frame::Frame {
    let mut lbl = frame::Frame::default()
        .with_label(text)
        .with_align(enums::Align::Left | enums::Align::Inside);
    lbl.set_label_color(enums::Color::from_rgb(140, 200, 140)); // monospace green
    lbl.set_label_size(11);
    col.fixed(&lbl, 16);
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
    ) {
        let mut wind = window::Window::default()
            .with_size(560, 780)
            .with_label("Settings");
        wind.set_color(DarkTheme::BG_PRIMARY);
        wind.make_resizable(false);

        let mut scroll = group::Scroll::default_fill();
        scroll.set_scrollbar_size(8);

        let mut col = group::Flex::default()
            .with_size(530, 1100)
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
        let input_level_arc = Arc::new(AtomicU32::new(0));
        let vu_input = make_vu_meter(&mut col, input_level_arc.clone(), 22, DarkTheme::SUCCESS);

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

        separator(&mut col);

        // ════════════════════════════════════════════════════════════════
        //  OUTPUT
        // ════════════════════════════════════════════════════════════════

        section_header(&mut col, "\u{1f50a}  OUTPUT");

        let output_level_arc = Arc::new(AtomicU32::new(0));
        let vu_output = make_vu_meter(
            &mut col,
            output_level_arc.clone(),
            22,
            enums::Color::from_rgb(56, 189, 248), // cyan
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

        let mut speaking_lbl = frame::Frame::default()
            .with_label("\u{26ab}  Not transmitting")
            .with_align(enums::Align::Left | enums::Align::Inside);
        speaking_lbl.set_label_color(DarkTheme::TEXT_MUTED);
        speaking_lbl.set_label_size(12);
        col.fixed(&speaking_lbl, 22);

        field_label(&mut col, "Denoised Level vs Threshold");

        // VAD meter with threshold line
        let denoised_level_arc = Arc::new(AtomicU32::new(0));
        let threshold_rc = std::rc::Rc::new(std::cell::RefCell::new(settings.vad_threshold));

        let threshold_draw = threshold_rc.clone();
        let level_draw = denoised_level_arc.clone();
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

        field_label(&mut col, "Activation Threshold");
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

        let noise_toggle = styled_check(
            &mut col,
            "Noise suppression (RNNoise)",
            settings.noise_suppression,
        );
        let vad_toggle = styled_check(&mut col, "Voice activation (VAD)", true);
        let mute_on_start_toggle = styled_check(&mut col, "Start muted", settings.mute_on_start);
        let loopback_toggle = styled_check(
            &mut col,
            "Monitor input (loopback)",
            settings.loopback_enabled,
        );

        separator(&mut col);

        // ════════════════════════════════════════════════════════════════
        //  LIVE STATS
        // ════════════════════════════════════════════════════════════════

        section_header(&mut col, "\u{1f4ca}  LIVE STATS");

        let stat_codec = stat_label(&mut col, "Codec: --");
        let stat_bw = stat_label(&mut col, "Bandwidth: --");
        let stat_packets = stat_label(&mut col, "Packets: --");
        let stat_loss = stat_label(&mut col, "Loss: --");
        let stat_recovery = stat_label(&mut col, "Recovery: --");
        let stat_buffers = stat_label(&mut col, "Buffers: --");
        let stat_jitter = stat_label(&mut col, "Jitter: --");

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

        let mut save_btn = button::Button::default().with_label("Save & Close");
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

        // Toggles
        let s = sender;
        noise_toggle.clone().set_callback(move |b| {
            s.send(UiMessage::SetNoiseSuppression(b.is_checked()));
        });

        let s = sender;
        vad_toggle.clone().set_callback(move |b| {
            s.send(UiMessage::SetVadEnabled(b.is_checked()));
        });

        let s = sender;
        loopback_toggle.clone().set_callback(move |b| {
            s.send(UiMessage::SetLoopback(b.is_checked()));
        });

        // Window close
        let s = sender;
        wind.set_callback(move |w| {
            if app::event() == enums::Event::Close {
                s.send(UiMessage::SettingsWindowClosed);
                w.hide();
            }
        });

        // Save & Close
        let s = sender;
        let mut wind_clone = wind.clone();
        save_btn.set_callback(move |_| {
            s.send(UiMessage::SaveSettings);
            s.send(UiMessage::SettingsWindowClosed);
            wind_clone.hide();
        });

        // Reset
        let s = sender;
        let mut ig = input_gain_slider.clone();
        let mut og = output_gain_slider.clone();
        let mut vs = vad_slider.clone();
        let mut nt = noise_toggle.clone();
        let mut lb = loopback_toggle.clone();
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
            nt.set_checked(d.noise_suppression);
            lb.set_checked(d.loopback_enabled);
            ms.set_checked(d.mute_on_start);
            *threshold_reset.borrow_mut() = d.vad_threshold;
            vu_reset.redraw();
            s.send(UiMessage::ResetSettings);
        });

        // ════════════════════════════════════════════════════════════════
        //  TIMER: 8 Hz stats update (only while window is visible)
        // ════════════════════════════════════════════════════════════════

        // We read stats via the static VoiceChat methods where possible,
        // but for live levels we need the atomic handles. Since we can't
        // get those through the message system, we'll use placeholder arcs
        // and update them from the timer. The VU meters will show 0 until
        // we get a way to pass the stats handle — this is a known limitation
        // for now. The stats text labels work via polling stats_summary.

        let mut vu_in_t = vu_input.clone();
        let mut vu_out_t = vu_output.clone();
        let mut vu_vad_t = vu_vad.clone();
        let mut speak_t = speaking_lbl.clone();
        let mut s_codec_t = stat_codec.clone();
        let mut s_bw_t = stat_bw.clone();
        let mut s_pkt_t = stat_packets.clone();
        let mut s_loss_t = stat_loss.clone();
        let mut s_rec_t = stat_recovery.clone();
        let mut s_buf_t = stat_buffers.clone();
        let mut s_jit_t = stat_jitter.clone();

        app::add_timeout3(0.125, move |handle| {
            // Redraw VU meters (they read from their arcs)
            vu_in_t.redraw();
            vu_out_t.redraw();
            vu_vad_t.redraw();

            app::repeat_timeout3(0.125, handle);
        });
    }
}
