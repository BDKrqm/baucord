//! # voice-chat-core
//!
//! Low-latency voice chat engine with noise suppression.
//!
//! Pipeline: capture → gain → resample → denoise → AGC → VAD gate → Opus encode → network
//!           network → Opus decode (+ FEC/PLC) → jitter buffer → gain → resample → playback
//!
//! ## Device compatibility
//! The engine accepts whatever sample rate and channel count the OS gives us
//! via `default_input_config()` / `default_output_config()`. Bluetooth headphones,
//! USB mics, audio interfaces — they all just work. We resample to 48 kHz internally.

pub mod agc;
mod config;
mod denoise;
mod input;
mod jitter_buffer;
mod output;
mod packet;
mod resampler;
mod stats;
mod vad;

pub use config::{PeerAudioSettings, VoiceChatConfig};
pub use packet::AudioPacket;
pub use stats::AudioStats;

use crossbeam_channel::{bounded, Receiver, Sender};
use log::{error, info, warn};
use opus::{Application, Bitrate, Channels, Decoder, Encoder};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait};
use input::AudioInput;
use output::AudioOutput;

/// Internal pipeline sample rate (Opus native).
pub const OPUS_SAMPLE_RATE: u32 = 48_000;

/// Samples per 10 ms frame at 48 kHz.
pub const FRAME_SIZE: usize = 480;

/// Default Opus bitrate (bits/s). 24 kbps for 10 ms VoIP frames.
const OPUS_BITRATE: i32 = 24_000;

/// The main voice chat engine.
///
/// Owns the full audio pipeline from capture to playback.
/// The caller is responsible for shuttling [`AudioPacket`]s to/from the network.
pub struct VoiceChat {
    input: Option<AudioInput>,
    output: Option<AudioOutput>,

    encoder: Arc<Mutex<Encoder>>,
    decoders: Arc<Mutex<HashMap<u64, Decoder>>>,

    audio_to_encoder: (Sender<Vec<f32>>, Receiver<Vec<f32>>),
    encoder_to_network: (Sender<AudioPacket>, Receiver<AudioPacket>),
    network_to_decoder: (Sender<AudioPacket>, Receiver<AudioPacket>),
    decoder_to_audio: (Sender<(u64, Vec<f32>)>, Receiver<(u64, Vec<f32>)>),

    encoder_thread: Option<thread::JoinHandle<()>>,
    decoder_thread: Option<thread::JoinHandle<()>>,

    user_settings: Arc<Mutex<HashMap<u64, PeerAudioSettings>>>,
    stats: Arc<AudioStats>,
    is_speaking: Arc<AtomicBool>,
    loopback_enabled: Arc<AtomicBool>,
    output_gain: Arc<AtomicU32>,

    config: VoiceChatConfig,
}

impl VoiceChat {
    pub fn new(config: VoiceChatConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let mut encoder = Encoder::new(OPUS_SAMPLE_RATE, Channels::Mono, Application::Voip)?;

        // ── Encoder tuning for voice chat ────────────────────────────────
        //
        // Bitrate: 24 kbps is the sweet spot for mono voice with 10ms frames.
        // Higher doesn't meaningfully improve intelligibility for speech.
        // Lower starts sounding muddy on sibilants (s, f, sh sounds).
        encoder.set_bitrate(Bitrate::Bits(OPUS_BITRATE))?;
        //
        // VBR: essential for voice chat. Saves bandwidth during silence and
        // simple vowel sounds without any quality loss on transients.
        encoder.set_vbr(true)?;
        //
        // Complexity: 5 is the sweet spot for real-time voice.
        // 10 sounds marginally better on music but burns 2-3x more CPU
        // with no perceptible benefit on speech.
        // Below 5, quality drops noticeably on sibilants and fricatives.
        encoder.set_complexity(5)?;
        //
        // Bandwidth: Wideband (50-8000 Hz) is the right choice for voice.
        // Fullband (50-20000 Hz) wastes bits encoding frequencies that
        // barely exist in human speech. Narrowband (50-4000 Hz) sounds
        // tinny and telephonic. With VBR, Opus may adaptively use
        // superwideband when the signal has significant high-frequency
        // content, but Wideband as the target keeps the average bitrate
        // reasonable.
        encoder.set_bandwidth(opus::Bandwidth::Wideband)?;
        //
        // FEC: embeds a low-bitrate redundant copy of the PREVIOUS frame
        // into the current packet. When packet N is lost but packet N+1
        // arrives, the decoder can recover frame N from N+1's FEC data.
        // Costs ~20-30% bandwidth overhead but is the single most impactful
        // feature for audio quality on lossy networks.
        encoder.set_inband_fec(true)?;
        //
        // Expected loss rate hint: tells the encoder how much FEC redundancy
        // to embed. 5% is a safe default — aggressive enough to be useful
        // on WiFi/mobile, conservative enough to not waste bandwidth on
        // reliable connections. Could be made adaptive based on actual loss
        // rate from stats in a future version.
        encoder.set_packet_loss_perc(5)?;
        //
        // DTX: during silence/low-activity, Opus emits tiny (~1-2 byte)
        // comfort noise descriptor packets instead of full frames.
        // Combined with VAD gating, a silent user uses almost zero bandwidth.
        encoder.set_dtx(true)?;

        Ok(Self {
            input: None,
            output: None,
            encoder: Arc::new(Mutex::new(encoder)),
            decoders: Arc::new(Mutex::new(HashMap::new())),
            audio_to_encoder: bounded(32),
            encoder_to_network: bounded(64),
            network_to_decoder: bounded(128),
            decoder_to_audio: bounded(32),
            encoder_thread: None,
            decoder_thread: None,
            user_settings: Arc::new(Mutex::new(HashMap::new())),
            stats: Arc::new(AudioStats::new()),
            is_speaking: Arc::new(AtomicBool::new(false)),
            loopback_enabled: Arc::new(AtomicBool::new(false)),
            output_gain: Arc::new(AtomicU32::new(config.output_gain.to_bits())),
            config,
        })
    }

    /// Start the full pipeline (capture + encode + decode + playback).
    pub fn start(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.start_encoder_thread();
        self.start_decoder_thread();
        self.start_input()?;
        self.start_output()?;
        Ok(())
    }

    fn start_input(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let mut input = AudioInput::new(
            self.config.input_device.as_deref(),
            self.audio_to_encoder.0.clone(),
            Some(self.decoder_to_audio.0.clone()),
            self.stats.clone(),
            self.config.vad_threshold,
            self.config.noise_suppression,
            self.config.input_gain,
        )?;

        if self.config.mute_on_start {
            input.set_muted(true);
        }
        input.set_vad_enabled(self.config.vad_enabled);
        input.set_loopback(self.loopback_enabled.load(Ordering::Relaxed));
        input.start()?;
        self.is_speaking = input.speaking_handle();
        self.input = Some(input);
        Ok(())
    }

    fn start_output(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let mut output = AudioOutput::new(
            self.config.output_device.as_deref(),
            self.decoder_to_audio.1.clone(),
            self.user_settings.clone(),
            self.stats.clone(),
            self.config.output_gain,
        )?;
        self.output_gain = output.output_gain_handle();
        output.start()?;
        self.output = Some(output);
        Ok(())
    }

    fn start_encoder_thread(&mut self) {
        let encoder = self.encoder.clone();
        let rx = self.audio_to_encoder.1.clone();
        let tx = self.encoder_to_network.0.clone();
        let stats = self.stats.clone();

        self.encoder_thread = Some(thread::spawn(move || {
            let mut seq = 0u32;
            let mut ts_48k = 0u32;
            let mut scratch = vec![0u8; 256];

            info!("🎤 Encoder thread started");

            while let Ok(samples) = rx.recv() {
                if samples.len() != FRAME_SIZE {
                    error!(
                        "Encoder: expected {} samples, got {}",
                        FRAME_SIZE,
                        samples.len()
                    );
                    continue;
                }

                let samples_i16: Vec<i16> = samples
                    .iter()
                    .map(|&x| (x.clamp(-1.0, 1.0) * 32767.0) as i16)
                    .collect();

                let encode_start = Instant::now();
                match encoder.lock().unwrap().encode(&samples_i16, &mut scratch) {
                    Ok(size) => {
                        let encode_us = encode_start.elapsed().as_micros() as u64;
                        stats.last_encode_us.store(encode_us, Ordering::Relaxed);

                        // DTX: Opus produces ≤2 byte packets during silence
                        stats.dtx_active.store(size <= 2, Ordering::Relaxed);

                        let packet = AudioPacket::new(seq, ts_48k, 0, scratch[..size].to_vec());

                        seq = seq.wrapping_add(1);
                        ts_48k = ts_48k.wrapping_add(FRAME_SIZE as u32);
                        stats.record_packet_sent(size);

                        if tx.send(packet).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        error!("Encode error: {}", e);
                        stats.encode_errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }

            info!("🎤 Encoder thread stopped");
        }));
    }

    fn start_decoder_thread(&mut self) {
        let decoders = self.decoders.clone();
        let rx = self.network_to_decoder.1.clone();
        let tx = self.decoder_to_audio.0.clone();
        let stats = self.stats.clone();

        self.decoder_thread = Some(thread::spawn(move || {
            let mut frame_buffer = vec![0i16; FRAME_SIZE * 2];
            let mut last_seq: HashMap<u64, u32> = HashMap::new();

            info!("🔊 Decoder thread started");

            while let Ok(packet) = rx.recv() {
                let ssrc = packet.header.ssrc;
                let curr = packet.header.seq;

                let mut decoders_map = decoders.lock().unwrap();
                let decoder = decoders_map.entry(ssrc).or_insert_with(|| {
                    info!("Adding decoder for peer {}", ssrc);
                    Decoder::new(OPUS_SAMPLE_RATE, Channels::Mono).unwrap()
                });

                // ── Sequence gap detection & recovery ───────────────────
                let last = *last_seq.get(&ssrc).unwrap_or(&curr.wrapping_sub(1));
                let expect = last.wrapping_add(1);
                let ahead = curr.wrapping_sub(expect);
                let behind = expect.wrapping_sub(curr);

                // Late packet — already played past this, drop it
                if behind > 0 && behind <= 10 {
                    continue;
                }

                // Gap detected — recover missing frames
                if ahead > 0 && ahead <= 20 {
                    let gap = ahead.min(10) as usize; // cap at 100ms of PLC
                    stats
                        .packets_lost
                        .fetch_add(ahead as u64, Ordering::Relaxed);

                    // ── FEC recovery for the LAST missing frame ─────────
                    //
                    // Opus FEC embeds redundant data for the frame BEFORE
                    // the current one. So if packets 5,6,7 are lost and
                    // packet 8 arrives, we can recover frame 7 (and only
                    // frame 7) from packet 8's FEC data.
                    //
                    // Strategy for a gap of N frames:
                    //   frames 1..N-1 → PLC (Opus extrapolation from state)
                    //   frame N       → FEC (decode current packet with fec=true)
                    //   then          → normal decode of current packet

                    // PLC for frames 1..N-1 (all but the last gap frame)
                    if gap > 1 {
                        for i in 0..(gap - 1) {
                            match decoder.decode(&[], &mut frame_buffer, false) {
                                Ok(n) => {
                                    let samples: Vec<f32> = frame_buffer[..n]
                                        .iter()
                                        .map(|&x| x as f32 / 32768.0)
                                        .collect();
                                    let _ = tx.send((ssrc, samples));
                                    stats.plc_frames.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(e) => {
                                    warn!(
                                        "PLC decode failed for peer {} gap frame {}: {}",
                                        ssrc, i, e
                                    );
                                }
                            }
                        }
                    }

                    // FEC for the last missing frame (recovered from current packet)
                    match decoder.decode(&packet.opus_data, &mut frame_buffer, true) {
                        Ok(n) => {
                            let samples: Vec<f32> = frame_buffer[..n]
                                .iter()
                                .map(|&x| x as f32 / 32768.0)
                                .collect();
                            let _ = tx.send((ssrc, samples));
                            stats.fec_recoveries.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(_) => {
                            // FEC data not available — fall back to PLC
                            if let Ok(n) = decoder.decode(&[], &mut frame_buffer, false) {
                                let samples: Vec<f32> = frame_buffer[..n]
                                    .iter()
                                    .map(|&x| x as f32 / 32768.0)
                                    .collect();
                                let _ = tx.send((ssrc, samples));
                                stats.plc_frames.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                } else if ahead > 20 {
                    // Huge jump — reset decoder state to avoid garbage
                    warn!("Peer {} seq jumped by {} — resetting decoder", ssrc, ahead);
                    *decoder = Decoder::new(OPUS_SAMPLE_RATE, Channels::Mono).unwrap();
                }

                // ── Decode current packet normally ──────────────────────
                let t0 = Instant::now();
                match decoder.decode(&packet.opus_data, &mut frame_buffer, false) {
                    Ok(n) => {
                        stats
                            .last_decode_us
                            .store(t0.elapsed().as_micros() as u64, Ordering::Relaxed);
                        stats.record_packet_received(packet.opus_data.len());
                        let samples: Vec<f32> = frame_buffer[..n]
                            .iter()
                            .map(|&x| x as f32 / 32768.0)
                            .collect();
                        if tx.send((ssrc, samples)).is_err() {
                            break;
                        }
                        last_seq.insert(ssrc, curr);
                    }
                    Err(e) => {
                        error!("Decode error for peer {}: {}", ssrc, e);
                        stats.decode_errors.fetch_add(1, Ordering::Relaxed);
                        // PLC fallback on decode failure
                        if let Ok(n) = decoder.decode(&[], &mut frame_buffer, false) {
                            let samples: Vec<f32> = frame_buffer[..n]
                                .iter()
                                .map(|&x| x as f32 / 32768.0)
                                .collect();
                            let _ = tx.send((ssrc, samples));
                            stats.plc_frames.fetch_add(1, Ordering::Relaxed);
                        }
                        last_seq.insert(ssrc, curr);
                    }
                }
            }

            info!("🔊 Decoder thread stopped");
        }));
    }

    // ── Public API ───────────────────────────────────────────────────────

    pub fn receive_packet(&self, packet: AudioPacket) {
        let _ = self.network_to_decoder.0.try_send(packet);
    }

    pub fn take_outgoing_packets(&self) -> Vec<AudioPacket> {
        let mut packets = Vec::new();
        while let Ok(p) = self.encoder_to_network.1.try_recv() {
            packets.push(p);
        }
        packets
    }

    pub fn outgoing_receiver(&self) -> Receiver<AudioPacket> {
        self.encoder_to_network.1.clone()
    }

    pub fn incoming_sender(&self) -> Sender<AudioPacket> {
        self.network_to_decoder.0.clone()
    }

    pub fn add_peer(&mut self, peer_id: u64) {
        self.decoders
            .lock()
            .unwrap()
            .entry(peer_id)
            .or_insert_with(|| {
                info!("Adding decoder for peer {}", peer_id);
                Decoder::new(OPUS_SAMPLE_RATE, Channels::Mono).unwrap()
            });
    }

    pub fn remove_peer(&mut self, peer_id: u64) {
        self.decoders.lock().unwrap().remove(&peer_id);
    }

    pub fn set_peer_settings(&self, peer_id: u64, settings: PeerAudioSettings) {
        self.user_settings.lock().unwrap().insert(peer_id, settings);
    }

    pub fn set_muted(&self, muted: bool) {
        if let Some(ref input) = self.input {
            input.set_muted(muted);
        }
        if muted {
            self.is_speaking.store(false, Ordering::Relaxed);
        }
    }

    pub fn is_muted(&self) -> bool {
        self.input.as_ref().map_or(false, |i| i.is_muted())
    }

    pub fn set_vad_threshold(&self, threshold: f32) {
        if let Some(ref input) = self.input {
            input.set_vad_threshold(threshold);
        }
    }

    pub fn set_vad_enabled(&self, enabled: bool) {
        if let Some(ref input) = self.input {
            input.set_vad_enabled(enabled);
        }
    }

    pub fn set_noise_suppression(&self, enabled: bool) {
        if let Some(ref input) = self.input {
            input.set_noise_suppression(enabled);
        }
    }

    pub fn set_input_gain(&self, gain: f32) {
        if let Some(ref input) = self.input {
            input.set_input_gain(gain);
        }
    }

    pub fn set_output_gain(&self, gain: f32) {
        let clamped = gain.clamp(0.0, 2.0);
        self.output_gain.store(clamped.to_bits(), Ordering::Relaxed);
    }

    pub fn set_loopback(&self, enabled: bool) {
        self.loopback_enabled.store(enabled, Ordering::Relaxed);
        if let Some(ref input) = self.input {
            input.set_loopback(enabled);
        }
    }

    pub fn is_loopback(&self) -> bool {
        self.loopback_enabled.load(Ordering::Relaxed)
    }

    pub fn is_speaking(&self) -> bool {
        if self.is_muted() {
            return false;
        }
        self.is_speaking.load(Ordering::Relaxed)
    }

    pub fn input_level(&self) -> f32 {
        self.input.as_ref().map_or(0.0, |i| {
            f32::from_bits(i.input_level_handle().load(Ordering::Relaxed))
        })
    }

    pub fn output_level(&self) -> f32 {
        f32::from_bits(self.stats.output_level.load(Ordering::Relaxed))
    }

    /// Get the post-denoise input level (0.0–1.0).
    /// This is what the VAD actually sees. Use for VAD threshold visualization.
    pub fn denoised_level(&self) -> f32 {
        f32::from_bits(self.stats.denoised_level.load(Ordering::Relaxed))
    }

    pub fn stats_summary(&self) -> String {
        self.stats.get_summary()
    }

    pub fn stats(&self) -> &Arc<AudioStats> {
        &self.stats
    }

    // ── Device hot-swap ──────────────────────────────────────────────────

    pub fn set_input_device(
        &mut self,
        device_name: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.input.take();
        self.config.input_device = device_name.map(String::from);
        self.start_input()?;
        info!("Input device changed");
        Ok(())
    }

    pub fn set_output_device(
        &mut self,
        device_name: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.output.take();
        self.config.output_device = device_name.map(String::from);
        self.start_output()?;
        info!("Output device changed");
        Ok(())
    }

    pub fn list_input_devices() -> Result<Vec<(String, bool)>, Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let default_name = host.default_input_device().and_then(|d| d.name().ok());
        let mut devices = Vec::new();
        for device in host.input_devices()? {
            if let Ok(name) = device.name() {
                let is_default = default_name.as_deref() == Some(&name);
                if let Ok(cfg) = device.default_input_config() {
                    info!(
                        "  Input device: \"{}\" {}Hz {}ch {:?}{}",
                        name,
                        cfg.sample_rate().0,
                        cfg.channels(),
                        cfg.sample_format(),
                        if is_default { " [DEFAULT]" } else { "" },
                    );
                }
                devices.push((name, is_default));
            }
        }
        info!("Found {} input devices", devices.len());
        Ok(devices)
    }

    pub fn list_output_devices() -> Result<Vec<(String, bool)>, Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let default_name = host.default_output_device().and_then(|d| d.name().ok());
        let mut devices = Vec::new();
        for device in host.output_devices()? {
            if let Ok(name) = device.name() {
                let is_default = default_name.as_deref() == Some(&name);
                if let Ok(cfg) = device.default_output_config() {
                    info!(
                        "  Output device: \"{}\" {}Hz {}ch {:?}{}",
                        name,
                        cfg.sample_rate().0,
                        cfg.channels(),
                        cfg.sample_format(),
                        if is_default { " [DEFAULT]" } else { "" },
                    );
                }
                devices.push((name, is_default));
            }
        }
        info!("Found {} output devices", devices.len());
        Ok(devices)
    }
}

impl Drop for VoiceChat {
    fn drop(&mut self) {
        // AudioInput and AudioOutput Drop impls handle stream + thread cleanup
    }
}
