// examples/stats.rs
//
// Run with: `cargo run --example stats`
//
// Live stats dashboard for voice-chat-core.
// Demonstrates the full pipeline API: devices, gain, VAD gate, codec stats,
// bandwidth estimation, and buffer health — all wired to real-time controls.
//
// Theming + custom widgets live in the `voice-chat-ui` crate so this file
// can focus entirely on the audio library surface.

use fltk::{app, enums, frame, group, prelude::*, window};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use core::{VoiceChat, VoiceChatConfig};
use ui::{gate_bar, helpers, theme, vu_meter};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // ── Init FLTK first (before any audio threads — avoids COM/Win32 conflicts)
    let app_handle = app::App::default();
    theme::apply();

    // ── Create voice engine (no threads yet — .start() is deferred)
    let config = VoiceChatConfig::default();
    let vc = Rc::new(RefCell::new(VoiceChat::new(config)?));

    let input_devices = VoiceChat::list_input_devices().unwrap_or_default();
    let output_devices = VoiceChat::list_output_devices().unwrap_or_default();

    // ── Window ──────────────────────────────────────────────────────
    let mut wind = window::Window::default()
        .with_size(620, 860)
        .with_label("voice-chat-core \u{2014} Stats Dashboard");
    wind.make_resizable(false);

    let mut scroll = group::Scroll::default_fill();
    scroll.set_scrollbar_size(8);

    let mut col = group::Flex::default()
        .with_size(590, 1180)
        .with_pos(10, 10)
        .column();
    col.set_spacing(4);

    // ═══════════════════════════════════════════════════════════════
    //  DEVICES
    // ═══════════════════════════════════════════════════════════════

    helpers::section_header(&mut col, "\u{1f3a7}", "DEVICES");

    helpers::field_label(&mut col, "Input Device");
    let mut input_choice = helpers::choice(&mut col);
    helpers::populate_device_choice(&mut input_choice, &input_devices);

    helpers::field_label(&mut col, "Output Device");
    let mut output_choice = helpers::choice(&mut col);
    helpers::populate_device_choice(&mut output_choice, &output_devices);

    helpers::hint_label(
        &mut col,
        &format!(
            "{} input, {} output devices found",
            input_devices.len(),
            output_devices.len()
        ),
    );
    helpers::separator(&mut col);

    // ═══════════════════════════════════════════════════════════════
    //  INPUT
    // ═══════════════════════════════════════════════════════════════

    helpers::section_header(&mut col, "\u{1f3a4}", "INPUT");

    helpers::field_label(&mut col, "Capture Level (pre-denoise)");
    let input_level = Arc::new(AtomicU32::new(0));
    let _vu_input = vu_meter::make(&mut col, input_level.clone(), 22, theme::SUCCESS);

    helpers::field_label(&mut col, "Input Gain");
    let mut input_gain_slider =
        helpers::slider(&mut col, 0.0, 2.0, 1.0, 0.05, enums::Color::Selection);
    let mut input_gain_lbl = helpers::hint_label(&mut col, "Gain: 100%");

    helpers::separator(&mut col);

    // ═══════════════════════════════════════════════════════════════
    //  OUTPUT
    // ═══════════════════════════════════════════════════════════════

    helpers::section_header(&mut col, "\u{1f50a}", "OUTPUT");

    let output_level = Arc::new(AtomicU32::new(0));
    let _vu_output = vu_meter::make(&mut col, output_level.clone(), 22, theme::CYAN);

    helpers::field_label(&mut col, "Master Volume");
    let mut output_gain_slider =
        helpers::slider(&mut col, 0.0, 2.0, 1.0, 0.05, theme::CYAN);
    let mut output_gain_lbl = helpers::hint_label(&mut col, "Volume: 100%");

    helpers::separator(&mut col);

    // ═══════════════════════════════════════════════════════════════
    //  VOICE GATE
    // ═══════════════════════════════════════════════════════════════

    helpers::section_header(&mut col, "\u{26a1}", "VOICE GATE");

    let mut speaking_lbl = frame::Frame::default()
        .with_label("\u{26ab}  Not transmitting")
        .with_align(enums::Align::Left | enums::Align::Inside);
    speaking_lbl.set_label_color(theme::TEXT_MUTED);
    speaking_lbl.set_label_size(12);
    col.fixed(&speaking_lbl, 22);

    helpers::field_label(&mut col, "Denoised Level vs Threshold");

    let denoised_level = Arc::new(AtomicU32::new(0));
    let threshold: Rc<RefCell<f32>> = Rc::new(RefCell::new(0.020));
    let mut vu_vad = gate_bar::make(&mut col, denoised_level.clone(), threshold.clone(), 28);

    helpers::field_label(&mut col, "Gate Threshold");
    let mut vad_slider =
        helpers::slider(&mut col, 0.005, 0.10, 0.020, 0.001, enums::Color::Selection);
    let mut vad_sens_lbl = helpers::hint_label(&mut col, &helpers::format_sensitivity(0.020));

    let noise_toggle = helpers::check(&mut col, "Noise suppression (RNNoise)", true);
    let mute_toggle = helpers::check(&mut col, "Mute microphone", false);
    let loopback_toggle = helpers::check(&mut col, "Monitor input (loopback)", false);

    helpers::separator(&mut col);

    // ═══════════════════════════════════════════════════════════════
    //  CODEC & BANDWIDTH
    // ═══════════════════════════════════════════════════════════════

    helpers::section_header(&mut col, "\u{1f4ca}", "CODEC & BANDWIDTH");

    let stat_opus = helpers::stat_label(&mut col, "Opus: --");
    let stat_bw = helpers::stat_label(&mut col, "Bandwidth: --");
    let stat_timing = helpers::stat_label(&mut col, "Timing: --");
    let stat_state = helpers::stat_label(&mut col, "State: --");

    helpers::separator(&mut col);

    // ═══════════════════════════════════════════════════════════════
    //  PIPELINE HEALTH
    // ═══════════════════════════════════════════════════════════════

    helpers::section_header(&mut col, "\u{1fa7a}", "PIPELINE HEALTH");

    let stat_packets = helpers::stat_label(&mut col, "Packets: --");
    let stat_loss = helpers::stat_label(&mut col, "Loss: --");
    let stat_recovery = helpers::stat_label(&mut col, "Recovery: --");
    let stat_buffers = helpers::stat_label(&mut col, "Buffers: --");
    let stat_jitter = helpers::stat_label(&mut col, "Jitter buffer: --");
    let stat_uptime = helpers::stat_label(&mut col, "Uptime: --");

    helpers::separator(&mut col);

    helpers::section_header(&mut col, "\u{1f4cb}", "RAW SUMMARY");
    let mut stat_raw = frame::Frame::default()
        .with_label("...")
        .with_align(enums::Align::Left | enums::Align::Inside | enums::Align::Wrap);
    stat_raw.set_label_color(theme::TEXT_MUTED);
    stat_raw.set_label_size(9);
    col.fixed(&stat_raw, 42);

    col.end();
    scroll.end();
    wind.end();
    wind.show();

    // ═══════════════════════════════════════════════════════════════
    //  DEFERRED START — spawn audio threads after the window is up
    // ═══════════════════════════════════════════════════════════════

    let vc_start = vc.clone();
    app::add_timeout3(0.05, move |_| {
        if let Err(e) = vc_start.borrow_mut().start() {
            eprintln!("Failed to start voice engine: {}", e);
        }
    });

    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    //  CALLBACKS — wire controls to the voice engine
    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

    // Device selection
    let input_devs = input_devices.clone();
    let vc_c = vc.clone();
    input_choice.set_callback(move |c| {
        let idx = c.value() as usize;
        let dev = if idx == 0 { None } else { input_devs.get(idx - 1).map(|(n, _)| n.as_str()) };
        if let Err(e) = vc_c.borrow_mut().set_input_device(dev) {
            eprintln!("Input device error: {}", e);
        }
    });

    let output_devs = output_devices.clone();
    let vc_c = vc.clone();
    output_choice.set_callback(move |c| {
        let idx = c.value() as usize;
        let dev = if idx == 0 { None } else { output_devs.get(idx - 1).map(|(n, _)| n.as_str()) };
        if let Err(e) = vc_c.borrow_mut().set_output_device(dev) {
            eprintln!("Output device error: {}", e);
        }
    });

    // Gain sliders
    let vc_c = vc.clone();
    input_gain_slider.set_callback(move |s| {
        let val = s.value() as f32;
        vc_c.borrow().set_input_gain(val);
        input_gain_lbl.set_label(&format!("Gain: {}%", (val * 100.0) as i32));
    });

    let vc_c = vc.clone();
    output_gain_slider.set_callback(move |s| {
        let val = s.value() as f32;
        vc_c.borrow().set_output_gain(val);
        output_gain_lbl.set_label(&format!("Volume: {}%", (val * 100.0) as i32));
    });

    // Gate threshold slider
    let threshold_cb = threshold.clone();
    let vc_c = vc.clone();
    let mut vu_gate = vu_vad.clone(); // clone before the closure moves vu_vad
    vad_slider.set_callback(move |s| {
        let val = s.value() as f32;
        *threshold_cb.borrow_mut() = val;
        vad_sens_lbl.set_label(&helpers::format_sensitivity(s.value()));
        vu_vad.redraw();
        vc_c.borrow().set_gate_threshold(val);
    });

    // Toggles
    let vc_c = vc.clone();
    noise_toggle.clone().set_callback(move |b| {
        vc_c.borrow().set_noise_suppression(b.is_checked());
    });
    let vc_c = vc.clone();
    mute_toggle.clone().set_callback(move |b| {
        vc_c.borrow().set_muted(b.is_checked());
    });
    let vc_c = vc.clone();
    loopback_toggle.clone().set_callback(move |b| {
        vc_c.borrow().set_loopback(b.is_checked());
    });

    wind.set_callback(|w| {
        if app::event() == enums::Event::Close {
            w.hide();
        }
    });

    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    //  TIMER — 8 Hz stat updates
    // ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

    let vc_timer = vc.clone();
    let in_lvl = input_level.clone();
    let out_lvl = output_level.clone();
    let dn_lvl = denoised_level.clone();

    let mut s_opus = stat_opus.clone();
    let mut s_bw = stat_bw.clone();
    let mut s_timing = stat_timing.clone();
    let mut s_state = stat_state.clone();
    let mut s_pkt = stat_packets.clone();
    let mut s_loss = stat_loss.clone();
    let mut s_rec = stat_recovery.clone();
    let mut s_buf = stat_buffers.clone();
    let mut s_jit = stat_jitter.clone();
    let mut s_up = stat_uptime.clone();
    let mut s_raw = stat_raw.clone();
    let mut speak = speaking_lbl.clone();

    let mut vu_in = _vu_input.clone();
    let mut vu_out = _vu_output.clone();

    let prev_sent = Rc::new(RefCell::new(0u64));
    let prev_speak = Rc::new(RefCell::new(false));

    app::add_timeout3(0.125, move |handle| {
        let vc_ref = vc_timer.borrow();
        let stats = vc_ref.stats();

        // ── Levels ──────────────────────────────────────────────
        in_lvl.store(vc_ref.input_level().to_bits(), Ordering::Relaxed);
        out_lvl.store(vc_ref.output_level().to_bits(), Ordering::Relaxed);
        dn_lvl.store(vc_ref.denoised_level().to_bits(), Ordering::Relaxed);
        vu_in.redraw();
        vu_out.redraw();
        vu_gate.redraw();

        // ── Speaking indicator ──────────────────────────────────
        let speaking = vc_ref.is_speaking();
        if speaking != *prev_speak.borrow() {
            *prev_speak.borrow_mut() = speaking;
            if speaking {
                speak.set_label("\u{1f7e2}  SPEAKING \u{2014} transmitting audio");
                speak.set_label_color(theme::SPEAKING);
            } else {
                speak.set_label("\u{26ab}  Not transmitting");
                speak.set_label_color(theme::TEXT_MUTED);
            }
            speak.redraw();
        }

        // ── Codec stats ────────────────────────────────────────
        let last_pkt = stats.last_packet_size.load(Ordering::Relaxed);
        let avg_pkt = f32::from_bits(stats.avg_packet_size.load(Ordering::Relaxed));
        s_opus.set_label(&format!(
            "Opus: {}B last, {:.0}B avg  |  24kbps VBR, 10ms frames",
            last_pkt, avg_pkt,
        ));

        let bw_send = stats.estimated_send_kbps();
        let bw_recv = stats.estimated_recv_kbps();
        let total_sent_kb = stats.bytes_sent.load(Ordering::Relaxed) as f32 / 1024.0;
        let total_recv_kb = stats.bytes_received.load(Ordering::Relaxed) as f32 / 1024.0;
        s_bw.set_label(&format!(
            "BW: \u{2191} {:.1} kbps ({:.0} KB)  \u{2193} {:.1} kbps ({:.0} KB)",
            bw_send, total_sent_kb, bw_recv, total_recv_kb,
        ));

        let enc_us = stats.last_encode_us.load(Ordering::Relaxed);
        let dec_us = stats.last_decode_us.load(Ordering::Relaxed);
        let budget_pct = (enc_us as f32 / 10000.0) * 100.0;
        s_timing.set_label(&format!(
            "Encode: {}\u{00b5}s  Decode: {}\u{00b5}s  (budget: {:.0}% of 10ms)",
            enc_us, dec_us, budget_pct,
        ));

        let dtx = stats.dtx_active.load(Ordering::Relaxed);
        let denoise = stats.denoise_active.load(Ordering::Relaxed);
        let vad_on = stats.vad_active.load(Ordering::Relaxed);
        let gate_on = stats.gate_active.load(Ordering::Relaxed);
        s_state.set_label(&format!(
            "DTX: {}  |  Denoise: {}  |  Gate: {}  |  VAD: {}",
            if dtx { "ON (silence)" } else { "off" },
            if denoise { "ON" } else { "off" },
            if gate_on { "OPEN" } else { "closed" },
            if vad_on { "SPEECH" } else { "quiet" },
        ));

        // ── Pipeline health ────────────────────────────────────
        let sent = stats.packets_sent.load(Ordering::Relaxed);
        let recv = stats.packets_received.load(Ordering::Relaxed);
        let lost = stats.packets_lost.load(Ordering::Relaxed);

        let prev = *prev_sent.borrow();
        let delta = sent.saturating_sub(prev);
        *prev_sent.borrow_mut() = sent;
        let pps = delta as f32 * 8.0;

        let pps_note = if pps < 0.1 && sent > 0 { "  (idle: VAD gating)" } else { "" };
        s_pkt.set_label(&format!(
            "Packets: sent {} recv {} lost {}  |  {:.0} pkt/s{}",
            sent, recv, lost, pps, pps_note,
        ));

        let loss_pct = if recv + lost > 0 {
            (lost as f32 / (recv + lost) as f32) * 100.0
        } else {
            0.0
        };
        s_loss.set_label(&format!("Loss rate: {:.1}%", loss_pct));

        let fec = stats.fec_recoveries.load(Ordering::Relaxed);
        let plc = stats.plc_frames.load(Ordering::Relaxed);
        s_rec.set_label(&format!("FEC recoveries: {}  PLC frames: {}", fec, plc));

        let overruns = stats.input_overruns.load(Ordering::Relaxed);
        let underruns = stats.output_underruns.load(Ordering::Relaxed);
        s_buf.set_label(&format!("Overruns: {}  Underruns: {}", overruns, underruns));

        let jitter_ms = stats.jitter_buffer_occupancy_ms.load(Ordering::Relaxed);
        s_jit.set_label(&format!("Jitter buffer: {} ms", jitter_ms));

        let uptime = stats.start_time.elapsed().as_secs();
        s_up.set_label(&format!("Uptime: {}m {}s", uptime / 60, uptime % 60));

        s_raw.set_label(&vc_ref.stats_summary());

        app::repeat_timeout3(0.125, handle);
    });

    app_handle.run()?;
    Ok(())
}