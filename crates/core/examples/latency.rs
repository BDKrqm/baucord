//! Latency profiling for the voice-chat audio pipeline.
//!
//! Measures time spent at each stage by injecting timestamps through the pipeline.
//! Run with: cargo run --example latency_profile
//!
//! This example uses loopback mode: mic → encode → (skip network) → decode → speaker.
//! It prints per-stage timing every second so you can see exactly where latency hides.

use core::{AudioStats, VoiceChat, VoiceChatConfig, FRAME_SIZE};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    println!("╔══════════════════════════════════════════════════════╗");
    println!("║       Audio Pipeline Latency Profiler               ║");
    println!("╠══════════════════════════════════════════════════════╣");
    println!("║ This measures where time is spent in the pipeline.  ║");
    println!("║ Speak into your mic — latency is printed per stage. ║");
    println!("║ Press Ctrl+C to stop.                               ║");
    println!("╚══════════════════════════════════════════════════════╝");
    println!();

    // ── Build with loopback so we measure the full local round-trip ──
    let config = VoiceChatConfig {
        noise_suppression: true,
        gate_threshold: 0.008, // low threshold to trigger easily
        ..Default::default()
    };

    let mut vc = VoiceChat::new(config).expect("Failed to create VoiceChat");
    vc.start().expect("Failed to start VoiceChat");
    vc.set_loopback(true);

    let stats = vc.stats().clone();

    println!("Pipeline started. Loopback enabled.");
    println!("Speak to see latency measurements.\n");
    println!(
        "{:<10} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>10} {:>6} {:>6}",
        "Time",
        "Enc(µs)",
        "Dec(µs)",
        "JitBuf",
        "Pkts↑",
        "Pkts↓",
        "Under",
        "Speaking",
        "Gate",
        "VAD"
    );
    println!("{}", "-".repeat(100));

    let start = Instant::now();
    let mut last_sent = 0u64;
    let mut last_recv = 0u64;

    loop {
        std::thread::sleep(Duration::from_secs(1));

        let elapsed = start.elapsed().as_secs();
        let enc_us = stats.last_encode_us.load(Ordering::Relaxed);
        let dec_us = stats.last_decode_us.load(Ordering::Relaxed);
        let jitter_ms = stats.jitter_buffer_occupancy_ms.load(Ordering::Relaxed);
        let sent = stats.packets_sent.load(Ordering::Relaxed);
        let recv = stats.packets_received.load(Ordering::Relaxed);
        let underruns = stats.output_underruns.load(Ordering::Relaxed);
        let overruns = stats.input_overruns.load(Ordering::Relaxed);
        let speaking = stats.vad_active.load(Ordering::Relaxed);
        let gate = stats.gate_active.load(Ordering::Relaxed);
        let denoise = stats.denoise_active.load(Ordering::Relaxed);
        let dtx = stats.dtx_active.load(Ordering::Relaxed);

        let denoised_lvl = f32::from_bits(stats.denoised_level.load(Ordering::Relaxed));
        let pipeline_lvl = f32::from_bits(stats.pipeline_level.load(Ordering::Relaxed));
        let output_lvl = f32::from_bits(stats.output_level.load(Ordering::Relaxed));

        let pkt_rate_sent = sent - last_sent;
        let pkt_rate_recv = recv - last_recv;
        last_sent = sent;
        last_recv = recv;

        let speak_str = if speaking { "SPEAK" } else { "quiet" };
        let gate_str = if gate { "OPEN" } else { "shut" };
        let vad_str = if speaking { "YES" } else { "no" };

        println!(
            "{:<10} {:>8} {:>8} {:>6}ms {:>8} {:>8} {:>6} {:>10} {:>6} {:>6}",
            format!("{}s", elapsed),
            enc_us,
            dec_us,
            jitter_ms,
            pkt_rate_sent,
            pkt_rate_recv,
            underruns,
            speak_str,
            gate_str,
            vad_str,
        );

        // Detailed level info when speaking
        if speaking || gate {
            println!(
                "           Levels: denoised={:.3} pipeline={:.3} output={:.3} | BW: ↑{:.1}kbps ↓{:.1}kbps | Pkt: {}B (avg {:.0}B)",
                denoised_lvl,
                pipeline_lvl,
                output_lvl,
                stats.estimated_send_kbps(),
                stats.estimated_recv_kbps(),
                stats.last_packet_size.load(Ordering::Relaxed),
                f32::from_bits(stats.avg_packet_size.load(Ordering::Relaxed)),
            );
        }

        // Print overrun warnings
        if overruns > 0 {
            println!(
                "  ⚠️  Input overruns: {} (ring buffer full — input worker can't keep up)",
                overruns
            );
        }

        // Latency budget estimate
        if speaking {
            // These are the measurable components.
            // cpal buffer sizes are not directly observable but we estimate.
            let encode_ms = enc_us as f32 / 1000.0;
            let decode_ms = dec_us as f32 / 1000.0;
            let jitter_ms_f = jitter_ms as f32;

            // Frame accumulation: 10ms (we need 480 samples @ 48kHz)
            let frame_accum_ms = 10.0;
            // Output worker tick: avg 5ms (worst 10ms from the sleep loop)
            let output_tick_ms = 5.0;
            // cpal buffers (estimated): 10-20ms each side
            let cpal_input_est = 15.0;
            let cpal_output_est = 15.0;

            let total_est = cpal_input_est
                + frame_accum_ms
                + encode_ms
                + 1.0  // network (LAN)
                + decode_ms
                + output_tick_ms
                + jitter_ms_f
                + cpal_output_est;

            println!(
                "  📊 Latency budget: cpal_in≈{:.0}ms + frame={:.0}ms + enc={:.1}ms + net≈1ms + dec={:.1}ms + tick≈{:.0}ms + jitter={:.0}ms + cpal_out≈{:.0}ms = ~{:.0}ms",
                cpal_input_est,
                frame_accum_ms,
                encode_ms,
                decode_ms,
                output_tick_ms,
                jitter_ms_f,
                cpal_output_est,
                total_est,
            );
        }
    }
}
