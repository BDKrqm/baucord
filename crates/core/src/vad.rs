//! Voice Activity Detection using earshot (WebRTC VAD port).
//!
//! Provides hysteresis (separate start/stop thresholds + cooldowns) to
//! prevent rapid on/off toggling that drives users crazy.

use earshot::{VoiceActivityDetector, VoiceActivityProfile};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Real-time VAD processor with hysteresis.
///
/// earshot returns a boolean (speech/no-speech). We wrap it with:
/// - Separate start/stop thresholds derived from a single user-facing threshold
/// - Cooldown timers to prevent flapping
/// - Buffering to handle mismatched frame sizes
pub struct VADProcessor {
    vad: VoiceActivityDetector,
    buffer: Vec<i16>,
    is_speaking: Arc<AtomicBool>,
    last_transition: Instant,

    // Thresholds (updated at runtime via set_threshold)
    // earshot is binary, so we use a confidence accumulator:
    // run N chunks, count how many detect speech, compare ratio to threshold.
    start_threshold: f32,
    stop_threshold: f32,
    start_cooldown: Duration,
    stop_cooldown: Duration,
}

/// The chunk size earshot expects for 48 kHz: 480 samples (10 ms).
const EARSHOT_CHUNK_48K: usize = 480;

impl VADProcessor {
    pub fn new(threshold: f32) -> Self {
        Self {
            vad: VoiceActivityDetector::new(VoiceActivityProfile::AGGRESSIVE),
            buffer: Vec::with_capacity(960),
            is_speaking: Arc::new(AtomicBool::new(false)),
            last_transition: Instant::now(),
            start_threshold: threshold,
            stop_threshold: threshold * 0.5,
            start_cooldown: Duration::from_millis(50),
            stop_cooldown: Duration::from_millis(800),
        }
    }

    /// Update thresholds at runtime.
    pub fn set_threshold(&mut self, threshold: f32) {
        self.start_threshold = threshold;
        self.stop_threshold = threshold * 0.5;
    }

    /// Get a handle to the speaking state for reading from other threads.
    pub fn speaking_handle(&self) -> Arc<AtomicBool> {
        self.is_speaking.clone()
    }

    /// Process f32 audio samples (normalized −1.0..1.0). Returns `true` if speech detected.
    pub fn process(&mut self, samples: &[f32]) -> bool {
        // Convert to i16 for earshot
        let samples_i16: Vec<i16> = samples
            .iter()
            .map(|&x| (x.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();

        self.process_i16(&samples_i16)
    }

    fn process_i16(&mut self, samples: &[i16]) -> bool {
        // Fast path: exact chunk size
        if samples.len() == EARSHOT_CHUNK_48K {
            let detected = self.vad.predict_48khz(samples).unwrap_or(false);
            let confidence = if detected { 1.0 } else { 0.0 };
            return self.update_state(confidence);
        }

        // Accumulate for mismatched sizes
        self.buffer.extend_from_slice(samples);

        let mut total = 0.0f32;
        let mut chunks = 0;

        while self.buffer.len() >= EARSHOT_CHUNK_48K {
            let chunk: Vec<i16> = self.buffer.drain(..EARSHOT_CHUNK_48K).collect();
            let detected = self.vad.predict_48khz(&chunk).unwrap_or(false);
            total += if detected { 1.0 } else { 0.0 };
            chunks += 1;
        }

        if chunks > 0 {
            self.update_state(total / chunks as f32)
        } else {
            self.is_speaking.load(Ordering::Relaxed)
        }
    }

    fn update_state(&mut self, confidence: f32) -> bool {
        let was_speaking = self.is_speaking.load(Ordering::Relaxed);
        let elapsed = self.last_transition.elapsed();

        let should_speak = if was_speaking {
            confidence > self.stop_threshold
        } else {
            confidence > self.start_threshold
        };

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
