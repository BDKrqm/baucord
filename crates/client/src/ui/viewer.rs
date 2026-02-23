// ui/viewer.rs — Screen share viewer window.
//
// One window per watched peer. Receives RGBA pixel data from the screen
// thread (via ScreenEvent::FrameReady) and blits it into an FLTK image.
//
// The window is non-resizable at the stream resolution. Closing it sends
// an unsubscribe message so the server stops forwarding frames.

use fltk::{enums, image::RgbImage, prelude::*, *};
use std::cell::RefCell;
use std::rc::Rc;

use crate::ui::messages::UiMessage;
use ui::theme;

/// State for a single viewer window.
pub struct ViewerWindow {
    pub client_id: u64,
    pub width: u32,
    pub height: u32,
    wind: window::Window,
    canvas: frame::Frame,
    /// Shared pixel buffer — written by update_frame(), read by the draw callback.
    pixels: Rc<RefCell<Vec<u8>>>,
    /// Frame counter for the title bar stats.
    frame_count: u64,
}

impl ViewerWindow {
    /// Create and show a viewer window for a remote peer's stream.
    ///
    /// `username` is displayed in the title bar.
    /// The window starts at the stream's native resolution (capped to
    /// screen size minus padding so it always fits).
    pub fn new(
        client_id: u64,
        username: &str,
        width: u32,
        height: u32,
        sender: app::Sender<UiMessage>,
    ) -> Self {
        // Cap window size to 90% of screen
        let (screen_w, screen_h) = app::screen_size();
        let max_w = (screen_w * 0.9) as u32;
        let max_h = (screen_h * 0.9) as u32;

        let win_w = width.min(max_w) as i32;
        let win_h = height.min(max_h) as i32;

        let title = format!("{username}'s screen ({width}x{height})");

        let mut wind = window::Window::default()
            .with_size(win_w, win_h)
            .with_label(&title);
        wind.make_resizable(false);
        wind.set_color(enums::Color::Black);
        crate::ui::set_window_icon(&mut wind);

        // Center on screen
        let cx = ((screen_w as i32) - win_w) / 2;
        let cy = ((screen_h as i32) - win_h) / 2;
        wind.set_pos(cx, cy);

        // Canvas frame fills the entire window
        let mut canvas = frame::Frame::default()
            .with_pos(0, 0)
            .with_size(win_w, win_h);
        canvas.set_frame(enums::FrameType::FlatBox);
        canvas.set_color(enums::Color::Black);

        // Shared pixel buffer for the draw callback
        let pixels = Rc::new(RefCell::new(vec![0u8; (width * height * 4) as usize]));
        let pixels_draw = pixels.clone();
        let draw_w = width;
        let draw_h = height;

        canvas.draw(move |frm| {
            let px = pixels_draw.borrow();
            if px.is_empty() {
                return;
            }

            // Create an RgbImage from the pixel buffer and draw it
            // fltk RgbImage: depth 4 = RGBA
            if let Ok(mut img) =
                RgbImage::new(&px, draw_w as i32, draw_h as i32, enums::ColorDepth::Rgba8)
            {
                img.draw(frm.x(), frm.y(), frm.w(), frm.h());
            }
        });

        wind.end();
        wind.show();

        // Close → unsubscribe from this peer's stream
        let cid = client_id;
        wind.set_callback(move |w| {
            if app::event() == enums::Event::Close {
                sender.send(UiMessage::UnwatchPeer { client_id: cid });
                w.hide();
            }
        });

        Self {
            client_id,
            width,
            height,
            wind,
            canvas,
            pixels,
            frame_count: 0,
        }
    }

    /// Push a new RGBA frame into the viewer. Redraws the canvas.
    pub fn update_frame(&mut self, rgba: Vec<u8>, width: u32, height: u32) {
        // If resolution changed, we'd need to resize — for now just check
        if width != self.width || height != self.height {
            log::warn!(
                "Viewer resolution mismatch: expected {}x{}, got {}x{}",
                self.width,
                self.height,
                width,
                height
            );
            return;
        }

        // Swap in the new pixel data
        *self.pixels.borrow_mut() = rgba;

        self.frame_count += 1;
        self.canvas.redraw();
    }

    /// Returns true if the window has been closed by the user.
    pub fn is_closed(&self) -> bool {
        !self.wind.shown()
    }

    /// Close the window programmatically (e.g. peer stopped sharing).
    pub fn close(&mut self) {
        self.wind.hide();
    }
}
