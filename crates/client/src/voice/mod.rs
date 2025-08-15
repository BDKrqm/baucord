// voice/mod.rs - Enhanced version with better device handling
mod vad;
use crate::settings::{Settings, UserAudioSettings};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleRate, SupportedStreamConfig};
use log::{error, info, warn};
use opus::{Application, Bitrate, Channels, Decoder, Encoder};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use vad::{VADConfig, VADProcessor, VADProfile};

// ============= OPTIMIZED CONSTANTS FOR LOW LATENCY =============
const OPUS_SAMPLE_RATE: u32 = 48000;
const FRAME_SIZE: usize = 480; // 10ms at 48kHz for lower latency
const OPUS_BITRATE: i32 = 24000; // 24kbps is sufficient for 10ms frames

// Minimal buffering for low latency
const TARGET_BUFFER_SIZE: usize = 960; // 20ms buffer (2 frames)
const MAX_BUFFER_SIZE: usize = 2400; // 50ms max (5 frames)

use rubato::{FastFixedIn, PolynomialDegree, Resampler};

// ============= DEVICE VALIDATION AND SELECTION =============

/// Device capability information
#[derive(Debug, Clone)]
struct DeviceCapabilities {
    name: String,
    sample_rates: Vec<u32>,
    channels: Vec<u16>,
    preferred_config: Option<SupportedStreamConfig>,
}

impl DeviceCapabilities {
    pub fn supports_sample_rate(&self, rate: u32) -> bool {
        self.sample_rates.contains(&rate)
    }

    // Add method to check channel configuration
    pub fn supports_channels(&self, channels: u16) -> bool {
        self.channels.contains(&channels)
    }

    // Use get_best_config in device selection
    pub fn get_optimal_config(&self, is_input: bool) -> Option<(u32, u16)> {
        // Prefer 48kHz for Opus compatibility
        self.get_best_config(Some(48000))
    }
    fn from_device(
        device: &cpal::Device,
        is_input: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let name = device.name().unwrap_or_else(|_| "Unknown".to_string());

        let configs: Vec<_> = if is_input {
            device.supported_input_configs()?.collect()
        } else {
            device.supported_output_configs()?.collect()
        };

        let mut sample_rates = Vec::new();
        let mut channels = Vec::new();

        for config in &configs {
            let min_rate = config.min_sample_rate().0;
            let max_rate = config.max_sample_rate().0;

            // Common sample rates to check
            for rate in [8000, 16000, 44100, 48000, 96000, 192000] {
                if rate >= min_rate && rate <= max_rate && !sample_rates.contains(&rate) {
                    sample_rates.push(rate);
                }
            }

            let ch = config.channels();
            if !channels.contains(&ch) {
                channels.push(ch);
            }
        }

        // Find preferred config (prefer 48kHz mono/stereo)
        let preferred_config = configs
            .into_iter()
            .find(|c| {
                c.min_sample_rate() <= SampleRate(48000)
                    && c.max_sample_rate() >= SampleRate(48000)
                    && (c.channels() == 1 || c.channels() == 2)
            })
            .or_else(|| {
                // Fallback to 44.1kHz
                if is_input {
                    device.supported_input_configs().ok()?.find(|c| {
                        c.min_sample_rate() <= SampleRate(44100)
                            && c.max_sample_rate() >= SampleRate(44100)
                    })
                } else {
                    device.supported_output_configs().ok()?.find(|c| {
                        c.min_sample_rate() <= SampleRate(44100)
                            && c.max_sample_rate() >= SampleRate(44100)
                    })
                }
            })
            .map(|c| c.with_max_sample_rate());

        Ok(Self {
            name,
            sample_rates,
            channels,
            preferred_config,
        })
    }

    fn get_best_config(&self, preferred_rate: Option<u32>) -> Option<(u32, u16)> {
        // Priority: requested rate > 48kHz > 96kHz > 44.1kHz > anything
        let rate = if let Some(r) = preferred_rate {
            if self.sample_rates.contains(&r) {
                r
            } else {
                *self
                    .sample_rates
                    .iter()
                    .min_by_key(|&&sr| (sr as i32 - r as i32).abs())
                    .unwrap_or(&48000)
            }
        } else if self.sample_rates.contains(&48000) {
            48000
        } else if self.sample_rates.contains(&96000) {
            96000
        } else if self.sample_rates.contains(&44100) {
            44100
        } else {
            *self.sample_rates.first()?
        };

        // Prefer mono for input, stereo for output
        let channels = if self.channels.contains(&1) {
            1
        } else if self.channels.contains(&2) {
            2
        } else {
            *self.channels.first()?
        };

        Some((rate, channels))
    }
}

/// Helper to get a device with validation and fallback
fn get_device_with_fallback(
    host: &cpal::Host,
    device_name: Option<&str>,
    is_input: bool,
) -> Result<(cpal::Device, DeviceCapabilities), Box<dyn std::error::Error>> {
    // Try to get the requested device
    if let Some(name) = device_name {
        let devices = if is_input {
            host.input_devices()?
        } else {
            host.output_devices()?
        };

        for device in devices {
            if device.name().ok().as_deref() == Some(name) {
                match DeviceCapabilities::from_device(&device, is_input) {
                    Ok(caps) => {
                        // Use get_best_config to validate
                        if let Some(config) = caps.get_best_config(Some(48000)) {
                            info!(
                                "✅ Using requested device: {} ({}Hz, {} ch)",
                                name, config.0, config.1
                            );
                            return Ok((device, caps));
                        } else {
                            warn!("⚠️ Device '{}' found but has no suitable config", name);
                        }
                    }
                    Err(e) => {
                        warn!("⚠️ Device '{}' found but failed validation: {}", name, e);
                    }
                }
            }
        }

        warn!(
            "⚠️ Requested device '{}' not found or not suitable, falling back to default",
            name
        );
    }

    // Fall back to default device
    let device = if is_input {
        host.default_input_device()
            .ok_or("No input device available")?
    } else {
        host.default_output_device()
            .ok_or("No output device available")?
    };

    let caps = DeviceCapabilities::from_device(&device, is_input)?;

    // Validate using get_best_config
    if caps.get_best_config(Some(48000)).is_none() {
        return Err("Default device has no suitable configuration".into());
    }

    info!(
        "✅ Using default device: {} (supports {} sample rates, {} channel configs)",
        caps.name,
        caps.sample_rates.len(),
        caps.channels.len()
    );
    Ok((device, caps))
}

// 4. Update VoiceChat to expose get_device_info properly
impl VoiceChat {
    /// Get detailed device information (NOW USED IN SIDEBAR)
    pub fn get_device_info(&self) -> String {
        let mut info = String::new();

        // Show INPUT device info
        if let Some(ref caps) = self.input_capabilities {
            info.push_str(&format!("Input: {}\n", caps.name));

            // Show supported rates
            if !caps.sample_rates.is_empty() {
                info.push_str(&format!("  Rates: {:?} Hz\n", caps.sample_rates));
            }

            // Show supported channels
            if !caps.channels.is_empty() {
                info.push_str(&format!("  Channels: {:?}\n", caps.channels));
            }

            // Show ACTUAL configuration in use (from input_config)
            if let Some(ref config) = *self.input_config.lock().unwrap() {
                info.push_str(&format!(
                    "  Using: {}Hz, {} ch\n",
                    config.sample_rate, config.channels
                ));

                // Show if resampling is happening
                if config.sample_rate != OPUS_SAMPLE_RATE {
                    info.push_str(&format!(
                        "  ⚠️ Resampling: {} → 48000 Hz\n",
                        config.sample_rate
                    ));
                }
            } else {
                // Fallback to preferred config if stream not started
                if let Some(config) = caps.get_best_config(Some(48000)) {
                    info.push_str(&format!("  Preferred: {}Hz, {} ch\n", config.0, config.1));
                }
            }
        } else {
            info.push_str("Input: Not initialized\n");
        }

        info.push_str("\n");

        // Show OUTPUT device info
        if let Some(ref caps) = self.output_capabilities {
            info.push_str(&format!("Output: {}\n", caps.name));

            // Show supported rates
            if !caps.sample_rates.is_empty() {
                info.push_str(&format!("  Rates: {:?} Hz\n", caps.sample_rates));
            }

            // Show supported channels
            if !caps.channels.is_empty() {
                info.push_str(&format!("  Channels: {:?}\n", caps.channels));
            }

            // Show ACTUAL configuration in use (from output_config)
            if let Some(ref config) = *self.output_config.lock().unwrap() {
                info.push_str(&format!(
                    "  Using: {}Hz, {} ch\n",
                    config.sample_rate, config.channels
                ));

                // Show if resampling is happening
                if config.sample_rate != OPUS_SAMPLE_RATE {
                    info.push_str(&format!(
                        "  ⚠️ Resampling: 48000 → {} Hz\n",
                        config.sample_rate
                    ));
                }
            } else {
                // Fallback to preferred config if stream not started
                if let Some(config) = caps.get_best_config(Some(48000)) {
                    info.push_str(&format!("  Preferred: {}Hz, {} ch\n", config.0, config.1));
                }
            }
        } else {
            info.push_str("Output: Not initialized\n");
        }

        // Add audio pipeline summary
        info.push_str("\n📊 Audio Pipeline:\n");

        let input_rate = self
            .input_config
            .lock()
            .unwrap()
            .as_ref()
            .map(|c| c.sample_rate)
            .unwrap_or(0);
        let output_rate = self
            .output_config
            .lock()
            .unwrap()
            .as_ref()
            .map(|c| c.sample_rate)
            .unwrap_or(0);

        if input_rate > 0 && output_rate > 0 {
            info.push_str(&format!(
                "  {}Hz → Opus 48kHz → {}Hz\n",
                input_rate, output_rate
            ));

            // Warn about quality issues
            if input_rate != 48000 || output_rate != 48000 {
                info.push_str("  ⚠️ Resampling may affect quality\n");

                // Provide specific recommendations
                if output_rate == 44100 {
                    info.push_str("  💡 Try 48kHz output device for best quality\n");
                }
            } else {
                info.push_str("  ✅ No resampling - best quality\n");
            }
        }

        info
    }
}

// ============= LOW LATENCY JITTER BUFFER (unchanged) =============
struct JitterBuffer {
    sample_rate: usize,
    target_size: usize,
    buffers: HashMap<renet::ClientId, VecDeque<f32>>,
    user_audio_settings: Arc<Mutex<HashMap<renet::ClientId, UserAudioSettings>>>,
}

impl JitterBuffer {
    fn new(
        sample_rate: usize,
        target_size: usize,
        user_audio_settings: Arc<Mutex<HashMap<renet::ClientId, UserAudioSettings>>>,
    ) -> Self {
        Self {
            sample_rate,
            target_size,
            buffers: HashMap::new(),
            user_audio_settings,
        }
    }

    fn push_frame(&mut self, client_id: renet::ClientId, samples: Vec<f32>) {
        let settings = self.user_audio_settings.lock().unwrap();
        if let Some(user_settings) = settings.get(&client_id) {
            if user_settings.is_muted {
                return;
            }
        }
        drop(settings);

        let buffer = self.buffers.entry(client_id).or_default();
        buffer.extend(samples);

        // Use MAX_BUFFER_SIZE constant properly
        while buffer.len() > MAX_BUFFER_SIZE {
            buffer.pop_front();
        }
    }

    fn get_mixed_samples(&mut self, count: usize) -> Vec<f32> {
        let mut mixed = vec![0.0f32; count];
        let mut active_streams = 0;
        let settings = self.user_audio_settings.lock().unwrap();

        for (client_id, buffer) in self.buffers.iter_mut() {
            let user_settings = settings.get(client_id).cloned().unwrap_or_default();

            if user_settings.is_muted {
                buffer.clear();
                continue;
            }

            // Use target_size for minimal buffering check
            if buffer.len() >= self.target_size || buffer.len() >= count {
                active_streams += 1;
                for i in 0..count.min(buffer.len()) {
                    if let Some(sample) = buffer.pop_front() {
                        mixed[i] += sample * user_settings.volume;
                    }
                }
            }
        }

        if active_streams > 1 {
            let scale = 1.0 / (active_streams as f32).sqrt();
            for sample in mixed.iter_mut() {
                *sample *= scale;
            }
        }

        mixed
    }
}

// ============= OPTIMIZED RESAMPLER (unchanged) =============
struct AudioResampler {
    resampler: Option<FastFixedIn<f32>>,
    input_buffer: Vec<Vec<f32>>,
    from_rate: u32,
    to_rate: u32,
}

impl AudioResampler {
    fn new(from_rate: u32, to_rate: u32) -> Self {
        if from_rate == to_rate {
            return Self {
                resampler: None,
                input_buffer: vec![],
                from_rate,
                to_rate,
            };
        }

        if from_rate == 96000 && to_rate == 48000 {
            info!("🎯 Using optimized 2:1 decimation for 96kHz -> 48kHz");
            return Self {
                resampler: None,
                input_buffer: vec![],
                from_rate,
                to_rate,
            };
        }

        let ratio = to_rate as f64 / from_rate as f64;

        // Special handling for 48k -> 44.1k conversion
        let (quality, degree, chunk_size) = if from_rate == 48000 && to_rate == 44100 {
            info!("🎵 Configuring high-quality resampler for 48kHz → 44.1kHz");
            (2.0, PolynomialDegree::Cubic, 1024)
        } else {
            (1.5, PolynomialDegree::Linear, 480)
        };

        let resampler = FastFixedIn::<f32>::new(ratio, quality, degree, chunk_size, 1)
            .expect("Failed to create resampler");

        Self {
            resampler: Some(resampler),
            input_buffer: vec![vec![0.0; chunk_size]],
            from_rate,
            to_rate,
        }
    }

    fn resample_f32(&mut self, input: &[f32]) -> Vec<f32> {
        if self.from_rate == self.to_rate {
            return input.to_vec();
        }

        if self.from_rate == 96000 && self.to_rate == 48000 {
            return input.iter().step_by(2).copied().collect();
        }

        if let Some(resampler) = &mut self.resampler {
            let chunk_size = resampler.input_frames_next();
            let mut output = Vec::new();

            for chunk in input.chunks(chunk_size) {
                // Make sure we have a complete chunk
                if chunk.len() == chunk_size {
                    self.input_buffer[0].copy_from_slice(chunk);
                } else {
                    // Pad with zeros if incomplete
                    self.input_buffer[0][..chunk.len()].copy_from_slice(chunk);
                    for i in chunk.len()..chunk_size {
                        self.input_buffer[0][i] = 0.0;
                    }
                }

                if let Ok(resampled) = resampler.process(&self.input_buffer, None) {
                    output.extend_from_slice(&resampled[0]);
                }
            }

            output
        } else {
            input.to_vec()
        }
    }
}

// ============= VOICE PACKET (unchanged) =============
#[derive(Debug, Clone)]
pub enum VoicePacket {
    AudioData {
        client_id: renet::ClientId,
        opus_data: Vec<u8>,
    },
    Speaking {
        client_id: renet::ClientId,
        is_speaking: bool,
    },
}

#[derive(Clone, Debug)]
struct AudioConfig {
    sample_rate: u32,
    channels: u16,
    device_name: String,
}

// ============= ENHANCED VOICE CHAT =============
pub struct VoiceChat {
    // Opus codec (always 48kHz)
    encoder: Arc<Mutex<Encoder>>,
    decoders: Arc<Mutex<HashMap<renet::ClientId, Decoder>>>,

    // Audio streams
    input_stream: Arc<Mutex<Option<cpal::Stream>>>,
    output_stream: Arc<Mutex<Option<cpal::Stream>>>,

    // Audio configuration
    input_config: Arc<Mutex<Option<AudioConfig>>>,
    output_config: Arc<Mutex<Option<AudioConfig>>>,

    // VAD processor
    vad_processor: Arc<Mutex<VADProcessor>>,

    // Audio buffers
    input_buffer: Arc<Mutex<VecDeque<i16>>>,

    // Network channels
    outgoing_tx: mpsc::Sender<VoicePacket>,
    outgoing_rx: Arc<Mutex<mpsc::Receiver<VoicePacket>>>,
    incoming_packets: Arc<Mutex<VecDeque<VoicePacket>>>,

    // Controls
    is_muted: Arc<Mutex<bool>>,
    is_speaking: Arc<AtomicBool>,
    loopback_enabled: Arc<Mutex<bool>>,

    // Debug counters
    packets_sent: Arc<AtomicU64>,
    packets_received: Arc<AtomicU64>,

    // Devices and capabilities
    input_device: Option<cpal::Device>,
    output_device: Option<cpal::Device>,
    input_capabilities: Option<DeviceCapabilities>,
    output_capabilities: Option<DeviceCapabilities>,
    user_audio_settings: Arc<Mutex<HashMap<renet::ClientId, UserAudioSettings>>>,
}

impl VoiceChat {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let mut encoder = Encoder::new(OPUS_SAMPLE_RATE, Channels::Mono, Application::Voip)?;
        encoder.set_bitrate(Bitrate::Bits(OPUS_BITRATE))?;
        encoder.set_vbr(false)?;
        encoder.set_inband_fec(false)?;
        encoder.set_packet_loss_perc(0)?;

        let vad_config = VADConfig {
            sample_rate: OPUS_SAMPLE_RATE,
            chunk_size: FRAME_SIZE,
            threshold: 0.15,
            profile: VADProfile::Aggressive,
        };

        let mut vad_processor = VADProcessor::new(vad_config)?;
        let (outgoing_tx, outgoing_rx) = mpsc::channel();
        vad_processor.set_sender(outgoing_tx.clone());

        Ok(Self {
            encoder: Arc::new(Mutex::new(encoder)),
            decoders: Arc::new(Mutex::new(HashMap::new())),
            input_stream: Arc::new(Mutex::new(None)),
            output_stream: Arc::new(Mutex::new(None)),
            input_device: None,
            output_device: None,
            input_capabilities: None,
            output_capabilities: None,
            input_config: Arc::new(Mutex::new(None)),
            output_config: Arc::new(Mutex::new(None)),
            vad_processor: Arc::new(Mutex::new(vad_processor)),
            input_buffer: Arc::new(Mutex::new(VecDeque::new())),
            outgoing_tx,
            outgoing_rx: Arc::new(Mutex::new(outgoing_rx)),
            incoming_packets: Arc::new(Mutex::new(VecDeque::new())),
            is_muted: Arc::new(Mutex::new(false)),
            is_speaking: Arc::new(AtomicBool::new(false)),
            loopback_enabled: Arc::new(Mutex::new(false)),
            packets_sent: Arc::new(AtomicU64::new(0)),
            packets_received: Arc::new(AtomicU64::new(0)),
            user_audio_settings: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn start(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        info!("Starting LOW LATENCY voice chat...");
        info!(
            "Frame size: {}ms",
            FRAME_SIZE as f32 * 1000.0 / OPUS_SAMPLE_RATE as f32
        );

        // Start with more detailed error handling
        let input_result = self.start_input_stream();
        let output_result = self.start_output_stream();

        match (input_result, output_result) {
            (Ok(_), Ok(_)) => {
                info!("✅ Voice chat started successfully");
                Ok(())
            }
            (Err(e), Ok(_)) => {
                error!("❌ Input device error: {}", e);
                // Still allow output-only mode
                warn!("⚠️ Running in listen-only mode (no microphone)");
                Ok(())
            }
            (Ok(_), Err(e)) => {
                error!("❌ Output device error: {}", e);
                // Still allow input-only mode
                warn!("⚠️ Running in speak-only mode (no speakers)");
                Ok(())
            }
            (Err(ie), Err(oe)) => {
                error!("❌ Both devices failed - Input: {}, Output: {}", ie, oe);
                Err(format!("No audio devices available").into())
            }
        }
    }

    /// Get detailed device information

    /// Get list of available input devices with validation
    pub fn get_input_devices(&self) -> Result<Vec<(String, bool)>, Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let mut devices = Vec::new();

        for device in host.input_devices()? {
            if let Ok(name) = device.name() {
                let is_valid = DeviceCapabilities::from_device(&device, true)
                    .map(|c| c.preferred_config.is_some())
                    .unwrap_or(false);
                devices.push((name, is_valid));
            }
        }

        Ok(devices)
    }

    /// Get list of available output devices with validation
    pub fn get_output_devices(&self) -> Result<Vec<(String, bool)>, Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let mut devices = Vec::new();

        for device in host.output_devices()? {
            if let Ok(name) = device.name() {
                let is_valid = DeviceCapabilities::from_device(&device, false)
                    .map(|c| c.preferred_config.is_some())
                    .unwrap_or(false);
                devices.push((name, is_valid));
            }
        }

        Ok(devices)
    }

    pub fn set_input_device(
        &mut self,
        device_name: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let (device, caps) = get_device_with_fallback(&host, device_name, true)?;

        self.input_device = Some(device);
        self.input_capabilities = Some(caps);

        if self.is_active() {
            self.restart_input_stream()?;
        }

        Ok(())
    }

    pub fn set_output_device(
        &mut self,
        device_name: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let (device, caps) = get_device_with_fallback(&host, device_name, false)?;

        self.output_device = Some(device);
        self.output_capabilities = Some(caps);

        if self.is_active() {
            self.restart_output_stream()?;
        }

        Ok(())
    }

    fn start_input_stream(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let host = cpal::default_host();

        // Get device with fallback
        let (device, caps) =
            if let (Some(d), Some(c)) = (&self.input_device, &self.input_capabilities) {
                (d.clone(), c.clone())
            } else {
                let device_name = self.input_device.as_ref().and_then(|d| d.name().ok());
                get_device_with_fallback(&host, device_name.as_deref(), true)?
            };

        // Store for future use
        self.input_device = Some(device.clone());
        self.input_capabilities = Some(caps.clone());

        let device_name = caps.name.clone();
        info!("🎤 Input device: {}", device_name);

        // Get the best configuration
        let config = caps
            .preferred_config
            .ok_or("No suitable input configuration found")?;

        let stream_config: cpal::StreamConfig = config.clone().into();
        let channels = config.channels();
        let sample_rate = stream_config.sample_rate.0;

        info!("📊 Input: {} channels @ {}Hz", channels, sample_rate);

        *self.input_config.lock().unwrap() = Some(AudioConfig {
            sample_rate,
            channels,
            device_name,
        });

        // Create resampler if needed
        let mut resampler = if sample_rate != OPUS_SAMPLE_RATE {
            Some(AudioResampler::new(sample_rate, OPUS_SAMPLE_RATE))
        } else {
            info!("✅ Native 48kHz - no resampling needed!");
            None
        };

        // Clone all the Arcs for the closure
        let input_buffer = self.input_buffer.clone();
        let vad_processor = self.vad_processor.clone();
        let is_muted = self.is_muted.clone();
        let is_speaking = self.is_speaking.clone();
        let encoder = self.encoder.clone();
        let outgoing_tx = self.outgoing_tx.clone();
        let loopback_enabled = self.loopback_enabled.clone();
        let incoming_packets = self.incoming_packets.clone();
        let packets_sent = self.packets_sent.clone();

        let input_stream = device.build_input_stream(
            &stream_config,
            move |data: &[f32], _: &_| {
                if *is_muted.lock().unwrap() {
                    is_speaking.store(false, Ordering::Relaxed);
                    return;
                }

                let mono_samples: Vec<f32> = if channels == 2 {
                    data.chunks_exact(2).map(|c| (c[0] + c[1]) * 0.5).collect()
                } else {
                    data.to_vec()
                };

                let resampled_samples = if let Some(ref mut r) = resampler {
                    r.resample_f32(&mono_samples)
                } else {
                    mono_samples
                };

                let samples_i16: Vec<i16> = resampled_samples
                    .iter()
                    .map(|&x| (x.clamp(-1.0, 1.0) * 32767.0) as i16)
                    .collect();

                let mut buffer = input_buffer.lock().unwrap();
                buffer.extend(&samples_i16);

                while buffer.len() >= FRAME_SIZE {
                    let frame: Vec<i16> = buffer.drain(..FRAME_SIZE).collect();
                    let speaking = vad_processor.lock().unwrap().process_audio(&frame);
                    is_speaking.store(speaking, Ordering::Relaxed);

                    if speaking {
                        let mut output = vec![0u8; 256];
                        match encoder.lock().unwrap().encode(&frame, &mut output) {
                            Ok(size) => {
                                output.truncate(size);
                                let _ = outgoing_tx.send(VoicePacket::AudioData {
                                    client_id: 0,
                                    opus_data: output.clone(),
                                });

                                if *loopback_enabled.lock().unwrap() {
                                    incoming_packets.lock().unwrap().push_back(
                                        VoicePacket::AudioData {
                                            client_id: 999999,
                                            opus_data: output,
                                        },
                                    );
                                }
                                packets_sent.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(e) => error!("❌ Encoding error: {}", e),
                        }
                    }
                }
            },
            |err| error!("❌ Input stream error: {}", err),
            None,
        )?;

        input_stream.play()?;
        *self.input_stream.lock().unwrap() = Some(input_stream);

        Ok(())
    }

    fn start_output_stream(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let host = cpal::default_host();

        // Get device with fallback
        let (device, caps) =
            if let (Some(d), Some(c)) = (&self.output_device, &self.output_capabilities) {
                (d.clone(), c.clone())
            } else {
                let device_name = self.output_device.as_ref().and_then(|d| d.name().ok());
                get_device_with_fallback(&host, device_name.as_deref(), false)?
            };

        // Store for future use
        self.output_device = Some(device.clone());
        self.output_capabilities = Some(caps.clone());

        let device_name = caps.name.clone();
        info!("🔊 Output device: {}", device_name);

        // Get the best configuration - but prefer 48kHz if available
        let config = if let Some(ref caps) = self.output_capabilities {
            // Try to find 48kHz config first
            let configs: Vec<_> = device.supported_output_configs()?.collect();

            configs
                .iter()
                .find(|c| {
                    c.min_sample_rate() <= SampleRate(48000)
                        && c.max_sample_rate() >= SampleRate(48000)
                        && c.channels() == 4 // Your EVO4 has 4 channels
                })
                .map(|c| c.clone().with_sample_rate(SampleRate(48000)))
                .or_else(|| {
                    // Fall back to whatever is available
                    caps.preferred_config.clone()
                })
                .ok_or("No suitable output configuration found")?
        } else {
            return Err("No output capabilities".into());
        };

        let stream_config: cpal::StreamConfig = config.clone().into();
        let channels = config.channels();
        let sample_rate = stream_config.sample_rate.0;

        info!("📊 Output: {} channels @ {}Hz", channels, sample_rate);

        *self.output_config.lock().unwrap() = Some(AudioConfig {
            sample_rate,
            channels,
            device_name,
        });

        // Create resampler if needed - FIXED LOGIC
        let output_resampler = if sample_rate != OPUS_SAMPLE_RATE {
            info!("🔄 Creating resampler: 48000Hz → {}Hz", sample_rate);

            // Use higher quality resampling for 44.1kHz
            let ratio = sample_rate as f64 / OPUS_SAMPLE_RATE as f64;

            // For 44.1kHz, we need a high-quality resampler
            let resampler = if sample_rate == 44100 {
                // Use cubic interpolation for 48k->44.1k
                FastFixedIn::<f32>::new(
                    ratio,
                    2.0,                     // Higher quality
                    PolynomialDegree::Cubic, // Good balance of quality/performance
                    1024,                    // Larger chunk size for 44.1kHz
                    1,
                )
                .expect("Failed to create 44.1kHz resampler")
            } else {
                // Generic resampler for other rates
                FastFixedIn::<f32>::new(ratio, 1.5, PolynomialDegree::Linear, 480, 1)
                    .expect("Failed to create resampler")
            };

            Some(Arc::new(Mutex::new(AudioResampler {
                resampler: Some(resampler),
                input_buffer: vec![vec![0.0; 1024]], // Larger buffer for 44.1k
                from_rate: OPUS_SAMPLE_RATE,
                to_rate: sample_rate,
            })))
        } else {
            info!("✅ Native 48kHz output - no resampling needed!");
            None
        };

        let decoders = self.decoders.clone();
        let incoming_packets = self.incoming_packets.clone();
        let packets_received = self.packets_received.clone();

        let jitter_buffer = Arc::new(Mutex::new(JitterBuffer::new(
            OPUS_SAMPLE_RATE as usize,
            TARGET_BUFFER_SIZE,
            self.user_audio_settings.clone(),
        )));

        let jitter_buffer_clone = jitter_buffer.clone();
        let user_audio_settings = self.user_audio_settings.clone();

        // Decoder thread
        std::thread::spawn(move || {
            let mut frame_buffer = vec![0i16; FRAME_SIZE * 2];

            loop {
                std::thread::sleep(std::time::Duration::from_millis(2));

                let packets: Vec<VoicePacket> = {
                    let mut incoming = incoming_packets.lock().unwrap();
                    incoming.drain(..).collect()
                };

                for packet in packets {
                    match packet {
                        VoicePacket::AudioData {
                            client_id,
                            opus_data,
                        } => {
                            let should_process = {
                                let settings = user_audio_settings.lock().unwrap();
                                settings
                                    .get(&client_id)
                                    .map(|s| !s.is_muted)
                                    .unwrap_or(true)
                            };

                            if !should_process {
                                continue;
                            }

                            packets_received.fetch_add(1, Ordering::Relaxed);

                            let mut decoders_map = decoders.lock().unwrap();
                            let decoder = decoders_map.entry(client_id).or_insert_with(|| {
                                Decoder::new(OPUS_SAMPLE_RATE, Channels::Mono).unwrap()
                            });

                            match decoder.decode(&opus_data, &mut frame_buffer, false) {
                                Ok(samples) => {
                                    let samples_f32: Vec<f32> = frame_buffer[..samples]
                                        .iter()
                                        .map(|&x| x as f32 / 32768.0)
                                        .collect();

                                    jitter_buffer_clone
                                        .lock()
                                        .unwrap()
                                        .push_frame(client_id, samples_f32);
                                }
                                Err(e) => error!("❌ Decode error: {}", e),
                            }
                        }
                        _ => {}
                    }
                }
            }
        });

        // Output stream callback - FIXED SAMPLE CALCULATION
        let output_stream = device.build_output_stream(
            &stream_config,
            move |data: &mut [f32], _: &_| {
                // Calculate how many samples we need from the buffer
                let device_samples_needed = data.len() / channels as usize;

                // FIXED: Calculate opus samples correctly
                // For 44.1kHz output from 48kHz opus: we need MORE opus samples
                // 44100/48000 = 0.91875, so we need 1/0.91875 = 1.088 times more input samples
                let opus_samples_needed = if sample_rate != OPUS_SAMPLE_RATE {
                    // This is the key fix: proper sample count calculation
                    ((device_samples_needed as f64 * OPUS_SAMPLE_RATE as f64) / sample_rate as f64)
                        .ceil() as usize
                } else {
                    device_samples_needed
                };

                // Get the mixed samples at opus rate (48kHz)
                let mixed_samples = jitter_buffer
                    .lock()
                    .unwrap()
                    .get_mixed_samples(opus_samples_needed);

                // Resample if needed
                let device_samples = if let Some(ref resampler) = output_resampler {
                    let mut r = resampler.lock().unwrap();

                    // Process in chunks that the resampler expects
                    if sample_rate == 44100 {
                        // Special handling for 44.1kHz
                        let mut output = Vec::with_capacity(device_samples_needed);

                        // Process available samples
                        if !mixed_samples.is_empty() {
                            let resampled = r.resample_f32(&mixed_samples);
                            output.extend_from_slice(&resampled);
                        }

                        // Pad with silence if needed
                        while output.len() < device_samples_needed {
                            output.push(0.0);
                        }

                        // Trim if we got too many
                        output.truncate(device_samples_needed);
                        output
                    } else {
                        r.resample_f32(&mixed_samples)
                    }
                } else {
                    mixed_samples
                };

                // Apply limiting to prevent clipping
                let limited: Vec<f32> = device_samples
                    .iter()
                    .map(|&x| x.clamp(-0.99, 0.99))
                    .collect();

                // Fill the output buffer based on channel configuration
                match channels {
                    1 => {
                        // Mono
                        for (out, sample) in data.iter_mut().zip(limited.iter().cycle()) {
                            *out = *sample;
                        }
                    }
                    2 => {
                        // Stereo
                        for (chunk, sample) in data.chunks_mut(2).zip(limited.iter().cycle()) {
                            chunk[0] = *sample;
                            chunk[1] = *sample;
                        }
                    }
                    4 => {
                        // 4 channels (your EVO4) - output to first 2 channels only
                        for (chunk, sample) in data.chunks_mut(4).zip(limited.iter().cycle()) {
                            chunk[0] = *sample; // Left
                            chunk[1] = *sample; // Right
                            chunk[2] = 0.0; // Unused
                            chunk[3] = 0.0; // Unused
                        }
                    }
                    _ => {
                        // Generic multi-channel: output to first 2 channels
                        let ch = channels as usize;
                        for (chunk, sample) in data.chunks_mut(ch).zip(limited.iter().cycle()) {
                            chunk[0] = *sample;
                            if ch > 1 {
                                chunk[1] = *sample;
                            }
                            for i in 2..ch {
                                chunk[i] = 0.0;
                            }
                        }
                    }
                }
            },
            |err| error!("❌ Output stream error: {}", err),
            None,
        )?;

        output_stream.play()?;
        *self.output_stream.lock().unwrap() = Some(output_stream);

        Ok(())
    }

    fn restart_input_stream(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        {
            let mut stream_guard = self.input_stream.lock().unwrap();
            if let Some(stream) = stream_guard.take() {
                drop(stream);
            }
        }
        self.start_input_stream()
    }

    fn restart_output_stream(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        {
            let mut stream_guard = self.output_stream.lock().unwrap();
            if let Some(stream) = stream_guard.take() {
                drop(stream);
            }
        }
        self.start_output_stream()
    }

    // ... rest of the methods remain the same as in your original implementation ...

    pub fn add_client(&mut self, client_id: renet::ClientId) {
        let mut decoders = self.decoders.lock().unwrap();
        decoders.entry(client_id).or_insert_with(|| {
            info!("➕ Adding decoder for client {}", client_id);
            Decoder::new(OPUS_SAMPLE_RATE, Channels::Mono).unwrap()
        });
    }

    pub fn remove_client(&mut self, client_id: renet::ClientId) {
        let mut decoders = self.decoders.lock().unwrap();
        if decoders.remove(&client_id).is_some() {
            info!("➖ Removed decoder for client {}", client_id);
        }
    }

    pub fn receive_packet(&mut self, packet: VoicePacket) {
        self.incoming_packets.lock().unwrap().push_back(packet);
    }

    pub fn get_outgoing_packets(&mut self) -> Vec<VoicePacket> {
        let mut packets = Vec::new();
        if let Ok(rx) = self.outgoing_rx.lock() {
            while let Ok(packet) = rx.try_recv() {
                packets.push(packet);
            }
        }
        packets
    }

    pub fn is_speaking(&self) -> bool {
        self.is_speaking.load(Ordering::Relaxed)
    }

    pub fn is_active(&self) -> bool {
        let input_active = self.input_stream.lock().unwrap().is_some();
        let output_active = self.output_stream.lock().unwrap().is_some();
        input_active || output_active
    }

    pub fn set_muted(&mut self, muted: bool) {
        *self.is_muted.lock().unwrap() = muted;
        if muted {
            self.vad_processor.lock().unwrap().reset();
            self.is_speaking.store(false, Ordering::Relaxed);
        }
        info!("🔇 Mute: {}", if muted { "ON" } else { "OFF" });
    }

    pub fn set_loopback(&mut self, enabled: bool) {
        *self.loopback_enabled.lock().unwrap() = enabled;
        info!("🔄 Loopback: {}", if enabled { "ON" } else { "OFF" });
    }

    pub fn get_stats(&self) -> String {
        let incoming = self.incoming_packets.lock().unwrap().len();
        let sent = self.packets_sent.load(Ordering::Relaxed);
        let received = self.packets_received.load(Ordering::Relaxed);

        let input_info = self
            .input_config
            .lock()
            .unwrap()
            .as_ref()
            .map(|c| format!("{} @ {}Hz", c.device_name, c.sample_rate))
            .unwrap_or_else(|| "Not initialized".to_string());

        let output_info = self
            .output_config
            .lock()
            .unwrap()
            .as_ref()
            .map(|c| format!("{} @ {}Hz", c.device_name, c.sample_rate))
            .unwrap_or_else(|| "Not initialized".to_string());

        format!(
            "Packets - Sent: {}, Received: {}, Queue: {}\n\
             Input: {}\n\
             Output: {}",
            sent, received, incoming, input_info, output_info
        )
    }

    pub fn get_current_input_device(&self) -> Option<String> {
        self.input_capabilities.as_ref().map(|c| c.name.clone())
    }

    pub fn get_current_output_device(&self) -> Option<String> {
        self.output_capabilities.as_ref().map(|c| c.name.clone())
    }

    pub fn set_user_audio_settings(
        &mut self,
        client_id: renet::ClientId,
        settings: UserAudioSettings,
    ) {
        self.user_audio_settings
            .lock()
            .unwrap()
            .insert(client_id, settings);
    }

    pub fn get_user_audio_settings(&self, client_id: renet::ClientId) -> UserAudioSettings {
        self.user_audio_settings
            .lock()
            .unwrap()
            .get(&client_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn clear_user_audio_settings(&mut self, client_id: renet::ClientId) {
        self.user_audio_settings.lock().unwrap().remove(&client_id);
    }
}

