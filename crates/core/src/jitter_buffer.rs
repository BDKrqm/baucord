//! Adaptive jitter buffer with per-peer sequence tracking and mixing.
//!
//! Buffers decoded audio per-peer, mixes to mono, and adapts its target
//! depth based on inter-arrival jitter estimation.
//!
//! ## v0.4 changes from v0.3
//!
//! - **Sequence-aware**: Tracks seq numbers per peer to detect gaps, reorder,
//!   and drop late packets before they reach the buffer.
//! - **Jitter estimation**: Uses inter-arrival variance (simplified WebRTC
//!   approach) instead of just buffer occupancy for adaptation.
//! - **Gap reporting**: Exposes gap info so the decoder thread can trigger
//!   PLC/FEC for the right number of missing frames.
//! - **Still mixes whatever is available** (no hard gate on target depth).

use crate::config::PeerAudioSettings;
use crate::FRAME_SIZE;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Per-peer state inside the jitter buffer.
struct PeerBuffer {
    /// Audio sample FIFO (decoded f32 at 48 kHz).
    samples: VecDeque<f32>,
    /// Last sequence number we accepted (for gap detection).
    last_seq: Option<u32>,
    /// Arrival time of the last packet (for jitter estimation).
    last_arrival: Option<Instant>,
    /// Smoothed jitter estimate in seconds (exponential moving average).
    jitter_estimate: f32,
}

impl PeerBuffer {
    fn new() -> Self {
        Self {
            samples: VecDeque::with_capacity(4800), // 100ms at 48kHz
            last_seq: None,
            last_arrival: None,
            jitter_estimate: 0.010, // start at 10ms assumption
        }
    }
}

/// Result of pushing a packet, telling the caller what recovery is needed.
#[derive(Debug)]
pub enum PushResult {
    /// Packet was in order, no recovery needed.
    Ok,
    /// Packet arrived but we detected `gap_count` missing frames before it.
    /// The caller should generate PLC/FEC frames for the gap, then push
    /// those samples via `push_plc_samples` before pushing this packet's audio.
    Gap { gap_count: usize },
    /// Packet was late (behind what we've already played). Dropped.
    Late,
    /// Sequence jumped too far — peer buffer was reset.
    Reset,
    /// Peer is locally muted, packet ignored.
    Muted,
}

pub struct AdaptiveJitterBuffer {
    sample_rate: usize,
    target_samples: usize,
    min_samples: usize,
    max_samples: usize,

    buffers: HashMap<u64, PeerBuffer>,
    peer_settings: Arc<Mutex<HashMap<u64, PeerAudioSettings>>>,
}

impl AdaptiveJitterBuffer {
    pub fn new(
        sample_rate: usize,
        initial_target_ms: usize,
        peer_settings: Arc<Mutex<HashMap<u64, PeerAudioSettings>>>,
    ) -> Self {
        Self {
            sample_rate,
            target_samples: (initial_target_ms * sample_rate) / 1000,
            min_samples: (2 * sample_rate) / 1000,
            max_samples: (60 * sample_rate) / 1000,
            buffers: HashMap::new(),
            peer_settings,
        }
    }

    /// Register an incoming packet's sequence number and detect gaps.
    ///
    /// Call this BEFORE decoding. The returned `PushResult` tells the caller
    /// how many PLC/FEC frames to generate before decoding the current packet.
    ///
    /// After handling the gap (if any), call `push_decoded_audio` with the
    /// decoded samples.
    pub fn check_packet(&mut self, peer_id: u64, seq: u32) -> PushResult {
        // Check mute
        {
            let settings = self.peer_settings.lock().unwrap();
            if let Some(s) = settings.get(&peer_id) {
                if s.is_muted {
                    return PushResult::Muted;
                }
            }
        }

        let peer = self.buffers.entry(peer_id).or_insert_with(PeerBuffer::new);
        let now = Instant::now();

        // Update jitter estimate
        if let Some(last_arrival) = peer.last_arrival {
            let arrival_delta = now.duration_since(last_arrival).as_secs_f32();
            // Expected delta for 10ms frames
            let expected_delta = FRAME_SIZE as f32 / self.sample_rate as f32;
            let jitter_sample = (arrival_delta - expected_delta).abs();
            // EMA: jitter = 0.9 * jitter + 0.1 * sample
            peer.jitter_estimate = peer.jitter_estimate * 0.9 + jitter_sample * 0.1;
        }
        peer.last_arrival = Some(now);

        let Some(last) = peer.last_seq else {
            // First packet from this peer
            peer.last_seq = Some(seq);
            return PushResult::Ok;
        };

        let expected = last.wrapping_add(1);

        if seq == expected {
            // Perfect — in order
            peer.last_seq = Some(seq);
            PushResult::Ok
        } else {
            // Use wrapping arithmetic for comparison
            let forward = seq.wrapping_sub(expected);
            let backward = expected.wrapping_sub(seq);

            if backward > 0 && backward <= 10 {
                // Late packet — we've already moved past this seq
                PushResult::Late
            } else if forward > 0 && forward <= 20 {
                // Gap: we missed `forward` packets
                let gap = forward.min(10) as usize; // cap PLC at 10 frames (100ms)
                peer.last_seq = Some(seq);
                PushResult::Gap { gap_count: gap }
            } else {
                // Huge jump — reset this peer's state
                peer.samples.clear();
                peer.last_seq = Some(seq);
                peer.jitter_estimate = 0.010;
                PushResult::Reset
            }
        }
    }

    /// Push decoded audio samples for a peer.
    /// Use for both normal decoded audio and PLC/FEC recovery frames.
    pub fn push_decoded_audio(&mut self, peer_id: u64, samples: Vec<f32>) {
        let peer = self.buffers.entry(peer_id).or_insert_with(PeerBuffer::new);
        peer.samples.extend(samples);

        // Cap buffer size to prevent runaway growth
        let max = self.max_samples * 2; // give some headroom
        while peer.samples.len() > max {
            peer.samples.pop_front();
        }
    }

    /// Pull exactly `n` mixed samples, filling with silence if needed.
    ///
    /// Always mixes whatever audio is available for each peer (no hard gate).
    /// Normalization is based on peers that actually contribute meaningful
    /// audio energy to this frame, not just peers with non-empty buffers.
    /// This prevents quiet peers (DTX residue, silence) from reducing the
    /// volume of active speakers in large channels.
    pub fn pull_mixed(&mut self, n: usize, out: &mut [f32]) {
        debug_assert_eq!(out.len(), n);
        out.iter_mut().for_each(|s| *s = 0.0);

        let settings = self.peer_settings.lock().unwrap();
        let mut contributing_peers = 0u32;

        for (peer_id, peer) in self.buffers.iter_mut() {
            let peer_settings = settings.get(peer_id).cloned().unwrap_or_default();
            if peer_settings.is_muted {
                peer.samples.clear();
                continue;
            }

            if peer.samples.is_empty() {
                continue;
            }

            let available = peer.samples.len().min(n);
            let mut peer_energy = 0.0f32;
            for i in 0..available {
                if let Some(sample) = peer.samples.pop_front() {
                    let scaled = sample * peer_settings.volume;
                    out[i] += scaled;
                    peer_energy += scaled * scaled;
                }
            }

            // Only count peers with meaningful audio (not silence/DTX residue).
            // Threshold: ~-60 dBFS per sample average.
            if available > 0 && (peer_energy / available as f32) > 1e-6 {
                contributing_peers += 1;
            }
        }

        // Soft-knee normalization: no scaling for 1-2 speakers,
        // gentle sqrt scaling above that. This keeps duets at full
        // volume while preventing clipping in large group conversations.
        if contributing_peers > 2 {
            let scale = 1.0 / ((contributing_peers as f32 - 1.0).sqrt());
            for s in out.iter_mut() {
                *s = (*s * scale).clamp(-0.99, 0.99);
            }
        }
    }

    /// Adapt target depth based on jitter estimates across peers.
    pub fn adapt(&mut self) {
        if self.buffers.is_empty() {
            return;
        }

        // Use max jitter across active peers as the target
        let max_jitter_ms = self
            .buffers
            .values()
            .filter(|p| p.last_arrival.is_some())
            .map(|p| p.jitter_estimate * 1000.0)
            .fold(0.0f32, f32::max);

        // Target = 2x jitter estimate, clamped to [min, max]
        let target_ms = (max_jitter_ms * 2.0).clamp(
            (self.min_samples * 1000 / self.sample_rate) as f32,
            (self.max_samples * 1000 / self.sample_rate) as f32,
        );

        self.target_samples = ((target_ms * self.sample_rate as f32) / 1000.0) as usize;
    }

    pub fn occupancy_ms(&self) -> f32 {
        if self.buffers.is_empty() {
            return 0.0;
        }
        let avg: f32 = self
            .buffers
            .values()
            .map(|p| p.samples.len() as f32)
            .sum::<f32>()
            / self.buffers.len() as f32;
        (avg * 1000.0) / self.sample_rate as f32
    }

    /// Number of active (non-empty) peer buffers.
    pub fn active_peers(&self) -> usize {
        self.buffers
            .values()
            .filter(|p| !p.samples.is_empty())
            .count()
    }

    /// Current target depth in milliseconds (for stats).
    pub fn target_ms(&self) -> f32 {
        (self.target_samples as f32 * 1000.0) / self.sample_rate as f32
    }
}