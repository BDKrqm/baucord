use fltk::{prelude::*, *};

// Dark theme color scheme
pub struct DarkTheme;

impl DarkTheme {
    pub const BG_PRIMARY: enums::Color = enums::Color::from_rgb(30, 30, 30);
    pub const BG_SECONDARY: enums::Color = enums::Color::from_rgb(40, 40, 40);
    pub const BG_TERTIARY: enums::Color = enums::Color::from_rgb(50, 50, 50);
    pub const BG_INPUT: enums::Color = enums::Color::from_rgb(25, 25, 25);

    pub const TEXT_PRIMARY: enums::Color = enums::Color::from_rgb(220, 220, 220);
    pub const TEXT_SECONDARY: enums::Color = enums::Color::from_rgb(180, 180, 180);
    pub const TEXT_MUTED: enums::Color = enums::Color::from_rgb(120, 120, 120);

    pub const ACCENT: enums::Color = enums::Color::from_rgb(88, 101, 242); // Discord-like blue
    pub const SUCCESS: enums::Color = enums::Color::from_rgb(87, 242, 135);
    pub const WARNING: enums::Color = enums::Color::from_rgb(254, 231, 92);
    pub const DANGER: enums::Color = enums::Color::from_rgb(237, 66, 69);

    pub const SPEAKING: enums::Color = enums::Color::from_rgb(67, 181, 129); // Green for speaking
    pub const BORDER: enums::Color = enums::Color::from_rgb(20, 20, 20);

    pub fn apply_to_app() {
        // Extract RGB components from the Color's internal u32 representation
        fn extract_rgb(color: enums::Color) -> (u8, u8, u8) {
            let bits = color.bits(); // or color.to_u32() depending on the API
            let r = ((bits >> 24) & 0xFF) as u8;
            let g = ((bits >> 16) & 0xFF) as u8;
            let b = ((bits >> 8) & 0xFF) as u8;
            (r, g, b)
        }

        let (r, g, b) = extract_rgb(Self::BG_PRIMARY);
        app::set_background_color(r, g, b);

        let (r, g, b) = extract_rgb(Self::BG_SECONDARY);
        app::set_background2_color(r, g, b);

        let (r, g, b) = extract_rgb(Self::TEXT_PRIMARY);
        app::set_foreground_color(r, g, b);

        let (r, g, b) = extract_rgb(Self::ACCENT);
        app::set_selection_color(r, g, b);

        let (r, g, b) = extract_rgb(Self::TEXT_MUTED);
        app::set_inactive_color(r, g, b);
    }

    pub fn style_window(wind: &mut window::Window) {
        wind.set_color(Self::BG_PRIMARY);
    }

    pub fn style_button(btn: &mut button::Button) {
        btn.set_color(Self::BG_TERTIARY);
        btn.set_label_color(Self::TEXT_PRIMARY);
        btn.set_selection_color(Self::ACCENT);
        btn.set_frame(enums::FrameType::FlatBox);
    }

    pub fn style_input(input: &mut input::Input) {
        input.set_color(Self::BG_INPUT);
        input.set_text_color(Self::TEXT_PRIMARY);
        input.set_cursor_color(Self::ACCENT);
        input.set_selection_color(Self::ACCENT);
        input.set_frame(enums::FrameType::BorderBox);
    }

    pub fn style_text_display(display: &mut text::TextDisplay) {
        display.set_color(Self::BG_INPUT);
        display.set_text_color(Self::TEXT_PRIMARY);
        display.set_cursor_color(Self::ACCENT);
        display.set_selection_color(Self::ACCENT);
        display.set_frame(enums::FrameType::BorderBox);
    }
}
