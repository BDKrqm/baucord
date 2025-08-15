// ============================================================================
// voice/packet.rs - RTP-like packet structure (point 7)
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct PacketHeader {
    pub seq: u16,           // Sequence number
    pub timestamp: u64,     // Capture timestamp
    pub ssrc: u32,          // Synchronization source
    pub vad: bool,          // Voice activity flag
}

#[derive(Debug, Clone)]
pub struct AudioPacket {
    pub header: PacketHeader,
    pub opus_data: Vec<u8>,
}

impl AudioPacket {
    pub fn new(seq: u16, capture_time: Instant, ssrc: u32, opus_data: Vec<u8>) -> Self {
        Self {
            header: PacketHeader {
                seq,
                timestamp: capture_time.elapsed().as_micros() as u64,
                ssrc,
                vad: true,  // Set by VAD processor
            },
            opus_data,
        }
    }
}
