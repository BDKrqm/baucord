//! Screen capture via `xcap`.
//!
//! Captures a monitor's screen and optionally scales to the target resolution.
//! Reuses internal buffers to avoid per-frame allocations.

use std::time::Instant;

use log::info;
use xcap::Monitor;

use crate::{CaptureError, Frame, Quality};

/// Summary of a connected monitor.
#[derive(Debug, Clone)]
pub struct MonitorInfo {
    pub index: usize,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
}

/// Lists available monitors.
///
/// # Errors
///
/// Returns [`CaptureError::MonitorList`] if the OS fails to enumerate monitors.
pub fn list_monitors() -> Result<Vec<MonitorInfo>, CaptureError> {
    let monitors =
        Monitor::all().map_err(|e| CaptureError::MonitorList(e.to_string()))?;

    Ok(monitors
        .into_iter()
        .enumerate()
        .map(|(i, m)| MonitorInfo {
            index: i,
            name: m.name().unwrap_or_else(|_| format!("Monitor {i}")),
            width: m.width().unwrap_or(0),
            height: m.height().unwrap_or(0),
            is_primary: m.is_primary().unwrap_or(false),
        })
        .collect())
}

/// Captures frames from a specific monitor.
///
/// Reuses an internal scale buffer when the source resolution differs from the
/// target quality preset, avoiding a fresh allocation every frame.
pub struct ScreenCapturer {
    monitor: Monitor,
    quality: Quality,
    frame_count: u64,
    start_time: Instant,
    /// Reusable buffer for scaled RGBA output. Empty when no scaling is needed.
    scale_buf: Vec<u8>,
}

impl ScreenCapturer {
    /// Create a capturer for the given monitor index and quality preset.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::MonitorList`] if enumeration fails, or
    /// [`CaptureError::MonitorNotFound`] if the index is out of range.
    pub fn new(monitor_index: usize, quality: Quality) -> Result<Self, CaptureError> {
        let monitors =
            Monitor::all().map_err(|e| CaptureError::MonitorList(e.to_string()))?;

        let monitor = monitors
            .into_iter()
            .nth(monitor_index)
            .ok_or(CaptureError::MonitorNotFound(monitor_index))?;

        let name = monitor.name().unwrap_or_default();
        let w = monitor.width().unwrap_or(0);
        let h = monitor.height().unwrap_or(0);
        info!("Capturer: monitor='{name}' ({w}x{h}), target={quality}");

        Ok(Self {
            monitor,
            quality,
            frame_count: 0,
            start_time: Instant::now(),
            scale_buf: Vec::new(),
        })
    }

    /// Create a capturer for the primary monitor.
    pub fn primary(quality: Quality) -> Result<Self, CaptureError> {
        let monitors =
            Monitor::all().map_err(|e| CaptureError::MonitorList(e.to_string()))?;

        let primary_idx = monitors
            .iter()
            .position(|m| m.is_primary().unwrap_or(false))
            .unwrap_or(0);

        // Reuse the already-fetched list by picking the monitor directly
        let monitor = monitors
            .into_iter()
            .nth(primary_idx)
            .ok_or(CaptureError::MonitorNotFound(primary_idx))?;

        let name = monitor.name().unwrap_or_default();
        let w = monitor.width().unwrap_or(0);
        let h = monitor.height().unwrap_or(0);
        info!("Capturer: monitor='{name}' ({w}x{h}), target={quality}");

        Ok(Self {
            monitor,
            quality,
            frame_count: 0,
            start_time: Instant::now(),
            scale_buf: Vec::new(),
        })
    }

    /// Capture one frame, scaled to the quality preset's resolution.
    ///
    /// Returns RGBA pixel data at the target width×height.
    pub fn capture(&mut self) -> Result<Frame, CaptureError> {
        let image = self
            .monitor
            .capture_image()
            .map_err(|e| CaptureError::Capture(e.to_string()))?;

        let target_w = self.quality.width();
        let target_h = self.quality.height();
        let src_w = image.width();
        let src_h = image.height();

        let timestamp_ms = self.start_time.elapsed().as_millis() as u64;
        self.frame_count += 1;

        let data = if src_w == target_w && src_h == target_h {
            // No scaling needed — take ownership of the image buffer directly
            image.into_raw()
        } else {
            let src = image.into_raw();
            scale_rgba_into(
                &src,
                src_w,
                src_h,
                target_w,
                target_h,
                &mut self.scale_buf,
            );
            // Move the buffer out, replace with empty Vec (no allocation)
            std::mem::take(&mut self.scale_buf)
        };

        Ok(Frame {
            data,
            width: target_w,
            height: target_h,
            timestamp_ms,
        })
    }

    /// Return the previously used RGBA buffer for reuse by the next capture.
    ///
    /// Call this after you're done with the [`Frame`] to avoid allocating a new
    /// scale buffer on the next `capture()` call. Not required — just an
    /// optimization when you own the frame and are done with it.
    pub fn return_buffer(&mut self, mut buf: Vec<u8>) {
        buf.clear();
        self.scale_buf = buf;
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }

    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }
}

/// Nearest-neighbor scale RGBA into a caller-provided buffer.
///
/// Resizes `dst` to fit `dst_w × dst_h × 4` bytes, reusing existing capacity.
fn scale_rgba_into(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    dst: &mut Vec<u8>,
) {
    let dst_size = (dst_w * dst_h * 4) as usize;
    dst.resize(dst_size, 0);

    for y in 0..dst_h {
        let src_y = (y as u64 * src_h as u64 / dst_h as u64) as u32;
        for x in 0..dst_w {
            let src_x = (x as u64 * src_w as u64 / dst_w as u64) as u32;
            let si = ((src_y * src_w + src_x) * 4) as usize;
            let di = ((y * dst_w + x) * 4) as usize;
            dst[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
}