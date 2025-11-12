// voice/resampler.rs - High-quality resampling with rubato
use log::{info, warn};
use rubato::{
    FastFixedOut, Resampler, SincFixedIn, SincFixedOut, 
    SincInterpolationParameters, SincInterpolationType, WindowFunction
};

// ============================================================================
// Core Configuration
// ============================================================================

/// Standard parameters for high-quality resampling
#[derive(Debug)]
pub struct ResamplerParams {
    pub sinc_len: usize,
    pub f_cutoff: f32,
    pub interpolation: SincInterpolationType,
    pub oversampling_factor: usize,
    pub window: WindowFunction,
}

impl ResamplerParams {
    /// High quality settings for audio resampling
    pub fn high_quality() -> Self {
        Self {
            sinc_len: 64,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 128,
            window: WindowFunction::BlackmanHarris2,
        }
    }

    /// Lower quality but faster settings
    pub fn low_latency() -> Self {
        Self {
            sinc_len: 32,
            f_cutoff: 0.90,
            interpolation: SincInterpolationType::Nearest,
            oversampling_factor: 64,
            window: WindowFunction::Blackman,
        }
    }

    fn to_sinc_params(&self) -> SincInterpolationParameters {
        SincInterpolationParameters {
            sinc_len: self.sinc_len,
            f_cutoff: self.f_cutoff,
            interpolation: match self.interpolation {
                SincInterpolationType::Cubic => SincInterpolationType::Cubic,
                SincInterpolationType::Quadratic => SincInterpolationType::Quadratic,
                SincInterpolationType::Linear => SincInterpolationType::Linear,
                SincInterpolationType::Nearest => SincInterpolationType::Nearest,
            },
            oversampling_factor: self.oversampling_factor,
            window: match self.window {
                WindowFunction::Hann => WindowFunction::Hann,
                WindowFunction::Hann2 => WindowFunction::Hann2,
                WindowFunction::Blackman => WindowFunction::Blackman,
                WindowFunction::Blackman2 => WindowFunction::Blackman2,
                WindowFunction::BlackmanHarris => WindowFunction::BlackmanHarris,
                WindowFunction::BlackmanHarris2 => WindowFunction::BlackmanHarris2,
            },
        }
    }
}

impl Default for ResamplerParams {
    fn default() -> Self {
        Self::high_quality()
    }
}

// ============================================================================
// Output Resampler Wrapper
// ============================================================================

/// Wrapper enum for different output resampler types
pub enum ResamplerOut {
    SincFixed(SincFixedOut<f32>),
    FastFixed(FastFixedOut<f32>),
    None, // For passthrough when no resampling needed
}

impl ResamplerOut {
    /// Get the number of input frames needed for next process call
    pub fn input_frames_next(&self) -> usize {
        match self {
            ResamplerOut::SincFixed(r) => r.input_frames_next(),
            ResamplerOut::FastFixed(r) => r.input_frames_next(),
            ResamplerOut::None => 0,
        }
    }

    /// Get the number of output frames that will be produced
    pub fn output_frames_next(&self) -> usize {
        match self {
            ResamplerOut::SincFixed(r) => r.output_frames_next(),
            ResamplerOut::FastFixed(r) => r.output_frames_next(),
            ResamplerOut::None => 0,
        }
    }

    /// Get the output delay in frames
    pub fn output_delay(&self) -> usize {
        match self {
            ResamplerOut::SincFixed(r) => r.output_delay(),
            ResamplerOut::FastFixed(r) => r.output_delay(),
            ResamplerOut::None => 0,
        }
    }

    /// Set the resample ratio dynamically
    pub fn set_resample_ratio(&mut self, ratio: f64, ramp: bool) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            ResamplerOut::SincFixed(r) => {
                r.set_resample_ratio(ratio, ramp)?;
                Ok(())
            }
            ResamplerOut::FastFixed(r) => {
                r.set_resample_ratio(ratio, ramp)?;
                Ok(())
            }
            ResamplerOut::None => Ok(()),
        }
    }

    /// Nudge the ratio by PPM for drift correction
    pub fn nudge_ratio(&mut self, base_ratio: f64, ppm: f64) -> Result<(), Box<dyn std::error::Error>> {
        let adjusted_ratio = base_ratio * (1.0 + ppm * 1e-6);
        self.set_resample_ratio(adjusted_ratio, true)
    }

    /// Process audio through the resampler
    pub fn process_into_buffer(
        &mut self,
        input: &[Vec<f32>],
        output: &mut [Vec<f32>],
    ) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            ResamplerOut::SincFixed(r) => {
                r.process_into_buffer(input, output, None)?;
                Ok(())
            }
            ResamplerOut::FastFixed(r) => {
                r.process_into_buffer(input, output, None)?;
                Ok(())
            }
            ResamplerOut::None => {
                // Passthrough - copy input to output
                for (i, o) in input.iter().zip(output.iter_mut()) {
                    o.clear();
                    o.extend_from_slice(i);
                }
                Ok(())
            }
        }
    }
}

// ============================================================================
// Builder Functions
// ============================================================================

/// Build a SincFixedOut resampler for output (48kHz → device rate)
pub fn build_sinc_fixed_out(
    device_rate: u32,
    chunk_frames: usize,
    params: Option<ResamplerParams>,
) -> Result<SincFixedOut<f32>, Box<dyn std::error::Error>> {
    let params = params.unwrap_or_default();
    let ratio = device_rate as f64 / 48000.0;
    
    info!(
        "📊 Building SincFixedOut: 48kHz → {}Hz (ratio: {:.4}), chunk: {} frames",
        device_rate, ratio, chunk_frames
    );

    // Determine max_resample_ratio_relative based on use case
    // For local playback, we might want small drift corrections (±1000 PPM = 0.1%)
    let max_resample_ratio_relative = 1.001; // Allow ±0.1% adjustment

    let resampler = SincFixedOut::new(
        ratio,
        max_resample_ratio_relative,
        params.to_sinc_params(),
        chunk_frames,
        1, // Mono channel
    )?;

    info!(
        "✅ SincFixedOut created: input_next={}, output_next={}, delay={}",
        resampler.input_frames_next(),
        resampler.output_frames_next(),
        resampler.output_delay()
    );

    Ok(resampler)
}

/// Build a FastFixedOut resampler for low-latency output
pub fn build_fast_fixed_out(
    device_rate: u32,
    chunk_frames: usize,
) -> Result<FastFixedOut<f32>, Box<dyn std::error::Error>> {
    let ratio = device_rate as f64 / 48000.0;
    
    info!(
        "⚡ Building FastFixedOut: 48kHz → {}Hz (ratio: {:.4}), chunk: {} frames",
        device_rate, ratio, chunk_frames
    );

    let resampler = FastFixedOut::new(
        ratio,
        1.001, // max_resample_ratio_relative
        rubato::PolynomialDegree::Linear,
        chunk_frames,
        1, // Mono channel
    )?;

    info!(
        "✅ FastFixedOut created: input_next={}, output_next={}, delay={}",
        resampler.input_frames_next(),
        resampler.output_frames_next(),
        resampler.output_delay()
    );

    Ok(resampler)
}

/// Build a SincFixedIn resampler for input (device rate → 48kHz)
pub fn build_sinc_fixed_in(
    input_rate: u32,
    chunk_frames: usize,
    params: Option<ResamplerParams>,
) -> Result<SincFixedIn<f32>, Box<dyn std::error::Error>> {
    let params = params.unwrap_or_default();
    let ratio = 48000.0 / input_rate as f64;
    
    info!(
        "📊 Building SincFixedIn: {}Hz → 48kHz (ratio: {:.4}), chunk: {} frames",
        input_rate, ratio, chunk_frames
    );

    let resampler = SincFixedIn::new(
        ratio,
        1.001, // max_resample_ratio_relative
        params.to_sinc_params(),
        chunk_frames,
        1, // Mono channel
    )?;

    info!(
        "✅ SincFixedIn created: input_next={}, output_frames_max={}",
        resampler.input_frames_next(),
        resampler.output_frames_max()
    );

    Ok(resampler)
}

/// Build an appropriate output resampler based on requirements
pub fn build_output_resampler(
    device_rate: u32,
    chunk_frames: usize,
    use_fast: bool,
) -> Result<ResamplerOut, Box<dyn std::error::Error>> {
    // No resampling needed if already at 48kHz
    if device_rate == 48000 {
        info!("✅ No output resampler needed - device already at 48kHz");
        return Ok(ResamplerOut::None);
    }

    if use_fast {
        Ok(ResamplerOut::FastFixed(build_fast_fixed_out(device_rate, chunk_frames)?))
    } else {
        Ok(ResamplerOut::SincFixed(build_sinc_fixed_out(device_rate, chunk_frames, None)?))
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Calculate the ideal frame size for a given duration at a sample rate
pub fn frames_for_duration_ms(rate: u32, ms: u32) -> usize {
    (rate as usize * ms as usize).div_ceil(1000)
}

/// Get 10ms frame size for a given sample rate
pub fn frames_for_10ms(rate: u32) -> usize {
    frames_for_duration_ms(rate, 10)
}

/// Get 5ms frame size for a given sample rate
pub fn frames_for_5ms(rate: u32) -> usize {
    frames_for_duration_ms(rate, 5)
}

// ============================================================================
// Input Resampler Wrapper (for input.rs)
// ============================================================================

pub struct InputResampler {
    inner: Option<SincFixedIn<f32>>,
    scratch_input: Vec<Vec<f32>>,
    scratch_output: Vec<Vec<f32>>,
}

impl InputResampler {
    pub fn new(input_rate: u32, chunk_frames: usize) -> Result<Self, Box<dyn std::error::Error>> {
        if input_rate == 48000 {
            info!("✅ No input resampler needed - device already at 48kHz");
            return Ok(Self {
                inner: None,
                scratch_input: vec![vec![]],
                scratch_output: vec![vec![]],
            });
        }

        let resampler = build_sinc_fixed_in(input_rate, chunk_frames, None)?;
        
        // Pre-allocate scratch buffers
        let max_output = resampler.output_frames_max();
        
        Ok(Self {
            inner: Some(resampler),
            scratch_input: vec![Vec::with_capacity(chunk_frames)],
            scratch_output: vec![Vec::with_capacity(max_output)],
        })
    }

    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let Some(ref mut resampler) = self.inner else {
            // Passthrough
            return input.to_vec();
        };

        // Prepare input in the format rubato expects
        self.scratch_input[0].clear();
        self.scratch_input[0].extend_from_slice(input);

        // Process
        match resampler.process(&self.scratch_input, None) {
            Ok(output) => {
                if !output.is_empty() && !output[0].is_empty() {
                    output[0].clone()
                } else {
                    Vec::new()
                }
            }
            Err(e) => {
                warn!("Input resampler error: {}", e);
                Vec::new()
            }
        }
    }

    pub fn input_frames_next(&self) -> usize {
        self.inner.as_ref().map_or(0, |r| r.input_frames_next())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_48k_to_96k_resampling() {
        // This is exactly your use case
        let chunk_frames = 512; // Typical CPAL callback size at 96kHz
        let mut resampler = build_output_resampler(96000, chunk_frames, false).unwrap();

        // Should need exactly 256 input frames to produce 512 output frames
        if let ResamplerOut::SincFixed(_) = resampler {
            assert_eq!(resampler.input_frames_next(), 256);
            assert_eq!(resampler.output_frames_next(), chunk_frames);
        }
    }

    #[test]
    fn test_48k_to_44100_resampling() {
        let chunk_frames = 441; // 10ms at 44.1kHz
        let mut resampler = build_output_resampler(44100, chunk_frames, false).unwrap();

        if let ResamplerOut::SincFixed(_) = resampler {
            // Should need ~480 input frames to produce 441 output frames
            let input_needed = resampler.input_frames_next();
            assert!(input_needed >= 479 && input_needed <= 481);
        }
    }

    #[test]
    fn test_no_resampling_needed() {
        let resampler = build_output_resampler(48000, 480, false).unwrap();
        assert!(matches!(resampler, ResamplerOut::None));
    }

    #[test]
    fn test_drift_correction() {
        let mut resampler = build_output_resampler(96000, 512, false).unwrap();
        
        // Apply +500 PPM drift correction
        let result = resampler.nudge_ratio(2.0, 500.0);
        assert!(result.is_ok());
        
        // New ratio should be 2.0 * 1.0005 = 2.001
    }
}