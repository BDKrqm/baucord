// voice/vad.rs - Modular Voice Activity Detection interface (updated for new architecture)

use log::info;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    Quality,        // Least aggressive
    LowBitRate,
    Aggressive,
    VeryAggressive, // Most aggressive
}

impl Default for VADConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000,  // Changed to 48kHz default for Opus
            chunk_size: 480,     // 10ms at 48kHz
            threshold: 0.15,     // Lower threshold for better sensitivity
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
// Simplified VAD Processor for RT-safe operation
// ============================================================================

pub struct VADProcessor {
    vad: Box<dyn VoiceActivityDetector>,
    buffer: Vec<i16>,
    is_speaking: Arc<AtomicBool>,
    last_transition: Instant,
    threshold: f32,
    
    // Hysteresis parameters for smoother transitions
    start_threshold: f32,
    stop_threshold: f32,
    start_cooldown_ms: u64,
    stop_cooldown_ms: u64,
}

impl VADProcessor {
    pub fn new(config: VADConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let threshold = config.threshold;
        let chunk_size = config.chunk_size;

        // For low latency, use shorter cooldowns
        let (start_cooldown_ms, stop_cooldown_ms) = if chunk_size <= 480 {
            (50, 800)   // Quick to start, slow to stop for 10ms frames
        } else {
            (100, 1000) // Standard for 20ms frames
        };

        let vad = create_vad(config)?;

        Ok(Self {
            vad,
            buffer: Vec::with_capacity(chunk_size * 2),
            is_speaking: Arc::new(AtomicBool::new(false)),
            last_transition: Instant::now(),
            threshold,
            start_threshold: threshold,
            stop_threshold: threshold * 0.5, // Lower threshold to stop
            start_cooldown_ms,
            stop_cooldown_ms,
        })
    }

    /// Process audio samples and return voice activity status
    pub fn process_audio(&mut self, samples: &[i16]) -> bool {
        let chunk_size = self.vad.chunk_size();

        // Direct processing for exact chunk size (common case)
        if samples.len() == chunk_size {
            let probability = self.vad.process(samples);
            return self.update_speaking_state(probability);
        }

        // Buffer handling for mismatched sizes
        self.buffer.extend_from_slice(samples);

        let mut speech_confidence = 0.0f32;
        let mut chunks_processed = 0;

        while self.buffer.len() >= chunk_size {
            let chunk: Vec<i16> = self.buffer.drain(..chunk_size).collect();

            let probability = self.vad.process(&chunk);
            speech_confidence += probability;
            chunks_processed += 1;
        }

        if chunks_processed > 0 {
            speech_confidence /= chunks_processed as f32;
            return self.update_speaking_state(speech_confidence);
        }

        self.is_speaking.load(Ordering::Relaxed)
    }

    /// Process f32 audio samples (convenience method)
    pub fn process_audio_f32(&mut self, samples: &[f32]) -> bool {
        // Convert f32 to i16
        let samples_i16: Vec<i16> = samples
            .iter()
            .map(|&x| (x.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();
        
        self.process_audio(&samples_i16)
    }

    fn update_speaking_state(&mut self, confidence: f32) -> bool {
        let was_speaking = self.is_speaking.load(Ordering::Relaxed);
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_transition);

        // Determine if we should be speaking based on thresholds
        let should_speak = if was_speaking {
            // Currently speaking - use lower threshold to continue
            confidence > self.stop_threshold
        } else {
            // Currently silent - use higher threshold to start
            confidence > self.start_threshold
        };

        // Apply cooldown to prevent rapid transitions
        let required_cooldown = if was_speaking && !should_speak {
            // Stopping: require longer cooldown to prevent cutting off
            Duration::from_millis(self.stop_cooldown_ms)
        } else if !was_speaking && should_speak {
            // Starting: allow quick start
            Duration::from_millis(self.start_cooldown_ms)
        } else {
            // No change
            Duration::from_millis(0)
        };

        // Only transition if cooldown period has passed
        if should_speak != was_speaking && elapsed > required_cooldown {
            self.is_speaking.store(should_speak, Ordering::Relaxed);
            self.last_transition = now;

            if should_speak {
                info!("🎤 VAD: Speaking started (confidence: {:.2})", confidence);
            } else {
                info!("🔇 VAD: Speaking stopped");
            }
        }

        self.is_speaking.load(Ordering::Relaxed)
    }

    pub fn is_speaking(&self) -> bool {
        self.is_speaking.load(Ordering::Relaxed)
    }

    pub fn reset(&mut self) {
        self.vad.reset();
        self.buffer.clear();
        self.is_speaking.store(false, Ordering::Relaxed);
        self.last_transition = Instant::now();
    }

    /// Get a clone of the speaking state for sharing across threads
    pub fn get_speaking_handle(&self) -> Arc<AtomicBool> {
        self.is_speaking.clone()
    }
}

// ============================================================================
// Tests (point 15)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vad_hysteresis() {
        let config = VADConfig {
            sample_rate: 48000,
            chunk_size: 480,
            threshold: 0.5,
            profile: VADProfile::Aggressive,
        };
        
        let mut processor = VADProcessor::new(config).unwrap();
        
        // Generate test samples (silence)
        let silence = vec![0i16; 480];
        assert!(!processor.process_audio(&silence));
        
        // Generate test samples (noise)
        let noise: Vec<i16> = (0..480)
            .map(|i| ((i as f32 * 0.1).sin() * 1000.0) as i16)
            .collect();
        
        // Process multiple times to test state transitions
        for _ in 0..10 {
            processor.process_audio(&noise);
        }
    }

    #[test]
    fn test_buffer_accumulation() {
        let config = VADConfig::default();
        let mut processor = VADProcessor::new(config).unwrap();
        
        // Send partial frames
        let partial = vec![0i16; 100];
        processor.process_audio(&partial);
        processor.process_audio(&partial);
        processor.process_audio(&partial);
        processor.process_audio(&partial);
        processor.process_audio(&partial); // Total: 500 samples
        
        // Should have processed one full frame of 480 samples
        assert_eq!(processor.buffer.len(), 20); // 500 - 480
    }
}