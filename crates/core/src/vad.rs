//! Voice Activity Detection using earshot (WebRTC VAD port).
//!
//! This module provides the **speech classifier** stage of VAD.
//! The cheap energy gate lives in `input.rs` and runs first to skip
//! all expensive processing on silent frames. earshot runs second
//! to distinguish speech from non-speech noise (keyboard, fan bursts).
//!
//! Hysteresis (cooldowns) prevents rapid on/off toggling.

use earshot::{VoiceActivityDetector, VoiceActivityProfile};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Real-time speech classifier wrapping earshot (WebRTC VAD port).
///
/// earshot returns binary speech/no-speech decisions. We wrap it with
/// cooldown timers to prevent flapping. The energy-based threshold
/// gating happens upstream in the input pipeline, not here.
pub struct VADProcessor {
    vad: VoiceActivityDetector,
    buffer: Vec<i16>,
    is_speaking: Arc<AtomicBool>,
    last_transition: Instant,

    start_cooldown: Duration,
    stop_cooldown: Duration,
}

/// The chunk size earshot expects for 48 kHz: 480 samples (10 ms).
const EARSHOT_CHUNK_48K: usize = 480;

impl VADProcessor {
    pub fn new() -> Self {
        Self {
            vad: VoiceActivityDetector::new(VoiceActivityProfile::AGGRESSIVE),
            buffer: Vec::with_capacity(960),
            is_speaking: Arc::new(AtomicBool::new(false)),
            last_transition: Instant::now(),
            start_cooldown: Duration::from_millis(50),
            stop_cooldown: Duration::from_millis(800),
        }
    }

    /// Get a handle to the speaking state for reading from other threads.
    pub fn speaking_handle(&self) -> Arc<AtomicBool> {
        self.is_speaking.clone()
    }

    /// Classify whether a frame contains speech. Returns `true` if speech detected.
    ///
    /// Only call this on frames that already passed the energy gate —
    /// earshot is more expensive than a simple RMS check.
    pub fn process(&mut self, samples: &[f32]) -> bool {
        let detected = self.run_earshot(samples);
        self.update_state(detected)
    }

    /// Run earshot on f32 samples, handling chunk size mismatches.
    fn run_earshot(&mut self, samples: &[f32]) -> bool {
        // Convert to i16 for earshot
        let samples_i16: Vec<i16> = samples
            .iter()
            .map(|&x| (x.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();

        // Fast path: exact chunk size (480 samples = our frame size)
        if samples_i16.len() == EARSHOT_CHUNK_48K {
            return self.vad.predict_48khz(&samples_i16).unwrap_or(false);
        }

        // Accumulate for mismatched sizes
        self.buffer.extend_from_slice(&samples_i16);

        let mut detected_any = false;
        let mut chunks = 0;

        while self.buffer.len() >= EARSHOT_CHUNK_48K {
            let chunk: Vec<i16> = self.buffer.drain(..EARSHOT_CHUNK_48K).collect();
            if self.vad.predict_48khz(&chunk).unwrap_or(false) {
                detected_any = true;
            }
            chunks += 1;
        }

        if chunks > 0 {
            // Any chunk detecting speech = speech (conservative: don't
            // miss speech onset even if only one chunk catches it)
            detected_any
        } else {
            // Not enough data yet — hold previous state
            self.is_speaking.load(Ordering::Relaxed)
        }
    }

    fn update_state(&mut self, should_speak: bool) -> bool {
        let was_speaking = self.is_speaking.load(Ordering::Relaxed);
        let elapsed = self.last_transition.elapsed();

        let cooldown = if was_speaking && !should_speak {
            self.stop_cooldown
        } else if !was_speaking && should_speak {
            self.start_cooldown
        } else {
            Duration::ZERO
        };

        if should_speak != was_speaking && elapsed > cooldown {
            self.is_speaking.store(should_speak, Ordering::Relaxed);
            self.last_transition = Instant::now();
        }

        self.is_speaking.load(Ordering::Relaxed)
    }
}