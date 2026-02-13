//! Audio input: capture → resample → denoise → AGC → gain → clamp → VAD → encode.
//!
//! Pipeline stages:
//! 1. **Capture** (cpal callback): downmix to mono, soft clamp ±0.98
//! 2. **Resample** (rubato): device rate → 48 kHz
//! 3. **Energy gate**: cheap RMS check against user threshold — silent frames
//!    skip all expensive processing below (denoise, AGC, earshot)
//! 4. **Denoise** (nnnoiseless/RNNoise): noise suppression on raw mic signal
//! 5. **AGC**: normalize clean signal to target RMS
//! 6. **Gain**: user's input gain preference on clean, normalized audio
//! 7. **Hard clamp** ±0.95: protect earshot from overflow
//! 8. **VAD** (earshot): speech classifier rejects non-speech noise
//! 9. **Encode** (Opus): speech frames sent to encoder thread
//!
//! Device compatibility strategy:
//! - Use cpal's `default_input_config()` — accept whatever the OS/driver gives us
//! - Resample to 48 kHz internally (rubato handles any rate)
//! - Downmix to mono by averaging all channels
//! - This works with Bluetooth HFP (8/16 kHz), USB mics (44.1/48/96 kHz),
//!   audio interfaces (any rate), and everything in between.

use crate::agc::Agc;
use crate::denoise::Denoiser;
use crate::resampler::build_input_resampler;
use crate::stats::AudioStats;
use crate::vad::VADProcessor;
use crate::FRAME_SIZE;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig, SupportedStreamConfig};
use crossbeam_channel::{Sender, TrySendError};
use dasp_sample::{FromSample, Sample as DaspSample, ToSample};
use log::{error, info, warn};
use rtrb::{Consumer, Producer, RingBuffer};
use rubato::{Resampler, SincFixedIn};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Sentinel SSRC used for loopback frames so they don't collide with real peers.
pub const LOOPBACK_SSRC: u64 = u64::MAX;

pub struct AudioInput {
    device: Device,
    device_config: SupportedStreamConfig,
    stream: Option<Stream>,
    encoder_tx: Sender<Vec<f32>>,

    /// Optional sender into the output path for loopback monitoring.
    /// Points to the same channel the decoder thread feeds into.
    loopback_tx: Option<Sender<(u64, Vec<f32>)>>,
    loopback_enabled: Arc<AtomicBool>,

    ring_producer: Option<Producer<f32>>,
    ring_consumer: Option<Consumer<f32>>,

    // Worker
    worker_thread: Option<thread::JoinHandle<()>>,
    should_stop: Arc<AtomicBool>,

    // Settings (shared with worker via atomics)
    is_muted: Arc<AtomicBool>,
    vad_enabled: Arc<AtomicBool>,
    vad_threshold: Arc<AtomicU32>, // f32 bits
    noise_suppression: Arc<AtomicBool>,
    /// Master input gain (f32 bits). Applied after denoise/AGC, before VAD.
    input_gain: Arc<AtomicU32>,

    // Observable state
    input_level: Arc<AtomicU32>, // f32 bits, 0.0–1.0 — raw mic capture level (no gain)
    is_speaking: Arc<AtomicBool>,

    // Stats
    stats: Arc<AudioStats>,
}

impl AudioInput {
    pub fn new(
        device_name: Option<&str>,
        encoder_tx: Sender<Vec<f32>>,
        loopback_tx: Option<Sender<(u64, Vec<f32>)>>,
        stats: Arc<AudioStats>,
        vad_threshold: f32,
        noise_suppression: bool,
        input_gain: f32,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let host = cpal::default_host();

        let device = if let Some(name) = device_name {
            host.input_devices()?
                .find(|d| d.name().ok().as_deref() == Some(name))
                .ok_or_else(|| format!("Input device '{}' not found", name))?
        } else {
            host.default_input_device()
                .ok_or("No default input device available")?
        };

        // Use whatever the OS says is best for this device.
        // This is the key to Bluetooth / weird device compatibility.
        let device_config = device
            .default_input_config()
            .map_err(|e| format!("Failed to get default input config: {}", e))?;

        let sample_rate = device_config.sample_rate().0;
        let channels = device_config.channels();

        info!(
            "🎤 Input device: {} @ {}Hz, {}ch, {:?}",
            device.name().unwrap_or_else(|_| "Unknown".into()),
            sample_rate,
            channels,
            device_config.sample_format(),
        );

        // Ring buffer: 100ms at device rate, mono (we downmix before pushing)
        let ring_size = ((sample_rate as usize) * 100) / 1000;
        let (ring_producer, ring_consumer) = RingBuffer::new(ring_size.max(4800));

        Ok(Self {
            device,
            device_config,
            stream: None,
            encoder_tx,
            loopback_tx,
            loopback_enabled: Arc::new(AtomicBool::new(false)),
            ring_producer: Some(ring_producer),
            ring_consumer: Some(ring_consumer),
            worker_thread: None,
            should_stop: Arc::new(AtomicBool::new(false)),
            is_muted: Arc::new(AtomicBool::new(false)),
            vad_enabled: Arc::new(AtomicBool::new(true)),
            vad_threshold: Arc::new(AtomicU32::new(vad_threshold.to_bits())),
            noise_suppression: Arc::new(AtomicBool::new(noise_suppression)),
            input_gain: Arc::new(AtomicU32::new(input_gain.to_bits())),
            input_level: Arc::new(AtomicU32::new(0.0_f32.to_bits())),
            is_speaking: Arc::new(AtomicBool::new(false)),
            stats,
        })
    }

    pub fn start(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let sample_rate = self.device_config.sample_rate().0;
        let channels = self.device_config.channels();
        let format = self.device_config.sample_format();

        self.start_worker_thread(sample_rate);

        let stream = match format {
            SampleFormat::I16 => self.build_stream::<i16>(sample_rate, channels)?,
            SampleFormat::U16 => self.build_stream::<u16>(sample_rate, channels)?,
            SampleFormat::F32 => self.build_stream::<f32>(sample_rate, channels)?,
            _ => return Err(format!("Unsupported sample format: {:?}", format).into()),
        };

        stream.play()?;
        self.stream = Some(stream);
        Ok(())
    }

    fn build_stream<T>(
        &mut self,
        sample_rate: u32,
        channels: u16,
    ) -> Result<Stream, Box<dyn std::error::Error>>
    where
        T: DaspSample + FromSample<f32> + ToSample<f32> + Send + 'static + cpal::SizedSample,
    {
        let config = StreamConfig {
            channels,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        let mut ring_producer = self
            .ring_producer
            .take()
            .ok_or("Ring producer already taken")?;
        let is_muted = self.is_muted.clone();
        let stats = self.stats.clone();
        let stats_err = self.stats.clone();
        let input_level = self.input_level.clone();
        let ch = channels as usize;

        let stream = self.device.build_input_stream(
            &config,
            move |data: &[T], _: &_| {
                if is_muted.load(Ordering::Relaxed) {
                    input_level.store(0.0_f32.to_bits(), Ordering::Relaxed);
                    return;
                }

                // Downmix to mono: average all channels per frame.
                // No gain here — gain is applied in the worker after denoise/AGC.
                let mut rms_acc = 0.0f32;
                let mut count = 0usize;

                for frame in data.chunks(ch) {
                    let sum: f32 = frame.iter().map(|s| s.to_sample::<f32>()).sum();
                    // Soft clamp: protect resampler and denoiser from digital clipping
                    let mono = (sum / ch as f32).clamp(-0.98, 0.98);

                    rms_acc += mono * mono;
                    count += 1;

                    if ring_producer.push(mono).is_err() {
                        stats.input_overruns.fetch_add(1, Ordering::Relaxed);
                        break;
                    }
                }

                // RMS for VU meter
                if count > 0 {
                    let rms = (rms_acc / count as f32).sqrt();
                    let scaled = (rms * 3.0).clamp(0.0, 1.0);
                    input_level.store(scaled.to_bits(), Ordering::Relaxed);
                }
            },
            move |err| {
                error!("Input stream error: {}", err);
                stats_err.input_overruns.fetch_add(1, Ordering::Relaxed);
            },
            None,
        )?;

        Ok(stream)
    }

    fn start_worker_thread(&mut self, device_sample_rate: u32) {
        let mut ring_consumer = self
            .ring_consumer
            .take()
            .expect("Ring consumer already taken");

        let should_stop = self.should_stop.clone();
        let stats = self.stats.clone();
        let encoder_tx = self.encoder_tx.clone();
        let is_speaking_handle = self.is_speaking.clone();
        let vad_enabled = self.vad_enabled.clone();
        let is_muted = self.is_muted.clone();
        let vad_threshold_atomic = self.vad_threshold.clone();
        let noise_suppression = self.noise_suppression.clone();
        let loopback_tx = self.loopback_tx.clone();
        let loopback_enabled = self.loopback_enabled.clone();
        let input_gain = self.input_gain.clone();

        self.worker_thread = Some(thread::spawn(move || {
            // ── Resampler (if device rate ≠ 48 kHz) ─────────────────────
            let needs_resample = device_sample_rate != 48000;
            let mut resampler: Option<SincFixedIn<f32>> = None;
            let mut resampler_input: Vec<Vec<f32>> = Vec::new();
            let mut resampler_accum = Vec::with_capacity(2048);

            if needs_resample {
                let chunk = if device_sample_rate > 48000 {
                    960
                } else {
                    crate::resampler::frames_for_ms(device_sample_rate, 10)
                };
                match build_input_resampler(device_sample_rate, chunk) {
                    Ok(r) => {
                        resampler_input = vec![Vec::with_capacity(r.input_frames_next())];
                        resampler = Some(r);
                    }
                    Err(e) => {
                        error!("Failed to create input resampler: {}", e);
                        return;
                    }
                }
            }

            // ── Denoiser ────────────────────────────────────────────────
            let mut denoiser = Denoiser::new();
            let mut agc = Agc::new(0.15);

            // ── VAD (earshot speech classifier — energy gate is above) ──
            let mut vad = VADProcessor::new();

            // ── Processing state ────────────────────────────────────────
            let mut read_buf = vec![0.0f32; 4096];
            let mut accumulator = Vec::with_capacity(FRAME_SIZE * 2);
            let mut consecutive_silence = 0usize;

            info!(
                "🎤 Input worker started (device {}Hz, resample={})",
                device_sample_rate, needs_resample
            );

            while !should_stop.load(Ordering::Relaxed) {
                // Drain ring when muted
                if is_muted.load(Ordering::Relaxed) {
                    is_speaking_handle.store(false, Ordering::Relaxed);
                    while ring_consumer.pop().is_ok() {}
                    thread::sleep(Duration::from_millis(10));
                    agc.reset();
                    denoiser.reset();
                    continue;
                }

                // ── Read from ring ──────────────────────────────────────
                let target = if let Some(ref r) = resampler {
                    r.input_frames_next()
                } else {
                    FRAME_SIZE
                };

                let mut n_read = 0;
                for i in 0..target.min(read_buf.len()) {
                    match ring_consumer.pop() {
                        Ok(s) => {
                            read_buf[i] = s;
                            n_read += 1;
                        }
                        Err(_) => break,
                    }
                }

                if n_read == 0 {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }

                // ── Resample to 48 kHz if needed ────────────────────────
                let at_48k = if let Some(ref mut r) = resampler {
                    resampler_accum.extend_from_slice(&read_buf[..n_read]);

                    if resampler_accum.len() >= r.input_frames_next() {
                        let n = r.input_frames_next();
                        resampler_input[0].clear();
                        resampler_input[0].extend_from_slice(&resampler_accum[..n]);
                        resampler_accum.drain(..n);

                        match r.process(&resampler_input, None) {
                            Ok(out) if !out.is_empty() && !out[0].is_empty() => out[0].clone(),
                            Ok(_) => Vec::new(),
                            Err(e) => {
                                error!("Resampler error: {}", e);
                                Vec::new()
                            }
                        }
                    } else {
                        continue;
                    }
                } else {
                    read_buf[..n_read].to_vec()
                };

                if at_48k.is_empty() {
                    continue;
                }

                // ── Accumulate into FRAME_SIZE (480) chunks ─────────────
                accumulator.extend_from_slice(&at_48k);

                while accumulator.len() >= FRAME_SIZE {
                    let mut frame: Vec<f32> = accumulator.drain(..FRAME_SIZE).collect();

                    // ── Energy gate (cheapest possible check) ────────────
                    // Compute frame RMS and compare against threshold.
                    // This is what the user's threshold slider controls.
                    let frame_rms = {
                        let sum_sq: f32 = frame.iter().map(|s| s * s).sum();
                        (sum_sq / frame.len() as f32).sqrt()
                    };

                    let vad_on = vad_enabled.load(Ordering::Relaxed);
                    let energy_pass = if vad_on {
                        // Quick energy pre-check: if clearly below gate,
                        // skip all expensive processing.
                        let was_speaking = is_speaking_handle.load(Ordering::Relaxed);
                        let energy_threshold = if was_speaking {
                            // Lower threshold to stop (hysteresis) — don't
                            // cut off speech tails prematurely.
                            0.002
                        } else {
                            // Use the VAD threshold to determine minimum energy.
                            let thr = f32::from_bits(vad_threshold_atomic.load(Ordering::Relaxed));
                            (thr * 0.2).max(0.002)
                        };
                        frame_rms >= energy_threshold
                    } else {
                        true // VAD disabled — process everything
                    };

                    if !energy_pass {
                        // ── Silence fast path ───────────────────────────
                        // Skip denoise, AGC, earshot — save all the CPU.
                        consecutive_silence += 1;

                        // Update stats to show silence
                        stats
                            .denoised_level
                            .store(0.0_f32.to_bits(), Ordering::Relaxed);
                        stats
                            .pipeline_level
                            .store(0.0_f32.to_bits(), Ordering::Relaxed);
                        is_speaking_handle.store(false, Ordering::Relaxed);
                        stats.vad_active.store(false, Ordering::Relaxed);

                        // Reset denoiser after prolonged silence so it doesn't
                        // hold stale state. The warmed_up flag handles the
                        // fade-in artifact on restart.
                        if consecutive_silence == 50 {
                            // 500ms
                            denoiser.reset();
                            agc.reset();
                        }

                        continue;
                    }

                    // ── Denoise (RNNoise sees raw mic signal at natural levels) ─
                    let denoise_on = noise_suppression.load(Ordering::Relaxed);
                    if denoise_on {
                        denoiser.process_frame(&mut frame);
                    }
                    stats.denoise_active.store(denoise_on, Ordering::Relaxed);

                    // ── Post-denoise level (before AGC/gain — for threshold viz) ─
                    {
                        let rms: f32 =
                            (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt();
                        let scaled = (rms * 3.0).clamp(0.0, 1.0);
                        stats
                            .denoised_level
                            .store(scaled.to_bits(), Ordering::Relaxed);
                    }

                    // ── AGC (normalizes clean signal to target RMS) ─────────
                    agc.process_frame(&mut frame);

                    // ── Input gain (user preference on clean, normalized audio)
                    let gain = f32::from_bits(input_gain.load(Ordering::Relaxed));
                    if (gain - 1.0).abs() > f32::EPSILON {
                        for s in frame.iter_mut() {
                            *s *= gain;
                        }
                    }

                    // ── Hard clamp (protect earshot from overflow) ───────────
                    for s in frame.iter_mut() {
                        *s = s.clamp(-0.95, 0.95);
                    }

                    // ── Pipeline output level (what encoder actually sees) ───
                    {
                        let rms: f32 =
                            (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt();
                        let scaled = (rms * 3.0).clamp(0.0, 1.0);
                        stats
                            .pipeline_level
                            .store(scaled.to_bits(), Ordering::Relaxed);
                    }

                    // ── Loopback tap (post-pipeline: hear what others hear) ──
                    if loopback_enabled.load(Ordering::Relaxed) {
                        if let Some(ref tx) = loopback_tx {
                            let _ = tx.try_send((LOOPBACK_SSRC, frame.clone()));
                        }
                    }

                    // ── VAD classification (earshot — rejects non-speech noise)
                    let is_speech = if vad_on { vad.process(&frame) } else { true };

                    is_speaking_handle.store(is_speech, Ordering::Relaxed);
                    stats.vad_active.store(is_speech, Ordering::Relaxed);

                    if is_speech {
                        consecutive_silence = 0;
                        match encoder_tx.try_send(frame.clone()) {
                            Ok(_) => {}
                            Err(TrySendError::Full(_)) => {
                                // Back-pressure: yield once and retry
                                thread::yield_now();
                                let _ = encoder_tx.try_send(frame);
                            }
                            Err(TrySendError::Disconnected(_)) => return,
                        }
                    } else {
                        consecutive_silence += 1;
                        if consecutive_silence > 10 {
                            is_speaking_handle.store(false, Ordering::Relaxed);
                        }
                    }
                }
            }
        }));
    }

    // ── Public controls ──────────────────────────────────────────────────

    pub fn set_muted(&self, muted: bool) {
        self.is_muted.store(muted, Ordering::Relaxed);
        if muted {
            self.input_level.store(0.0_f32.to_bits(), Ordering::Relaxed);
        }
    }

    pub fn set_vad_enabled(&self, enabled: bool) {
        self.vad_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn set_vad_threshold(&self, threshold: f32) {
        let clamped = threshold.clamp(0.01, 0.5);
        self.vad_threshold
            .store(clamped.to_bits(), Ordering::Relaxed);
    }

    pub fn set_noise_suppression(&self, enabled: bool) {
        self.noise_suppression.store(enabled, Ordering::Relaxed);
    }

    pub fn set_loopback(&self, enabled: bool) {
        self.loopback_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn set_input_gain(&self, gain: f32) {
        let clamped = gain.clamp(0.0, 2.0);
        self.input_gain.store(clamped.to_bits(), Ordering::Relaxed);
    }

    pub fn input_level_handle(&self) -> Arc<AtomicU32> {
        self.input_level.clone()
    }

    pub fn speaking_handle(&self) -> Arc<AtomicBool> {
        self.is_speaking.clone()
    }

    pub fn is_muted(&self) -> bool {
        self.is_muted.load(Ordering::Relaxed)
    }
}

impl Drop for AudioInput {
    fn drop(&mut self) {
        self.should_stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.worker_thread.take() {
            let _ = thread.join();
        }
    }
}
