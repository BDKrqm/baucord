// voice/mod.rs - Core module with rubato integration
pub mod diagnostics;
mod input;
mod jitter_buffer;
mod output;
mod packet;
mod resampler;
mod stats;
mod vad;

pub use input::AudioInput;
pub use output::AudioOutput;
pub use packet::AudioPacket;
pub use stats::AudioStats;

use crate::settings::{Settings, UserAudioSettings};
use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, SampleFormat, SampleRate, SupportedStreamConfig};
use crossbeam_channel::{Receiver, Sender, bounded};
use diagnostics::{Anomaly, AnomalyType, PipelineDiagnostics, ResamplerMetrics, TestMode};
use log::{error, info, warn};
use opus::{Application, Bitrate, Channels, Decoder, Encoder};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// ============= OPTIMIZED CONSTANTS =============
pub const OPUS_SAMPLE_RATE: u32 = 48000;
pub const FRAME_SIZE: usize = 480; // 10ms at 48kHz for lower latency
pub const OPUS_BITRATE: i32 = 24000; // 24kbps for 10ms frames

// Latency presets
#[derive(Debug, Clone, Copy)]
pub enum LatencyPreset {
    Low = 128,    // 2.7ms @ 48kHz - may glitch
    Normal = 256, // 5.3ms - good balance
    Safe = 512,   // 10.7ms - stable
}

impl LatencyPreset {
    fn from_usize(value: usize) -> Self {
        match value {
            128 => LatencyPreset::Low,
            512 => LatencyPreset::Safe,
            _ => LatencyPreset::Normal,
        }
    }
}

// ============= DEVICE CAPABILITIES =============
#[derive(Debug, Clone)]
pub struct DeviceCapabilities {
    pub name: String,
    pub sample_rates: Vec<u32>,
    pub channels: Vec<u16>,
    pub sample_formats: Vec<SampleFormat>,
    pub preferred_config: Option<SupportedStreamConfig>,
}

impl DeviceCapabilities {
    pub fn from_device(
        device: &Device,
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
        let mut sample_formats = Vec::new();

        for config in &configs {
            // Collect sample formats
            let format = config.sample_format();
            if !sample_formats.contains(&format) {
                sample_formats.push(format);
            }

            // Collect sample rates
            let min_rate = config.min_sample_rate().0;
            let max_rate = config.max_sample_rate().0;
            for rate in [8000, 16000, 44100, 48000, 96000, 192000] {
                if rate >= min_rate && rate <= max_rate && !sample_rates.contains(&rate) {
                    sample_rates.push(rate);
                }
            }

            // Collect channels
            let ch = config.channels();
            if !channels.contains(&ch) {
                channels.push(ch);
            }
        }

        // Find preferred config (prefer 48kHz for Opus)
        let preferred_config = configs
            .into_iter()
            .find(|c| {
                c.min_sample_rate() <= SampleRate(48000)
                    && c.max_sample_rate() >= SampleRate(48000)
                    && (c.channels() == 1 || c.channels() == 2)
            })
            .map(|c| c.with_sample_rate(SampleRate(48000)));

        Ok(Self {
            name,
            sample_rates,
            channels,
            sample_formats,
            preferred_config,
        })
    }

    pub fn get_best_config(&self, preferred_rate: Option<u32>) -> Option<(u32, u16, SampleFormat)> {
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
        } else {
            *self
                .sample_rates
                .iter()
                .find(|&&r| r == 48000)
                .or_else(|| self.sample_rates.first())
                .unwrap_or(&48000)
        };

        let channels = if self.channels.contains(&1) {
            1
        } else if self.channels.contains(&2) {
            2
        } else {
            *self.channels.first()?
        };

        // Prefer f32, then i16, then whatever is available
        let format = if self.sample_formats.contains(&SampleFormat::F32) {
            SampleFormat::F32
        } else if self.sample_formats.contains(&SampleFormat::I16) {
            SampleFormat::I16
        } else {
            *self.sample_formats.first()?
        };

        Some((rate, channels, format))
    }
}

// ============= CHANNEL MAPPING =============
#[derive(Debug, Clone)]
pub struct ChannelMapping {
    pub input_channels: Vec<usize>,  // Which input channels to capture
    pub output_pair: (usize, usize), // Which output pair to use
}

impl Default for ChannelMapping {
    fn default() -> Self {
        Self {
            input_channels: vec![0], // Default to first channel
            output_pair: (0, 1),     // Default to first stereo pair
        }
    }
}

impl ChannelMapping {
    pub fn process_input(&self, multi_channel: &[f32], channels: usize) -> f32 {
        // Average selected channels or pick specific one
        let sum: f32 = self
            .input_channels
            .iter()
            .filter_map(|&ch| {
                if ch < channels {
                    multi_channel.get(ch).copied()
                } else {
                    None
                }
            })
            .sum();

        let count = self.input_channels.len().min(channels);
        if count > 0 { sum / count as f32 } else { 0.0 }
    }

    pub fn process_output(&self, sample: f32, output: &mut [f32], channels: usize) {
        // Map to selected output pair
        let (left, right) = self.output_pair;
        if left < channels {
            output[left] = sample;
        }
        if right < channels {
            output[right] = sample;
        }

        // Zero other channels
        for i in 0..channels {
            if i != left && i != right {
                output[i] = 0.0;
            }
        }
    }
}

// ============= MAIN VOICE CHAT ORCHESTRATOR =============
pub struct VoiceChat {
    // Core components
    input: Option<AudioInput>,
    output: Option<AudioOutput>,

    // Opus codec
    encoder: Arc<Mutex<Encoder>>,
    decoders: Arc<Mutex<HashMap<renet::ClientId, Decoder>>>,

    // RT-safe channels
    audio_to_encoder: (Sender<Vec<f32>>, Receiver<Vec<f32>>),
    encoder_to_network: (Sender<AudioPacket>, Receiver<AudioPacket>),
    network_to_decoder: (Sender<AudioPacket>, Receiver<AudioPacket>),
    decoder_to_audio: (
        Sender<(renet::ClientId, Vec<f32>)>,
        Receiver<(renet::ClientId, Vec<f32>)>,
    ),

    // Worker threads
    encoder_thread: Option<thread::JoinHandle<()>>,
    decoder_thread: Option<thread::JoinHandle<()>>,

    // Settings
    latency_preset: Arc<AtomicUsize>,
    channel_mapping: Arc<Mutex<ChannelMapping>>,
    user_audio_settings: Arc<Mutex<HashMap<renet::ClientId, UserAudioSettings>>>,
    stats: Arc<AudioStats>,

    // Diagnostics
    pub diagnostics: Arc<PipelineDiagnostics>,

    // Device monitoring
    device_monitor_thread: Option<thread::JoinHandle<()>>,
    should_monitor: Arc<AtomicBool>,
    loopback_enabled: Arc<AtomicBool>,
    is_speaking: Arc<AtomicBool>,
}

impl VoiceChat {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        // Create Opus encoder with VoIP optimizations
        let mut encoder = Encoder::new(OPUS_SAMPLE_RATE, Channels::Mono, Application::Voip)?;
        encoder.set_bitrate(Bitrate::Bits(OPUS_BITRATE))?;
        encoder.set_vbr(true)?;
        encoder.set_inband_fec(true)?;
        encoder.set_packet_loss_perc(5)?;

        // Create bounded channels - NO SLEEPS in audio path!
        let audio_to_encoder = bounded(32);
        let encoder_to_network = bounded(64);
        let network_to_decoder = bounded(64);
        let decoder_to_audio = bounded(32);

        Ok(Self {
            input: None,
            output: None,
            encoder: Arc::new(Mutex::new(encoder)),
            decoders: Arc::new(Mutex::new(HashMap::new())),
            audio_to_encoder,
            encoder_to_network,
            network_to_decoder,
            decoder_to_audio,
            encoder_thread: None,
            decoder_thread: None,
            latency_preset: Arc::new(AtomicUsize::new(LatencyPreset::Normal as usize)),
            channel_mapping: Arc::new(Mutex::new(ChannelMapping::default())),
            user_audio_settings: Arc::new(Mutex::new(HashMap::new())),
            stats: Arc::new(AudioStats::new()),
            device_monitor_thread: None,
            should_monitor: Arc::new(AtomicBool::new(false)),
            loopback_enabled: Arc::new(AtomicBool::new(false)),
            is_speaking: Arc::new(AtomicBool::new(false)),
            diagnostics: Arc::new(PipelineDiagnostics::new()),
        })
    }

    pub fn start(&mut self, settings: &Settings) -> Result<(), Box<dyn std::error::Error>> {
        info!("🚀 Starting RT-safe voice chat with rubato resampling");

        // Start worker threads
        self.start_encoder_thread();
        self.start_decoder_thread();

        // Start audio I/O
        self.start_input(settings)?;
        self.start_output(settings)?;

        // Start device monitoring
        self.start_device_monitor();

        Ok(())
    }

    fn start_input(&mut self, settings: &Settings) -> Result<(), Box<dyn std::error::Error>> {
        let mut input = AudioInput::new(
            settings.input_device.as_deref(),
            self.audio_to_encoder.0.clone(),
            self.channel_mapping.clone(),
            self.stats.clone(),
            Some(self.diagnostics.clone()),
        )?;

        let preset = LatencyPreset::from_usize(self.latency_preset.load(Ordering::Relaxed));
        input.set_latency_preset(preset);

        input.start()?;

        // Connect the VAD's speaking state to our is_speaking field
        self.is_speaking = input.get_speaking_handle();

        self.input = Some(input);
        Ok(())
    }

    fn start_output(&mut self, settings: &Settings) -> Result<(), Box<dyn std::error::Error>> {
        let mut output = AudioOutput::new(
            settings.output_device.as_deref(),
            self.decoder_to_audio.1.clone(),
            self.channel_mapping.clone(),
            self.user_audio_settings.clone(),
            self.stats.clone(),
            Some(self.diagnostics.clone()),
        )?;

        let preset = LatencyPreset::from_usize(self.latency_preset.load(Ordering::Relaxed));
        output.set_latency_preset(preset);

        output.start()?;
        self.output = Some(output);
        Ok(())
    }

    fn start_encoder_thread(&mut self) {
        let encoder = self.encoder.clone();
        let rx = self.audio_to_encoder.1.clone();
        let tx = self.encoder_to_network.0.clone();
        let stats = self.stats.clone();
        let loopback = self.loopback_enabled.clone();
        let decoder_tx = self.network_to_decoder.0.clone();
        let diagnostics = self.diagnostics.clone();

        self.encoder_thread = Some(thread::spawn(move || {
            let mut seq = 0u32; // Changed to u32
            let mut ts_48k = 0u32; // Monotonic 48kHz timestamp
            let mut scratch = vec![0u8; 256];

            while let Ok(samples) = rx.recv() {
                static ENCODE_COUNTER: AtomicU64 = AtomicU64::new(0);
                let count = ENCODE_COUNTER.fetch_add(1, Ordering::Relaxed);
                if count % 100 == 0 {
                    info!("✅ Encoder: Processed {} frames", count);
                }

                // Check frame size BEFORE encoding
                if samples.len() != FRAME_SIZE {
                    error!(
                        "🚨 CRITICAL: Encoder input size {} != {}",
                        samples.len(),
                        FRAME_SIZE
                    );
                    diagnostics.record_anomaly(Anomaly {
                        stage: "encoder",
                        issue: AnomalyType::FrameSizeMismatch,
                        value: samples.len() as f64,
                        expected_range: (FRAME_SIZE as f64, FRAME_SIZE as f64),
                        timestamp: Instant::now(),
                    });
                    continue;
                }

                // Time the encoding
                let encode_start = Instant::now();

                // Convert to i16
                let samples_i16: Vec<i16> = samples
                    .iter()
                    .map(|&x| (x.clamp(-1.0, 1.0) * 32767.0) as i16)
                    .collect();

                // Encode with Opus
                match encoder.lock().unwrap().encode(&samples_i16, &mut scratch) {
                    Ok(size) => {
                        // Update waterfall timing
                        diagnostics.update_waterfall(|w| {
                            w.encode = encode_start.elapsed();
                        });

                        // Create packet with monotonic timestamp
                        let packet = AudioPacket::new(
                            seq,
                            ts_48k,
                            0, // SSRC for local
                            scratch[..size].to_vec(),
                        );

                        // Increment sequence and timestamp for next frame
                        seq = seq.wrapping_add(1);
                        ts_48k = ts_48k.wrapping_add(480); // 10ms = 480 samples at 48kHz

                        stats.packets_sent.fetch_add(1, Ordering::Relaxed);

                        // Send to network
                        if tx.send(packet.clone()).is_err() {
                            break;
                        }

                        // If loopback enabled, also send to decoder
                        if loopback.load(Ordering::Relaxed) {
                            let loopback_packet = AudioPacket::new(
                                seq,
                                ts_48k,
                                999999, // Special SSRC for loopback
                                scratch[..size].to_vec(),
                            );
                            let _ = decoder_tx.try_send(loopback_packet);
                        }
                    }
                    Err(e) => {
                        error!("Encode error: {}", e);
                        stats.encode_errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }

    fn start_decoder_thread(&mut self) {
        let decoders = self.decoders.clone();
        let rx = self.network_to_decoder.1.clone();
        let tx = self.decoder_to_audio.0.clone();
        let stats = self.stats.clone();

        self.decoder_thread = Some(thread::spawn(move || {
            let mut frame_buffer = vec![0i16; FRAME_SIZE * 2];
            let mut last_seq: HashMap<u64, u32> = HashMap::new(); // KEY: u64 ssrc (see fix #2)

            while let Ok(packet) = rx.recv() {
                let ssrc = packet.header.ssrc; // u64
                let curr = packet.header.seq; // u32

                let mut decoders_map = decoders.lock().unwrap();
                let decoder = decoders_map
                    .entry(ssrc)
                    .or_insert_with(|| Decoder::new(OPUS_SAMPLE_RATE, Channels::Mono).unwrap());

                let last = *last_seq.get(&ssrc).unwrap_or(&curr.wrapping_sub(1));
                let expect = last.wrapping_add(1);

                // Wrap-safe classification
                let ahead = curr.wrapping_sub(expect); // 0 = on time, small = forward gap
                let behind = expect.wrapping_sub(curr); // small = late

                // ❌ BUG WAS HERE: `behind <= 10` also matched 0 (on-time). Require `behind > 0`.
                if behind > 0 && behind <= 10 {
                    // Late packet: we already moved past this time slot (possibly with PLC/FEC) → drop it.
                    continue;
                }

                if ahead > 0 && ahead <= 20 {
                    // We missed `ahead` frames: try FEC for (curr-1) from the current packet…
                    if let Ok(n) = decoder.decode(&packet.opus_data, &mut frame_buffer, true) {
                        let s: Vec<f32> = frame_buffer[..n]
                            .iter()
                            .map(|&x| x as f32 / 32768.0)
                            .collect();
                        let _ = tx.send((ssrc, s));
                        stats.fec_recoveries.fetch_add(1, Ordering::Relaxed);
                    } else {
                        // …otherwise PLC one frame
                        if let Ok(n) = decoder.decode(&[], &mut frame_buffer, false) {
                            let s: Vec<f32> = frame_buffer[..n]
                                .iter()
                                .map(|&x| x as f32 / 32768.0)
                                .collect();
                            let _ = tx.send((ssrc, s));
                            stats.plc_frames.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    // Older missing frames (cap extra PLC at 1)
                    for _ in 0..ahead.saturating_sub(1).min(1) {
                        if let Ok(n) = decoder.decode(&[], &mut frame_buffer, false) {
                            let s: Vec<f32> = frame_buffer[..n]
                                .iter()
                                .map(|&x| x as f32 / 32768.0)
                                .collect();
                            let _ = tx.send((ssrc, s));
                            stats.plc_frames.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    stats
                        .packets_lost
                        .fetch_add(ahead as u64, Ordering::Relaxed);
                } else if ahead > 20 {
                    // Big jump/reset: re-init decoder
                    *decoder = Decoder::new(OPUS_SAMPLE_RATE, Channels::Mono).unwrap();
                }

                // Decode current frame
                match decoder.decode(&packet.opus_data, &mut frame_buffer, false) {
                    Ok(n) => {
                        let s: Vec<f32> = frame_buffer[..n]
                            .iter()
                            .map(|&x| x as f32 / 32768.0)
                            .collect();
                        if tx.send((ssrc, s)).is_err() {
                            break;
                        }
                        stats.packets_received.fetch_add(1, Ordering::Relaxed);
                        last_seq.insert(ssrc, curr); // advance only after accepting current
                    }
                    Err(e) => {
                        error!("Decode error: {}", e);
                        stats.decode_errors.fetch_add(1, Ordering::Relaxed);
                        if let Ok(n) = decoder.decode(&[], &mut frame_buffer, false) {
                            let s: Vec<f32> = frame_buffer[..n]
                                .iter()
                                .map(|&x| x as f32 / 32768.0)
                                .collect();
                            let _ = tx.send((ssrc, s));
                            stats.plc_frames.fetch_add(1, Ordering::Relaxed);
                        }
                        // Do not advance last_seq on error
                    }
                }
            }
        }));
    }

    fn start_device_monitor(&mut self) {
        self.should_monitor.store(true, Ordering::Relaxed);
        let should_monitor = self.should_monitor.clone();
        let stats = self.stats.clone();

        self.device_monitor_thread = Some(thread::spawn(move || {
            let host = cpal::default_host();

            while should_monitor.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_secs(1));

                // Check if devices are still available
                if host.input_devices().map(|d| d.count()).unwrap_or(0) == 0 {
                    warn!("Input device lost!");
                    stats.device_disconnects.fetch_add(1, Ordering::Relaxed);
                }

                if host.output_devices().map(|d| d.count()).unwrap_or(0) == 0 {
                    warn!("Output device lost!");
                    stats.device_disconnects.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }

    // Public API methods

    pub fn set_latency_preset(&mut self, preset: LatencyPreset) {
        self.latency_preset
            .store(preset as usize, Ordering::Relaxed);

        if let Some(ref mut input) = self.input {
            input.set_latency_preset(preset);
        }
        if let Some(ref mut output) = self.output {
            output.set_latency_preset(preset);
        }
    }

    pub fn set_channel_mapping(&mut self, mapping: ChannelMapping) {
        *self.channel_mapping.lock().unwrap() = mapping;
    }

    pub fn receive_packet(&mut self, packet: AudioPacket) {
        let _ = self.network_to_decoder.0.try_send(packet);
    }

    pub fn get_outgoing_packets(&mut self) -> Vec<AudioPacket> {
        let mut packets = Vec::new();
        while let Ok(packet) = self.encoder_to_network.1.try_recv() {
            packets.push(packet);
        }
        packets
    }

    pub fn get_stats(&self) -> String {
        self.stats.get_summary()
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

    pub fn add_client(&mut self, client_id: renet::ClientId) {
        let mut decoders = self.decoders.lock().unwrap();
        decoders.entry(client_id).or_insert_with(|| {
            info!("➕ Adding decoder for client {}", client_id);
            Decoder::new(OPUS_SAMPLE_RATE, Channels::Mono).unwrap()
        });
    }

    pub fn remove_client(&mut self, client_id: renet::ClientId) {
        if self.decoders.lock().unwrap().remove(&client_id).is_some() {
            info!("➖ Removed decoder for client {}", client_id);
        }
    }

    /// Get list of available input devices
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

    /// Get list of available output devices
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
        info!("🎤 Input device change requested: {:?}", device_name);

        // Stop current input if running
        if let Some(mut input) = self.input.take() {
            drop(input);
        }

        // Create new input with the requested device
        let mut new_input = AudioInput::new(
            device_name,
            self.audio_to_encoder.0.clone(),
            self.channel_mapping.clone(),
            self.stats.clone(),
            Some(self.diagnostics.clone()),
        )?;

        // Apply current latency preset
        let preset = LatencyPreset::from_usize(self.latency_preset.load(Ordering::Relaxed));
        new_input.set_latency_preset(preset);

        // Start the new input
        new_input.start()?;
        self.input = Some(new_input);

        info!("✅ Input device changed successfully");
        Ok(())
    }

    pub fn set_output_device(
        &mut self,
        device_name: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        info!("🔊 Output device change requested: {:?}", device_name);

        // Stop current output if running
        if let Some(mut output) = self.output.take() {
            drop(output);
        }

        // Create new output with the requested device
        let mut new_output = AudioOutput::new(
            device_name,
            self.decoder_to_audio.1.clone(),
            self.channel_mapping.clone(),
            self.user_audio_settings.clone(),
            self.stats.clone(),
            Some(self.diagnostics.clone()),
        )?;

        // Apply current latency preset
        let preset = LatencyPreset::from_usize(self.latency_preset.load(Ordering::Relaxed));
        new_output.set_latency_preset(preset);

        // Start the new output
        new_output.start()?;
        self.output = Some(new_output);

        info!("✅ Output device changed successfully");
        Ok(())
    }

    /// Enable/disable loopback for testing
    pub fn set_loopback(&mut self, enabled: bool) {
        self.loopback_enabled.store(enabled, Ordering::Relaxed);
        info!("🔄 Loopback: {}", if enabled { "ON" } else { "OFF" });
    }

    /// Get device capabilities info
    pub fn get_device_info(&self) -> String {
        let mut info = String::new();

        if let Some(ref input) = self.input {
            info.push_str(&input.get_device_info());
            info.push_str("\n\n");
        } else {
            info.push_str("Input: Not initialized\n\n");
        }

        if let Some(ref output) = self.output {
            info.push_str(&output.get_device_info());
        } else {
            info.push_str("Output: Not initialized\n");
        }

        info
    }

    // Override the is_speaking method to check mute state:
    pub fn is_speaking(&self) -> bool {
        // Never report speaking when muted
        if self.is_muted() {
            return false;
        }
        self.is_speaking.load(Ordering::Relaxed)
    }

    pub fn enable_diagnostics(&self, enabled: bool) {
        if enabled {
            self.diagnostics.enable();
        } else {
            self.diagnostics.disable();
        }
    }

    pub fn set_diagnostic_test_mode(&self, mode: TestMode) {
        self.diagnostics.set_test_mode(mode);
    }

    pub fn get_diagnostic_report(&self) -> String {
        self.diagnostics.get_report()
    }

    pub fn run_sanity_test(&self) -> Result<(), String> {
        self.diagnostics.sanity_test_codec()
    }
    pub fn diagnostics_enabled(&self) -> bool {
        self.diagnostics.is_enabled()
    }
    pub fn is_loopback_enabled(&self) -> bool {
        self.loopback_enabled.load(Ordering::Relaxed)
    }

    pub fn reset_diagnostics(&self) {
        // Reset ring buffer monitors
        self.diagnostics
            .input_ring
            .samples_pushed
            .store(0, Ordering::Relaxed);
        self.diagnostics
            .input_ring
            .samples_popped
            .store(0, Ordering::Relaxed);
        self.diagnostics
            .output_ring
            .samples_pushed
            .store(0, Ordering::Relaxed);
        self.diagnostics
            .output_ring
            .samples_popped
            .store(0, Ordering::Relaxed);

        // Reset callback counters
        self.diagnostics
            .input_callback_count
            .store(0, Ordering::Relaxed);
        self.diagnostics
            .output_callback_count
            .store(0, Ordering::Relaxed);

        // Clear anomalies
        if let Ok(mut anomalies) = self.diagnostics.anomalies.lock() {
            anomalies.clear();
        }

        // Reset resampler metrics
        if let Ok(mut input_resampler) = self.diagnostics.input_resampler.lock() {
            *input_resampler = ResamplerMetrics::new("input_resampler");
        }
        if let Ok(mut output_resampler) = self.diagnostics.output_resampler.lock() {
            *output_resampler = ResamplerMetrics::new("output_resampler");
        }

        // Reset stats
        self.stats.packets_sent.store(0, Ordering::Relaxed);
        self.stats.packets_received.store(0, Ordering::Relaxed);
        self.stats.input_overruns.store(0, Ordering::Relaxed);
        self.stats.output_underruns.store(0, Ordering::Relaxed);
        self.stats.encode_errors.store(0, Ordering::Relaxed);
        self.stats.decode_errors.store(0, Ordering::Relaxed);

        info!("📊 Diagnostics reset");
    }

    pub fn get_ring_buffer_info(&self) -> String {
        format!(
            "Input Ring: pushed={} popped={} occupancy={}\n\
             Output Ring: pushed={} popped={} occupancy={}",
            self.diagnostics
                .input_ring
                .samples_pushed
                .load(Ordering::Relaxed),
            self.diagnostics
                .input_ring
                .samples_popped
                .load(Ordering::Relaxed),
            self.diagnostics.input_ring.get_occupancy(),
            self.diagnostics
                .output_ring
                .samples_pushed
                .load(Ordering::Relaxed),
            self.diagnostics
                .output_ring
                .samples_popped
                .load(Ordering::Relaxed),
            self.diagnostics.output_ring.get_occupancy()
        )
    }
    pub fn set_muted(&mut self, muted: bool) {
        if let Some(ref mut input) = self.input {
            input.set_muted(muted);
        }

        // Force speaking to false immediately when muted
        if muted {
            self.is_speaking.store(false, Ordering::Relaxed);
        }

        info!("🔇 Microphone: {}", if muted { "MUTED" } else { "UNMUTED" });
    }

    pub fn is_muted(&self) -> bool {
        if let Some(ref input) = self.input {
            input.is_muted()
        } else {
            false
        }
    }
}

impl Drop for VoiceChat {
    fn drop(&mut self) {
        self.should_monitor.store(false, Ordering::Relaxed);
    }
}
