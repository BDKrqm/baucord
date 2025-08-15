// vad.rs - Modular Voice Activity Detection interface

use crate::voice::VoicePacket;
use log::info;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

/// Generic VAD trait for easy swapping between implementations
pub trait VoiceActivityDetector: Send + Sync {
    /// Initialize the VAD with a specific configuration
    fn new(config: VADConfig) -> Result<Self, Box<dyn std::error::Error>>
    where
        Self: Sized;

    /// Process audio samples and return voice activity probability
    fn process(&mut self, samples: &[i16]) -> f32;

    /// Reset internal state
    fn reset(&mut self);

    /// Get the expected sample rate
    fn sample_rate(&self) -> u32;

    /// Get the expected chunk size
    fn chunk_size(&self) -> usize;
}

#[derive(Debug, Clone)]
pub struct VADConfig {
    pub sample_rate: u32,
    pub chunk_size: usize,
    pub threshold: f32,
    pub profile: VADProfile,
}

#[derive(Debug, Clone)]
pub enum VADProfile {
    Quality, // Least aggressive
    LowBitRate,
    Aggressive,
    VeryAggressive, // Most aggressive
}

impl Default for VADConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16000,
            chunk_size: 512,
            threshold: 0.5,
            profile: VADProfile::Aggressive,
        }
    }
}

// ============================================================================
// Earshot VAD Implementation
// ============================================================================

#[cfg(feature = "earshot")]
pub struct EarshotVAD {
    detector: earshot::VoiceActivityDetector,
    config: VADConfig,
    downsample_buffer: Vec<i16>,
    downsample_state: [i32; 2],
}

#[cfg(feature = "earshot")]
impl VoiceActivityDetector for EarshotVAD {
    fn new(config: VADConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let profile = match config.profile {
            VADProfile::Quality => earshot::VoiceActivityProfile::QUALITY,
            VADProfile::LowBitRate => earshot::VoiceActivityProfile::LBR,
            VADProfile::Aggressive => earshot::VoiceActivityProfile::AGGRESSIVE,
            VADProfile::VeryAggressive => earshot::VoiceActivityProfile::VERY_AGGRESSIVE,
        };

        let detector = earshot::VoiceActivityDetector::new(profile);

        Ok(Self {
            detector,
            config,
            downsample_buffer: Vec::with_capacity(512),
            downsample_state: [0; 2],
        })
    }

    fn process(&mut self, samples: &[i16]) -> f32 {
        // Earshot expects specific frame sizes
        // For 10ms frames at 48kHz (480 samples), we need to handle this specially

        let result = match self.config.sample_rate {
            8000 => {
                // 10ms = 80 samples, 20ms = 160 samples
                if samples.len() == 80 || samples.len() == 160 {
                    self.detector.predict_8khz(samples).unwrap_or(false)
                } else {
                    false
                }
            }
            16000 => {
                // 10ms = 160 samples, 20ms = 320 samples
                if samples.len() == 160 || samples.len() == 320 {
                    self.detector.predict_16khz(samples).unwrap_or(false)
                } else {
                    false
                }
            }
            32000 => {
                // 10ms = 320 samples, 20ms = 640 samples
                if samples.len() == 320 || samples.len() == 640 {
                    self.detector.predict_32khz(samples).unwrap_or(false)
                } else {
                    false
                }
            }
            48000 => {
                // 10ms = 480 samples, 20ms = 960 samples
                // Earshot accepts 480, 960, or 1440 samples at 48kHz
                if samples.len() == 480 || samples.len() == 960 {
                    self.detector.predict_48khz(samples).unwrap_or(false)
                } else {
                    false
                }
            }
            _ => false,
        };

        if result { 1.0 } else { 0.0 }
    }

    fn reset(&mut self) {
        self.detector.reset();
        self.downsample_state = [0; 2];
    }

    fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    fn chunk_size(&self) -> usize {
        // Return the actual chunk size from config (could be 10ms or 20ms)
        self.config.chunk_size
    }
}

// ============================================================================
// Silero VAD Implementation
// ============================================================================

#[cfg(feature = "silero")]
pub struct SileroVAD {
    detector: voice_activity_detector::VoiceActivityDetector,
    config: VADConfig,
}

#[cfg(feature = "silero")]
impl VoiceActivityDetector for SileroVAD {
    fn new(config: VADConfig) -> Result<Self, Box<dyn std::error::Error>> {
        // Silero VAD requires 8kHz or 16kHz
        let sample_rate = if config.sample_rate >= 16000 {
            16000
        } else {
            8000
        };

        let detector = voice_activity_detector::VoiceActivityDetector::builder()
            .sample_rate(sample_rate)
            .chunk_size(config.chunk_size)
            .build()?;

        Ok(Self { detector, config })
    }

    fn process(&mut self, samples: &[i16]) -> f32 {
        self.detector.predict(samples.iter().copied())
    }

    fn reset(&mut self) {
        self.detector.reset();
    }

    fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    fn chunk_size(&self) -> usize {
        self.config.chunk_size
    }
}

// ============================================================================
// Factory function to create VAD based on features
// ============================================================================

pub fn create_vad(
    config: VADConfig,
) -> Result<Box<dyn VoiceActivityDetector>, Box<dyn std::error::Error>> {
    #[cfg(feature = "earshot")]
    {
        Ok(Box::new(EarshotVAD::new(config)?))
    }

    #[cfg(all(feature = "silero", not(feature = "earshot")))]
    {
        Ok(Box::new(SileroVAD::new(config)?))
    }

    #[cfg(not(any(feature = "earshot", feature = "silero")))]
    {
        Err("No VAD implementation enabled. Enable either 'earshot' or 'silero' feature.".into())
    }
}

// ============================================================================
// Optimized VAD Processor for Low Latency
// ============================================================================

pub struct VADProcessor {
    vad: Box<dyn VoiceActivityDetector>,
    buffer: Vec<i16>,
    is_speaking: Arc<Mutex<bool>>,
    cooldown: Arc<Mutex<std::time::Instant>>,
    threshold: f32,
    // For low latency, we might want faster response
    cooldown_ms: u64,
    outgoing_tx: Option<mpsc::Sender<VoicePacket>>,
}

impl VADProcessor {
    pub fn new(config: VADConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let threshold = config.threshold;
        let chunk_size = config.chunk_size; // Use the actual configured chunk size

        // For low latency, use shorter cooldown
        let cooldown_ms = if chunk_size <= 480 {
            150 // Faster response for 10ms frames
        } else {
            300 // Standard for 20ms frames
        };

        let vad = create_vad(config)?;

        Ok(Self {
            vad,
            buffer: Vec::with_capacity(chunk_size * 2),
            is_speaking: Arc::new(Mutex::new(false)),
            cooldown: Arc::new(Mutex::new(std::time::Instant::now())),
            threshold,
            cooldown_ms,
            outgoing_tx: None,
        })
    }

    pub fn process_audio(&mut self, samples: &[i16]) -> bool {
        // For low latency, process immediately if we have a full chunk
        let chunk_size = self.vad.chunk_size();

        // Direct processing for exact chunk size (common case)
        if samples.len() == chunk_size {
            let probability = self.vad.process(samples);
            return self.update_speaking_state(probability);
        }

        // Buffer handling for mismatched sizes
        self.buffer.extend_from_slice(samples);

        let mut any_speech = false;
        let mut speech_confidence = 0.0f32;
        let mut chunks_processed = 0;

        while self.buffer.len() >= chunk_size {
            let chunk: Vec<i16> = self.buffer.drain(..chunk_size).collect();

            let probability = self.vad.process(&chunk);
            speech_confidence += probability;
            chunks_processed += 1;

            any_speech = any_speech || (probability > self.threshold);
        }

        if chunks_processed > 0 {
            speech_confidence /= chunks_processed as f32;
            return self.update_speaking_state(speech_confidence);
        }

        *self.is_speaking.lock().unwrap()
    }

    pub fn set_sender(&mut self, tx: mpsc::Sender<VoicePacket>) {
        self.outgoing_tx = Some(tx);
    }

    fn update_speaking_state(&mut self, confidence: f32) -> bool {
        let was_speaking = *self.is_speaking.lock().unwrap();
        let mut cooldown = self.cooldown.lock().unwrap();
        let now = std::time::Instant::now();

        let start_threshold = self.threshold;
        let stop_threshold = self.threshold * 0.5; // Lower stop threshold (was 0.7)

        let now_speaking = if was_speaking {
            confidence > stop_threshold
        } else {
            confidence > start_threshold
        };

        // Different cooldowns for different transitions
        let required_cooldown = if was_speaking && !now_speaking {
            // Stopping: require longer cooldown to prevent cutting off
            std::time::Duration::from_millis(800) // Wait longer before stopping
        } else if !was_speaking && now_speaking {
            // Starting: allow quick start
            std::time::Duration::from_millis(50) // Quick response to start
        } else {
            std::time::Duration::from_millis(0) // No change, no cooldown
        };

        if now.duration_since(*cooldown) > required_cooldown {
            if was_speaking != now_speaking {
                *self.is_speaking.lock().unwrap() = now_speaking;
                *cooldown = now;

                // Send the speaking state change event
                if let Some(ref tx) = self.outgoing_tx {
                    let _ = tx.send(VoicePacket::Speaking {
                        client_id: 0,
                        is_speaking: now_speaking,
                    });
                }

                if now_speaking {
                    info!("🎤 VAD: Speaking (confidence: {:.2})", confidence);
                } else {
                    info!("🔇 VAD: Silent");
                }
            }
        }

        *self.is_speaking.lock().unwrap()
    }

    pub fn is_speaking(&self) -> bool {
        *self.is_speaking.lock().unwrap()
    }

    pub fn reset(&mut self) {
        self.vad.reset();
        self.buffer.clear();
        *self.is_speaking.lock().unwrap() = false;
    }
}
