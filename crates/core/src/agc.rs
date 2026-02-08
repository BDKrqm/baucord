//! Automatic Gain Control for voice input.
//!
//! Simple peak-tracking compressor with slow attack / slower release,
//! tuned for voice chat. Sits after denoise and before VAD in the
//! input pipeline.
//!
//! Goals:
//! - Quiet talkers become audible without manual mic boost
//! - Loud talkers don't clip
//! - Doesn't pump or breathe noticeably on speech
//! - Minimal CPU (runs every frame, ~480 samples at 48 kHz)

/// Real-time AGC processor.
///
/// Tracks the peak envelope of the signal and applies gain to bring
/// it toward a target level. Uses asymmetric attack/release to avoid
/// pumping artifacts on speech transients.
pub struct Agc {
    /// Target RMS level (linear). Speech should hover around this.
    target_level: f32,
    /// Current gain being applied (smoothed).
    current_gain: f32,
    /// Minimum gain (don't amplify more than this — avoids boosting noise floor).
    min_gain: f32,
    /// Maximum gain (don't cut more than this).
    max_gain: f32,
    /// Attack coefficient (fast — respond to loud transients quickly).
    /// Per-sample smoothing factor when gain needs to decrease.
    attack_coeff: f32,
    /// Release coefficient (slow — don't pump on pauses).
    /// Per-sample smoothing factor when gain needs to increase.
    release_coeff: f32,
    /// Smoothed RMS envelope tracker.
    envelope: f32,
    /// Envelope smoothing coefficient.
    envelope_coeff: f32,
    /// Gate threshold — don't adjust gain during silence (avoids boosting noise).
    gate_threshold: f32,
}

impl Agc {
    /// Create a new AGC tuned for voice at 48 kHz.
    ///
    /// - `target_level`: desired output RMS (0.0–1.0). Default 0.15 is
    ///   a good level that leaves headroom for peaks.
    pub fn new(target_level: f32) -> Self {
        // At 48 kHz, 480 samples = 10ms frame
        // attack_time ~5ms → coeff ≈ 1 - e^(-1/(0.005*48000)) ≈ 0.004
        // release_time ~300ms → coeff ≈ 1 - e^(-1/(0.3*48000)) ≈ 0.00007
        // envelope_time ~20ms → coeff ≈ 1 - e^(-1/(0.02*48000)) ≈ 0.001
        Self {
            target_level: target_level.clamp(0.01, 0.5),
            current_gain: 1.0,
            min_gain: 0.1,  // -20 dB max cut
            max_gain: 10.0, // +20 dB max boost
            attack_coeff: 0.004,
            release_coeff: 0.00007,
            envelope: 0.0,
            envelope_coeff: 0.001,
            gate_threshold: 0.005, // ~-46 dBFS — below this is noise floor
        }
    }

    /// Process a frame of audio in-place.
    ///
    /// Call once per 480-sample frame (10ms at 48 kHz).
    pub fn process_frame(&mut self, samples: &mut [f32]) {
        // Compute frame RMS
        let rms = Self::compute_rms(samples);

        // Update envelope (smooth RMS tracker)
        let coeff = if rms > self.envelope {
            // Rising — use faster attack-like tracking
            self.envelope_coeff * 10.0
        } else {
            self.envelope_coeff
        };
        self.envelope = self.envelope * (1.0 - coeff) + rms * coeff;

        // Don't adjust gain during silence / very quiet signal
        if self.envelope < self.gate_threshold {
            // Still apply current gain (don't snap to 1.0)
            for s in samples.iter_mut() {
                *s = (*s * self.current_gain).clamp(-0.99, 0.99);
            }
            return;
        }

        // Compute desired gain
        let desired_gain = (self.target_level / self.envelope).clamp(self.min_gain, self.max_gain);

        // Smooth gain changes (asymmetric: fast attack, slow release)
        // "Attack" = gain decreasing (signal got louder)
        // "Release" = gain increasing (signal got quieter)
        for s in samples.iter_mut() {
            let coeff = if desired_gain < self.current_gain {
                self.attack_coeff
            } else {
                self.release_coeff
            };
            self.current_gain = self.current_gain * (1.0 - coeff) + desired_gain * coeff;
            *s = (*s * self.current_gain).clamp(-0.99, 0.99);
        }
    }

    /// Reset AGC state (e.g., after device switch or unmute).
    pub fn reset(&mut self) {
        self.current_gain = 1.0;
        self.envelope = 0.0;
    }

    fn compute_rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum_sq: f32 = samples.iter().map(|&s| s * s).sum();
        (sum_sq / samples.len() as f32).sqrt()
    }
}
