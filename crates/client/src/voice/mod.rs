mod vad;
use crate::settings::{Settings, UserAudioSettings};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use log::info;
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

// ============= LOW LATENCY JITTER BUFFER =============
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
        // Check if this user is muted
        let settings = self.user_audio_settings.lock().unwrap();
        if let Some(user_settings) = settings.get(&client_id) {
            if user_settings.is_muted {
                // Don't add muted user's audio
                return;
            }
        }
        drop(settings);

        let buffer = self.buffers.entry(client_id).or_insert_with(VecDeque::new);
        buffer.extend(samples);

        // Aggressive trimming for low latency - max 50ms
        let max_size = self.sample_rate * 50 / 1000;
        while buffer.len() > max_size {
            buffer.pop_front();
        }
    }

    fn get_mixed_samples(&mut self, count: usize) -> Vec<f32> {
        let mut mixed = vec![0.0f32; count];
        let mut active_streams = 0;
        let settings = self.user_audio_settings.lock().unwrap();

        for (client_id, buffer) in self.buffers.iter_mut() {
            // Get user-specific settings
            let user_settings = settings.get(client_id).cloned().unwrap_or_default();

            // Skip if muted
            if user_settings.is_muted {
                buffer.clear(); // Clear buffer for muted users
                continue;
            }

            if buffer.len() >= count {
                active_streams += 1;
                for i in 0..count {
                    if let Some(sample) = buffer.pop_front() {
                        // Apply user-specific volume adjustment
                        mixed[i] += sample * user_settings.volume;
                    }
                }
            } else if !buffer.is_empty() {
                // Don't wait or stretch - just play what we have for lowest latency
                active_streams += 1;
                while let Some(sample) = buffer.pop_front() {
                    if mixed.len() > 0 {
                        // Apply user-specific volume adjustment
                        mixed[0] += sample * user_settings.volume;
                    }
                }
            }
        }

        // Simple mixing
        if active_streams > 1 {
            let scale = 1.0 / (active_streams as f32).sqrt();
            for sample in mixed.iter_mut() {
                *sample *= scale;
            }
        }

        mixed
    }
}

// ============= OPTIMIZED RESAMPLER FOR LOW LATENCY =============
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

        // Special optimization for 96kHz -> 48kHz (common with pro audio interfaces)
        if from_rate == 96000 && to_rate == 48000 {
            info!("🎯 Using optimized 2:1 decimation for 96kHz -> 48kHz");
            return Self {
                resampler: None, // Will use fast decimation
                input_buffer: vec![],
                from_rate,
                to_rate,
            };
        }

        let ratio = to_rate as f64 / from_rate as f64;

        // Use Linear for low latency - it's good enough for voice
        let resampler = FastFixedIn::<f32>::new(
            ratio,
            1.1,                      // Minimal adjustment range
            PolynomialDegree::Linear, // Fast and acceptable for voice
            480,                      // Match our smaller frame size
            1,
        )
        .expect("Failed to create resampler");

        Self {
            resampler: Some(resampler),
            input_buffer: vec![vec![0.0; 480]],
            from_rate,
            to_rate,
        }
    }

    fn is_needed(&self) -> bool {
        self.from_rate != self.to_rate
    }

    fn resample_f32(&mut self, input: &[f32]) -> Vec<f32> {
        if self.from_rate == self.to_rate {
            return input.to_vec();
        }

        // Fast path for 96kHz -> 48kHz
        if self.from_rate == 96000 && self.to_rate == 48000 {
            // Simple 2:1 decimation - just take every other sample
            return input.iter().step_by(2).copied().collect();
        }

        // Use resampler for other rates
        if let Some(resampler) = &mut self.resampler {
            let chunk_size = resampler.input_frames_next();
            let mut output = Vec::new();

            for chunk in input.chunks(chunk_size) {
                if chunk.len() == chunk_size {
                    self.input_buffer[0].copy_from_slice(chunk);
                } else {
                    // Pad last chunk
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

/// Messages for network layer
#[derive(Debug, Clone)]
pub enum VoicePacket {
    /// Compressed audio data
    AudioData {
        client_id: renet::ClientId,
        opus_data: Vec<u8>,
    },
    /// VAD status update
    Speaking {
        client_id: renet::ClientId,
        is_speaking: bool,
    },
}

/// Audio device configuration
#[derive(Clone, Debug)]
struct AudioConfig {
    sample_rate: u32,
    channels: u16,
    device_name: String,
}

/// Voice chat handler with modular VAD and dynamic sample rate
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
    // devices
    input_device: Option<cpal::Device>,
    output_device: Option<cpal::Device>,
    user_audio_settings: Arc<Mutex<HashMap<renet::ClientId, UserAudioSettings>>>,
}

impl VoiceChat {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        // Initialize Opus encoder optimized for low latency
        let mut encoder = Encoder::new(OPUS_SAMPLE_RATE, Channels::Mono, Application::Voip)?;

        // Optimize for low latency voice
        encoder.set_bitrate(Bitrate::Bits(OPUS_BITRATE))?;
        encoder.set_vbr(false)?; // CBR is more predictable for real-time
        encoder.set_inband_fec(false)?; // Skip FEC for lower latency
        encoder.set_packet_loss_perc(0)?; // No FEC overhead

        // Initialize VAD for 10ms frames
        let vad_config = VADConfig {
            sample_rate: OPUS_SAMPLE_RATE,
            chunk_size: FRAME_SIZE,
            threshold: 0.15, // Slightly higher threshold for faster response
            profile: VADProfile::Aggressive,
        };

        let mut vad_processor = VADProcessor::new(vad_config)?;
        // Create network channels
        let (outgoing_tx, outgoing_rx) = mpsc::channel();

        vad_processor.set_sender(outgoing_tx.clone());

        Ok(Self {
            encoder: Arc::new(Mutex::new(encoder)),
            decoders: Arc::new(Mutex::new(HashMap::new())),
            input_stream: Arc::new(Mutex::new(None)),
            output_stream: Arc::new(Mutex::new(None)),
            input_device: None,
            output_device: None,
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

        // Try to start input stream
        match self.start_input_stream() {
            Ok(_) => info!("✅ Input stream started"),
            Err(e) => {
                info!("❌ Failed to start input: {}", e);
                return Err(format!("Input device error: {}", e).into());
            }
        }

        // Try to start output stream
        match self.start_output_stream() {
            Ok(_) => info!("✅ Output stream started"),
            Err(e) => {
                info!("❌ Failed to start output: {}", e);
                return Err(format!("Output device error: {}", e).into());
            }
        }

        info!("✅ Voice chat started successfully");
        Ok(())
    }

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
            "📊 Voice Stats:\n\
                Packets - Sent: {}, Received: {}, Queue: {}\n\
                Input: {}\n\
                Output: {}",
            sent, received, incoming, input_info, output_info
        )
    }

    pub fn get_audio_info(&self) -> (Option<AudioConfig>, Option<AudioConfig>) {
        (
            self.input_config.lock().unwrap().clone(),
            self.output_config.lock().unwrap().clone(),
        )
    }
    /// Check if voice chat is currently active (streams are running)
    pub fn is_active(&self) -> bool {
        let input_active = self.input_stream.lock().unwrap().is_some();
        let output_active = self.output_stream.lock().unwrap().is_some();
        input_active || output_active
    }

    /// Get list of available input devices
    pub fn get_input_devices(&self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        use cpal::traits::{DeviceTrait, HostTrait};

        let host = cpal::default_host();
        let mut devices = Vec::new();

        for device in host.input_devices()? {
            if let Ok(name) = device.name() {
                devices.push(name);
            }
        }

        Ok(devices)
    }

    /// Get list of available output devices
    pub fn get_output_devices(&self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        use cpal::traits::{DeviceTrait, HostTrait};

        let host = cpal::default_host();
        let mut devices = Vec::new();

        for device in host.output_devices()? {
            if let Ok(name) = device.name() {
                devices.push(name);
            }
        }

        Ok(devices)
    }

    /// Set the input device (microphone)
    /// Pass None to use system default
    pub fn set_input_device(
        &mut self,
        device_name: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use cpal::traits::{DeviceTrait, HostTrait};

        let host = cpal::default_host();

        let device = match device_name {
            None => host.default_input_device(),
            Some(name) => host
                .input_devices()?
                .find(|d| d.name().ok().as_deref() == Some(name)),
        };

        if let Some(device) = device {
            // Store the device
            self.input_device = Some(device);

            // If currently streaming, restart with new device
            if self.is_active() {
                self.restart_input_stream()?;
            }

            Ok(())
        } else {
            Err("Device not found".into())
        }
    }

    /// Set the output device (speakers/headphones)
    /// Pass None to use system default
    pub fn set_output_device(
        &mut self,
        device_name: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use cpal::traits::{DeviceTrait, HostTrait};

        let host = cpal::default_host();

        let device = match device_name {
            None => host.default_output_device(),
            Some(name) => host
                .output_devices()?
                .find(|d| d.name().ok().as_deref() == Some(name)),
        };

        if let Some(device) = device {
            // Store the device
            self.output_device = Some(device);

            // If currently streaming, restart with new device
            if self.is_active() {
                self.restart_output_stream()?;
            }

            Ok(())
        } else {
            Err("Device not found".into())
        }
    }

    /// Get the name of the current input device
    pub fn get_current_input_device(&self) -> Option<String> {
        use cpal::traits::DeviceTrait;

        self.input_device.as_ref().and_then(|d| d.name().ok())
    }

    /// Get the name of the current output device
    pub fn get_current_output_device(&self) -> Option<String> {
        use cpal::traits::DeviceTrait;

        self.output_device.as_ref().and_then(|d| d.name().ok())
    }

    /// Helper method to restart input stream with new device
    fn restart_input_stream(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Stop current input stream by taking it from the mutex and dropping it
        {
            let mut stream_guard = self.input_stream.lock().unwrap();
            if let Some(stream) = stream_guard.take() {
                drop(stream); // This will stop the stream
            }
        }

        // Create new stream with the new device
        self.start_input_stream()?;

        Ok(())
    }

    /// Helper method to restart output stream with new device
    fn restart_output_stream(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Stop current output stream by taking it from the mutex and dropping it
        {
            let mut stream_guard = self.output_stream.lock().unwrap();
            if let Some(stream) = stream_guard.take() {
                drop(stream); // This will stop the stream
            }
        }

        // Create new stream with the new device
        self.start_output_stream()?;

        Ok(())
    }

    /// Initialize audio with specific devices from settings
    pub fn initialize_with_devices(
        &mut self,
        settings: &Settings,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Set devices from settings
        if let Some(ref input_device) = settings.input_device {
            self.set_input_device(Some(input_device))?;
        }

        if let Some(ref output_device) = settings.output_device {
            self.set_output_device(Some(output_device))?;
        }

        // Apply other settings
        self.set_loopback(settings.loopback_enabled);

        // Set VAD threshold if you have that method
        // self.set_vad_threshold(settings.vad_threshold);

        Ok(())
    }

    // Modified start_input_stream to use stored device
    fn start_input_stream(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let host = cpal::default_host();

        // Use stored device or fall back to default
        let input_device = self
            .input_device
            .as_ref()
            .cloned()
            .or_else(|| host.default_input_device())
            .ok_or("No input device available")?;

        let device_name = input_device
            .name()
            .unwrap_or_else(|_| "Unknown".to_string());
        info!("🎤 Input device: {}", device_name);

        let supported_config = input_device.default_input_config()?;
        let stream_config: cpal::StreamConfig = supported_config.clone().into();
        let channels = supported_config.channels();
        let sample_rate = stream_config.sample_rate.0;

        info!("📊 Input: {} channels @ {}Hz", channels, sample_rate);

        // Save configuration
        *self.input_config.lock().unwrap() = Some(AudioConfig {
            sample_rate,
            channels,
            device_name,
        });

        // Create optimized resampler
        let mut resampler = if sample_rate != OPUS_SAMPLE_RATE {
            if sample_rate == 96000 {
                info!("🚀 Using fast 2:1 decimation for 96kHz input");
            } else {
                info!(
                    "🔄 Resampling {} Hz -> {} Hz",
                    sample_rate, OPUS_SAMPLE_RATE
                );
            }
            Some(AudioResampler::new(sample_rate, OPUS_SAMPLE_RATE))
        } else {
            info!("✅ Native 48kHz - no resampling needed!");
            None
        };

        let input_buffer = self.input_buffer.clone();
        let vad_processor = self.vad_processor.clone();
        let is_muted = self.is_muted.clone();
        let is_speaking = self.is_speaking.clone();
        let encoder = self.encoder.clone();
        let outgoing_tx = self.outgoing_tx.clone();
        let loopback_enabled = self.loopback_enabled.clone();
        let incoming_packets = self.incoming_packets.clone();
        let packets_sent = self.packets_sent.clone();

        let input_stream = input_device.build_input_stream(
            &stream_config,
            move |data: &[f32], _: &_| {
                // Check if muted
                if *is_muted.lock().unwrap() {
                    is_speaking.store(false, Ordering::Relaxed);
                    return;
                }

                // Fast mono conversion
                let mono_samples: Vec<f32> = if channels == 2 {
                    data.chunks_exact(2).map(|c| (c[0] + c[1]) * 0.5).collect()
                } else {
                    data.to_vec()
                };

                // Resample if needed
                let resampled_samples = if let Some(ref mut r) = resampler {
                    r.resample_f32(&mono_samples)
                } else {
                    mono_samples
                };

                // Convert to i16 for VAD and Opus
                let samples_i16: Vec<i16> = resampled_samples
                    .iter()
                    .map(|&x| (x.clamp(-1.0, 1.0) * 32767.0) as i16)
                    .collect();

                // Add to buffer
                let mut buffer = input_buffer.lock().unwrap();
                buffer.extend(&samples_i16);

                while buffer.len() >= FRAME_SIZE {
                    let frame: Vec<i16> = buffer.drain(..FRAME_SIZE).collect();

                    // Let VAD handle all speaking state logic
                    let speaking = vad_processor.lock().unwrap().process_audio(&frame);
                    is_speaking.store(speaking, Ordering::Relaxed);

                    // Only encode and send audio if speaking
                    if speaking {
                        let mut output = vec![0u8; 256];
                        match encoder.lock().unwrap().encode(&frame, &mut output) {
                            Ok(size) => {
                                output.truncate(size);

                                let count = packets_sent.fetch_add(1, Ordering::Relaxed) + 1;
                                if count % 200 == 0 {
                                    info!("📤 Sent {} packets ({} bytes)", count, size);
                                }

                                // Send audio packet
                                let _ = outgoing_tx.send(VoicePacket::AudioData {
                                    client_id: 0,
                                    opus_data: output.clone(),
                                });

                                // Loopback if enabled
                                if *loopback_enabled.lock().unwrap() {
                                    incoming_packets.lock().unwrap().push_back(
                                        VoicePacket::AudioData {
                                            client_id: 999999,
                                            opus_data: output,
                                        },
                                    );
                                }
                            }
                            Err(e) => info!("❌ Encoding error: {}", e),
                        }
                    }
                }
            },
            |err| info!("❌ Input stream error: {}", err),
            None,
        )?;

        input_stream.play()?;
        *self.input_stream.lock().unwrap() = Some(input_stream);

        // Store the device reference if not already stored
        if self.input_device.is_none() {
            self.input_device = Some(input_device);
        }

        Ok(())
    }

    // Modified start_output_stream to use stored device
    fn start_output_stream(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let host = cpal::default_host();

        // Use stored device or fall back to default
        let output_device = self
            .output_device
            .as_ref()
            .cloned()
            .or_else(|| host.default_output_device())
            .ok_or("No output device available")?;

        let device_name = output_device
            .name()
            .unwrap_or_else(|_| "Unknown".to_string());
        info!("🔊 Output device: {}", device_name);

        let supported_config = output_device.default_output_config()?;
        let stream_config: cpal::StreamConfig = supported_config.clone().into();
        let channels = supported_config.channels();
        let sample_rate = stream_config.sample_rate.0;

        info!("📊 Output: {} channels @ {}Hz", channels, sample_rate);

        // Save configuration
        *self.output_config.lock().unwrap() = Some(AudioConfig {
            sample_rate,
            channels,
            device_name,
        });

        // Create optimized resampler for output
        let output_resampler = if sample_rate != OPUS_SAMPLE_RATE {
            if sample_rate == 96000 {
                info!("🚀 Using fast 1:2 interpolation for 96kHz output");
            } else {
                info!(
                    "🔄 Output resampling {} Hz -> {} Hz",
                    OPUS_SAMPLE_RATE, sample_rate
                );
            }
            Some(Arc::new(Mutex::new(AudioResampler::new(
                OPUS_SAMPLE_RATE,
                sample_rate,
            ))))
        } else {
            info!("✅ Native 48kHz output - no resampling needed!");
            None
        };

        let decoders = self.decoders.clone();
        let incoming_packets = self.incoming_packets.clone();
        let packets_received = self.packets_received.clone();

        // Low latency jitter buffer
        let jitter_buffer = Arc::new(Mutex::new(JitterBuffer::new(
            OPUS_SAMPLE_RATE as usize,
            TARGET_BUFFER_SIZE,
            self.user_audio_settings.clone(),
        )));

        // Fast decoder thread with user audio settings
        let jitter_buffer_clone = jitter_buffer.clone();
        let user_audio_settings = self.user_audio_settings.clone();

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
                            // Check if user is muted before decoding
                            let should_process = {
                                let settings = user_audio_settings.lock().unwrap();
                                settings
                                    .get(&client_id)
                                    .map(|s| !s.is_muted)
                                    .unwrap_or(true)
                            };

                            if !should_process {
                                continue; // Skip decoding for muted users
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
                                Err(e) => info!("❌ Decode error: {}", e),
                            }
                        }
                        _ => {}
                    }
                }
            }
        });

        // Low latency output callback
        let output_stream = output_device.build_output_stream(
            &stream_config,
            move |data: &mut [f32], _: &_| {
                let device_samples_needed = if channels == 2 {
                    data.len() / 2
                } else {
                    data.len()
                };

                // Calculate 48kHz samples needed
                let opus_samples_needed = if sample_rate != OPUS_SAMPLE_RATE {
                    (device_samples_needed as f64 * OPUS_SAMPLE_RATE as f64 / sample_rate as f64)
                        as usize
                } else {
                    device_samples_needed
                };

                // Get samples immediately - don't wait
                let mixed_samples = jitter_buffer
                    .lock()
                    .unwrap()
                    .get_mixed_samples(opus_samples_needed);

                // Resample if needed
                let device_samples = if let Some(ref resampler) = output_resampler {
                    let mut r = resampler.lock().unwrap();
                    r.resample_f32(&mixed_samples)
                } else {
                    mixed_samples
                };

                // Simple limiting to prevent clipping
                let limited: Vec<f32> = device_samples
                    .iter()
                    .map(|&x| x.clamp(-0.99, 0.99))
                    .collect();

                // Write to output
                if channels == 2 {
                    for (chunk, sample) in data.chunks_mut(2).zip(limited.iter().cycle()) {
                        chunk[0] = *sample;
                        chunk[1] = *sample;
                    }
                } else {
                    for (out, sample) in data.iter_mut().zip(limited.iter().cycle()) {
                        *out = *sample;
                    }
                }
            },
            |err| info!("❌ Output stream error: {}", err),
            None,
        )?;

        output_stream.play()?;
        *self.output_stream.lock().unwrap() = Some(output_stream);

        // Store the device reference if not already stored
        if self.output_device.is_none() {
            self.output_device = Some(output_device);
        }

        Ok(())
    }
    pub fn load_user_audio_settings(&mut self, settings: &Settings) {
        // Note: You'll need to map usernames to client_ids when users connect
        // This is a placeholder - you'll need to implement the mapping logic
        info!(
            "Loaded {} user audio settings",
            settings.user_audio_settings.len()
        );
    }
    /// Set audio settings for a specific user
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

    /// Get audio settings for a specific user
    pub fn get_user_audio_settings(&self, client_id: renet::ClientId) -> UserAudioSettings {
        self.user_audio_settings
            .lock()
            .unwrap()
            .get(&client_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Clear audio settings for a disconnected user
    pub fn clear_user_audio_settings(&mut self, client_id: renet::ClientId) {
        self.user_audio_settings.lock().unwrap().remove(&client_id);
    }

    // Update the jitter buffer to apply user audio settings
    fn create_adjusted_jitter_buffer(&self) -> Arc<Mutex<JitterBuffer>> {
        Arc::new(Mutex::new(JitterBuffer::new(
            OPUS_SAMPLE_RATE as usize,
            TARGET_BUFFER_SIZE,
            self.user_audio_settings.clone(),
        )))
    }
}
