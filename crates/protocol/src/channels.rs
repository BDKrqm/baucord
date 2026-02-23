//! Channel IDs and shared network constants.
//!
//! Renet multiplexes multiple logical channels over one UDP connection.
//! Each channel has its own reliability and ordering semantics.

/// Application-level protocol identifier.
///
/// Both client and server must agree on this value or the netcode
/// handshake will reject the connection.
pub const PROTOCOL_ID: u64 = 27;

// ── Channel IDs ─────────────────────────────────────────────────────────

/// Reliable ordered channel for chat, signaling, presence, and control
/// messages. Guaranteed delivery, in-order, with automatic retransmit.
pub const RELIABLE_CHANNEL: u8 = 2;

/// Unreliable channel for voice (Opus) packets.
/// Low latency, no retransmit — late packets are useless for real-time audio.
pub const VOICE_CHANNEL: u8 = 3;

/// Unreliable channel for video (H.264) packets.
/// Separate from voice so a large keyframe never delays audio delivery.
pub const VIDEO_CHANNEL: u8 = 4;
pub const RELIABLE_VIDEO_CHANNEL: u8 = 5; // keyframes only (reliable)

// ── Buffer sizes ────────────────────────────────────────────────────────

/// Default memory budget for the reliable channel (bytes).
pub const RELIABLE_BUFFER_BYTES: usize = 1024 * 1024; // 1 MB

/// Default memory budget for the voice channel (bytes).
pub const VOICE_BUFFER_BYTES: usize = 5 * 1024 * 1024; // 5 MB

/// Default memory budget for the video channel (bytes).
/// Larger than voice because H.264 keyframes can be 60+ KB and renet
/// buffers fragments until reassembly completes.
pub const VIDEO_BUFFER_BYTES: usize = 10 * 1024 * 1024; // 10 MB
pub const RELIABLE_VIDEO_BUFFER_BYTES: usize = 5 * 1024 * 1024; // 5 MB
