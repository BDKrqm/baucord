//! Latency profiling for the voice-chat audio pipeline.
//!
//! Measures ACTUAL pipeline latency by detecting impulse propagation:
//! when pipeline_level spikes (input detected sound), start a timer.
//! When output_level spikes (output played it back via loopback), stop the timer.
//! The delta is the real, measured pipeline latency — no human reaction time.
//!
//! Run with: cargo run --example latency_profile
//!
//! How to use:
//! 1. Run the example (it enables loopback automatically)
//! 2. Make a sharp loud noise (snap fingers, clap, tap the mic)
//! 3. The profiler detects the impulse traveling through the pipeline
//!    and prints the measured latency
//! 4. Wait 1-2 seconds between snaps for clean measurements

use core::{AudioStats, VoiceChat, VoiceChatConfig, FRAME_SIZE};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Minimum level to consider an "impulse" on the input side.
/// This needs to be high enough to ignore background noise but low
/// enough to catch a clap/snap. 0.15 works well for most mics.
const INPUT_IMPULSE_THRESHOLD: f32 = 0.15;

/// Minimum level to consider the impulse arrived at output.
/// Lower than input because loopback + mixing + gain may attenuate.
const OUTPUT_IMPULSE_THRESHOLD: f32 = 0.02;

/// Cooldown between measurements to avoid double-triggers.
const MEASUREMENT_COOLDOWN: Duration = Duration::from_millis(1500);

/// How fast we poll the level atomics. 0.5ms gives us ±0.5ms accuracy.
const POLL_INTERVAL: Duration = Duration::from_micros(500);

/// Maximum time to wait for output impulse after input detection.
/// If exceeded, the measurement is discarded (sound didn't make it through).
const MAX_IMPULSE_WAIT: Duration = Duration::from_millis(500);

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║       Audio Pipeline Latency Profiler v2               ║");
    println!("╠══════════════════════════════════════════════════════════╣");
    println!("║ Measures REAL pipeline latency via impulse detection.   ║");
    println!("║                                                        ║");
    println!("║ 1. Make a SHARP loud noise (clap, snap, tap the mic)   ║");
    println!("║ 2. Wait for the measurement to appear                  ║");
    println!("║ 3. Repeat — wait ~2s between each snap                 ║");
    println!("║                                                        ║");
    println!("║ Press Ctrl+C to stop.                                  ║");
    println!("╚══════════════════════════════════════════════════════════╝");
    println!();

    let config = VoiceChatConfig {
        noise_suppression: true,
        gate_threshold: 0.005, // very low — don't want the gate to eat our impulse
        ..Default::default()
    };

    let mut vc = VoiceChat::new(config).expect("Failed to create VoiceChat");
    vc.start().expect("Failed to start VoiceChat");
    vc.set_loopback(true);

    let stats = vc.stats().clone();

    println!("Pipeline started. Loopback enabled.\n");
    println!("Waiting for impulses... make a sharp noise!\n");

    // ── Measurement state ───────────────────────────────────────────
    let mut measurements: Vec<f64> = Vec::new();
    let mut state = MeasureState::WaitingForInput;
    let mut input_detected_at = Instant::now();
    let mut input_peak: f32 = 0.0;
    let mut last_measurement = Instant::now() - MEASUREMENT_COOLDOWN; // allow immediate first

    // ── Also print periodic stats ───────────────────────────────────
    let mut last_stats_print = Instant::now();
    let start = Instant::now();

    loop {
        std::thread::sleep(POLL_INTERVAL);

        let pipeline_lvl = f32::from_bits(stats.pipeline_level.load(Ordering::Relaxed));
        let output_lvl = f32::from_bits(stats.output_level.load(Ordering::Relaxed));

        match state {
            MeasureState::WaitingForInput => {
                // Check cooldown
                if last_measurement.elapsed() < MEASUREMENT_COOLDOWN {
                    continue;
                }

                // Detect input impulse
                if pipeline_lvl >= INPUT_IMPULSE_THRESHOLD {
                    input_detected_at = Instant::now();
                    input_peak = pipeline_lvl;
                    state = MeasureState::WaitingForOutput;
                }
            }

            MeasureState::WaitingForOutput => {
                // Track rising input (the impulse may span multiple polls)
                if pipeline_lvl > input_peak {
                    input_peak = pipeline_lvl;
                }

                // Detect output impulse
                if output_lvl >= OUTPUT_IMPULSE_THRESHOLD {
                    let latency = input_detected_at.elapsed();
                    let latency_ms = latency.as_secs_f64() * 1000.0;

                    measurements.push(latency_ms);
                    let count = measurements.len();

                    // Compute running stats
                    let avg = measurements.iter().sum::<f64>() / count as f64;
                    let min = measurements.iter().cloned().fold(f64::MAX, f64::min);
                    let max = measurements.iter().cloned().fold(0.0f64, f64::max);

                    // Compute p50 (median)
                    let mut sorted = measurements.clone();
                    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    let p50 = if count % 2 == 0 && count >= 2 {
                        (sorted[count / 2 - 1] + sorted[count / 2]) / 2.0
                    } else {
                        sorted[count / 2]
                    };

                    println!(
                        "  🎯 #{:<3} Latency: {:.1}ms  (in={:.3} out={:.3})  │  avg={:.1}ms  min={:.1}ms  max={:.1}ms  p50={:.1}ms  [n={}]",
                        count,
                        latency_ms,
                        input_peak,
                        output_lvl,
                        avg,
                        min,
                        max,
                        p50,
                        count,
                    );

                    last_measurement = Instant::now();
                    state = MeasureState::WaitingForInput;
                }

                // Timeout — impulse didn't make it through
                if input_detected_at.elapsed() > MAX_IMPULSE_WAIT {
                    println!(
                        "  ⏱️  Timeout: input impulse detected (level={:.3}) but no output after {}ms — discarded",
                        input_peak,
                        MAX_IMPULSE_WAIT.as_millis(),
                    );
                    last_measurement = Instant::now();
                    state = MeasureState::WaitingForInput;
                }
            }
        }

        // ── Periodic status (every 5s) ──────────────────────────────
        if last_stats_print.elapsed() >= Duration::from_secs(5) {
            last_stats_print = Instant::now();
            let elapsed = start.elapsed().as_secs();
            let jitter_ms = stats.jitter_buffer_occupancy_ms.load(Ordering::Relaxed);
            let underruns = stats.output_underruns.load(Ordering::Relaxed);
            let overruns = stats.input_overruns.load(Ordering::Relaxed);
            let speaking = stats.vad_active.load(Ordering::Relaxed);
            let gate = stats.gate_active.load(Ordering::Relaxed);

            println!(
                "  ── {}s │ jitter_buf={}ms │ underruns={} │ overruns={} │ gate={} │ vad={} │ in={:.3} │ out={:.3} ──",
                elapsed,
                jitter_ms,
                underruns,
                overruns,
                if gate { "OPEN" } else { "shut" },
                if speaking { "YES" } else { "no" },
                pipeline_lvl,
                output_lvl,
            );

            if measurements.is_empty() {
                println!("  💡 No measurements yet — try clapping or tapping your mic!");
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum MeasureState {
    /// Waiting for a loud impulse to appear on the input pipeline.
    WaitingForInput,
    /// Input impulse detected — waiting for it to appear on output.
    WaitingForOutput,
}
