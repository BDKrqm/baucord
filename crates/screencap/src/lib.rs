//! screencap — Lightweight screen capture + H.264 encode/decode wrapper.
//!
//! Built on `xcap` (screen capture) with pluggable codec backends:
//! - `openh264-codec` (default) — zero system deps, software-only
//! - `ffmpeg-codec` — fast, hardware-accelerated, needs FFmpeg installed
//!
//! # Architecture
//!
//! ```text
//! ┌─────────┐     ┌─────────┐     ┌──────────────┐
//! │ Capture │ ──▶ │ Encoder │ ──▶ │ encoded bytes │ ──▶ network / file
//! └─────────┘     └─────────┘     └──────────────┘
//!
//! network / file ──▶ ┌─────────┐ ──▶ ┌────────────┐
//!                    │ Decoder │     │ RGBA frame  │ ──▶ display
//!                    └─────────┘     └────────────┘
//! ```

pub mod capture;
pub mod color;

#[cfg(feature = "openh264-codec")]
pub mod codec;

#[cfg(feature = "ffmpeg-codec")]
pub mod codec_ffmpeg;

use std::fmt;

/// Quality preset for screen sharing.
///
/// Each variant encodes resolution, frame rate, and target bitrate tuned
/// for screen content (sharp text, flat gradients, occasional motion).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Quality {
    /// 1280×720 @ 30 FPS — lowest bandwidth, good for slow connections.
    HD30,
    /// 1280×720 @ 60 FPS — smooth scrolling at modest bandwidth.
    HD60,
    /// 1920×1080 @ 30 FPS — sharp text, moderate motion.
    FHD30,
    /// 1920×1080 @ 60 FPS — best quality, needs hardware encoder.
    FHD60,
}

impl Quality {
    pub const fn width(self) -> u32 {
        match self {
            Self::HD30 | Self::HD60 => 1280,
            Self::FHD30 | Self::FHD60 => 1920,
        }
    }

    pub const fn height(self) -> u32 {
        match self {
            Self::HD30 | Self::HD60 => 720,
            Self::FHD30 | Self::FHD60 => 1080,
        }
    }

    pub const fn fps(self) -> u32 {
        match self {
            Self::HD30 | Self::FHD30 => 30,
            Self::HD60 | Self::FHD60 => 60,
        }
    }

    /// Target bitrate in bits/sec. Tuned for screen content.
    pub const fn bitrate(self) -> u32 {
        match self {
            Self::HD30 => 1_500_000,
            Self::HD60 => 2_500_000,
            Self::FHD30 => 3_000_000,
            Self::FHD60 => 4_500_000,
        }
    }

    /// Duration of one frame.
    pub const fn frame_duration(self) -> std::time::Duration {
        std::time::Duration::from_nanos(1_000_000_000 / self.fps() as u64)
    }

    /// Pixel count for this resolution.
    pub const fn pixel_count(self) -> usize {
        self.width() as usize * self.height() as usize
    }

    /// Byte count for one RGBA frame at this resolution.
    pub const fn rgba_size(self) -> usize {
        self.pixel_count() * 4
    }

    /// Byte count for one YUV420 frame at this resolution.
    pub const fn yuv420_size(self) -> usize {
        self.pixel_count() * 3 / 2
    }

    pub const ALL: &[Quality] = &[Self::HD30, Self::HD60, Self::FHD30, Self::FHD60];
}

impl fmt::Display for Quality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HD30 => f.write_str("720p 30fps"),
            Self::HD60 => f.write_str("720p 60fps"),
            Self::FHD30 => f.write_str("1080p 30fps"),
            Self::FHD60 => f.write_str("1080p 60fps"),
        }
    }
}

// ---------------------------------------------------------------------------
// Frame types
// ---------------------------------------------------------------------------

/// An RGBA frame ready for encoding or display.
///
/// Not `Clone` by design — RGBA buffers are large (3.7 MB at 1080p).
/// Pass by reference or transfer ownership explicitly.
pub struct Frame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub timestamp_ms: u64,
}

/// Encoded H.264 data for one frame.
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub timestamp_ms: u64,
    pub is_keyframe: bool,
}

/// Decoded RGBA frame ready for display.
///
/// Not `Clone` by design — same reasoning as [`Frame`].
pub struct DecodedFrame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub timestamp_ms: u64,
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors from screen capture operations.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("failed to enumerate monitors: {0}")]
    MonitorList(String),
    #[error("monitor index {0} not found")]
    MonitorNotFound(usize),
    #[error("capture failed: {0}")]
    Capture(String),
}

/// Errors from encode/decode operations.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("encoder init failed: {0}")]
    EncoderInit(String),
    #[error("decoder init failed: {0}")]
    DecoderInit(String),
    #[error("encode failed: {0}")]
    Encode(String),
    #[error("decode failed: {0}")]
    Decode(String),
    #[error("color conversion failed: {0}")]
    ColorConversion(String),
    #[error("no H.264 encoder found (tried: {0})")]
    NoEncoder(String),
}

// ---------------------------------------------------------------------------
// Codec traits — allows swapping backends generically
// ---------------------------------------------------------------------------

/// Encoder that converts RGBA frames into H.264 bitstream.
pub trait VideoEncoder {
    fn encode(&mut self, frame: &Frame) -> Result<EncodedFrame, CodecError>;
    fn quality(&self) -> Quality;
}

/// Decoder that converts H.264 bitstream back into RGBA frames.
pub trait VideoDecoder {
    /// Returns `Ok(None)` when the decoder is still buffering (needs more NALs).
    fn decode(&mut self, encoded: &EncodedFrame) -> Result<Option<DecodedFrame>, CodecError>;
}