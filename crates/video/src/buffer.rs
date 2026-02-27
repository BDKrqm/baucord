//! Video jitter buffer — decouples packet receipt from decode.
//!
//! The core problem: when the screen thread decodes a keyframe (10-16ms),
//! it can't drain the crossbeam channel. Packets that arrive during decode
//! are dropped by `try_send` on the network thread side. This manifests as
//! consistent sequence gaps immediately after every keyframe.
//!
//! The jitter buffer fixes this by separating receipt (fast `push`) from
//! consumption (decoder-paced `pull_next`). Incoming packets go into a
//! `BTreeMap` keyed by sequence number — insertion is O(log n) and takes
//! nanoseconds. The decoder pulls packets at its own rate, immune to
//! transient stalls.
//!
//! ## Adaptive depth
//!
//! The buffer automatically tracks inter-packet arrival jitter and adjusts
//! its target depth to match real network conditions. Stable connections
//! get ~1-2 packets of buffering (barely noticeable); flaky ones get more
//! automatically. No user-facing knob needed.
//!
//! ## Design differences from the voice jitter buffer
//!
//! | Property          | Voice                    | Video                          |
//! |-------------------|--------------------------|--------------------------------|
//! | Packet size       | Uniform (~160B Opus)     | Variable (1KB–170KB)           |
//! | Decode cost       | ~0.1ms                   | 1–16ms (keyframes expensive)   |
//! | Loss tolerance    | PLC conceals 1-2 frames  | Any gap corrupts until keyframe|
//! | Buffering unit    | PCM samples (time-based) | Encoded packets (seq-based)    |

use std::collections::BTreeMap;
use std::time::Instant;

/// An encoded video packet waiting in the jitter buffer.
#[derive(Debug)]
pub struct BufferedPacket {
    pub seq: u32,
    pub is_keyframe: bool,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub arrival_time: Instant,
}

/// Statistics exposed for diagnostics and UI display.
#[derive(Debug, Clone, Default)]
pub struct JitterBufferStats {
    /// Total packets pushed into the buffer.
    pub packets_received: u64,
    /// Total packets pulled out for decoding.
    pub packets_delivered: u64,
    /// Packets dropped due to buffer overflow.
    pub packets_dropped_overflow: u64,
    /// Packets dropped because they arrived too late (seq < next_seq).
    pub packets_dropped_late: u64,
    /// Number of sequence gaps detected during pull.
    pub gaps_detected: u64,
    /// Number of keyframe resets.
    pub keyframe_resets: u64,
    /// Current number of packets in the buffer.
    pub current_depth: usize,
}

/// Sequence-based jitter buffer for encoded H.264 video packets.
///
/// Holds encoded packets sorted by sequence number and releases them
/// to the decoder at a controlled rate. This decouples network receipt
/// (which must be fast) from decode (which can stall on keyframes).
///
/// # Buffer depth
///
/// The buffer depth is managed by an adaptive algorithm that tracks
/// inter-packet arrival jitter and sets the target depth accordingly.
/// Stable connections converge to ~2 packets of buffering; flaky ones
/// automatically increase. The depth is always active — not just during
/// initial fill — so it continuously absorbs jitter throughout the stream.
pub struct VideoJitterBuffer {
    /// Packets waiting to be decoded, keyed by sequence number.
    /// BTreeMap gives automatic ordering and O(log n) insert/lookup.
    pending: BTreeMap<u32, BufferedPacket>,

    /// Next sequence number we expect to release to the decoder.
    next_seq: Option<u32>,

    /// Whether we've received at least one keyframe and can start releasing.
    primed: bool,

    /// Stream frame rate (used for depth_ms calculation and max_depth).
    fps: u32,

    /// Maximum packets to hold before evicting. Safety valve against
    /// runaway growth from stalled decoders or network bursts.
    max_depth: u32,

    /// Whether playback has started (first non-keyframe delta delivered).
    /// Once true, stays true for the lifetime of the stream — we never
    /// re-gate on buffer depth after the initial fill. Recovery from
    /// gaps is handled by skipping to the next keyframe, not by pausing
    /// playback to refill.
    playing: bool,

    /// Adaptive depth tracker — measures jitter and sets target depth.
    adaptive: AdaptiveDepth,

    /// Diagnostics.
    pub stats: JitterBufferStats,
}

impl VideoJitterBuffer {
    /// Create a new jitter buffer.
    ///
    /// # Arguments
    ///
    /// * `fps` — expected frame rate of the incoming stream (e.g. 30 or 60).
    ///   Used for adaptive depth calculations and max_depth sizing.
    ///
    /// # Example
    ///
    /// ```
    /// use video::buffer::VideoJitterBuffer;
    /// let buf = VideoJitterBuffer::new(30);
    /// assert!(buf.depth() == 0);
    /// ```
    pub fn new(fps: u32) -> Self {
        let fps = fps.max(1); // avoid division by zero
                              // Max depth: 3 seconds of video. Safety valve against unbounded growth.
        let max_depth = fps * 3;

        Self {
            pending: BTreeMap::new(),
            next_seq: None,
            primed: false,
            fps,
            max_depth,
            playing: false,
            adaptive: AdaptiveDepth::new(fps),
            stats: JitterBufferStats::default(),
        }
    }

    /// Push an incoming packet into the buffer.
    ///
    /// This is the hot path called from the channel drain loop — it must
    /// be fast. Insertion into the BTreeMap is O(log n) with n typically
    /// < 10 packets. No decoding happens here.
    ///
    /// Keyframes trigger a reset only when they represent a genuine stream
    /// recovery (large forward jump in sequence numbers). Regular in-sequence
    /// keyframes at GOP boundaries are inserted normally without disrupting
    /// playback — they're just another packet in the buffer.
    pub fn push(&mut self, packet: BufferedPacket) {
        self.stats.packets_received += 1;
        self.adaptive.record_arrival();

        // Drop packets that are behind our current play position.
        // These arrived too late to be useful.
        if let Some(next) = self.next_seq {
            if is_seq_before(packet.seq, next) {
                self.stats.packets_dropped_late += 1;
                return;
            }
        }

        if packet.is_keyframe {
            let kf_seq = packet.seq;

            // Determine if this keyframe represents a stream recovery
            // (significant forward jump) vs a regular GOP keyframe.
            //
            // A keyframe is "recovery" only if it jumps far ahead of where
            // we expect to be. The threshold uses the adaptive target depth:
            // packets in the buffer are naturally ahead of next_seq by up to
            // target_depth packets. A regular GOP keyframe at seq N+30 when
            // next_seq is N+18 (12 packets buffered) is NOT recovery — it's
            // just the next keyframe sitting in the buffer as expected.
            //
            // We use 2× adaptive target as the threshold: anything within
            // that range is normal buffer operation. Beyond it means real
            // packet loss or a stream reset.
            let adaptive_target = self.adaptive.target_depth();
            let is_recovery = match self.next_seq {
                Some(next) => {
                    let gap = kf_seq.wrapping_sub(next);
                    is_seq_before(next, kf_seq) && gap > adaptive_target * 2 + 1
                }
                None => true, // first keyframe ever — always prime
            };

            if is_recovery {
                self.pending.retain(|&seq, _| !is_seq_before(seq, kf_seq));
                self.next_seq = Some(kf_seq);
                if self.primed {
                    self.stats.keyframe_resets += 1;
                }
                // NOTE: we do NOT reset self.playing here. Once playback has
                // started, recovery should be seamless — we have a keyframe
                // ready, so there's no reason to pause and wait for the buffer
                // to refill. The keyframe will be released immediately by
                // pull_next() (keyframes bypass depth gating).
            }

            if !self.primed {
                self.next_seq = Some(kf_seq);
                self.primed = true;
            }
        }

        // Overflow protection: if we exceed max_depth, evict the oldest
        // non-keyframe packets. This shouldn't happen in normal operation
        // but protects against stalled decoders.
        while self.pending.len() >= self.max_depth as usize {
            // Find the oldest non-keyframe to evict
            let evict_seq = self
                .pending
                .iter()
                .find(|(_, p)| !p.is_keyframe)
                .map(|(&seq, _)| seq);

            if let Some(seq) = evict_seq {
                self.pending.remove(&seq);
                self.stats.packets_dropped_overflow += 1;
            } else {
                // All packets are keyframes (shouldn't happen) — drop oldest
                if let Some((&seq, _)) = self.pending.iter().next() {
                    self.pending.remove(&seq);
                    self.stats.packets_dropped_overflow += 1;
                }
                break;
            }
        }

        self.pending.insert(packet.seq, packet);
        self.stats.current_depth = self.pending.len();
    }

    /// Pull the next packet to decode, if the buffer is ready.
    ///
    /// Returns `None` if:
    /// - The buffer hasn't been primed (no keyframe yet)
    /// - The adaptive depth gate hasn't been satisfied
    /// - The next expected packet hasn't arrived yet
    ///
    /// The depth gate is always active and uses the adaptive target depth,
    /// which tracks real network jitter. Keyframes always bypass the gate
    /// so the decoder can start/recover immediately.
    ///
    /// When a gap is detected (expected seq missing but later packets
    /// exist), the buffer skips forward to the next available keyframe
    /// for clean recovery. If no keyframe is available, it skips to
    /// the next available packet and lets the decoder handle artifacts.
    pub fn pull_next(&mut self) -> Option<BufferedPacket> {
        if !self.primed {
            return None;
        }
        let next = self.next_seq?;

        // Check if the next expected packet is available
        if self.pending.contains_key(&next) {
            // Depth gating: keyframes always bypass — release immediately
            // so the decoder can start/recover ASAP.
            let is_keyframe = self
                .pending
                .get(&next)
                .map(|p| p.is_keyframe)
                .unwrap_or(false);

            if !is_keyframe && self.pending.len() < self.adaptive.target_depth() as usize {
                return None;
            }

            let packet = self.pending.remove(&next)?;
            self.next_seq = Some(next.wrapping_add(1));
            self.stats.packets_delivered += 1;
            self.stats.current_depth = self.pending.len();
            if !self.playing && !packet.is_keyframe {
                self.playing = true;
            }
            return Some(packet);
        }

        // The packet we need isn't here. Check if we have later packets
        // (meaning this one was truly lost, not just delayed).
        let have_future = self.pending.keys().any(|&s| is_seq_after(s, next));
        if !have_future {
            return None; // hasn't arrived yet — wait
        }

        // Gap detected. Prefer skipping to the next keyframe for clean
        // recovery. If no keyframe ahead, skip to the next available
        // packet — decoder will produce artifacts until next keyframe.
        self.stats.gaps_detected += 1;

        // Look for the nearest keyframe ahead of our position
        let keyframe_seq = self
            .pending
            .iter()
            .find(|(_, p)| p.is_keyframe && is_seq_after_or_eq(p.seq, next))
            .map(|(&seq, _)| seq);

        if let Some(kf_seq) = keyframe_seq {
            // Clean recovery: skip to keyframe, discard everything before it
            self.pending.retain(|&seq, _| !is_seq_before(seq, kf_seq));
            self.next_seq = Some(kf_seq);
            // Don't deliver yet — let the next pull_next() get it so
            // the buffer depth check runs against the cleaned-up state
            return None;
        }

        // No keyframe ahead — skip to the next available packet.
        // The decoder will produce artifacts but at least keeps going.
        let &next_available = self.pending.keys().next()?;
        self.next_seq = Some(next_available);
        let packet = self.pending.remove(&next_available)?;
        self.next_seq = Some(next_available.wrapping_add(1));
        self.stats.packets_delivered += 1;
        self.stats.current_depth = self.pending.len();
        Some(packet)
    }

    /// Current number of packets in the buffer.
    pub fn depth(&self) -> usize {
        self.pending.len()
    }

    /// Current buffer depth expressed in milliseconds (approximate).
    pub fn depth_ms(&self) -> f32 {
        self.pending.len() as f32 * 1000.0 / self.fps.max(1) as f32
    }

    /// Current adaptive target depth in milliseconds (for UI display).
    pub fn adaptive_target_ms(&self) -> f32 {
        self.adaptive.target_depth_ms()
    }

    /// Current jitter estimate in milliseconds (for UI display).
    pub fn adaptive_jitter_ms(&self) -> f32 {
        self.adaptive.jitter_ms()
    }

    /// Whether the buffer has been primed with a keyframe.
    pub fn is_primed(&self) -> bool {
        self.primed
    }

    /// Reset the buffer state. Called when a peer stops sharing.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.next_seq = None;
        self.primed = false;
        self.playing = false;
        self.adaptive = AdaptiveDepth::new(self.fps);
        self.stats.current_depth = 0;
    }
}

// ── Adaptive depth ──────────────────────────────────────────────────────

/// Adaptive jitter buffer depth that tracks inter-packet arrival variance
/// and adjusts automatically. No user knob needed.
///
/// Tracks an exponential moving average (EMA) of the deviation between
/// actual and expected inter-packet intervals. The target depth is set
/// to ~2× the observed jitter, giving enough cushion to absorb bursts
/// without adding unnecessary latency on stable connections.
///
/// Seeded at 100ms of jitter so the buffer starts conservative (~6 packets
/// at 30fps). With α=0.1, it converges to reality within ~20 packets
/// (~0.7s), so the initial conservatism is short-lived but covers the
/// riskiest window (stream startup, no jitter data yet).
pub struct AdaptiveDepth {
    /// Exponential moving average of inter-packet jitter (ms).
    jitter_ema: f32,
    /// Smoothing factor: 0.05 = slow adaptation, 0.2 = fast.
    alpha: f32,
    /// Last packet arrival time.
    last_arrival: Option<Instant>,
    /// Expected interval between packets (1/fps in ms).
    expected_interval_ms: f32,
    /// Minimum depth in packets (floor).
    min_depth: u32,
    /// Maximum depth in packets (ceiling).
    max_depth: u32,
}

impl AdaptiveDepth {
    pub fn new(fps: u32) -> Self {
        let fps = fps.max(1);
        let expected_interval_ms = 1000.0 / fps as f32;
        Self {
            // Seed at ~100ms of jitter so we start conservative.
            jitter_ema: 100.0,
            alpha: 0.1,
            last_arrival: None,
            expected_interval_ms,
            min_depth: 2,
            max_depth: (fps * 3).max(10),
        }
    }

    /// Call on every packet arrival to update jitter estimate.
    pub fn record_arrival(&mut self) {
        let now = Instant::now();
        if let Some(last) = self.last_arrival {
            let actual_ms = now.duration_since(last).as_secs_f32() * 1000.0;
            let deviation = (actual_ms - self.expected_interval_ms).abs();
            self.jitter_ema = self.jitter_ema * (1.0 - self.alpha) + deviation * self.alpha;
        }
        self.last_arrival = Some(now);
    }

    /// Current recommended depth in packets.
    /// Buffers ~2× the observed jitter, converted to packet count.
    pub fn target_depth(&self) -> u32 {
        let depth_ms = self.jitter_ema * 2.0;
        let packets = (depth_ms / self.expected_interval_ms).ceil() as u32;
        packets.clamp(self.min_depth, self.max_depth)
    }

    /// Current jitter estimate in ms (for display).
    pub fn jitter_ms(&self) -> f32 {
        self.jitter_ema
    }

    /// Current target depth in ms (for display).
    pub fn target_depth_ms(&self) -> f32 {
        self.target_depth() as f32 * self.expected_interval_ms
    }
    #[cfg(test)]
    fn freeze_at(&mut self, target_packets: u32) {
        self.jitter_ema = target_packets as f32 * self.expected_interval_ms / 2.0;
        self.min_depth = self.min_depth.min(target_packets);
        self.alpha = 0.0;
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Returns true if `a` is sequentially before `b`, handling u32 wraparound.
///
/// Uses the standard "half-space" comparison: if the distance from a to b
/// (wrapping) is less than half the sequence space, a is before b.
fn is_seq_before(a: u32, b: u32) -> bool {
    a.wrapping_sub(b) > u32::MAX / 2
}

/// Returns true if `a` is sequentially after `b`, handling u32 wraparound.
fn is_seq_after(a: u32, b: u32) -> bool {
    is_seq_before(b, a)
}

/// Returns true if `a` is sequentially after or equal to `b`.
fn is_seq_after_or_eq(a: u32, b: u32) -> bool {
    a == b || is_seq_after(a, b)
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_packet(seq: u32, is_keyframe: bool) -> BufferedPacket {
        BufferedPacket {
            seq,
            is_keyframe,
            width: 1280,
            height: 720,
            data: vec![0u8; if is_keyframe { 100_000 } else { 5_000 }],
            arrival_time: Instant::now(),
        }
    }

    /// Create a buffer with a known fixed adaptive depth for deterministic tests.
    /// Seeds jitter_ema so that target_depth() returns exactly `target_packets`.
    fn make_buffer_with_target(fps: u32, target_packets: u32) -> VideoJitterBuffer {
        let mut buf = VideoJitterBuffer::new(fps);
        buf.adaptive.freeze_at(target_packets);
        assert_eq!(buf.adaptive.target_depth(), target_packets);
        buf
    }

    #[test]
    fn buffer_requires_keyframe_before_releasing() {
        let mut buf = VideoJitterBuffer::new(30);

        buf.push(make_packet(1, false));
        buf.push(make_packet(2, false));
        assert!(
            buf.pull_next().is_none(),
            "should not release without keyframe"
        );

        buf.push(make_packet(3, true));
        let pkt = buf.pull_next().unwrap();
        assert_eq!(pkt.seq, 3);
        assert!(pkt.is_keyframe);
    }

    #[test]
    fn packets_released_in_sequence_order() {
        let mut buf = make_buffer_with_target(30, 1);

        // Push out of order
        buf.push(make_packet(10, true));
        buf.push(make_packet(12, false));
        buf.push(make_packet(11, false));

        assert_eq!(buf.pull_next().unwrap().seq, 10);
        assert_eq!(buf.pull_next().unwrap().seq, 11);
        assert_eq!(buf.pull_next().unwrap().seq, 12);
        assert!(buf.pull_next().is_none());
    }

    #[test]
    fn adaptive_depth_delays_initial_release() {
        // Fixed target of 3 packets
        let mut buf = make_buffer_with_target(30, 3);

        buf.push(make_packet(0, true));
        // Keyframes bypass depth check
        let pkt = buf.pull_next().unwrap();
        assert_eq!(pkt.seq, 0);

        // Delta frames respect depth check
        buf.push(make_packet(1, false));
        assert!(buf.pull_next().is_none(), "depth 1 < target 3");

        buf.push(make_packet(2, false));
        assert!(buf.pull_next().is_none(), "depth 2 < target 3");

        buf.push(make_packet(3, false));
        // Now depth = 3, target = 3, should release
        let pkt = buf.pull_next().unwrap();
        assert_eq!(pkt.seq, 1);
    }

    #[test]
    fn genuine_recovery_keyframe_does_trigger_reset() {
        let mut buf = make_buffer_with_target(30, 3);

        // Start playing
        buf.push(make_packet(0, true));
        let _ = buf.pull_next();

        buf.push(make_packet(1, false));
        buf.push(make_packet(2, false));
        buf.push(make_packet(3, false));
        let _ = buf.pull_next(); // playing = true
        let _ = buf.pull_next();
        let _ = buf.pull_next();

        // Large forward jump: well beyond 2*3+1=7
        buf.push(make_packet(100, true));
        assert_eq!(
            buf.stats.keyframe_resets, 1,
            "large jump should trigger recovery"
        );

        // Recovery keyframe should be immediately available
        let pkt = buf.pull_next().unwrap();
        assert_eq!(pkt.seq, 100);
        assert!(pkt.is_keyframe);
    }

    #[test]
    fn regular_keyframe_does_not_reset_playing() {
        let mut buf = make_buffer_with_target(30, 3);

        // Prime + start playing
        buf.push(make_packet(0, true));
        buf.push(make_packet(1, false));
        buf.push(make_packet(2, false));
        buf.push(make_packet(3, false));
        buf.push(make_packet(4, false));
        // Drain: keyframe(0) bypasses, then 1,2,3 (depth 4→3→2)
        let _ = buf.pull_next(); // seq 0 (keyframe)
        let _ = buf.pull_next(); // seq 1 (depth was 4 ≥ 3)
        let _ = buf.pull_next(); // seq 2 (depth was 3 ≥ 3)
                                 // seq 3 blocked (depth 2 < 3), seq 4 still in buffer
                                 // playing = true, next_seq = 3

        let resets_before = buf.stats.keyframe_resets;

        // In-sequence keyframe at seq 5 — gap from next_seq(3) = 2, threshold = 7
        buf.push(make_packet(5, true));

        assert_eq!(
            buf.stats.keyframe_resets, resets_before,
            "in-sequence keyframe should not trigger recovery"
        );

        // Keyframe at seq 5 is in the buffer but next_seq is 3.
        // Push more to meet depth gate, then drain to keyframe.
        buf.push(make_packet(6, false));
        // pending: {3, 4, 5, 6} — depth 4 ≥ 3
        assert_eq!(buf.pull_next().unwrap().seq, 3);
        assert_eq!(buf.pull_next().unwrap().seq, 4);
        // depth now 2, but seq 5 is keyframe → bypasses gate
        let pkt = buf.pull_next().unwrap();
        assert_eq!(pkt.seq, 5);
        assert!(pkt.is_keyframe);
    }

    #[test]
    fn recovery_does_not_regate_on_depth() {
        let mut buf = make_buffer_with_target(30, 3);

        buf.push(make_packet(0, true));
        let _ = buf.pull_next();

        buf.push(make_packet(1, false));
        buf.push(make_packet(2, false));
        buf.push(make_packet(3, false));
        let _ = buf.pull_next();
        let _ = buf.pull_next();
        let _ = buf.pull_next();

        // Recovery keyframe at 50
        buf.push(make_packet(50, true));
        let pkt = buf.pull_next().unwrap();
        assert_eq!(pkt.seq, 50, "recovery keyframe should release immediately");

        // Deltas need depth >= target to release
        buf.push(make_packet(51, false));
        buf.push(make_packet(52, false));
        buf.push(make_packet(53, false));
        let pkt = buf.pull_next().unwrap();
        assert_eq!(pkt.seq, 51, "post-recovery delta releases once depth met");
    }

    #[test]
    fn late_packets_are_dropped() {
        let mut buf = make_buffer_with_target(30, 1);

        buf.push(make_packet(5, true));
        let _ = buf.pull_next(); // consume seq 5, next_seq = 6

        // Late packet (seq 3) arrives after we've moved past it
        buf.push(make_packet(3, false));
        assert_eq!(buf.stats.packets_dropped_late, 1);
        assert_eq!(buf.depth(), 0);
    }

    #[test]
    fn gap_skips_to_keyframe_for_clean_recovery() {
        let mut buf = make_buffer_with_target(30, 1);

        buf.push(make_packet(0, true));
        let _ = buf.pull_next(); // next_seq = 1

        // Seq 1-4 lost, but seq 5 (keyframe) and 6 arrive
        buf.push(make_packet(5, true));
        buf.push(make_packet(6, false));

        // push already recovered to seq 5, so first pull delivers it
        let pkt = buf.pull_next().unwrap();
        assert_eq!(pkt.seq, 5);
        assert!(pkt.is_keyframe);
    }

    #[test]
    fn seq_wraparound_comparison() {
        // Normal case
        assert!(is_seq_before(5, 10));
        assert!(!is_seq_before(10, 5));
        // Wraparound: u32::MAX is before 0
        assert!(is_seq_before(u32::MAX, 0));
        assert!(is_seq_after(0, u32::MAX));
    }

    #[test]
    fn adaptive_depth_converges() {
        let mut ad = AdaptiveDepth::new(30);
        // Seeded at 100ms jitter — should start high
        assert!(ad.target_depth() >= 5);

        // Simulate stable arrivals (~33ms apart, matching 30fps)
        let mut t = Instant::now();
        ad.last_arrival = Some(t);
        for _ in 0..100 {
            t += std::time::Duration::from_millis(33);
            // Manually set last_arrival to control timing in tests
            let actual_ms = 33.0_f32;
            let deviation = (actual_ms - ad.expected_interval_ms).abs();
            ad.jitter_ema = ad.jitter_ema * (1.0 - ad.alpha) + deviation * ad.alpha;
        }

        // After 100 stable packets, jitter should be low → small target
        assert!(
            ad.target_depth() <= 3,
            "expected convergence to low depth, got {}",
            ad.target_depth()
        );
    }
}
