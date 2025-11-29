// ============================================================================
// Audio Fingerprinting with Discontinuity Detection
// ============================================================================

#[derive(Debug, Clone, Copy)]
pub struct AudioFingerprint {
    pub dc_offset: f32,
    pub avg_magnitude: f32,
    pub rms: f32,
    pub silent_samples: u32,
    pub peak: f32,
    pub zero_crossings: u32,
    pub last_sample: f32,
    pub discontinuities: u32,
}

impl AudioFingerprint {
    pub fn from_samples(samples: &[f32]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }

        let len = samples.len() as f32;
        let dc_offset = samples.iter().sum::<f32>() / len;
        let avg_magnitude = samples.iter().map(|x| x.abs()).sum::<f32>() / len;
        let rms = (samples.iter().map(|x| x * x).sum::<f32>() / len).sqrt();
        let silent_samples = samples.iter().filter(|&&x| x.abs() < 0.001).count() as u32;
        let peak = samples.iter().map(|x| x.abs()).fold(0.0f32, f32::max);

        let mut zero_crossings = 0u32;
        let mut discontinuities = 0u32;

        for i in 1..samples.len() {
            // Zero crossing detection
            if samples[i - 1].signum() != samples[i].signum() && samples[i].abs() > 0.001 {
                zero_crossings += 1;
            }

            // Discontinuity detection - sudden jumps
            let diff = (samples[i] - samples[i - 1]).abs();
            if diff > 0.5 && samples[i].abs() > 0.01 && samples[i - 1].abs() > 0.01 {
                discontinuities += 1;
            }
        }

        Self {
            dc_offset,
            avg_magnitude,
            rms,
            silent_samples,
            peak,
            zero_crossings,
            last_sample: samples.last().copied().unwrap_or(0.0),
            discontinuities,
        }
    }



    pub fn is_silent(&self) -> bool {
        self.rms < 0.001
    }

    pub fn is_clipping(&self) -> bool {
        self.peak > 0.98
    }

    pub fn has_discontinuities(&self) -> bool {
        self.discontinuities > 0
    }
}

impl Default for AudioFingerprint {
    fn default() -> Self {
        Self {
            dc_offset: 0.0,
            avg_magnitude: 0.0,
            rms: 0.0,
            silent_samples: 0,
            peak: 0.0,
            zero_crossings: 0,
            last_sample: 0.0,
            discontinuities: 0,
        }
    }
}