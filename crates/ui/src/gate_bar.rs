//! Voice gate bar widget.
//!
//! Shows the **post-denoise** audio level alongside a threshold marker.
//! Bar turns green when the level exceeds the threshold (gate open),
//! stays dim when below (gate closed).
//!
//! **Important**: The `level` atomic holds the *display-scaled* value
//! (raw RMS × 3, clamped to 0–1) written by the input pipeline.
//! The `threshold` is the *raw* RMS threshold (e.g. 0.02).
//! We apply the same × 3 scaling to the threshold before drawing
//! so that the visual comparison matches what the real `NoiseGate` sees.

use crate::theme;
use fltk::{draw, enums, frame, group, prelude::*};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// Display scaling factor — must match the `rms * 3.0` in input.rs
/// where `stats.denoised_level` is written.
const LEVEL_DISPLAY_SCALE: f32 = 3.0;

/// Create a gate bar and add it to `col`.
///
/// - `level`: shared `AtomicU32` (bit-cast `f32` in 0.0–1.0) — post-denoise,
///   already multiplied by [`LEVEL_DISPLAY_SCALE`].
/// - `threshold`: shared *raw* threshold value (e.g. 0.02 RMS).
///   Drawn with the same display scaling so the line sits at the
///   correct position relative to the level bar.
pub fn make(
    col: &mut group::Flex,
    level: Arc<AtomicU32>,
    threshold: Rc<RefCell<f32>>,
    height: i32,
) -> frame::Frame {
    let mut f = frame::Frame::default();
    f.set_frame(enums::FrameType::FlatBox);
    f.set_color(theme::SURFACE);
    col.fixed(&f, height);

    f.draw(move |frm| {
        // Background
        draw::set_draw_color(theme::SURFACE);
        draw::draw_rectf(frm.x(), frm.y(), frm.w(), frm.h());

        let val = f32::from_bits(level.load(Ordering::Relaxed)).clamp(0.0, 1.0);
        let raw_thr = *threshold.borrow();

        // Scale the threshold by the same factor used on the level,
        // so the red line sits where the gate actually triggers.
        let thr_scaled = (raw_thr * LEVEL_DISPLAY_SCALE).clamp(0.0, 1.0);

        let bar_w = ((frm.w() - 4) as f32 * val) as i32;

        // Bar: green when above threshold, dim when below
        let color = if val > thr_scaled {
            theme::SUCCESS
        } else {
            theme::SUBTLE
        };
        if bar_w > 0 {
            draw::set_draw_color(color);
            draw::draw_rectf(frm.x() + 2, frm.y() + 4, bar_w, frm.h() - 8);
        }

        // Threshold marker (red vertical line)
        let thr_x = frm.x() + 2 + ((frm.w() - 4) as f32 * thr_scaled) as i32;
        draw::set_draw_color(theme::DANGER);
        draw::set_line_style(draw::LineStyle::Solid, 2);
        draw::draw_line(thr_x, frm.y() + 2, thr_x, frm.y() + frm.h() - 2);
        draw::set_line_style(draw::LineStyle::Solid, 1);

        // Border
        draw::set_draw_color(theme::BORDER);
        draw::draw_rect(frm.x(), frm.y(), frm.w(), frm.h());
    });

    f
}