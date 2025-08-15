// voice/output.rs - RT-safe output with proper chunk alignment
use super::diagnostics::{AudioFingerprint, PipelineDiagnostics, TestMode};
use super::{ChannelMapping, DeviceCapabilities, LatencyPreset, OPUS_SAMPLE_RATE};
use crate::settings::UserAudioSettings;
use crate::voice::diagnostics::{Anomaly, AnomalyType};
use crate::voice::jitter_buffer::AdaptiveJitterBuffer;
use crate::voice::resampler::{ResamplerOut, build_output_resampler, frames_for_10ms};
use crate::voice::stats::AudioStats;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, SampleFormat, Stream, StreamConfig};
use crossbeam_channel::Receiver;
use dasp_sample::{FromSample, Sample as DaspSample, ToSample};
use log::{debug, error, info, warn};
use renet::ClientId;
use rtrb::{Consumer, Producer, RingBuffer};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub struct AudioOutput {
    device: Device,
    capabilities: DeviceCapabilities,
    stream: Option<Stream>,
    decoder_rx: Option<Receiver<(ClientId, Vec<f32>)>>,

    // Adaptive ring buffer size
    ring_producer: Option<Producer<f32>>,
    ring_consumer: Option<Consumer<f32>>,
    ring_capacity: Arc<AtomicUsize>,

    // Worker thread
    worker_thread: Option<thread::JoinHandle<()>>,
    should_stop: Arc<AtomicBool>,

    // Settings
    channel_mapping: Arc<Mutex<ChannelMapping>>,
    latency_preset: LatencyPreset,
    user_audio_settings: Arc<Mutex<HashMap<ClientId, UserAudioSettings>>>,

    // Stats
    stats: Arc<AudioStats>,
    diagnostics: Option<Arc<PipelineDiagnostics>>,

    // Drift tracking
    drift_ppm: Arc<Mutex<f64>>,
}

impl AudioOutput {
    pub fn new(
        device_name: Option<&str>,
        decoder_rx: Receiver<(ClientId, Vec<f32>)>,
        channel_mapping: Arc<Mutex<ChannelMapping>>,
        user_audio_settings: Arc<Mutex<HashMap<ClientId, UserAudioSettings>>>,
        stats: Arc<AudioStats>,
        diagnostics: Option<Arc<PipelineDiagnostics>>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let host = cpal::default_host();

        let device = if let Some(name) = device_name {
            host.output_devices()?
                .find(|d| d.name().ok().as_deref() == Some(name))
                .ok_or_else(|| format!("Output device '{}' not found", name))?
        } else {
            host.default_output_device()
                .ok_or("No default output device available")?
        };

        let capabilities = DeviceCapabilities::from_device(&device, false)?;

        // Start with larger buffer when resampling needed
        let (sample_rate, _, _) = capabilities
            .get_best_config(Some(OPUS_SAMPLE_RATE))
            .ok_or("No suitable output configuration")?;

        // Use fixed size ring buffer, not adaptive
        let ring_size = if sample_rate != OPUS_SAMPLE_RATE {
            4800 // 100ms at 48kHz for resampling
        } else {
            2400 // 50ms at 48kHz for direct
        };

        let (ring_producer, ring_consumer) = RingBuffer::new(ring_size);

        info!(
            "📊 Creating output ring buffer: {} samples ({}ms at {} Hz)",
            ring_size,
            (ring_size * 1000) / OPUS_SAMPLE_RATE as usize,
            OPUS_SAMPLE_RATE
        );

        Ok(Self {
            device,
            capabilities,
            stream: None,
            ring_producer: Some(ring_producer),
            ring_consumer: Some(ring_consumer),
            ring_capacity: Arc::new(AtomicUsize::new(ring_size)),
            worker_thread: None,
            should_stop: Arc::new(AtomicBool::new(false)),
            channel_mapping,
            latency_preset: LatencyPreset::Normal,
            user_audio_settings,
            stats,
            decoder_rx: Some(decoder_rx),
            diagnostics,
            drift_ppm: Arc::new(Mutex::new(0.0)),
        })
    }

    pub fn start(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let (sample_rate, channels, format) = self
            .capabilities
            .get_best_config(Some(OPUS_SAMPLE_RATE))
            .ok_or("No suitable output configuration")?;

        info!(
            "🔊 Output: {} @ {}Hz, {} ch, {:?} format",
            self.capabilities.name, sample_rate, channels, format
        );

        // Start worker thread first with proper chunk sizing
        if let (Some(ring_producer), Some(decoder_rx)) =
            (self.ring_producer.take(), self.decoder_rx.take())
        {
            self.worker_thread = Some(Self::start_worker_thread(
                ring_producer,
                decoder_rx,
                self.user_audio_settings.clone(),
                self.stats.clone(),
                self.diagnostics.clone(),
                self.ring_capacity.clone(),
                self.drift_ppm.clone(),
                sample_rate, // Pass sample rate for proper chunk calculation
            ));
        }

        // Build stream
        let stream = match format {
            SampleFormat::I16 => self.build_stream::<i16>(sample_rate, channels)?,
            SampleFormat::U16 => self.build_stream::<u16>(sample_rate, channels)?,
            SampleFormat::F32 => self.build_stream::<f32>(sample_rate, channels)?,
            _ => return Err(format!("Unsupported format: {:?}", format).into()),
        };

        stream.play()?;
        info!("▶️ Output stream started successfully");
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
        // Let the device choose its natural buffer size for stability
        let config = cpal::StreamConfig {
            channels,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        info!("🔊 Using default buffer size for stability");

        let mut ring_consumer = self
            .ring_consumer
            .take()
            .ok_or("Ring buffer consumer already taken")?;

        let channel_mapping = self.channel_mapping.clone();
        let stats = self.stats.clone();
        let stats_err = self.stats.clone();
        let diagnostics = self.diagnostics.clone();
        let drift_ppm = self.drift_ppm.clone();

        // State for the callback
        let mut resampler: Option<ResamplerOut> = None;
        let mut input_buffer = Vec::with_capacity(4096);
        let mut output_scratch = Vec::with_capacity(8192);
        let mut resampler_input = vec![Vec::with_capacity(4096)];
        let mut resampler_output = vec![Vec::with_capacity(8192)];
        let mut initialized = false;
        let mut underrun_count = 0u64;
        let mut last_drift_update = Instant::now();

        let stream = self.device.build_output_stream(
            &config,
            move |data: &mut [T], _: &_| {
                let samples_needed = data.len() / channels as usize;

                // Initialize resampler on first callback
                if !initialized {
                    initialized = true;
                    info!(
                        "🎯 First output callback: {} samples requested, {} total buffer size",
                        samples_needed,
                        data.len()
                    );

                    if sample_rate != OPUS_SAMPLE_RATE {
                        // Calculate the ideal chunk size for the resampler
                        // We want to process in multiples that align well
                        let chunk_size = if samples_needed > 1024 {
                            // Large callbacks - use half the size for lower latency
                            samples_needed / 2
                        } else {
                            samples_needed
                        };

                        match build_output_resampler(sample_rate, chunk_size, false) {
                            Ok(r) => {
                                info!(
                                    "📊 Output resampler: {} input → {} output frames, delay {}",
                                    r.input_frames_next(),
                                    r.output_frames_next(),
                                    r.output_delay()
                                );

                                // Pre-allocate buffers
                                input_buffer.resize(r.input_frames_next(), 0.0);
                                resampler_input[0].reserve(r.input_frames_next());
                                resampler_output[0].reserve(chunk_size * 2);

                                resampler = Some(r);
                            }
                            Err(e) => {
                                error!("Failed to create resampler: {}", e);
                            }
                        }
                    } else {
                        input_buffer.resize(samples_needed, 0.0);
                    }
                    output_scratch.resize(data.len(), 0.0);
                }

                // Apply drift compensation periodically
                if let Some(ref mut r) = resampler {
                    let now = Instant::now();
                    if now.duration_since(last_drift_update) > Duration::from_millis(100) {
                        if let Ok(ppm) = drift_ppm.try_lock() {
                            if ppm.abs() > 1.0 {
                                let base_ratio = sample_rate as f64 / 48000.0;
                                let _ = r.nudge_ratio(base_ratio, *ppm * 0.1); // Apply 10% of drift
                                last_drift_update = now;
                            }
                        }
                    }
                }

                // Process based on resampler requirements
                if let Some(ref mut r) = resampler {
                    // We need to accumulate enough samples for the resampler
                    let mut total_output = Vec::with_capacity(samples_needed);

                    while total_output.len() < samples_needed {
                        let input_needed = r.input_frames_next();

                        // Read exactly what resampler needs
                        let mut got = 0;
                        for i in 0..input_needed {
                            match ring_consumer.pop() {
                                Ok(sample) => {
                                    input_buffer[i] = sample;
                                    got += 1;
                                }
                                Err(_) => {
                                    // Fill rest with silence
                                    for j in i..input_needed {
                                        input_buffer[j] = 0.0;
                                    }
                                    break;
                                }
                            }
                        }

                        if got < input_needed / 2 {
                            // Severe underrun - fill with silence
                            underrun_count += 1;
                            if underrun_count % 100 == 0 {
                                warn!("⚠️ Output underrun #{}", underrun_count);
                            }
                            stats.output_underruns.fetch_add(1, Ordering::Relaxed);

                            // Fill rest with silence
                            for _ in total_output.len()..samples_needed {
                                total_output.push(0.0);
                            }
                            break;
                        }

                        // Resample with timing
                        let resample_start = Instant::now();
                        resampler_input[0].clear();
                        resampler_input[0].extend_from_slice(&input_buffer[..input_needed]);

                        // Make sure output buffer is large enough
                        let expected_output = r.output_frames_next();
                        resampler_output[0].resize(expected_output * 2, 0.0);

                        match r.process_into_buffer(&resampler_input, &mut resampler_output) {
                            Ok(()) => {
                                // Track resampler performance (only occasionally to reduce spam)
                                static DIAG_COUNTER: AtomicU64 = AtomicU64::new(0);
                                let count = DIAG_COUNTER.fetch_add(1, Ordering::Relaxed);
                                if count % 100 == 0 {
                                    // Only update every 100th callback
                                    if let Some(ref diag) = diagnostics {
                                        let resample_time =
                                            resample_start.elapsed().as_micros() as u64;
                                        diag.update_resampler_metrics(
                                            false, // output resampler
                                            input_needed,
                                            expected_output,
                                            expected_output,
                                            resample_time,
                                            sample_rate as f64 / 48000.0, // Pass the expected ratio
                                        );
                                    }
                                }

                                // Take only what we need
                                let produced = expected_output.min(resampler_output[0].len());
                                total_output.extend_from_slice(&resampler_output[0][..produced]);
                            }
                            Err(e) => {
                                error!("Resampler error: {}", e);
                                for _ in total_output.len()..samples_needed {
                                    total_output.push(0.0);
                                }
                                break;
                            }
                        }
                    }

                    // Truncate if we got too much
                    total_output.truncate(samples_needed);

                    // Map to channels
                    let mapping = channel_mapping.lock().unwrap();
                    let mut out_idx = 0;
                    for sample in total_output.iter() {
                        for _ in 0..channels {
                            if out_idx < output_scratch.len() {
                                output_scratch[out_idx] = 0.0;
                                out_idx += 1;
                            }
                        }
                        let channel_start = out_idx.saturating_sub(channels as usize);
                        let scratch_len = output_scratch.len();
                        if channel_start < scratch_len {
                            let end_idx = out_idx.min(scratch_len);
                            mapping.process_output(
                                *sample,
                                &mut output_scratch[channel_start..end_idx],
                                channels as usize,
                            );
                        }
                    }
                } else {
                    // No resampling - direct path
                    let mut samples_read = 0;
                    for i in 0..samples_needed {
                        match ring_consumer.pop() {
                            Ok(sample) => {
                                input_buffer[i] = sample;
                                samples_read += 1;
                            }
                            Err(_) => {
                                for j in i..samples_needed {
                                    input_buffer[j] = 0.0;
                                }
                                break;
                            }
                        }
                    }

                    if samples_read < samples_needed {
                        stats.output_underruns.fetch_add(1, Ordering::Relaxed);
                    }

                    // Map to channels
                    let mapping = channel_mapping.lock().unwrap();
                    let mut out_idx = 0;
                    for sample in input_buffer[..samples_needed].iter() {
                        for _ in 0..channels {
                            if out_idx < output_scratch.len() {
                                output_scratch[out_idx] = 0.0;
                                out_idx += 1;
                            }
                        }
                        let channel_start = out_idx.saturating_sub(channels as usize);
                        let scratch_len = output_scratch.len();
                        if channel_start < scratch_len {
                            let end_idx = out_idx.min(scratch_len);
                            mapping.process_output(
                                *sample,
                                &mut output_scratch[channel_start..end_idx],
                                channels as usize,
                            );
                        }
                    }
                }

                // Convert to output format
                for (out, &sample) in data.iter_mut().zip(output_scratch.iter()) {
                    *out = T::from_sample(sample.clamp(-0.99, 0.99));
                }

                stats.output_callbacks.fetch_add(1, Ordering::Relaxed);

                // Also update diagnostics callback counter
                if let Some(ref diag) = diagnostics {
                    diag.output_callback_count.fetch_add(1, Ordering::Relaxed);
                }
            },
            move |err| {
                error!("Output stream error: {}", err);
                stats_err.stream_errors.fetch_add(1, Ordering::Relaxed);
            },
            None,
        )?;

        Ok(stream)
    }

    fn start_worker_thread(
        mut ring_producer: Producer<f32>,
        decoder_rx: Receiver<(ClientId, Vec<f32>)>,
        user_audio_settings: Arc<Mutex<HashMap<ClientId, UserAudioSettings>>>,
        stats: Arc<AudioStats>,
        diagnostics: Option<Arc<PipelineDiagnostics>>,
        _ring_capacity_hint: Arc<AtomicUsize>, // not used; rtrb capacity is fixed
        drift_ppm: Arc<Mutex<f64>>,
        device_sample_rate: u32,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            // --- Engine is 48k; mix/jitter happen at 48k ---
            let mut jitter_buffer = AdaptiveJitterBuffer::new(
                OPUS_SAMPLE_RATE as usize,
                if device_sample_rate != OPUS_SAMPLE_RATE {
                    40
                } else {
                    20
                },
                user_audio_settings,
            );

            // Real ring capacity & watermarks
            let capacity = ring_producer.buffer().capacity();
            let low_wm = capacity / 4; // 25%
            let high_wm = capacity * 3 / 4; // 75%
            let very_high_wm = capacity * 85 / 100; // 85%

            // 48k-aligned processing (use bigger hop when resampling to reduce SRC pressure downstream)
            let base_hop = 480usize; // 10 ms @ 48k
            let process_chunk = if device_sample_rate != OPUS_SAMPLE_RATE {
                base_hop * 2
            } else {
                base_hop
            };

            // Working buffers
            let mut accumulator: Vec<f32> = vec![0.0; process_chunk];

            let mut last_diag_flush = Instant::now();
            let mut last_drift_calc = Instant::now();
            let mut producer_xruns = 0u64; // producer-side overflow events

            loop {
                // 1) Ingest decoded audio quickly (bounded)
                const MAX_BATCH: usize = 16;
                let mut batched = 0;
                while batched < MAX_BATCH {
                    match decoder_rx.try_recv() {
                        Ok((client_id, samples)) => {
                            jitter_buffer.push_frame(client_id, samples);
                            stats.jitter_buffer_frames.fetch_add(1, Ordering::Relaxed);
                            batched += 1;
                        }
                        Err(_) => break,
                    }
                }

                // 2) Backpressure from ring occupancy
                let slots = ring_producer.slots();
                let occ = capacity - slots;

                if occ > very_high_wm {
                    // Near overflow: back off a touch
                    producer_xruns += 1;
                    // Do not log here (hot path)
                    thread::sleep(Duration::from_millis(2));
                    continue;
                } else if occ > high_wm {
                    thread::sleep(Duration::from_micros(400));
                    continue;
                }

                // 3) Build an exact 48k chunk (no zero-padding logic needed; your API fills silence)
                jitter_buffer.pull_48k_exact(process_chunk, &mut accumulator[..]);

                // 4) Push to the ring
                let mut pushed = 0usize;
                for &s in &accumulator {
                    match ring_producer.push(s) {
                        Ok(()) => {
                            pushed += 1;
                        }
                        Err(_full) => {
                            // Producer overflow (consumer too slow)
                            producer_xruns += 1;
                            stats.producer_overruns.fetch_add(1, Ordering::Relaxed);
                            break;
                        }
                    }
                }

                // 5) Lightweight diagnostics/counters
                if let Some(ref diag) = diagnostics {
                    diag.output_ring.record_push(pushed);
                    diag.output_ring.update_occupancy(occ);
                }
                let jb_ms = jitter_buffer.get_occupancy_ms();
                stats
                    .jitter_buffer_occupancy
                    .store(jb_ms as usize, Ordering::Relaxed);

                // 6) Periodic drift estimate (smoothed) — no direct retarget since your buffer has no setters
                if last_drift_calc.elapsed() >= Duration::from_millis(500) {
                    let measured = jitter_buffer.calculate_drift_ppm(); // ±1000 ppm clamp inside
                    if let Ok(mut ppm) = drift_ppm.try_lock() {
                        *ppm = 0.9 * *ppm + 0.1 * measured;
                    }
                    last_drift_calc = Instant::now();
                }

                // 7) Let the jitter buffer apply its own occupancy-based policy
                jitter_buffer.adapt();

                // 8) Gentle CPU relief when plenty of space and nothing just arrived
                if batched == 0 && occ < low_wm {
                    thread::sleep(Duration::from_micros(200));
                }

                // 9) Infrequent anomaly publication off hot path
                if let Some(ref diag) = diagnostics {
                    if last_diag_flush.elapsed() >= Duration::from_secs(1) {
                        if producer_xruns > 0 {
                            diag.record_anomaly(Anomaly {
                                stage: "producer",
                                issue: AnomalyType::BufferOverflow,
                                value: producer_xruns as f64,
                                expected_range: (0.0, 0.0),
                                timestamp: Instant::now(),
                            });
                            producer_xruns = 0;
                        }
                        last_diag_flush = Instant::now();
                    }
                }
            }
        })
    }

    pub fn set_latency_preset(&mut self, preset: LatencyPreset) {
        self.latency_preset = preset;
    }

    pub fn get_device_info(&self) -> String {
        format!(
            "Output: {} - Rates: {:?}, Channels: {:?}, Formats: {:?}",
            self.capabilities.name,
            self.capabilities.sample_rates,
            self.capabilities.channels,
            self.capabilities.sample_formats
        )
    }
}

impl Drop for AudioOutput {
    fn drop(&mut self) {
        self.should_stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.worker_thread.take() {
            let _ = thread.join();
        }
    }
}
