//! # voice-chat-core
//!
//! Low-latency voice chat engine with noise suppression.
//!
//! Pipeline: capture → gain → resample → denoise → gate → VAD gate → Opus encode → network
//!           network → Opus decode (+ FEC/PLC) → jitter buffer → gain → resample → playback
//!
//! ## Device compatibility
//! The engine accepts whatever sample rate and channel count the OS gives us
//! via `default_input_config()` / `default_output_config()`. Bluetooth headphones,
//! USB mics, audio interfaces — they all just work. We resample to 48 kHz internally.
//!
//! ## Scaling
//! Decoding is parallelized across a worker pool (dispatched by peer SSRC),
//! allowing 30+ simultaneous speakers without garbling or starvation.

pub mod agc;
pub mod gate;
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
use jitter_buffer::DecodedFrame;
use output::AudioOutput;

/// Internal pipeline sample rate (Opus native).
pub const OPUS_SAMPLE_RATE: u32 = 48_000;

/// Samples per 10 ms frame at 48 kHz.
pub const FRAME_SIZE: usize = 480;

/// Default Opus bitrate (bits/s). 24 kbps for 10 ms VoIP frames.
const OPUS_BITRATE: i32 = 24_000;

/// Maximum expected peers. Used for channel sizing.
/// Can be overridden at construction time.
const DEFAULT_MAX_PEERS: usize = 32;

/// Number of decoder worker threads.
/// Each worker owns its own set of decoders — no shared mutex during decode.
const DECODER_WORKERS: usize = 4;

/// The main voice chat engine.
///
/// Owns the full audio pipeline from capture to playback.
/// The caller is responsible for shuttling [`AudioPacket`]s to/from the network.
pub struct VoiceChat {
    input: Option<AudioInput>,
    output: Option<AudioOutput>,

    audio_to_encoder: (Sender<Vec<f32>>, Receiver<Vec<f32>>),
    encoder_to_network: (Sender<AudioPacket>, Receiver<AudioPacket>),
    network_to_decoder: (Sender<AudioPacket>, Receiver<AudioPacket>),
    decoder_to_audio: (Sender<DecodedFrame>, Receiver<DecodedFrame>),

    encoder_thread: Option<thread::JoinHandle<()>>,
    decoder_dispatcher_thread: Option<thread::JoinHandle<()>>,
    decoder_worker_threads: Vec<thread::JoinHandle<()>>,

    /// Signal to stop all worker threads.
    should_stop: Arc<AtomicBool>,

    user_settings: Arc<Mutex<HashMap<u64, PeerAudioSettings>>>,
    /// Peers marked for removal. Workers check this and drop their decoders.
    /// Using a set + mutex is fine here since it's only checked on packet arrival,
    /// not in the audio callback hot path.
    removed_peers: Arc<Mutex<std::collections::HashSet<u64>>>,
    stats: Arc<AudioStats>,
    is_speaking: Arc<AtomicBool>,
    loopback_enabled: Arc<AtomicBool>,
    output_gain: Arc<AtomicU32>,
    deafened: Arc<AtomicBool>,

    config: VoiceChatConfig,
}

impl VoiceChat {
    pub fn new(config: VoiceChatConfig) -> Result<Self, Box<dyn std::error::Error>> {
        // Channel sizing scaled for max peers.
        // decoder_to_audio needs to hold at least 2 frames per peer to avoid
        // the decoder threads blocking and falling behind.
        let max_peers = DEFAULT_MAX_PEERS;

        Ok(Self {
            input: None,
            output: None,
            audio_to_encoder: bounded(4),
            encoder_to_network: bounded(8),
            network_to_decoder: bounded(max_peers * 4),     // was bounded(16)
            decoder_to_audio: bounded(max_peers * 2),        // was bounded(4)
            encoder_thread: None,
            decoder_dispatcher_thread: None,
            decoder_worker_threads: Vec::with_capacity(DECODER_WORKERS),
            should_stop: Arc::new(AtomicBool::new(false)),
            user_settings: Arc::new(Mutex::new(HashMap::new())),
            removed_peers: Arc::new(Mutex::new(std::collections::HashSet::new())),
            stats: Arc::new(AudioStats::new()),
            is_speaking: Arc::new(AtomicBool::new(false)),
            loopback_enabled: Arc::new(AtomicBool::new(false)),
            output_gain: Arc::new(AtomicU32::new(config.output_gain.to_bits())),
            deafened: Arc::new(AtomicBool::new(false)),
            config,
        })
    }

    /// Start the full pipeline (capture + encode + decode + playback).
    pub fn start(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.start_encoder_thread();
        self.start_decoder_pool();
        self.start_input()?;
        self.start_output()?;
        Ok(())
    }

    fn start_input(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let loopback_tx = self.decoder_to_audio.0.clone();
        let mut input = AudioInput::new(
            self.config.input_device.as_deref(),
            self.audio_to_encoder.0.clone(),
            Some(loopback_tx),
            self.stats.clone(),
            self.config.noise_suppression,
            self.config.input_gain,
            self.config.gate_threshold,
        )?;

        if self.config.mute_on_start {
            input.set_muted(true);
        }
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
        self.deafened = output.deafened_handle();
        output.start()?;
        self.output = Some(output);
        Ok(())
    }

    fn start_encoder_thread(&mut self) {
        let rx = self.audio_to_encoder.1.clone();
        let tx = self.encoder_to_network.0.clone();
        let stats = self.stats.clone();

        self.encoder_thread = Some(thread::spawn(move || {
            // Encoder is owned entirely by this thread — no mutex needed.
            let mut encoder = match Encoder::new(OPUS_SAMPLE_RATE, Channels::Mono, Application::Voip) {
                Ok(e) => e,
                Err(e) => {
                    error!("Failed to create Opus encoder: {}", e);
                    return;
                }
            };

            // ── Encoder tuning for voice chat ────────────────────────────
            let _ = encoder.set_bitrate(Bitrate::Bits(OPUS_BITRATE));
            let _ = encoder.set_vbr(true);
            // Complexity 7: encoder runs on one thread (local user only),
            // so we can afford slightly more CPU for better sibilant quality.
            let _ = encoder.set_complexity(7);
            let _ = encoder.set_bandwidth(opus::Bandwidth::Superwideband);
            let _ = encoder.set_inband_fec(true);
            let _ = encoder.set_packet_loss_perc(5);
            let _ = encoder.set_dtx(true);

            let mut seq = 0u32;
            let mut ts_48k = 0u32;
            let mut scratch = vec![0u8; 256];

            // Adaptive FEC: adjust expected loss hint based on measured loss.
            // Check every 200 packets (~2 seconds).
            let mut packets_since_fec_update = 0u32;

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

                let encode_start = Instant::now();
                match encoder.encode_float(&samples, &mut scratch) {
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

                // ── Adaptive FEC ─────────────────────────────────────────
                packets_since_fec_update += 1;
                if packets_since_fec_update >= 200 {
                    packets_since_fec_update = 0;
                    let received = stats.packets_received.load(Ordering::Relaxed);
                    let lost = stats.packets_lost.load(Ordering::Relaxed);
                    if received + lost > 50 {
                        let loss_pct = ((lost as f32 / (received + lost) as f32) * 100.0) as i32;
                        let fec_hint = loss_pct.clamp(0, 20);
                        let _ = encoder.set_packet_loss_perc(fec_hint);
                    }
                }
            }

            info!("🎤 Encoder thread stopped");
        }));
    }

    /// Start the parallel decoder pool.
    ///
    /// Architecture:
    /// - One dispatcher thread reads from `network_to_decoder` and routes
    ///   packets to worker threads by `ssrc % DECODER_WORKERS`.
    /// - Each worker thread owns its own `HashMap<u64, Decoder>` — no shared
    ///   mutex during decode operations.
    /// - All workers send decoded audio to `decoder_to_audio` for the output
    ///   worker to mix.
    ///
    /// This eliminates the serialized single-decoder bottleneck that caused
    /// garbled audio at 15+ peers.
    fn start_decoder_pool(&mut self) {
        let network_rx = self.network_to_decoder.1.clone();
        let decoded_tx = self.decoder_to_audio.0.clone();
        let stats = self.stats.clone();
        let should_stop = self.should_stop.clone();
        let removed_peers = self.removed_peers.clone();

        // Create per-worker channels
        let mut worker_txs: Vec<Sender<AudioPacket>> = Vec::with_capacity(DECODER_WORKERS);
        let mut worker_rxs: Vec<Option<Receiver<AudioPacket>>> = Vec::with_capacity(DECODER_WORKERS);

        for _ in 0..DECODER_WORKERS {
            let (tx, rx) = bounded::<AudioPacket>(32);
            worker_txs.push(tx);
            worker_rxs.push(Some(rx));
        }

        // ── Dispatcher thread (lightweight: just routes by ssrc) ─────────
        let dispatcher_txs = worker_txs.clone();
        let dispatcher_stop = should_stop.clone();
        self.decoder_dispatcher_thread = Some(thread::spawn(move || {
            info!("🔊 Decoder dispatcher started ({} workers)", DECODER_WORKERS);

            while let Ok(packet) = network_rx.recv() {
                if dispatcher_stop.load(Ordering::Relaxed) {
                    break;
                }
                let worker_idx = (packet.header.ssrc as usize) % DECODER_WORKERS;
                // try_send: if a worker is backed up, drop the packet rather
                // than blocking the dispatcher (which would stall ALL peers).
                if dispatcher_txs[worker_idx].try_send(packet).is_err() {
                    // Worker full — packet dropped. Stats will capture this
                    // via sequence gap detection in the worker.
                }
            }

            info!("🔊 Decoder dispatcher stopped");
        }));

        // ── Worker threads (each owns its own decoders) ──────────────────
        for i in 0..DECODER_WORKERS {
            let rx = worker_rxs[i].take().unwrap();
            let tx = decoded_tx.clone();
            let stats = stats.clone();
            let stop = should_stop.clone();
            let removed = removed_peers.clone();

            let handle = thread::spawn(move || {
                let mut decoders: HashMap<u64, Decoder> = HashMap::new();
                let mut frame_buffer = vec![0.0f32; FRAME_SIZE * 2];
                let mut last_seq: HashMap<u64, u32> = HashMap::new();

                info!("🔊 Decoder worker {} started", i);

                while let Ok(packet) = rx.recv() {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }

                    // Stamp arrival time NOW — before any decode work.
                    // This is the critical timing point for jitter estimation.
                    let arrival_time = Instant::now();

                    let ssrc = packet.header.ssrc;

                    // ── Check if peer was removed ───────────────────────
                    // Lock is brief (HashSet::remove) and only contended
                    // when remove_peer is actually called — not hot path.
                    {
                        let mut removed_set = removed.lock().unwrap();
                        if removed_set.contains(&ssrc) {
                            decoders.remove(&ssrc);
                            last_seq.remove(&ssrc);
                            removed_set.remove(&ssrc);
                            continue; // drop this packet
                        }
                    }
                    let curr = packet.header.seq;

                    let decoder = decoders.entry(ssrc).or_insert_with(|| {
                        info!("Worker {}: adding decoder for peer {}", i, ssrc);
                        Decoder::new(OPUS_SAMPLE_RATE, Channels::Mono).unwrap()
                    });

                    // ── Sequence gap detection & recovery ───────────────
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
                        let gap = ahead.min(10) as usize;
                        stats
                            .packets_lost
                            .fetch_add(ahead as u64, Ordering::Relaxed);

                        // PLC for frames 1..N-1
                        if gap > 1 {
                            for j in 0..(gap - 1) {
                                match decoder.decode_float(&[], &mut frame_buffer, false) {
                                    Ok(n) => {
                                        let _ = tx.send(DecodedFrame {
                                            peer_id: ssrc,
                                            seq: expect.wrapping_add(j as u32),
                                            arrival_time,
                                            samples: frame_buffer[..n].to_vec(),
                                            is_concealment: true,
                                        });
                                        stats.plc_frames.fetch_add(1, Ordering::Relaxed);
                                    }
                                    Err(e) => {
                                        warn!(
                                            "PLC decode failed for peer {} gap frame {}: {}",
                                            ssrc, j, e
                                        );
                                    }
                                }
                            }
                        }

                        // FEC for the last missing frame
                        match decoder.decode_float(&packet.opus_data, &mut frame_buffer, true) {
                            Ok(n) => {
                                let _ = tx.send(DecodedFrame {
                                    peer_id: ssrc,
                                    seq: curr.wrapping_sub(1),
                                    arrival_time,
                                    samples: frame_buffer[..n].to_vec(),
                                    is_concealment: true,
                                });
                                stats.fec_recoveries.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(_) => {
                                if let Ok(n) = decoder.decode_float(&[], &mut frame_buffer, false) {
                                    let _ = tx.send(DecodedFrame {
                                        peer_id: ssrc,
                                        seq: curr.wrapping_sub(1),
                                        arrival_time,
                                        samples: frame_buffer[..n].to_vec(),
                                        is_concealment: true,
                                    });
                                    stats.plc_frames.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    } else if ahead > 20 {
                        warn!("Peer {} seq jumped by {} — resetting decoder", ssrc, ahead);
                        *decoder = Decoder::new(OPUS_SAMPLE_RATE, Channels::Mono).unwrap();
                    }

                    // ── Decode current packet normally ──────────────────
                    let t0 = Instant::now();
                    match decoder.decode_float(&packet.opus_data, &mut frame_buffer, false) {
                        Ok(n) => {
                            stats
                                .last_decode_us
                                .store(t0.elapsed().as_micros() as u64, Ordering::Relaxed);
                            stats.record_packet_received(packet.opus_data.len());
                            if tx.send(DecodedFrame {
                                peer_id: ssrc,
                                seq: curr,
                                arrival_time,
                                samples: frame_buffer[..n].to_vec(),
                                is_concealment: false,
                            }).is_err() {
                                break;
                            }
                            last_seq.insert(ssrc, curr);
                        }
                        Err(e) => {
                            error!("Decode error for peer {}: {}", ssrc, e);
                            stats.decode_errors.fetch_add(1, Ordering::Relaxed);
                            if let Ok(n) = decoder.decode_float(&[], &mut frame_buffer, false) {
                                let _ = tx.send(DecodedFrame {
                                    peer_id: ssrc,
                                    seq: curr,
                                    arrival_time,
                                    samples: frame_buffer[..n].to_vec(),
                                    is_concealment: true,
                                });
                                stats.plc_frames.fetch_add(1, Ordering::Relaxed);
                            }
                            last_seq.insert(ssrc, curr);
                        }
                    }
                }

                info!("🔊 Decoder worker {} stopped", i);
            });

            self.decoder_worker_threads.push(handle);
        }
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

    pub fn set_peer_settings(&self, peer_id: u64, settings: PeerAudioSettings) {
        self.user_settings.lock().unwrap().insert(peer_id, settings);
    }

    /// Register a peer. Decoder workers auto-create decoders on first packet,
    /// so this just pre-populates default audio settings for the peer.
    pub fn add_peer(&self, peer_id: u64) {
        self.user_settings
            .lock()
            .unwrap()
            .entry(peer_id)
            .or_insert_with(PeerAudioSettings::default);
        // Remove from the removed set in case they're rejoining quickly
        self.removed_peers.lock().unwrap().remove(&peer_id);
        info!("Peer {} added", peer_id);
    }

    /// Remove a peer. Signals the decoder worker that owns this peer's
    /// decoder to drop it, and clears their audio settings.
    pub fn remove_peer(&self, peer_id: u64) {
        self.removed_peers.lock().unwrap().insert(peer_id);
        self.user_settings.lock().unwrap().remove(&peer_id);
        info!("Peer {} removed", peer_id);
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

    /// Deafen: silences all incoming audio without decoding.
    /// Unlike setting output gain to 0, this skips the decode/mix/resample
    /// pipeline entirely, saving CPU.
    pub fn set_deafened(&self, deafened: bool) {
        self.deafened.store(deafened, Ordering::Relaxed);
    }

    pub fn is_deafened(&self) -> bool {
        self.deafened.load(Ordering::Relaxed)
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

    /// Set the noise gate threshold at runtime.
    /// Values: 0.001–0.5 (RMS). See [`gate::NoiseGate::new`] for guidance.
    pub fn set_gate_threshold(&self, threshold: f32) {
        if let Some(ref input) = self.input {
            input.set_gate_threshold(threshold);
        }
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
    /// This is what the gate and VAD actually see. Use for threshold visualization.
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
        // Signal all threads to stop
        self.should_stop.store(true, Ordering::Release);

        // Drop channels to unblock recv() calls
        // (channels are dropped when VoiceChat is dropped, which causes
        // recv() to return Err and threads to exit their loops)

        // AudioInput and AudioOutput Drop impls handle their own threads.

        // Join encoder thread
        if let Some(thread) = self.encoder_thread.take() {
            let _ = thread.join();
        }

        // Join decoder dispatcher
        if let Some(thread) = self.decoder_dispatcher_thread.take() {
            let _ = thread.join();
        }

        // Join all decoder workers
        for thread in self.decoder_worker_threads.drain(..) {
            let _ = thread.join();
        }
    }
}