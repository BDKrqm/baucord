//! Reusable layout helpers for building panels.
//!
//! Thin wrappers that create commonly-used widgets and add them to a
//! [`group::Flex`] column with the right size and alignment.  With
//! `fltk-theme` handling global colours, these only need to override
//! the few domain-specific colours (stat readouts, muted hints, etc.).

use crate::theme;
use fltk::{enums, frame, group, input, menu, button, text, valuator, prelude::*};

// ── Section & field labels ──────────────────────────────────────────────

/// Bold section heading with an emoji icon, e.g. `("🎧", "DEVICES")`.
pub fn section_header(col: &mut group::Flex, icon: &str, text: &str) {
    let mut lbl = frame::Frame::default()
        .with_label(&format!("{}  {}", icon, text))
        .with_align(enums::Align::Left | enums::Align::Inside);
    lbl.set_label_size(13);
    col.fixed(&lbl, 24);
}

/// Small label above a control, e.g. "Input Device".
pub fn field_label(col: &mut group::Flex, text: &str) {
    let mut lbl = frame::Frame::default()
        .with_label(text)
        .with_align(enums::Align::Left | enums::Align::Inside);
    lbl.set_label_size(11);
    col.fixed(&lbl, 18);
}

/// Monospace-coloured label for live stat readouts.
pub fn stat_label(col: &mut group::Flex, text: &str) -> frame::Frame {
    let mut lbl = frame::Frame::default()
        .with_label(text)
        .with_align(enums::Align::Left | enums::Align::Inside);
    lbl.set_label_color(theme::MONO_TEXT);
    lbl.set_label_size(11);
    col.fixed(&lbl, 16);
    lbl
}

/// Tiny muted label for values / hints below a slider.
pub fn hint_label(col: &mut group::Flex, text: &str) -> frame::Frame {
    let mut lbl = frame::Frame::default()
        .with_label(text)
        .with_align(enums::Align::Left | enums::Align::Inside);
    lbl.set_label_color(theme::TEXT_MUTED);
    lbl.set_label_size(10);
    col.fixed(&lbl, 14);
    lbl
}

// ── Separators ──────────────────────────────────────────────────────────

/// 1px horizontal line.
pub fn separator(col: &mut group::Flex) {
    let mut sep = frame::Frame::default();
    sep.set_frame(enums::FrameType::FlatBox);
    sep.set_color(theme::SUBTLE);
    col.fixed(&sep, 1);
}

// ── Controls ────────────────────────────────────────────────────────────

/// Device / option dropdown.
pub fn choice(col: &mut group::Flex) -> menu::Choice {
    let c = menu::Choice::default();
    col.fixed(&c, 26);
    c
}

/// Populate a device [`Choice`] with "System Default" + device names.
pub fn populate_device_choice(choice: &mut menu::Choice, devices: &[(String, bool)]) {
    choice.clear();
    choice.add_choice("System Default");
    for (name, _) in devices {
        choice.add_choice(name);
    }
    choice.set_value(0);
}

/// Horizontal slider for gain / threshold values.
pub fn slider(
    col: &mut group::Flex,
    min: f64,
    max: f64,
    value: f64,
    step: f64,
    accent: enums::Color,
) -> valuator::HorSlider {
    let mut s = valuator::HorSlider::default();
    s.set_minimum(min);
    s.set_maximum(max);
    s.set_value(value);
    s.set_step(step, 1);
    s.set_selection_color(accent);
    col.fixed(&s, 22);
    s
}

/// Check box toggle.
pub fn check(col: &mut group::Flex, label: &str, checked: bool) -> button::CheckButton {
    let mut cb = button::CheckButton::default().with_label(label);
    cb.set_checked(checked);
    cb.set_label_size(11);
    col.fixed(&cb, 22);
    cb
}

/// Standard button (subdued background).
pub fn button(label: &str) -> button::Button {
    let mut btn = button::Button::default().with_label(label);
    btn.set_frame(enums::FrameType::FlatBox);
    btn.set_label_size(11);
    btn
}

/// Accent-coloured button (for primary actions like "Save", "Send").
pub fn accent_button(label: &str, color: enums::Color) -> button::Button {
    let mut btn = button::Button::default().with_label(label);
    btn.set_color(color);
    btn.set_label_color(enums::Color::White);
    btn.set_frame(enums::FrameType::FlatBox);
    btn
}

/// Populate a device [`Choice`] with "System Default" + device names,
/// pre-selecting the device matching `current` (if any).
pub fn populate_device_choice_with_current(
    choice: &mut menu::Choice,
    devices: &[(String, bool)],
    current: &Option<String>,
) {
    choice.clear();
    choice.add_choice("System Default");
    let mut selected = 0;
    for (i, (name, _)) in devices.iter().enumerate() {
        choice.add_choice(name);
        if let Some(cur) = current.as_ref() {
            if cur == name {
                selected = i + 1;
            }
        }
    }
    choice.set_value(selected as i32);
}

/// Apply dark styling to an [`input::Input`] widget.
pub fn style_input(input: &mut input::Input) {
    input.set_color(theme::SURFACE);
    input.set_text_color(enums::Color::Foreground);
    input.set_cursor_color(enums::Color::Selection);
    input.set_selection_color(enums::Color::Selection);
    input.set_frame(enums::FrameType::BorderBox);
}

/// Apply dark styling to a [`text::TextDisplay`] widget.
pub fn style_text_display(display: &mut text::TextDisplay) {
    display.set_color(theme::SURFACE);
    display.set_text_color(enums::Color::Foreground);
    display.set_cursor_color(enums::Color::Selection);
    display.set_selection_color(enums::Color::Selection);
    display.set_frame(enums::FrameType::BorderBox);
}

// ── Utilities ───────────────────────────────────────────────────────────

/// Human-readable sensitivity string from a threshold value.
pub fn format_sensitivity(value: f64) -> String {
    let name = match value {
        v if v < 0.010 => "Very High",
        v if v < 0.020 => "High",
        v if v < 0.040 => "Medium",
        v if v < 0.065 => "Low",
        _ => "Very Low",
    };
    format!("Sensitivity: {}  (threshold: {:.3} RMS)", name, value)
}