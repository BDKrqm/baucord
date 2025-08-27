// voice/jitter_buffer.rs - Adaptive jitter buffer with exact frame pulling
use crate::settings::UserAudioSettings;
use renet::ClientId;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use log::{debug, warn};

pub struct AdaptiveJitterBuffer {
    sample_rate: usize,
    target_ms: AtomicUsize,
    min_ms: usize,
    max_ms: usize,

    buffers: HashMap<ClientId, VecDeque<f32>>,
    user_audio_settings: Arc<Mutex<HashMap<ClientId, UserAudioSettings>>>,

    // Drift tracking
    total_samples_written: u64,
    total_samples_read: u64,
}

impl AdaptiveJitterBuffer {
    pub fn new(
        sample_rate: usize,
        initial_target_ms: usize,
        user_audio_settings: Arc<Mutex<HashMap<ClientId, UserAudioSettings>>>,
    ) -> Self {
        Self {
            sample_rate,
            target_ms: AtomicUsize::new(initial_target_ms),
            min_ms: 10, // Minimum 10ms
            max_ms: 50, // Maximum 50ms
            buffers: HashMap::new(),
            user_audio_settings,
            total_samples_written: 0,
            total_samples_read: 0,
        }
    }

    pub fn push_frame(&mut self, client_id: ClientId, samples: Vec<f32>) {
        // Check if user is muted
        let settings = self.user_audio_settings.lock().unwrap();
        if let Some(user_settings) = settings.get(&client_id) {
            if user_settings.is_muted {
                return;
            }
        }
        drop(settings);

        let buffer = self.buffers.entry(client_id).or_default();
        self.total_samples_written += samples.len() as u64;
        buffer.extend(samples);

        // Limit maximum buffer size
        let max_samples = (self.max_ms as usize * self.sample_rate) / 1000;
        while buffer.len() > max_samples {
            buffer.pop_front();
        }
    }

    /// Pull exactly n samples, filling with silence if needed
    pub fn pull_48k_exact(&mut self, n: usize, out: &mut [f32]) {
        debug_assert_eq!(out.len(), n, "Output buffer size mismatch");
        
        // Initialize output with silence
        for sample in out.iter_mut() {
            *sample = 0.0;
        }

        let settings = self.user_audio_settings.lock().unwrap();
        let target_samples = (self.target_ms.load(Ordering::Relaxed) as usize * self.sample_rate) / 1000;

        let mut active_streams = 0;
        let mut mixed = vec![0.0f32; n];

        for (client_id, buffer) in self.buffers.iter_mut() {
            let user_settings = settings.get(client_id).cloned().unwrap_or_default();

            if user_settings.is_muted {
                buffer.clear();
                continue;
            }

            // Only mix if we have enough buffered (or at least something to mix)
            if buffer.len() >= target_samples || buffer.len() >= n {
                active_streams += 1;
                
                // Take up to n samples from this buffer
                for i in 0..n.min(buffer.len()) {
                    if let Some(sample) = buffer.pop_front() {
                        mixed[i] += sample * user_settings.volume;
                    }
                }
            }
        }

        // Normalize for multiple streams
        if active_streams > 1 {
            let scale = 1.0 / (active_streams as f32).sqrt();
            for sample in mixed.iter_mut() {
                *sample *= scale;
                *sample = sample.clamp(-0.99, 0.99); // Prevent clipping
            }
        }

        // Copy mixed audio to output
        out.copy_from_slice(&mixed);
        self.total_samples_read += n as u64;
    }

    pub fn get_mixed_samples(&mut self, count: usize) -> Vec<f32> {
        let mut output = vec![0.0f32; count];
        self.pull_48k_exact(count, &mut output);
        output
    }

    pub fn adapt(&self) {
        // Calculate average buffer occupancy
        let avg_occupancy: f32 = self.buffers.values()
            .map(|b| b.len() as f32)
            .sum::<f32>() / self.buffers.len().max(1) as f32;

        let occupancy_ms = (avg_occupancy * 1000.0) / self.sample_rate as f32;
        let current_target = self.target_ms.load(Ordering::Relaxed);

        // Adjust target based on occupancy
        if occupancy_ms < 10.0 && current_target < self.max_ms {
            // Buffer too low, increase target
            self.target_ms.store(current_target + 5, Ordering::Relaxed);
        } else if occupancy_ms > 40.0 && current_target > self.min_ms {
            // Buffer too high, decrease target
            self.target_ms.store(current_target - 5, Ordering::Relaxed);
        }
    }

    pub fn handle_drift(&mut self, drift_us: i32) {
        let drift_samples = (drift_us.abs() as f32 * self.sample_rate as f32 / 1_000_000.0) as usize;

        if drift_us > 0 {
            // We're ahead - drop samples
            for buffer in self.buffers.values_mut() {
                for _ in 0..drift_samples.min(buffer.len() / 10) {
                    buffer.pop_front();
                }
            }
        } else {
            // We're behind - duplicate samples (simple approach)
            for buffer in self.buffers.values_mut() {
                if let Some(last) = buffer.back().copied() {
                    for _ in 0..drift_samples.min(48) {
                        // Max 1ms
                        buffer.push_back(last);
                    }
                }
            }
        }
    }

    pub fn get_occupancy_ms(&self) -> f32 {
        let avg_occupancy: f32 = self.buffers.values()
            .map(|b| b.len() as f32)
            .sum::<f32>() / self.buffers.len().max(1) as f32;

        (avg_occupancy * 1000.0) / self.sample_rate as f32
    }

    /// Calculate drift in PPM for feedback to resampler
    pub fn calculate_drift_ppm(&self) -> f64 {
        if self.total_samples_written == 0 || self.total_samples_read == 0 {
            return 0.0;
        }

        let ratio = self.total_samples_written as f64 / self.total_samples_read as f64;
        let drift = (ratio - 1.0) * 1_000_000.0; // Convert to PPM
        
        // Clamp to reasonable range
        drift.clamp(-1000.0, 1000.0)
    }
}