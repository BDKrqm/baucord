//! VU (Volume Unit) meter widget.
//!
//! A horizontal bar that reads an [`AtomicU32`] (bit-cast `f32` in 0..1)
//! and draws a colour-coded level indicator.  The colour shifts from
//! `accent` → [`WARNING`](crate::theme::WARNING) → [`DANGER`](crate::theme::DANGER)
//! as the level rises.

use crate::theme;
use fltk::{draw, enums, frame, group, prelude::*};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// Create a VU meter and add it to `col` with the given height.
///
/// `level` must be a shared `AtomicU32` whose bits are an `f32` in 0.0–1.0.
/// `accent` is the "normal" bar colour (e.g. `theme::SUCCESS` for input,
/// `theme::CYAN` for output).
pub fn make(
    col: &mut group::Flex,
    level: Arc<AtomicU32>,
    height: i32,
    accent: enums::Color,
) -> frame::Frame {
    let mut f = frame::Frame::default();
    f.set_frame(enums::FrameType::FlatBox);
    f.set_color(theme::SURFACE);
    col.fixed(&f, height);

    f.draw(move |frm| {
        // Background
        draw::set_draw_color(theme::SURFACE);
        draw::draw_rectf(frm.x(), frm.y(), frm.w(), frm.h());

        // Level bar
        let val = f32::from_bits(level.load(Ordering::Relaxed)).clamp(0.0, 1.0);
        let bar_w = ((frm.w() - 4) as f32 * val) as i32;

        let color = if val > 0.9 {
            theme::DANGER
        } else if val > 0.7 {
            theme::WARNING
        } else {
            accent
        };

        if bar_w > 0 {
            draw::set_draw_color(color);
            draw::draw_rectf(frm.x() + 2, frm.y() + 3, bar_w, frm.h() - 6);
        }

        // Border
        draw::set_draw_color(theme::BORDER);
        draw::draw_rect(frm.x(), frm.y(), frm.w(), frm.h());
    });

    f
}
