//! # voice-chat-core — Stats Dashboard
//!
//! Run with: `cargo run --example stats_dashboard`
//!
//! All-in-one settings + diagnostics panel for a TeamSpeak-style voice client.
//! Shows real-time audio pipeline stats alongside full controls.

use fltk::{prelude::*, *};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use voice_chat_core::{VoiceChat, VoiceChatConfig};

// ─── Dark theme ───────────────────────────────────────────────────────────

struct Theme;
#[allow(dead_code)]
impl Theme {
    const BG_PRIMARY: enums::Color   = enums::Color::from_rgb(24, 24, 28);
    const BG_SECONDARY: enums::Color = enums::Color::from_rgb(34, 34, 40);
    const BG_TERTIARY: enums::Color  = enums::Color::from_rgb(44, 44, 52);
    const BG_INPUT: enums::Color     = enums::Color::from_rgb(18, 18, 22);

    const TEXT_PRIMARY: enums::Color   = enums::Color::from_rgb(220, 220, 225);
    const TEXT_SECONDARY: enums::Color = enums::Color::from_rgb(160, 160, 170);
    const TEXT_MUTED: enums::Color     = enums::Color::from_rgb(100, 100, 115);
    const TEXT_MONO: enums::Color      = enums::Color::from_rgb(140, 200, 140);

    const ACCENT: enums::Color   = enums::Color::from_rgb(88, 101, 242);
    const SUCCESS: enums::Color  = enums::Color::from_rgb(67, 181, 129);
    const WARNING: enums::Color  = enums::Color::from_rgb(250, 166, 26);
    const DANGER: enums::Color   = enums::Color::from_rgb(237, 66, 69);
    const SPEAKING: enums::Color = enums::Color::from_rgb(67, 181, 129);
    const CYAN: enums::Color     = enums::Color::from_rgb(56, 189, 248);
    const BORDER: enums::Color   = enums::Color::from_rgb(16, 16, 20);

    fn apply() {
        fn rgb(c: enums::Color) -> (u8, u8, u8) {
            let b = c.bits();
            (((b >> 24) & 0xFF) as u8, ((b >> 16) & 0xFF) as u8, ((b >> 8) & 0xFF) as u8)
        }
        let (r, g, b) = rgb(Self::BG_PRIMARY);
        app::set_background_color(r, g, b);
        let (r, g, b) = rgb(Self::BG_SECONDARY);
        app::set_background2_color(r, g, b);
        let (r, g, b) = rgb(Self::TEXT_PRIMARY);
        app::set_foreground_color(r, g, b);
        let (r, g, b) = rgb(Self::ACCENT);
        app::set_selection_color(r, g, b);
        let (r, g, b) = rgb(Self::TEXT_MUTED);
        app::set_inactive_color(r, g, b);
    }
}

// ─── Widget helpers ───────────────────────────────────────────────────────

fn section_header(col: &mut group::Flex, icon: &str, text: &str) {
    let mut lbl = frame::Frame::default()
        .with_label(&format!("{}  {}", icon, text))
        .with_align(enums::Align::Left | enums::Align::Inside);
    lbl.set_label_color(Theme::TEXT_PRIMARY);
    lbl.set_label_size(13);
    col.fixed(&lbl, 24);
}

fn stat_label(col: &mut group::Flex, text: &str) -> frame::Frame {
    let mut lbl = frame::Frame::default()
        .with_label(text)
        .with_align(enums::Align::Left | enums::Align::Inside);
    lbl.set_label_color(Theme::TEXT_MONO);
    lbl.set_label_size(11);
    col.fixed(&lbl, 16);
    lbl
}

fn field_label(col: &mut group::Flex, text: &str) {
    let mut lbl = frame::Frame::default()
        .with_label(text)
        .with_align(enums::Align::Left | enums::Align::Inside);
    lbl.set_label_color(Theme::TEXT_SECONDARY);
    lbl.set_label_size(11);
    col.fixed(&lbl, 18);
}

fn separator(col: &mut group::Flex) {
    let mut sep = frame::Frame::default();
    sep.set_frame(enums::FrameType::FlatBox);
    sep.set_color(Theme::BG_TERTIARY);
    col.fixed(&sep, 1);
}

fn styled_check(col: &mut group::Flex, label: &str, checked: bool) -> button::CheckButton {
    let mut cb = button::CheckButton::default().with_label(label);
    cb.set_checked(checked);
    cb.set_color(Theme::BG_PRIMARY);
    cb.set_label_color(Theme::TEXT_PRIMARY);
    cb.set_selection_color(Theme::ACCENT);
    cb.set_label_size(11);
    col.fixed(&cb, 22);
    cb
}

fn styled_choice(col: &mut group::Flex) -> menu::Choice {
    let mut c = menu::Choice::default();
    c.set_color(Theme::BG_INPUT);
    c.set_text_color(Theme::TEXT_PRIMARY);
    c.set_frame(enums::FrameType::BorderBox);
    col.fixed(&c, 26);
    c
}

fn populate_device_choice(choice: &mut menu::Choice, devices: &[(String, bool)]) {
    choice.clear();
    choice.add_choice("System Default");
    for (name, _) in devices {
        choice.add_choice(name);
    }
    choice.set_value(0);
}

fn make_vu_meter(
    col: &mut group::Flex,
    level_arc: Arc<AtomicU32>,
    height: i32,
    color: enums::Color,
) -> frame::Frame {
    let mut f = frame::Frame::default();
    f.set_frame(enums::FrameType::BorderBox);
    f.set_color(Theme::BG_INPUT);
    col.fixed(&f, height);

    f.draw(move |frm| {
        draw::set_draw_color(Theme::BG_INPUT);
        draw::draw_rectf(frm.x(), frm.y(), frm.w(), frm.h());

        let level = f32::from_bits(level_arc.load(Ordering::Relaxed)).clamp(0.0, 1.0);
        let bar_w = ((frm.w() - 4) as f32 * level) as i32;

        let c = if level > 0.9 {
            Theme::DANGER
        } else if level > 0.7 {
            Theme::WARNING
        } else {
            color
        };

        if bar_w > 0 {
            draw::set_draw_color(c);
            draw::draw_rectf(frm.x() + 2, frm.y() + 3, bar_w, frm.h() - 6);
        }

        draw::set_draw_color(Theme::BORDER);
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

// ─── Main ─────────────────────────────────────────────────────────────────

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // ── Boot voice engine ────────────────────────────────────────────
    let config = VoiceChatConfig::default();
    let mut vc = VoiceChat::new(config)?;
    vc.start()?;

    let vc = Rc::new(RefCell::new(vc));

    let input_devices = VoiceChat::list_input_devices().unwrap_or_default();
    let output_devices = VoiceChat::list_output_devices().unwrap_or_default();

    // ── FLTK app ────────────────────────────────────────────────────
    let app_handle = app::App::default();
    Theme::apply();

    let mut wind = window::Window::default()
        .with_size(620, 860)
        .with_label("voice-chat-core \u{2014} Stats Dashboard");
    wind.set_color(Theme::BG_PRIMARY);
    wind.make_resizable(false);

    let mut scroll = group::Scroll::default_fill();
    scroll.set_scrollbar_size(8);

    let mut col = group::Flex::default()
        .with_size(590, 1180)
        .with_pos(10, 10)
        .column();
    col.set_spacing(4);

    // ══════════════════════════════════════════════════════════════════
    //  DEVICES
    // ══════════════════════════════════════════════════════════════════

    section_header(&mut col, "\u{1f3a7}", "DEVICES");

    field_label(&mut col, "Input Device");
    let mut input_choice = styled_choice(&mut col);
    populate_device_choice(&mut input_choice, &input_devices);

    field_label(&mut col, "Output Device");
    let mut output_choice = styled_choice(&mut col);
    populate_device_choice(&mut output_choice, &output_devices);

    let mut device_info = frame::Frame::default()
        .with_label(&format!(
            "{} input, {} output devices found  (check terminal for details)",
            input_devices.len(),
            output_devices.len()
        ))
        .with_align(enums::Align::Left | enums::Align::Inside);
    device_info.set_label_color(Theme::TEXT_MUTED);
    device_info.set_label_size(10);
    col.fixed(&device_info, 14);

    separator(&mut col);

    // ══════════════════════════════════════════════════════════════════
    //  INPUT
    // ══════════════════════════════════════════════════════════════════

    section_header(&mut col, "\u{1f3a4}", "INPUT");

    field_label(&mut col, "Capture Level (pre-denoise)");
    let input_level_arc: Arc<AtomicU32> = Arc::new(AtomicU32::new(0));
    let vu_input = make_vu_meter(&mut col, input_level_arc.clone(), 22, Theme::SUCCESS);

    field_label(&mut col, "Input Gain");
    let mut input_gain_slider = valuator::HorSlider::default();
    input_gain_slider.set_minimum(0.0);
    input_gain_slider.set_maximum(2.0);
    input_gain_slider.set_value(1.0);
    input_gain_slider.set_step(0.05, 1);
    input_gain_slider.set_color(Theme::BG_TERTIARY);
    input_gain_slider.set_selection_color(Theme::ACCENT);
    col.fixed(&input_gain_slider, 22);

    let mut input_gain_lbl = frame::Frame::default()
        .with_label("Gain: 100%")
        .with_align(enums::Align::Left | enums::Align::Inside);
    input_gain_lbl.set_label_color(Theme::TEXT_MUTED);
    input_gain_lbl.set_label_size(10);
    col.fixed(&input_gain_lbl, 14);

    separator(&mut col);

    // ══════════════════════════════════════════════════════════════════
    //  OUTPUT
    // ══════════════════════════════════════════════════════════════════

    section_header(&mut col, "\u{1f50a}", "OUTPUT");

    let output_level_arc: Arc<AtomicU32> = Arc::new(AtomicU32::new(0));
    let vu_output = make_vu_meter(&mut col, output_level_arc.clone(), 22, Theme::CYAN);

    field_label(&mut col, "Master Volume");
    let mut output_gain_slider = valuator::HorSlider::default();
    output_gain_slider.set_minimum(0.0);
    output_gain_slider.set_maximum(2.0);
    output_gain_slider.set_value(1.0);
    output_gain_slider.set_step(0.05, 1);
    output_gain_slider.set_color(Theme::BG_TERTIARY);
    output_gain_slider.set_selection_color(Theme::CYAN);
    col.fixed(&output_gain_slider, 22);

    let mut output_gain_lbl = frame::Frame::default()
        .with_label("Volume: 100%")
        .with_align(enums::Align::Left | enums::Align::Inside);
    output_gain_lbl.set_label_color(Theme::TEXT_MUTED);
    output_gain_lbl.set_label_size(10);
    col.fixed(&output_gain_lbl, 14);

    separator(&mut col);

    // ══════════════════════════════════════════════════════════════════
    //  VOICE ACTIVATION
    //  The VAD bar uses the POST-DENOISE level — this is what the VAD
    //  actually sees. Noise that gets suppressed won't show here.
    // ══════════════════════════════════════════════════════════════════

    section_header(&mut col, "\u{26a1}", "VOICE ACTIVATION");

    let mut speaking_lbl = frame::Frame::default()
        .with_label("\u{26ab}  Not transmitting")
        .with_align(enums::Align::Left | enums::Align::Inside);
    speaking_lbl.set_label_color(Theme::TEXT_MUTED);
    speaking_lbl.set_label_size(12);
    col.fixed(&speaking_lbl, 22);

    field_label(&mut col, "Denoised Level vs Threshold");

    // VAD bar: post-denoise level with threshold line
    let denoised_level_arc: Arc<AtomicU32> = Arc::new(AtomicU32::new(0));
    let threshold_rc: Rc<RefCell<f32>> = Rc::new(RefCell::new(0.15));

    let threshold_draw = threshold_rc.clone();
    let level_draw_vad = denoised_level_arc.clone();
    let mut vu_vad = frame::Frame::default();
    vu_vad.set_frame(enums::FrameType::BorderBox);
    vu_vad.set_color(Theme::BG_INPUT);
    col.fixed(&vu_vad, 28);

    vu_vad.draw(move |f| {
        draw::set_draw_color(Theme::BG_INPUT);
        draw::draw_rectf(f.x(), f.y(), f.w(), f.h());

        let level = f32::from_bits(level_draw_vad.load(Ordering::Relaxed)).clamp(0.0, 1.0);
        let bar_w = ((f.w() - 4) as f32 * level) as i32;
        let thr = *threshold_draw.borrow();

        let color = if level > thr { Theme::SUCCESS } else { Theme::BG_TERTIARY };
        if bar_w > 0 {
            draw::set_draw_color(color);
            draw::draw_rectf(f.x() + 2, f.y() + 4, bar_w, f.h() - 8);
        }

        let thr_x = f.x() + 2 + ((f.w() - 4) as f32 * thr) as i32;
        draw::set_draw_color(Theme::DANGER);
        draw::set_line_style(draw::LineStyle::Solid, 2);
        draw::draw_line(thr_x, f.y() + 2, thr_x, f.y() + f.h() - 2);
        draw::set_line_style(draw::LineStyle::Solid, 1);

        draw::set_draw_color(Theme::BORDER);
        draw::draw_rect(f.x(), f.y(), f.w(), f.h());
    });

    field_label(&mut col, "Activation Threshold");
    let mut vad_slider = valuator::HorSlider::default();
    vad_slider.set_minimum(0.01);
    vad_slider.set_maximum(0.50);
    vad_slider.set_value(0.15);
    vad_slider.set_step(0.01, 1);
    vad_slider.set_color(Theme::BG_TERTIARY);
    vad_slider.set_selection_color(Theme::ACCENT);
    col.fixed(&vad_slider, 22);

    let mut vad_sens_lbl = frame::Frame::default()
        .with_label(&format_sensitivity(0.15))
        .with_align(enums::Align::Left | enums::Align::Inside);
    vad_sens_lbl.set_label_color(Theme::TEXT_MUTED);
    vad_sens_lbl.set_label_size(10);
    col.fixed(&vad_sens_lbl, 14);

    let noise_toggle = styled_check(&mut col, "Noise suppression (RNNoise)", true);
    let vad_toggle = styled_check(&mut col, "Voice activation (VAD)", true);
    let mute_toggle = styled_check(&mut col, "Mute microphone", false);
    let loopback_toggle = styled_check(&mut col, "Monitor input (loopback)", false);

    separator(&mut col);

    // ══════════════════════════════════════════════════════════════════
    //  CODEC & BANDWIDTH
    // ══════════════════════════════════════════════════════════════════

    section_header(&mut col, "\u{1f4ca}", "CODEC & BANDWIDTH");

    let stat_opus  = stat_label(&mut col, "Opus: --");
    let stat_bw    = stat_label(&mut col, "Bandwidth: --");
    let stat_timing = stat_label(&mut col, "Timing: --");
    let stat_state = stat_label(&mut col, "State: --");

    separator(&mut col);

    // ══════════════════════════════════════════════════════════════════
    //  PIPELINE HEALTH
    // ══════════════════════════════════════════════════════════════════

    section_header(&mut col, "\u{1fa7a}", "PIPELINE HEALTH");

    let stat_packets  = stat_label(&mut col, "Packets: --");
    let stat_loss     = stat_label(&mut col, "Loss: --");
    let stat_recovery = stat_label(&mut col, "Recovery: --");
    let stat_buffers  = stat_label(&mut col, "Buffers: --");
    let stat_jitter   = stat_label(&mut col, "Jitter buffer: --");
    let stat_uptime   = stat_label(&mut col, "Uptime: --");

    separator(&mut col);

    section_header(&mut col, "\u{1f4cb}", "RAW SUMMARY");
    let mut stat_raw = frame::Frame::default()
        .with_label("...")
        .with_align(enums::Align::Left | enums::Align::Inside | enums::Align::Wrap);
    stat_raw.set_label_color(Theme::TEXT_MUTED);
    stat_raw.set_label_size(9);
    col.fixed(&stat_raw, 42);

    col.end();
    scroll.end();
    wind.end();
    wind.show();

    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    // CALLBACKS
    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

    let input_devs = input_devices.clone();
    let vc_c = vc.clone();
    input_choice.set_callback(move |c| {
        let idx = c.value() as usize;
        let device = if idx == 0 { None } else { input_devs.get(idx - 1).map(|(n, _)| n.as_str()) };
        if let Err(e) = vc_c.borrow_mut().set_input_device(device) {
            eprintln!("Failed to set input device: {}", e);
        }
    });

    let output_devs = output_devices.clone();
    let vc_c = vc.clone();
    output_choice.set_callback(move |c| {
        let idx = c.value() as usize;
        let device = if idx == 0 { None } else { output_devs.get(idx - 1).map(|(n, _)| n.as_str()) };
        if let Err(e) = vc_c.borrow_mut().set_output_device(device) {
            eprintln!("Failed to set output device: {}", e);
        }
    });

    let vc_c = vc.clone();
    let mut ig_lbl = input_gain_lbl.clone();
    input_gain_slider.set_callback(move |s| {
        let val = s.value() as f32;
        vc_c.borrow().set_input_gain(val);
        ig_lbl.set_label(&format!("Gain: {}%", (val * 100.0) as i32));
    });

    let vc_c = vc.clone();
    let mut og_lbl = output_gain_lbl.clone();
    output_gain_slider.set_callback(move |s| {
        let val = s.value() as f32;
        vc_c.borrow().set_output_gain(val);
        og_lbl.set_label(&format!("Volume: {}%", (val * 100.0) as i32));
    });

    let threshold_slider = threshold_rc.clone();
    let mut vu_redraw = vu_vad.clone();
    let vc_c = vc.clone();
    let mut vad_lbl_cb = vad_sens_lbl.clone();
    vad_slider.set_callback(move |s| {
        let val = s.value() as f32;
        *threshold_slider.borrow_mut() = val;
        vad_lbl_cb.set_label(&format_sensitivity(s.value()));
        vu_redraw.redraw();
        vc_c.borrow().set_vad_threshold(val);
    });

    let vc_c = vc.clone();
    noise_toggle.clone().set_callback(move |b| { vc_c.borrow().set_noise_suppression(b.is_checked()); });
    let vc_c = vc.clone();
    vad_toggle.clone().set_callback(move |b| { vc_c.borrow().set_vad_enabled(b.is_checked()); });
    let vc_c = vc.clone();
    mute_toggle.clone().set_callback(move |b| { vc_c.borrow().set_muted(b.is_checked()); });
    let vc_c = vc.clone();
    loopback_toggle.clone().set_callback(move |b| { vc_c.borrow().set_loopback(b.is_checked()); });

    wind.set_callback(|w| {
        if app::event() == enums::Event::Close {
            w.hide();
        }
    });

    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    // TIMER: 8 Hz — updates levels + all stats
    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

    let vc_timer = vc.clone();
    let in_level_timer  = input_level_arc.clone();
    let out_level_timer = output_level_arc.clone();
    let denoised_timer  = denoised_level_arc.clone();

    let mut vu_in_t  = vu_input.clone();
    let mut vu_out_t = vu_output.clone();
    let mut vu_vad_t = vu_vad.clone();
    let mut speak_t  = speaking_lbl.clone();

    let mut s_opus_t   = stat_opus.clone();
    let mut s_bw_t     = stat_bw.clone();
    let mut s_timing_t = stat_timing.clone();
    let mut s_state_t  = stat_state.clone();
    let mut s_pkt_t    = stat_packets.clone();
    let mut s_loss_t   = stat_loss.clone();
    let mut s_rec_t    = stat_recovery.clone();
    let mut s_buf_t    = stat_buffers.clone();
    let mut s_jit_t    = stat_jitter.clone();
    let mut s_up_t     = stat_uptime.clone();
    let mut s_raw_t    = stat_raw.clone();

    let prev_sent  = Rc::new(RefCell::new(0u64));
    let prev_speak = Rc::new(RefCell::new(false));

    app::add_timeout3(0.125, move |handle| {
        let vc_ref = vc_timer.borrow();
        let stats = vc_ref.stats();

        // ── Levels ──────────────────────────────────────────────────
        in_level_timer.store(vc_ref.input_level().to_bits(), Ordering::Relaxed);
        out_level_timer.store(vc_ref.output_level().to_bits(), Ordering::Relaxed);
        denoised_timer.store(vc_ref.denoised_level().to_bits(), Ordering::Relaxed);

        vu_in_t.redraw();
        vu_out_t.redraw();
        vu_vad_t.redraw();

        // ── Speaking indicator (real VAD state from engine) ──────────
        let speaking = vc_ref.is_speaking();
        if speaking != *prev_speak.borrow() {
            *prev_speak.borrow_mut() = speaking;
            if speaking {
                speak_t.set_label("\u{1f7e2}  SPEAKING \u{2014} transmitting audio");
                speak_t.set_label_color(Theme::SPEAKING);
            } else {
                speak_t.set_label("\u{26ab}  Not transmitting");
                speak_t.set_label_color(Theme::TEXT_MUTED);
            }
            speak_t.redraw();
        }

        // ── Codec stats ─────────────────────────────────────────────
        let last_pkt = stats.last_packet_size.load(Ordering::Relaxed);
        let avg_pkt  = f32::from_bits(stats.avg_packet_size.load(Ordering::Relaxed));
        s_opus_t.set_label(&format!(
            "Opus: {}B last, {:.0}B avg  |  24kbps VBR, 10ms frames",
            last_pkt, avg_pkt,
        ));

        let bw_send = stats.estimated_send_kbps();
        let bw_recv = stats.estimated_recv_kbps();
        let total_sent_kb = stats.bytes_sent.load(Ordering::Relaxed) as f32 / 1024.0;
        let total_recv_kb = stats.bytes_received.load(Ordering::Relaxed) as f32 / 1024.0;
        s_bw_t.set_label(&format!(
            "BW: \u{2191} {:.1} kbps ({:.0} KB)  \u{2193} {:.1} kbps ({:.0} KB)",
            bw_send, total_sent_kb, bw_recv, total_recv_kb,
        ));

        let enc_us = stats.last_encode_us.load(Ordering::Relaxed);
        let dec_us = stats.last_decode_us.load(Ordering::Relaxed);
        let budget_pct = (enc_us as f32 / 10000.0) * 100.0;
        s_timing_t.set_label(&format!(
            "Encode: {}\u{00b5}s  Decode: {}\u{00b5}s  (budget: {:.0}% of 10ms)",
            enc_us, dec_us, budget_pct,
        ));

        let dtx = stats.dtx_active.load(Ordering::Relaxed);
        let denoise = stats.denoise_active.load(Ordering::Relaxed);
        let vad_state = stats.vad_active.load(Ordering::Relaxed);
        s_state_t.set_label(&format!(
            "DTX: {}  |  Denoise: {}  |  VAD: {}",
            if dtx { "ON (silence)" } else { "off" },
            if denoise { "ON" } else { "off" },
            if vad_state { "SPEECH" } else { "quiet" },
        ));

        // ── Pipeline health ─────────────────────────────────────────
        let sent = stats.packets_sent.load(Ordering::Relaxed);
        let recv = stats.packets_received.load(Ordering::Relaxed);
        let lost = stats.packets_lost.load(Ordering::Relaxed);

        // packets/sec rate (timer runs at 8 Hz)
        let prev = *prev_sent.borrow();
        let delta = sent.saturating_sub(prev);
        *prev_sent.borrow_mut() = sent;
        let pps = delta as f32 * 8.0;

        let pps_note = if pps < 0.1 && sent > 0 {
            "  (idle: VAD gating)"
        } else {
            ""
        };
        s_pkt_t.set_label(&format!(
            "Packets: sent {} recv {} lost {}  |  {:.0} pkt/s{}",
            sent, recv, lost, pps, pps_note,
        ));

        let loss_pct = if recv + lost > 0 {
            (lost as f32 / (recv + lost) as f32) * 100.0
        } else {
            0.0
        };
        s_loss_t.set_label(&format!("Loss rate: {:.1}%", loss_pct));

        let fec = stats.fec_recoveries.load(Ordering::Relaxed);
        let plc = stats.plc_frames.load(Ordering::Relaxed);
        s_rec_t.set_label(&format!("FEC recoveries: {}  PLC frames: {}", fec, plc));

        let overruns  = stats.input_overruns.load(Ordering::Relaxed);
        let underruns = stats.output_underruns.load(Ordering::Relaxed);
        s_buf_t.set_label(&format!("Overruns: {}  Underruns: {}", overruns, underruns));

        let jitter_ms = stats.jitter_buffer_occupancy_ms.load(Ordering::Relaxed);
        s_jit_t.set_label(&format!("Jitter buffer: {} ms", jitter_ms));

        let uptime = stats.start_time.elapsed().as_secs();
        s_up_t.set_label(&format!("Uptime: {}m {}s", uptime / 60, uptime % 60));

        s_raw_t.set_label(&vc_ref.stats_summary());

        app::repeat_timeout3(0.125, handle);
    });

    app_handle.run()?;
    Ok(())
}
