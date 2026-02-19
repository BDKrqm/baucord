//! Adaptive jitter buffer with per-peer sequence tracking and mixing.
//!
//! Buffers decoded audio per-peer, mixes to mono, and adapts its target
//! depth based on inter-arrival jitter estimation and buffer health feedback.
//!
//! ## v0.5 changes from v0.4
//!
//! - **Bidirectional adaptation**: Target depth now decreases when conditions
//!   are stable, not just increases on jitter spikes. Uses health-based
//!   feedback (late packets, gaps) as the "increase" signal and stable time
//!   as the "decrease" signal.
//! - **Asymmetric jitter EMA**: Fast attack (0.3) to react to spikes,
//!   slower release (0.03) to avoid oscillation, but not so slow it
//!   never comes down (old code used 0.1 symmetric — too sticky).
//! - **75th percentile jitter**: Uses p75 across peers instead of max,
//!   so one bad peer doesn't penalize everyone.
//! - **Convergence on stable networks**: On LAN with <1ms jitter, target
//!   converges to minimum (~2ms) within seconds and stays there.
//!   On internet with 20ms jitter, settles around 40-50ms.
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
    /// Uses asymmetric coefficients: fast attack, slower release.
    jitter_estimate: f32,
}

impl PeerBuffer {
    fn new() -> Self {
        Self {
            samples: VecDeque::with_capacity(4800), // 100ms at 48kHz
            last_seq: None,
            last_arrival: None,
            jitter_estimate: 0.002, // start at 2ms — optimistic, will rise if needed
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

    // ── Adaptation state ────────────────────────────────────────────
    /// Counts adapt() calls since the last late/gap event.
    /// When this is high, conditions are stable and we can drain.
    stable_ticks: u32,
    /// Number of late packets since last adapt() call.
    late_count: u32,
    /// Number of gap events since last adapt() call.
    gap_count: u32,
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
            min_samples: (2 * sample_rate) / 1000, // 2ms floor
            max_samples: (120 * sample_rate) / 1000, // 120ms ceiling
            buffers: HashMap::new(),
            peer_settings,
            stable_ticks: 0,
            late_count: 0,
            gap_count: 0,
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

        // Update jitter estimate with asymmetric EMA:
        // - Rising (spike): fast attack, coeff 0.3 (reach ~80% in 3 packets)
        // - Falling (stable): slow release, coeff 0.03 (decay over ~30 packets / 300ms)
        // This reacts to jitter spikes within 30ms but doesn't get permanently
        // inflated by a single burst.
        if let Some(last_arrival) = peer.last_arrival {
            let arrival_delta = now.duration_since(last_arrival).as_secs_f32();
            let expected_delta = FRAME_SIZE as f32 / self.sample_rate as f32;
            let jitter_sample = (arrival_delta - expected_delta).abs();

            let coeff = if jitter_sample > peer.jitter_estimate {
                0.3 // fast attack — react to spikes quickly
            } else {
                0.03 // slow release — don't oscillate, but DO come down
            };
            peer.jitter_estimate = peer.jitter_estimate * (1.0 - coeff) + jitter_sample * coeff;
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
                self.late_count += 1;
                PushResult::Late
            } else if forward > 0 && forward <= 20 {
                // Gap: we missed `forward` packets
                let gap = forward.min(10) as usize; // cap PLC at 10 frames (100ms)
                peer.last_seq = Some(seq);
                self.gap_count += 1;
                PushResult::Gap { gap_count: gap }
            } else {
                // Huge jump — reset this peer's state
                peer.samples.clear();
                peer.last_seq = Some(seq);
                peer.jitter_estimate = 0.002;
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

        drop(settings); // release lock before drain

        // ── Drain excess: prevent unbounded growth from clock drift ──
        // The input and output clocks are independent (different devices,
        // or worker threads paced by OS timers). Even 0.01% drift causes
        // ~5 samples/sec of accumulation — enough to add 100ms+ of latency
        // over a few minutes. Fix: after each pull, if any peer's buffer
        // exceeds target + 1 frame of headroom, discard the oldest excess
        // samples. This keeps occupancy pinned near the target without
        // affecting normal operation.
        //let headroom = self.target_samples + n; // target + 1 frame
        let headroom = self.target_samples + (n * 4);
        for peer in self.buffers.values_mut() {
            if peer.samples.len() > headroom {
                let excess = peer.samples.len() - headroom;
                // Drop from the front (oldest samples — already stale)
                peer.samples.drain(..excess);
            }
        }
    }

    /// Adapt target depth based on jitter estimates and buffer health.
    ///
    /// Called every 10ms by the output worker. Uses two signals:
    ///
    /// 1. **Jitter estimates** (from inter-arrival times) — tells us what
    ///    the network looks like right now.
    /// 2. **Health feedback** (late packets, gaps) — tells us if the current
    ///    target is actually too low.
    ///
    /// The target moves **up fast** on problems (within 1-2 ticks) and
    /// **down slowly** during stable periods (over 50-100 ticks / 0.5-1s).
    /// This avoids both underruns on bad networks and latency creep on
    /// good ones.
    pub fn adapt(&mut self) {
        if self.buffers.is_empty() {
            return;
        }

        // ── Compute jitter-based target ─────────────────────────────
        // Use 75th percentile across active peers instead of max.
        // This prevents one bad peer from inflating the buffer for everyone.
        let mut jitter_values: Vec<f32> = self
            .buffers
            .values()
            .filter(|p| p.last_arrival.is_some())
            .map(|p| p.jitter_estimate * 1000.0) // convert to ms
            .collect();

        let jitter_target_ms = if jitter_values.is_empty() {
            0.0
        } else {
            jitter_values.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let p75_idx = (jitter_values.len() * 3) / 4;
            let p75_jitter = jitter_values[p75_idx.min(jitter_values.len() - 1)];
            // Target = 2× p75 jitter — enough headroom without being excessive
            p75_jitter * 2.0
        };

        // ── Health-based adjustment ─────────────────────────────────
        let had_problems = self.late_count > 0 || self.gap_count > 0;

        if had_problems {
            // Problems detected — increase target immediately.
            // Jump by 10ms per event, capped at one jump per adapt cycle.
            // This is the "fast up" path.
            let bump_ms = 10.0;
            let current_ms = self.target_ms();
            let new_target_ms = (current_ms + bump_ms).min(self.max_ms());
            self.target_samples = self.ms_to_samples(new_target_ms);
            self.stable_ticks = 0;
        } else {
            // No problems — count stable ticks and slowly drain toward
            // the jitter-based target (which itself is slowly decreasing
            // via the asymmetric EMA).
            self.stable_ticks = self.stable_ticks.saturating_add(1);

            // After 50 stable ticks (~500ms with no issues), start draining.
            // Drain rate: 0.5ms per tick = ~5ms/sec of target reduction.
            // This is conservative enough to not cause glitches but fast
            // enough to converge within a few seconds on LAN.
            if self.stable_ticks > 50 {
                let current_ms = self.target_ms();
                // Drain toward the jitter-based target, but never below min.
                let floor_ms = jitter_target_ms.max(self.min_ms());
                let new_target_ms = (current_ms - 0.5).max(floor_ms);
                self.target_samples = self.ms_to_samples(new_target_ms);
            }
        }

        // ── Always respect jitter-based minimum ─────────────────────
        // If jitter estimate says we need more than current target,
        // jump up immediately (don't wait for late packets to prove it).
        let jitter_samples = self.ms_to_samples(jitter_target_ms);
        if jitter_samples > self.target_samples {
            self.target_samples = jitter_samples;
        }

        // ── Clamp to [min, max] ─────────────────────────────────────
        self.target_samples = self
            .target_samples
            .clamp(self.min_samples, self.max_samples);

        // Reset event counters for next cycle
        self.late_count = 0;
        self.gap_count = 0;
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

    // ── Internal helpers ────────────────────────────────────────────

    fn min_ms(&self) -> f32 {
        (self.min_samples as f32 * 1000.0) / self.sample_rate as f32
    }

    fn max_ms(&self) -> f32 {
        (self.max_samples as f32 * 1000.0) / self.sample_rate as f32
    }

    fn ms_to_samples(&self, ms: f32) -> usize {
        ((ms * self.sample_rate as f32) / 1000.0) as usize
    }
}
