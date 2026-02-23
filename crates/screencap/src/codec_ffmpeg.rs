//! H.264 encode/decode via FFmpeg (`ffmpeg-next` crate).
//!
//! Tries hardware encoders in priority order, falls back to libx264.
//! Reuses FFmpeg frame objects and output buffers across calls.
//!
//! # System requirements
//!
//! - **Windows (static):** `vcpkg install ffmpeg[x264,gpl]:x64-windows-static-md`
//!   then set `VCPKG_ROOT`.
//! - **Linux:** `apt install libavcodec-dev libavformat-dev libavutil-dev libswscale-dev`
//! - **macOS:** `brew install ffmpeg`

extern crate ffmpeg_next as ffmpeg;

use ffmpeg::codec::{self, encoder, packet::Packet};
use ffmpeg::format::Pixel;
use ffmpeg::software::scaling::{Context as SwsContext, Flags};
use ffmpeg::util::frame::video::Video as VideoFrame;
use ffmpeg::{Dictionary, Rational};

use crate::{CodecError, DecodedFrame, EncodedFrame, Frame, Quality, VideoDecoder, VideoEncoder};

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Hardware-accelerated (or software fallback) H.264 encoder via FFmpeg.
///
/// Reuses the RGBA input frame, YUV working frame, and output packet buffer
/// across encode calls — the only per-frame allocation is the final `EncodedFrame::data`
/// which must be owned by the caller anyway.
pub struct FfmpegEncoder {
    encoder: ffmpeg::encoder::Video,
    sws_ctx: SwsContext,
    quality: Quality,
    frame_count: i64,
    keyframe_interval: i64,
    /// Reusable FFmpeg frame for RGBA input — avoids alloc per encode.
    rgba_frame: VideoFrame,
    /// Reusable FFmpeg frame for YUV420P — fed to encoder.
    yuv_frame: VideoFrame,
    /// Reusable packet for receiving encoder output.
    pkt: Packet,
    /// Reusable buffer for collecting encoded NAL data.
    out_buf: Vec<u8>,
}

/// Encoder names to try, in priority order.
const ENCODER_PRIORITY: &[&str] = &["h264_nvenc", "h264_qsv", "h264_amf", "libx264"];

impl FfmpegEncoder {
    /// Create an encoder. Tries hardware encoders first, falls back to libx264.
    pub fn new(quality: Quality) -> Result<Self, CodecError> {
        ffmpeg::init().map_err(|e| CodecError::EncoderInit(e.to_string()))?;

        let (enc, enc_name) = find_encoder()?;

        let mut ctx = ffmpeg::codec::context::Context::new_with_codec(enc)
            .encoder()
            .video()
            .map_err(|e| CodecError::EncoderInit(e.to_string()))?;

        ctx.set_width(quality.width());
        ctx.set_height(quality.height());
        ctx.set_format(Pixel::YUV420P);
        ctx.set_time_base(Rational(1, quality.fps() as i32));
        ctx.set_frame_rate(Some(Rational(quality.fps() as i32, 1)));
        ctx.set_bit_rate(quality.bitrate() as usize);
        ctx.set_gop(quality.fps() * 2);
        ctx.set_max_b_frames(0);

        let opts = codec_options(enc_name);
        let encoder = ctx
            .open_with(opts)
            .map_err(|e| CodecError::EncoderInit(format!("{enc_name}: {e}")))?;

        log::info!("Using FFmpeg encoder: {enc_name}");

        let sws_ctx = SwsContext::get(
            Pixel::RGBA,
            quality.width(),
            quality.height(),
            Pixel::YUV420P,
            quality.width(),
            quality.height(),
            Flags::FAST_BILINEAR,
        )
        .map_err(|e| CodecError::ColorConversion(e.to_string()))?;

        let rgba_frame = VideoFrame::new(Pixel::RGBA, quality.width(), quality.height());
        let mut yuv_frame = VideoFrame::new(Pixel::YUV420P, quality.width(), quality.height());
        yuv_frame.set_format(Pixel::YUV420P);

        // Pre-allocate output buffer for typical keyframe size
        let estimated_frame_bytes = quality.bitrate() as usize / quality.fps() as usize / 8;
        let out_buf = Vec::with_capacity(estimated_frame_bytes * 2);

        Ok(Self {
            encoder,
            sws_ctx,
            quality,
            frame_count: 0,
            keyframe_interval: (quality.fps() * 2) as i64,
            rgba_frame,
            yuv_frame,
            pkt: Packet::empty(),
            out_buf,
        })
    }

    /// Encode one RGBA frame.
    pub fn encode(&mut self, frame: &Frame) -> Result<EncodedFrame, CodecError> {
        // Copy RGBA data into reusable FFmpeg frame (stride-aware)
        fill_rgba_frame(&mut self.rgba_frame, &frame.data, frame.width, frame.height);

        // RGBA → YUV420P via FFmpeg's optimized swscale
        self.sws_ctx
            .run(&self.rgba_frame, &mut self.yuv_frame)
            .map_err(|e| CodecError::ColorConversion(e.to_string()))?;

        self.yuv_frame.set_pts(Some(self.frame_count));

        if self.frame_count % self.keyframe_interval == 0 {
            self.yuv_frame.set_kind(ffmpeg::picture::Type::I);
        } else {
            self.yuv_frame.set_kind(ffmpeg::picture::Type::None);
        }

        self.encoder
            .send_frame(&self.yuv_frame)
            .map_err(|e| CodecError::Encode(e.to_string()))?;

        // Collect output packets into reusable buffer
        self.out_buf.clear();
        let mut is_keyframe = false;
        while self.encoder.receive_packet(&mut self.pkt).is_ok() {
            if let Some(data) = self.pkt.data() {
                self.out_buf.extend_from_slice(data);
            }
            if self.pkt.is_key() {
                is_keyframe = true;
            }
        }

        self.frame_count += 1;

        Ok(EncodedFrame {
            data: self.out_buf.clone(),
            timestamp_ms: frame.timestamp_ms,
            is_keyframe,
        })
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }

    /// Returns the name of the active encoder (e.g. "h264_nvenc", "libx264").
    pub fn encoder_name(&self) -> String {
        self.encoder
            .codec()
            .map(|c| c.name().to_owned())
            .unwrap_or_else(|| "unknown".into())
    }
}

impl VideoEncoder for FfmpegEncoder {
    fn encode(&mut self, frame: &Frame) -> Result<EncodedFrame, CodecError> {
        self.encode(frame)
    }

    fn quality(&self) -> Quality {
        self.quality
    }
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

/// H.264 decoder via FFmpeg.
///
/// Lazily initializes the YUV→RGBA scaler on first decoded frame
/// (dimensions aren't known until then). Reuses the RGBA output frame
/// and pixel buffer across calls.
pub struct FfmpegDecoder {
    decoder: ffmpeg::decoder::Video,
    sws_ctx: Option<SwsContext>,
    /// Reusable FFmpeg frame for decoded YUV output.
    decoded_frame: VideoFrame,
    /// Reusable FFmpeg frame for RGBA conversion output.
    rgba_frame: Option<VideoFrame>,
    /// Reusable pixel buffer for the final RGBA data.
    rgba_buf: Vec<u8>,
}

impl FfmpegDecoder {
    pub fn new() -> Result<Self, CodecError> {
        ffmpeg::init().map_err(|e| CodecError::DecoderInit(e.to_string()))?;

        let dec =
            ffmpeg::decoder::find(codec::Id::H264).ok_or(CodecError::DecoderInit(
                "H264 decoder not found in FFmpeg".into(),
            ))?;

        let ctx = ffmpeg::codec::context::Context::new_with_codec(dec)
            .decoder()
            .video()
            .map_err(|e| CodecError::DecoderInit(e.to_string()))?;

        Ok(Self {
            decoder: ctx,
            sws_ctx: None,
            decoded_frame: VideoFrame::empty(),
            rgba_frame: None,
            rgba_buf: Vec::new(),
        })
    }

    /// Decode H.264 NAL data into RGBA.
    ///
    /// Returns `Ok(None)` when the decoder needs more data (EAGAIN).
    pub fn decode(&mut self, encoded: &EncodedFrame) -> Result<Option<DecodedFrame>, CodecError> {
        if encoded.data.is_empty() {
            return Ok(None);
        }

        let mut pkt = Packet::copy(&encoded.data);
        pkt.set_pts(Some(encoded.timestamp_ms as i64));
        pkt.set_dts(Some(encoded.timestamp_ms as i64));

        self.decoder
            .send_packet(&pkt)
            .map_err(|e| CodecError::Decode(e.to_string()))?;

        match self.decoder.receive_frame(&mut self.decoded_frame) {
            Ok(()) => {}
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => {
                return Ok(None);
            }
            Err(e) => return Err(CodecError::Decode(e.to_string())),
        }

        let w = self.decoded_frame.width();
        let h = self.decoded_frame.height();

        // Lazily create SWS + RGBA frame on first decode (dimensions unknown until now)
        if self.sws_ctx.is_none() {
            self.sws_ctx = Some(
                SwsContext::get(
                    self.decoded_frame.format(),
                    w,
                    h,
                    Pixel::RGBA,
                    w,
                    h,
                    Flags::FAST_BILINEAR,
                )
                .map_err(|e| CodecError::ColorConversion(e.to_string()))?,
            );
            self.rgba_frame = Some(VideoFrame::new(Pixel::RGBA, w, h));
        }

        let rgba_frame = self.rgba_frame.as_mut().expect("initialized above");
        let sws = self.sws_ctx.as_mut().expect("initialized above");

        sws.run(&self.decoded_frame, rgba_frame)
            .map_err(|e| CodecError::ColorConversion(e.to_string()))?;

        // Extract RGBA data (stride-aware copy)
        let stride = rgba_frame.stride(0);
        let src = rgba_frame.data(0);
        let rgba_size = (w * h * 4) as usize;
        self.rgba_buf.resize(rgba_size, 0);

        let row_bytes = w as usize * 4;
        for row in 0..h as usize {
            let src_start = row * stride;
            let dst_start = row * row_bytes;
            self.rgba_buf[dst_start..dst_start + row_bytes]
                .copy_from_slice(&src[src_start..src_start + row_bytes]);
        }

        // Hand off the buffer, replace with empty
        let data = std::mem::take(&mut self.rgba_buf);

        Ok(Some(DecodedFrame {
            data,
            width: w,
            height: h,
            timestamp_ms: encoded.timestamp_ms,
        }))
    }

    /// Return a previously-decoded RGBA buffer for reuse.
    pub fn return_buffer(&mut self, mut buf: Vec<u8>) {
        buf.clear();
        self.rgba_buf = buf;
    }
}

impl VideoDecoder for FfmpegDecoder {
    fn decode(&mut self, encoded: &EncodedFrame) -> Result<Option<DecodedFrame>, CodecError> {
        self.decode(encoded)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Find the best available H.264 encoder.
fn find_encoder() -> Result<(ffmpeg::Codec, &'static str), CodecError> {
    for &name in ENCODER_PRIORITY {
        if let Some(enc) = encoder::find_by_name(name) {
            return Ok((enc, name));
        }
    }
    Err(CodecError::NoEncoder(
        ENCODER_PRIORITY.join(", "),
    ))
}

/// Build codec-specific option dictionaries for low-latency encoding.
fn codec_options(name: &str) -> Dictionary<'static> {
    let mut opts = Dictionary::new();
    match name {
        "libx264" => {
            opts.set("preset", "ultrafast");
            opts.set("tune", "zerolatency");
        }
        "h264_nvenc" => {
            opts.set("preset", "p1");
            opts.set("tune", "ull");
            opts.set("rc", "cbr");
            opts.set("zerolatency", "1");
        }
        "h264_qsv" => {
            opts.set("preset", "veryfast");
            opts.set("low_power", "1");
        }
        "h264_amf" => {
            opts.set("usage", "ultralowlatency");
            opts.set("quality", "speed");
        }
        _ => {}
    }
    opts
}

/// Copy RGBA pixel data into an ffmpeg VideoFrame (stride-aware).
fn fill_rgba_frame(frame: &mut VideoFrame, rgba: &[u8], width: u32, height: u32) {
    let stride = frame.stride(0);
    let dst = frame.data_mut(0);
    let row_bytes = width as usize * 4;

    for row in 0..height as usize {
        let src_start = row * row_bytes;
        let dst_start = row * stride;
        dst[dst_start..dst_start + row_bytes]
            .copy_from_slice(&rgba[src_start..src_start + row_bytes]);
    }
}