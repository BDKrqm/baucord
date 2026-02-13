//! Noise suppression via nnnoiseless (Mozilla RNNoise, pure Rust).
//!
//! Operates on 480-sample frames (10 ms @ 48 kHz) — exactly our Opus frame size.
//! Sits in the input pipeline: capture → resample → **denoise** → VAD → encode.

use nnnoiseless::DenoiseState;

/// Real-time noise suppressor wrapping nnnoiseless.
///
/// The denoiser expects f32 samples scaled to i16 range (−32768..32767),
/// and returns cleaned samples in the same range. We handle the conversion
/// from/to normalized f32 (−1.0..1.0) internally.
pub struct Denoiser {
    state: Box<DenoiseState<'static>>,
    output_buf: [f32; DenoiseState::FRAME_SIZE],
    /// True after the first frame (we discard the first output due to fade-in artifacts).
    warmed_up: bool,
}

impl Denoiser {
    pub fn new() -> Self {
        Self {
            state: DenoiseState::new(),
            output_buf: [0.0; DenoiseState::FRAME_SIZE],
            warmed_up: false,
        }
    }

    /// Process one frame of 480 normalized f32 samples (−1.0..1.0) in-place.
    ///
    /// Returns the VAD probability from the RNN (0.0–1.0), which can be used
    /// as a secondary VAD signal if desired.
    pub fn process_frame(&mut self, samples: &mut [f32]) -> f32 {
        debug_assert_eq!(samples.len(), DenoiseState::FRAME_SIZE);

        // nnnoiseless expects i16-scaled floats
        let input: Vec<f32> = samples.iter().map(|&s| s * 32767.0).collect();

        let vad_prob = self.state.process_frame(&mut self.output_buf, &input);

        if !self.warmed_up {
            // First frame has fade-in artifacts — output silence
            self.warmed_up = true;
            for s in samples.iter_mut() {
                *s = 0.0;
            }
        } else {
            // Convert back to normalized f32
            for (out, &denoised) in samples.iter_mut().zip(self.output_buf.iter()) {
                *out = (denoised / 32767.0).clamp(-1.0, 1.0);
            }
        }

        vad_prob
    }

    /// Reset state (e.g., after device switch).
    pub fn reset(&mut self) {
        self.state = DenoiseState::new();
        self.output_buf = [0.0; DenoiseState::FRAME_SIZE];
        self.warmed_up = false;
    }
}
