//! H.264 encode/decode via `openh264`.
//!
//! Key optimizations over naive approach:
//! - Zero-clone encoding: implements `YUVSource` directly on a buffer wrapper
//!   instead of cloning ~1.5MB per frame into `YUVBuffer`.
//! - Integer color conversion: uses fixed-point BT.601 math (see `color` module).
//! - Reusable decode buffer: RGBA output buffer is retained across frames.

use openh264::decoder::Decoder;
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, FrameType, RateControlMode, UsageType,
};
use openh264::formats::YUVSource;
use openh264::Timestamp;

use crate::color;
use crate::{CodecError, DecodedFrame, EncodedFrame, Frame, Quality};

// ---------------------------------------------------------------------------
// Zero-copy YUV wrapper
// ---------------------------------------------------------------------------

/// Borrows a contiguous YUV420 planar buffer and presents it as a `YUVSource`.
///
/// Layout: `[Y: w*h][U: w*h/4][V: w*h/4]`.
/// This avoids the ~1.5MB-per-frame clone that `YUVBuffer::from_vec` requires,
/// since `Encoder::encode_at` only needs a shared reference.
struct YuvSlice<'a> {
    data: &'a [u8],
    w: usize,
    h: usize,
}

impl YUVSource for YuvSlice<'_> {
    fn dimensions(&self) -> (usize, usize) {
        (self.w, self.h)
    }

    fn y(&self) -> &[u8] {
        &self.data[..self.w * self.h]
    }

    fn u(&self) -> &[u8] {
        let offset = self.w * self.h;
        let size = (self.w / 2) * (self.h / 2);
        &self.data[offset..offset + size]
    }

    fn v(&self) -> &[u8] {
        let offset = self.w * self.h + (self.w / 2) * (self.h / 2);
        let size = (self.w / 2) * (self.h / 2);
        &self.data[offset..offset + size]
    }

    fn strides(&self) -> (usize, usize, usize) {
        (self.w, self.w / 2, self.w / 2)
    }
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Software H.264 encoder via openh264.
pub struct H264Encoder {
    encoder: Encoder,
    quality: Quality,
    frame_count: u64,
    keyframe_interval: u64,
    force_next_keyframe: bool,
    /// Reusable YUV420 planar buffer — written each frame, never cloned.
    yuv_buf: Vec<u8>,
}

impl H264Encoder {
    /// Create an encoder configured for the given quality preset.
    pub fn new(quality: Quality) -> Result<Self, CodecError> {
        let config = EncoderConfig::new()
            .max_frame_rate(FrameRate::from_hz(quality.fps() as f32))
            .bitrate(BitRate::from_bps(quality.bitrate()))
            .rate_control_mode(RateControlMode::Bitrate)
            .usage_type(UsageType::ScreenContentRealTime);

        let encoder = Encoder::with_api_config(openh264::OpenH264API::from_source(), config)
            .map_err(|e| CodecError::EncoderInit(e.to_string()))?;

        Ok(Self {
            encoder,
            quality,
            frame_count: 0,
            keyframe_interval: quality.fps() as u64 * 2,
            force_next_keyframe: false,
            yuv_buf: vec![0u8; quality.yuv420_size()],
        })
    }

    /// Encode one RGBA frame into H.264 NAL units.
    pub fn encode(&mut self, frame: &Frame) -> Result<EncodedFrame, CodecError> {
        let w = frame.width as usize;
        let h = frame.height as usize;

        if self.force_next_keyframe || self.frame_count % self.keyframe_interval == 0 {
            self.encoder.force_intra_frame();
            self.force_next_keyframe = false;
        }

        // Convert RGBA → YUV420 in-place (integer math, no allocation)
        color::rgba_to_yuv420(&frame.data, w, h, &mut self.yuv_buf);

        // Wrap as a borrowed YUVSource — no clone
        let yuv = YuvSlice {
            data: &self.yuv_buf,
            w,
            h,
        };

        let timestamp = Timestamp::from_millis(frame.timestamp_ms);

        let bitstream = self
            .encoder
            .encode_at(&yuv, timestamp)
            .map_err(|e| CodecError::Encode(e.to_string()))?;

        let is_keyframe = matches!(bitstream.frame_type(), FrameType::IDR | FrameType::I);
        let data = bitstream.to_vec();
        self.frame_count += 1;

        Ok(EncodedFrame {
            data,
            timestamp_ms: frame.timestamp_ms,
            is_keyframe,
        })
    }

    /// Force the next encoded frame to be a keyframe.
    pub fn force_keyframe(&mut self) {
        self.force_next_keyframe = true;
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }
}

impl crate::VideoEncoder for H264Encoder {
    fn encode(&mut self, frame: &Frame) -> Result<EncodedFrame, CodecError> {
        self.encode(frame)
    }

    fn quality(&self) -> Quality {
        self.quality
    }

    fn force_keyframe(&mut self) {
        self.force_keyframe();
    }
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

/// Software H.264 decoder via openh264.
pub struct H264Decoder {
    decoder: Decoder,
    /// Reusable RGBA output buffer — resized on first decode, reused thereafter.
    rgba_buf: Vec<u8>,
}

impl H264Decoder {
    pub fn new() -> Result<Self, CodecError> {
        let decoder = Decoder::new().map_err(|e| CodecError::DecoderInit(e.to_string()))?;
        Ok(Self {
            decoder,
            rgba_buf: Vec::new(),
        })
    }

    /// Decode H.264 NAL data into an RGBA frame.
    ///
    /// Returns `Ok(None)` if the decoder is still buffering (needs more NALs).
    pub fn decode(&mut self, encoded: &EncodedFrame) -> Result<Option<DecodedFrame>, CodecError> {
        let yuv = match self.decoder.decode(&encoded.data) {
            Ok(Some(yuv)) => yuv,
            Ok(None) => return Ok(None),
            Err(e) => return Err(CodecError::Decode(e.to_string())),
        };

        let (w, h) = yuv.dimensions();
        let (y_stride, u_stride, v_stride) = yuv.strides();

        // Ensure buffer is large enough, keep existing capacity
        let rgba_size = w * h * 4;
        self.rgba_buf.resize(rgba_size, 0);

        color::yuv420_to_rgba(
            yuv.y(),
            yuv.u(),
            yuv.v(),
            y_stride,
            u_stride,
            v_stride,
            w,
            h,
            &mut self.rgba_buf,
        );

        // Hand off the buffer, replace with empty (caller gets ownership)
        let data = std::mem::take(&mut self.rgba_buf);

        Ok(Some(DecodedFrame {
            data,
            width: w as u32,
            height: h as u32,
            timestamp_ms: encoded.timestamp_ms,
        }))
    }

    /// Return a previously-decoded RGBA buffer for reuse.
    ///
    /// Avoids allocating a new buffer on the next `decode()` call.
    pub fn return_buffer(&mut self, mut buf: Vec<u8>) {
        buf.clear();
        self.rgba_buf = buf;
    }
}

impl crate::VideoDecoder for H264Decoder {
    fn decode(&mut self, encoded: &EncodedFrame) -> Result<Option<DecodedFrame>, CodecError> {
        self.decode(encoded)
    }

    fn return_buffer(&mut self, buf: Vec<u8>) {
        self.return_buffer(buf);
    }
}
