//! Voice (Opus) packet wire format.
//!
//! Voice packets are sent as raw binary on [`VOICE_CHANNEL`] — **not**
//! bincode-serialized — for minimal overhead on the hot path.
//!
//! Wire layout:
//! ```text
//! [VoiceHeader: 24 bytes][Frame 0: 2-byte len + opus data][Frame 1: ...]...
//! ```
//!
//! The bundler on the sender packs 1–2 Opus frames per UDP packet to
//! reduce per-packet overhead without adding latency.

/// Voice sub-protocol version. Bump when the wire format changes.
pub const VOICE_PROTOCOL_VERSION: u8 = 1;

/// Maximum voice packet size (header + all frames).
/// Kept well under typical MTU (~1400) to avoid IP fragmentation.
pub const VOICE_MTU_BUDGET: usize = 800;

/// Maximum number of Opus frames bundled in one voice packet.
pub const VOICE_MAX_FRAMES_PER_PACKET: usize = 2;

/// Maximum size of a single Opus frame in bytes.
pub const VOICE_MAX_FRAME_SIZE: usize = 512;

/// Minimum number of frames before the bundler flushes a packet.
/// Set to 1 for lowest latency (send immediately).
pub const VOICE_BATCH_THRESHOLD: u8 = 1;

/// Fixed-layout header for voice packets.
///
/// Total size: 24 bytes. All multi-byte fields are little-endian.
///
/// ```text
/// offset  size  field
///   0       1   version (VOICE_PROTOCOL_VERSION)
///   1       3   reserved (zero)
///   4       8   client_id (u64 LE)
///  12       4   seq (u32 LE) — base sequence for first frame
///  16       4   ts_48k (u32 LE) — base timestamp in 48kHz samples
///  20       1   frame_count
///  21       3   padding (zero)
/// ```
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct VoiceHeader {
    pub version: u8,
    pub _reserved: [u8; 3],
    pub client_id: u64,
    pub seq: u32,
    pub ts_48k: u32,
    pub frame_count: u8,
    pub _padding: [u8; 3],
}

impl VoiceHeader {
    /// Total byte size of the header on the wire.
    pub const SIZE: usize = 24;
}
