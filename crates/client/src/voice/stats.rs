// ============================================================================
// voice/stats.rs - Comprehensive diagnostics (point 14)
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

pub struct AudioStats {
    // Packet stats
    pub packets_sent: AtomicU64,
    pub packets_received: AtomicU64,
    pub packets_lost: AtomicU64,
    
    // Codec stats
    pub encode_errors: AtomicU64,
    pub decode_errors: AtomicU64,
    pub fec_recoveries: AtomicU64,
    pub plc_frames: AtomicU64,
    
    // Buffer stats
    pub input_overruns: AtomicU64,
    pub output_underruns: AtomicU64,
    pub producer_overruns: AtomicU64,
    
    // Stream stats
    pub input_callbacks: AtomicU64,
    pub output_callbacks: AtomicU64,
    pub stream_errors: AtomicU64,
    
    // Jitter buffer stats
    pub jitter_buffer_frames: AtomicU64,
    pub jitter_buffer_occupancy: AtomicUsize,  // Changed to AtomicUsize
    pub drift_corrections: AtomicU64,
    
    // Device stats
    pub device_disconnects: AtomicU64,
    
    // Timing
    pub start_time: Instant,
}

impl AudioStats {
    pub fn new() -> Self {
        Self {
            packets_sent: AtomicU64::new(0),
            packets_received: AtomicU64::new(0),
            packets_lost: AtomicU64::new(0),
            encode_errors: AtomicU64::new(0),
            decode_errors: AtomicU64::new(0),
            fec_recoveries: AtomicU64::new(0),
            plc_frames: AtomicU64::new(0),
            input_overruns: AtomicU64::new(0),
            output_underruns: AtomicU64::new(0),
            producer_overruns: AtomicU64::new(0),
            input_callbacks: AtomicU64::new(0),
            output_callbacks: AtomicU64::new(0),
            stream_errors: AtomicU64::new(0),
            jitter_buffer_frames: AtomicU64::new(0),
            jitter_buffer_occupancy: AtomicUsize::new(0),
            drift_corrections: AtomicU64::new(0),
            device_disconnects: AtomicU64::new(0),
            start_time: Instant::now(),
        }
    }
    
    pub fn get_summary(&self) -> String {
        let elapsed = self.start_time.elapsed().as_secs();
        let sent = self.packets_sent.load(Ordering::Relaxed);
        let received = self.packets_received.load(Ordering::Relaxed);
        let lost = self.packets_lost.load(Ordering::Relaxed);
        
        let loss_rate = if received > 0 {
            (lost as f32 / (received + lost) as f32) * 100.0
        } else {
            0.0
        };
        
        format!(
            "📊 Audio Statistics ({}s)\n\
             Packets: Sent {}, Received {}, Lost {} ({:.1}%)\n\
             Codec: FEC recoveries {}, PLC frames {}\n\
             Buffers: Input overruns {}, Output underruns {}\n\
             Jitter: {} ms occupancy, {} drift corrections\n\
             Errors: Encode {}, Decode {}, Stream {}",
            elapsed,
            sent, received, lost, loss_rate,
            self.fec_recoveries.load(Ordering::Relaxed),
            self.plc_frames.load(Ordering::Relaxed),
            self.input_overruns.load(Ordering::Relaxed),
            self.output_underruns.load(Ordering::Relaxed),
            self.jitter_buffer_occupancy.load(Ordering::Relaxed),
            self.drift_corrections.load(Ordering::Relaxed),
            self.encode_errors.load(Ordering::Relaxed),
            self.decode_errors.load(Ordering::Relaxed),
            self.stream_errors.load(Ordering::Relaxed),
        )
    }
}