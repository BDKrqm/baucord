// voice/packet.rs - Standardized packet structure with 48kHz timestamps
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct PacketHeader {
    pub seq: u32,       // Changed from u16 to u32 - no truncation
    pub timestamp: u32, // 48kHz sample clock ticks
    pub ssrc: u64,      // Synchronization source (client ID)
    pub vad: bool,      // Voice activity flag
}

#[derive(Debug, Clone)]
pub struct AudioPacket {
    pub header: PacketHeader,
    pub opus_data: Vec<u8>,
}

impl AudioPacket {
    /// Create packet for local encoding
    pub fn new(seq: u32, ts_48k: u32, ssrc: u64, opus_data: Vec<u8>) -> Self {
        Self {
            header: PacketHeader {
                seq,
                timestamp: ts_48k,
                ssrc,
                vad: true, // Set by VAD processor
            },
            opus_data,
        }
    }

    pub fn from_network(seq: u32, ts_48k: u32, ssrc: u64, opus_data: Vec<u8>) -> Self {
        Self {
            header: PacketHeader {
                seq,
                timestamp: ts_48k,
                ssrc: ssrc,
                vad: true,
            },
            opus_data,
        }
    }

    /// Get the duration of this packet in 48kHz samples
    pub fn duration_samples(&self) -> u32 {
        // Each Opus frame at 48kHz is 480 samples (10ms)
        480
    }

    /// Check if this packet is newer than another (wraparound-safe)
    pub fn is_newer_than(&self, other: &AudioPacket) -> bool {
        // Use wraparound-safe comparison
        let diff = self.header.seq.wrapping_sub(other.header.seq) as i32;
        diff > 0 && diff < 0x7FFF_FFFF // Positive and less than half the u32 range
    }
}
