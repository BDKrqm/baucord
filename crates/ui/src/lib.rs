//! Shared FLTK widgets and theming for voice-chat-core.
//!
//! This crate provides:
//!
//! - **[`theme`]** — one-call dark theme backed by [`fltk_theme`].
//! - **[`vu_meter`]** — colour-coded audio level bar.
//! - **[`gate_bar`]** — post-denoise level with threshold marker.
//! - **[`helpers`]** — section headers, stat labels, sliders, device
//!   choosers, and other layout primitives.
//!
//! # Quick start
//!
//! ```no_run
//! use fltk::{prelude::*, *};
//!
//! fn main() {
//!     let app = app::App::default();
//!     voice_chat_ui::theme::apply();
//!     // ... build your window ...
//! }
//! ```

pub mod gate_bar;
pub mod helpers;
pub mod theme;
pub mod vu_meter;
