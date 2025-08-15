// voice/input.rs - RT-safe input with VAD gating
use super::diagnostics::{AudioFingerprint, PipelineDiagnostics, TestMode};
use super::{ChannelMapping, DeviceCapabilities, FRAME_SIZE, LatencyPreset, OPUS_SAMPLE_RATE};
use crate::voice::diagnostics::{Anomaly, AnomalyType};
use crate::voice::resampler::{build_sinc_fixed_in, frames_for_10ms};
use crate::voice::stats::AudioStats;
use crate::voice::vad::{VADConfig, VADProcessor, VADProfile};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, SampleFormat, Stream, StreamConfig};
use crossbeam_channel::Sender;
use dasp_sample::{FromSample, Sample as DaspSample, ToSample};
use log::{debug, error, info, warn};
use rtrb::{Consumer, Producer, RingBuffer};
use rubato::{Resampler, SincFixedIn};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub struct AudioInput {
    device: Device,
    capabilities: DeviceCapabilities,
    pub(crate) stream: Option<Stream>,
    encoder_tx: crossbeam_channel::Sender<Vec<f32>>,

    // RT-safe ring buffer
    ring_producer: Option<Producer<f32>>,
    ring_consumer: Option<Consumer<f32>>,
    ring_capacity: usize,

    // Worker thread
    worker_thread: Option<thread::JoinHandle<()>>,
    should_stop: Arc<AtomicBool>,

    // Settings
    channel_mapping: Arc<Mutex<ChannelMapping>>,
    latency_preset: LatencyPreset,
    is_muted: Arc<AtomicBool>,
    vad_enabled: Arc<AtomicBool>,

    // Stats
    stats: Arc<AudioStats>,
    is_speaking: Arc<AtomicBool>,
    diagnostics: Option<Arc<PipelineDiagnostics>>,
}

impl AudioInput {
    pub fn new(
        device_name: Option<&str>,
        encoder_tx: Sender<Vec<f32>>,
        channel_mapping: Arc<Mutex<ChannelMapping>>,
        stats: Arc<AudioStats>,
        diagnostics: Option<Arc<PipelineDiagnostics>>,
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

        let capabilities = DeviceCapabilities::from_device(&device, true)?;

        // Size ring buffer based on expected sample rate
        let (sample_rate, _, _) = capabilities
            .get_best_config(Some(OPUS_SAMPLE_RATE))
            .ok_or("No suitable input configuration")?;

        let buffer_ms = if sample_rate != OPUS_SAMPLE_RATE {
            100
        } else {
            50
        };
        let ring_size = (sample_rate as usize * buffer_ms) / 1000;

        let (ring_producer, ring_consumer) = RingBuffer::new(ring_size);

        Ok(Self {
            device,
            capabilities,
            stream: None,
            ring_producer: Some(ring_producer),
            ring_consumer: Some(ring_consumer),
            ring_capacity: ring_size,
            worker_thread: None,
            should_stop: Arc::new(AtomicBool::new(false)),
            channel_mapping,
            latency_preset: LatencyPreset::Normal,
            is_muted: Arc::new(AtomicBool::new(false)),
            vad_enabled: Arc::new(AtomicBool::new(true)),
            encoder_tx,
            stats,
            is_speaking: Arc::new(AtomicBool::new(false)),
            diagnostics,
        })
    }

    pub fn start(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let (sample_rate, channels, format) = self
            .capabilities
            .get_best_config(Some(OPUS_SAMPLE_RATE))
            .ok_or("No suitable input configuration")?;

        info!(
            "🎤 Input: {} @ {}Hz, {} ch, {:?} format",
            self.capabilities.name, sample_rate, channels, format
        );

        // Start worker thread
        self.start_worker_thread(sample_rate);

        // Build stream with natural buffer size
        let stream = match format {
            SampleFormat::I16 => self.build_stream::<i16>(sample_rate, channels)?,
            SampleFormat::U16 => self.build_stream::<u16>(sample_rate, channels)?,
            SampleFormat::F32 => self.build_stream::<f32>(sample_rate, channels)?,
            _ => return Err(format!("Unsupported format: {:?}", format).into()),
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
        // Use device's natural buffer size
        let config = StreamConfig {
            channels,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        info!("📊 Using device default buffer size");

        let mut ring_producer = self
            .ring_producer
            .take()
            .ok_or("Ring buffer producer already taken")?;

        let channel_mapping = self.channel_mapping.clone();
        let is_muted = self.is_muted.clone();
        let stats = self.stats.clone();
        let stats_err = self.stats.clone();
        let diagnostics = self.diagnostics.clone();

        // Pre-allocate scratch buffer
        let mut scratch = vec![0.0f32; 8192];
        let mut first_callback = true;

        let stream = self.device.build_input_stream(
            &config,
            move |data: &[T], _: &_| {
                if is_muted.load(Ordering::Relaxed) {
                    return;
                }

                if first_callback {
                    first_callback = false;
                    info!("🎯 Input callback size: {} samples", data.len());
                }

                let mapping = channel_mapping.lock().unwrap();

                // Convert and downmix to mono
                let mut write_idx = 0;
                for chunk in data.chunks(channels as usize) {
                    if write_idx >= scratch.len() {
                        break;
                    }

                    // Convert chunk to f32
                    let chunk_f32: Vec<f32> = chunk.iter().map(|s| s.to_sample::<f32>()).collect();

                    // Apply channel mapping (downmix to mono)
                    scratch[write_idx] = mapping.process_input(&chunk_f32, channels as usize);
                    write_idx += 1;
                }

                // Push to ring buffer (RT-safe, non-blocking)
                let mut written = 0;
                for &sample in &scratch[..write_idx] {
                    match ring_producer.push(sample) {
                        Ok(()) => written += 1,
                        Err(_) => {
                            stats.input_overruns.fetch_add(1, Ordering::Relaxed);
                            break;
                        }
                    }
                }

                if let Some(ref diag) = diagnostics {
                    diag.input_ring.record_push(written);
                }

                stats.input_callbacks.fetch_add(1, Ordering::Relaxed);
            },
            move |err| {
                error!("Input stream error: {}", err);
                stats_err.stream_errors.fetch_add(1, Ordering::Relaxed);
            },
            None,
        )?;

        Ok(stream)
    }

    fn start_worker_thread(&mut self, device_sample_rate: u32) {
        let mut ring_consumer = self
            .ring_consumer
            .take()
            .expect("Ring buffer consumer already taken");

        let should_stop = self.should_stop.clone();
        let stats = self.stats.clone();
        let encoder_tx = self.encoder_tx.clone();
        let diagnostics = self.diagnostics.clone();
        let is_speaking_handle = self.is_speaking.clone();
        let vad_enabled = self.vad_enabled.clone();

        self.worker_thread = Some(thread::spawn(move || {
            // Create resampler if needed
            let mut resampler: Option<SincFixedIn<f32>> = None;
            let mut resampler_input_buffer: Vec<Vec<f32>> = Vec::new();
            let mut resampler_accumulator = Vec::with_capacity(2048);

            if device_sample_rate != OPUS_SAMPLE_RATE {
                info!(
                    "📊 Creating input resampler: {} Hz -> {} Hz",
                    device_sample_rate, OPUS_SAMPLE_RATE
                );

                // For input, we want to process reasonable chunks
                let chunk_size = if device_sample_rate > OPUS_SAMPLE_RATE {
                    960 // Process ~10ms worth at higher rates
                } else {
                    frames_for_10ms(device_sample_rate)
                };

                match build_sinc_fixed_in(device_sample_rate, chunk_size, None) {
                    Ok(r) => {
                        resampler_input_buffer = vec![Vec::with_capacity(r.input_frames_next())];
                        info!(
                            "✅ Input resampler ready: needs {} input, produces up to {} output",
                            r.input_frames_next(),
                            r.output_frames_max()
                        );
                        resampler = Some(r);
                    }
                    Err(e) => {
                        error!("Failed to create input resampler: {}", e);
                    }
                }
            }

            // Create VAD processor
            let vad_config = VADConfig {
                sample_rate: OPUS_SAMPLE_RATE,
                chunk_size: FRAME_SIZE,
                threshold: 0.15,
                profile: VADProfile::Aggressive,
            };
            let mut vad = VADProcessor::new(vad_config).unwrap();

            // Processing state
            let mut read_buffer = vec![0.0f32; 4096];
            let mut accumulator = Vec::with_capacity(FRAME_SIZE * 2);
            let mut consecutive_silence_frames = 0;
            const SILENCE_THRESHOLD: usize = 50; // 500ms of silence before we stop processing

            while !should_stop.load(Ordering::Relaxed) {
                // Determine read size
                let target_read = if let Some(ref r) = resampler {
                    r.input_frames_next()
                } else {
                    FRAME_SIZE
                };

                // Read from ring buffer
                let mut samples_read = 0;
                for i in 0..target_read.min(read_buffer.len()) {
                    match ring_consumer.pop() {
                        Ok(sample) => {
                            read_buffer[i] = sample;
                            samples_read += 1;
                        }
                        Err(_) => break,
                    }
                }

                if samples_read == 0 {
                    thread::sleep(Duration::from_micros(500));
                    continue;
                }

                if let Some(ref diag) = diagnostics {
                    diag.input_ring.record_pop(samples_read);
                }

                // Early VAD check on raw input if no resampling
                // This saves CPU when silent
                if resampler.is_none() && vad_enabled.load(Ordering::Relaxed) {
                    let raw_fingerprint =
                        AudioFingerprint::from_samples(&read_buffer[..samples_read]);
                    if raw_fingerprint.is_silent() {
                        consecutive_silence_frames += 1;
                        if consecutive_silence_frames > SILENCE_THRESHOLD {
                            is_speaking_handle.store(false, Ordering::Relaxed);
                            // Skip processing if we've been silent for a while
                            thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                    } else {
                        consecutive_silence_frames = 0;
                    }
                }

                // Process through resampler if needed
                let processed = if let Some(ref mut r) = resampler {
                    // Accumulate samples until we have enough
                    resampler_accumulator.extend_from_slice(&read_buffer[..samples_read]);

                    if resampler_accumulator.len() >= r.input_frames_next() {
                        let input_size = r.input_frames_next();
                        resampler_input_buffer[0].clear();
                        resampler_input_buffer[0]
                            .extend_from_slice(&resampler_accumulator[..input_size]);
                        resampler_accumulator.drain(..input_size);

                        match r.process(&resampler_input_buffer, None) {
                            Ok(output) => {
                                if !output.is_empty() && !output[0].is_empty() {
                                    output[0].clone()
                                } else {
                                    Vec::new()
                                }
                            }
                            Err(e) => {
                                error!("Resampler error: {}", e);
                                Vec::new()
                            }
                        }
                    } else {
                        // Not enough samples yet
                        continue;
                    }
                } else {
                    read_buffer[..samples_read].to_vec()
                };

                if processed.is_empty() {
                    continue;
                }

                // Accumulate for frame processing
                accumulator.extend_from_slice(&processed);

                // Process complete frames
                while accumulator.len() >= FRAME_SIZE {
                    let frame: Vec<f32> = accumulator.drain(..FRAME_SIZE).collect();

                    // VAD processing (now at 48kHz)
                    let is_speech = if vad_enabled.load(Ordering::Relaxed) {
                        vad.process_audio_f32(&frame)
                    } else {
                        true // If VAD disabled, treat all as speech
                    };

                    is_speaking_handle.store(is_speech, Ordering::Relaxed);

                    // Only send to encoder if speaking
                    if is_speech {
                        consecutive_silence_frames = 0;

                        // Send to encoder thread
                        if encoder_tx
                            .send_timeout(frame, Duration::from_millis(5))
                            .is_err()
                        {
                            stats.producer_overruns.fetch_add(1, Ordering::Relaxed);
                        }
                    } else {
                        consecutive_silence_frames += 1;
                    }
                }
            }
        }));
    }

    pub fn set_latency_preset(&mut self, preset: LatencyPreset) {
        self.latency_preset = preset;
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.is_muted.store(muted, Ordering::Relaxed);
    }

    pub fn set_vad_enabled(&mut self, enabled: bool) {
        self.vad_enabled.store(enabled, Ordering::Relaxed);
        info!("🎤 VAD: {}", if enabled { "Enabled" } else { "Disabled" });
    }

    pub fn get_device_info(&self) -> String {
        format!(
            "Input: {} - Rates: {:?}, Channels: {:?}, Formats: {:?}",
            self.capabilities.name,
            self.capabilities.sample_rates,
            self.capabilities.channels,
            self.capabilities.sample_formats
        )
    }

    pub fn get_speaking_handle(&self) -> Arc<AtomicBool> {
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

