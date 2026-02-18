// ui/theme.rs
//
// Thin compatibility shim.  The real theme lives in the `ui` crate.
// New code should `use ui::theme` directly.  This exists so the rest of
// the client can migrate file-by-file without a big-bang rename.

/// Legacy alias — prefer `ui::theme::ACCENT` etc. in new code.
pub struct DarkTheme;

impl DarkTheme {
    pub const BG_PRIMARY: fltk::enums::Color = ui::theme::BG_PRIMARY;
    pub const BG_SECONDARY: fltk::enums::Color = ui::theme::BG_SECONDARY;
    pub const BG_TERTIARY: fltk::enums::Color = ui::theme::BG_TERTIARY;
    pub const BG_INPUT: fltk::enums::Color = ui::theme::SURFACE;

    pub const TEXT_PRIMARY: fltk::enums::Color = ui::theme::TEXT_PRIMARY;
    pub const TEXT_SECONDARY: fltk::enums::Color = ui::theme::TEXT_SECONDARY;
    pub const TEXT_MUTED: fltk::enums::Color = ui::theme::TEXT_MUTED;

    pub const ACCENT: fltk::enums::Color = ui::theme::ACCENT;
    pub const SUCCESS: fltk::enums::Color = ui::theme::SUCCESS;
    pub const WARNING: fltk::enums::Color = ui::theme::WARNING;
    pub const DANGER: fltk::enums::Color = ui::theme::DANGER;
    pub const SPEAKING: fltk::enums::Color = ui::theme::SPEAKING;
    pub const BORDER: fltk::enums::Color = ui::theme::BORDER;

    /// Replaces the old 20-line `apply_to_app()`.
    pub fn apply_to_app() {
        ui::theme::apply();
    }

    pub fn style_window(wind: &mut fltk::window::Window) {
        use fltk::prelude::WidgetExt;
        wind.set_color(ui::theme::BG_PRIMARY);
    }

    pub fn style_button(btn: &mut fltk::button::Button) {
        use fltk::prelude::WidgetExt;
        btn.set_color(ui::theme::BG_TERTIARY);
        btn.set_label_color(ui::theme::TEXT_PRIMARY);
        btn.set_selection_color(ui::theme::ACCENT);
        btn.set_frame(fltk::enums::FrameType::FlatBox);
    }

    pub fn style_input(input: &mut fltk::input::Input) {
        ui::helpers::style_input(input);
    }

    pub fn style_text_display(display: &mut fltk::text::TextDisplay) {
        ui::helpers::style_text_display(display);
    }
}