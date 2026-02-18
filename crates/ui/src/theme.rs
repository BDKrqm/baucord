//! App-wide theming.
//!
//! Calls [`fltk_theme`] for the heavy lifting (color map + widget scheme),
//! then layers on the handful of domain-specific colors the audio widgets need.

use fltk::{app, enums};
use fltk_theme::{color_themes, ColorTheme, SchemeType, WidgetScheme};

// ── Domain-specific colours ─────────────────────────────────────────────
// These are the only hand-picked values in the entire UI crate.
// Everything else comes from fltk-theme's Fleet Dark2 palette.

// ── Backgrounds ─────────────────────────────────────────────────────────
/// Primary window background.
pub const BG_PRIMARY: enums::Color = enums::Color::from_rgb(30, 30, 30);
/// Secondary panels (sidebar, scroll areas).
pub const BG_SECONDARY: enums::Color = enums::Color::from_rgb(40, 40, 40);
/// Tertiary / subtle element background (separators, inactive bars, button bg).
pub const BG_TERTIARY: enums::Color = enums::Color::from_rgb(50, 50, 50);

// ── Text ────────────────────────────────────────────────────────────────
/// Primary text (headings, labels).
pub const TEXT_PRIMARY: enums::Color = enums::Color::from_rgb(220, 220, 220);
/// Secondary text (field labels, descriptions).
pub const TEXT_SECONDARY: enums::Color = enums::Color::from_rgb(180, 180, 180);
/// Muted / secondary text (hints, inactive items).
pub const TEXT_MUTED: enums::Color = enums::Color::from_rgb(120, 120, 135);
/// Monospace stat readout text.
pub const MONO_TEXT: enums::Color = enums::Color::from_rgb(140, 200, 140);

// ── Accents & status ────────────────────────────────────────────────────
/// Primary interactive accent (buttons, selection, sliders).
pub const ACCENT: enums::Color = enums::Color::from_rgb(88, 101, 242);
/// VU meter bar when level is normal / success.
pub const SUCCESS: enums::Color = enums::Color::from_rgb(67, 181, 129);
/// VU meter bar when level is getting hot (>70%) / caution.
pub const WARNING: enums::Color = enums::Color::from_rgb(250, 166, 26);
/// VU meter bar when clipping / error / destructive actions.
pub const DANGER: enums::Color = enums::Color::from_rgb(237, 66, 69);
/// Speaking indicator / "gate open" colour.
pub const SPEAKING: enums::Color = enums::Color::from_rgb(67, 181, 129);
/// Output-channel accent (distinct from input green).
pub const CYAN: enums::Color = enums::Color::from_rgb(56, 189, 248);

// ── Surfaces ────────────────────────────────────────────────────────────
/// Background for custom-drawn widgets (VU meters, gate bar, inputs).
pub const SURFACE: enums::Color = enums::Color::from_rgb(18, 18, 22);
/// Hairline border around custom-drawn widgets.
pub const BORDER: enums::Color = enums::Color::from_rgb(16, 16, 20);
/// Alias for BG_TERTIARY — used in vu/gate draw closures for inactive bar.
pub const SUBTLE: enums::Color = enums::Color::from_rgb(44, 44, 52);

/// Apply the full theme.  Call once, right after `app::App::default()`.
pub fn apply() {
    app::set_visible_focus(false);

    // Base color palette  — Fleet Dark2 gives us a clean dark IDE look.
    ColorTheme::new(&color_themes::fleet::DARK2).apply();

    // Widget drawing scheme — Fleet1 gives subtle 3D frames that work
    // well in both dark and light palettes.
    WidgetScheme::new(SchemeType::Fleet1).apply();
}