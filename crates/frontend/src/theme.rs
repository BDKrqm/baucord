//! Color themes: the UI's palette, chosen in Settings → General.
//!
//! The `Theme` global in `theme.slint` holds the colors every widget
//! reads; this file decides what they are. A theme is three colors the
//! user can see and name — background, text, accent — and [`derive`] makes
//! the other twenty-odd from them: the panels a step lighter or darker
//! than the background, the hover and border tones, the muted text, the
//! accent's hover and text variants, a label color readable on the
//! accent, the tinted rows. The built-in themes are such triples, except
//! the first, *Dark*, which is the original look to the digit.
//!
//! Custom themes are typed as `#RRGGBB`; [`parse_hex`] takes what people
//! write (`1A1D21`, `#1a1d21`, ` #1A1D21 `). A combination nobody can read
//! (text too close to the background) is not refused but pulled apart:
//! [`Palette::from_triple`] moves the text toward black or white until it
//! has a contrast of at least 4.5 to the background, the accessibility
//! minimum for body text.

/// An opaque color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// A color with transparency, for the few the UI draws over others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba(pub u8, pub u8, pub u8, pub u8);

impl From<Rgb> for Rgba {
    fn from(c: Rgb) -> Self {
        Self(c.0, c.1, c.2, 255)
    }
}

/// Every color of the `Theme` global, by its name there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub bg_primary: Rgb,
    pub bg_secondary: Rgb,
    pub bg_tertiary: Rgb,
    pub surface: Rgb,
    pub hover: Rgb,
    pub subtle: Rgb,
    pub border: Rgb,
    pub scrim: Rgba,
    pub text_primary: Rgb,
    pub text_secondary: Rgb,
    pub text_muted: Rgb,
    pub mono_text: Rgb,
    pub on_accent: Rgb,
    pub accent: Rgb,
    pub accent_hover: Rgb,
    pub accent_text: Rgb,
    pub success: Rgb,
    pub warning: Rgb,
    pub gold: Rgb,
    pub admin: Rgb,
    pub danger: Rgb,
    pub danger_text: Rgb,
    pub meter: Rgb,
    pub thumb: Rgb,
    pub row_muted_tint: Rgb,
    pub row_volume_tint: Rgb,
}

/// A theme the picker offers, in its order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Dark,
    Midnight,
    Forest,
    Sunset,
    Light,
    HighContrast,
    Custom,
}

impl Preset {
    pub const ALL: [Self; 7] = [
        Self::Dark,
        Self::Midnight,
        Self::Forest,
        Self::Sunset,
        Self::Light,
        Self::HighContrast,
        Self::Custom,
    ];

    /// The key kept in `settings.json`.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Midnight => "midnight",
            Self::Forest => "forest",
            Self::Sunset => "sunset",
            Self::Light => "light",
            Self::HighContrast => "contrast",
            Self::Custom => "custom",
        }
    }

    /// The preset for a saved key; an unknown one (a newer version's, a
    /// typo) is the original dark.
    #[must_use]
    pub fn from_key(key: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|p| p.key() == key)
            .unwrap_or(Self::Dark)
    }

    /// Position in [`Self::ALL`] (the picker's index).
    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }

    #[must_use]
    pub fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or(Self::Dark)
    }

    /// The (background, text, accent) a built-in preset is made from;
    /// `None` for *Dark* (its palette is written out) and *Custom* (the
    /// user's).
    #[must_use]
    fn triple(self) -> Option<(Rgb, Rgb, Rgb)> {
        match self {
            Self::Dark | Self::Custom => None,
            Self::Midnight => Some((
                Rgb(0x0B, 0x10, 0x20),
                Rgb(0xD6, 0xDC, 0xEE),
                Rgb(0x5B, 0x7C, 0xFF),
            )),
            Self::Forest => Some((
                Rgb(0x13, 0x1C, 0x16),
                Rgb(0xD3, 0xE0, 0xD6),
                Rgb(0x2E, 0x9E, 0x62),
            )),
            Self::Sunset => Some((
                Rgb(0x1E, 0x15, 0x1F),
                Rgb(0xEC, 0xDD, 0xE6),
                Rgb(0xD9, 0x4F, 0x8C),
            )),
            Self::Light => Some((
                Rgb(0xF4, 0xF5, 0xF7),
                Rgb(0x1F, 0x23, 0x28),
                Rgb(0x1D, 0x6F, 0xD6),
            )),
            Self::HighContrast => Some((
                Rgb(0x00, 0x00, 0x00),
                Rgb(0xFF, 0xFF, 0xFF),
                Rgb(0xFF, 0xD2, 0x1F),
            )),
        }
    }
}

/// What a user types for a custom theme (`ui.theme_colors` in the
/// settings); its default is the original dark's three.
pub use client::config::ThemeColors as CustomColors;

/// `#RRGGBB` or `RRGGBB`, any case, spaces around.
#[must_use]
pub fn parse_hex(text: &str) -> Option<Rgb> {
    let t = text.trim();
    let t = t.strip_prefix('#').unwrap_or(t);
    if t.len() != 6 || !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(t, 16).ok()?;
    Some(Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// `#RRGGBB`.
#[must_use]
pub fn hex(c: Rgb) -> String {
    format!("#{:02X}{:02X}{:02X}", c.0, c.1, c.2)
}

// ── Color arithmetic ────────────────────────────────────────────────────

/// `a` toward `b` by `t` (0 = a, 1 = b), in sRGB, which is what the eye
/// of a UI expects of "a bit lighter".
fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let ch = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Rgb(ch(a.0, b.0), ch(a.1, b.1), ch(a.2, b.2))
}

const BLACK: Rgb = Rgb(0, 0, 0);
const WHITE: Rgb = Rgb(255, 255, 255);

/// WCAG relative luminance.
fn luminance(c: Rgb) -> f32 {
    let lin = |v: u8| {
        let s = f32::from(v) / 255.0;
        if s <= 0.039_28 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.0) + 0.7152 * lin(c.1) + 0.0722 * lin(c.2)
}

/// WCAG contrast ratio, 1 to 21.
#[must_use]
pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Black or white, whichever reads better on `c`.
fn readable_on(c: Rgb) -> Rgb {
    if contrast(WHITE, c) >= contrast(BLACK, c) {
        WHITE
    } else {
        BLACK
    }
}

/// `text` moved toward black or white (whichever is further from
/// `bg`) until it has `min` contrast to `bg`.
fn readable_text(text: Rgb, bg: Rgb, min: f32) -> Rgb {
    if contrast(text, bg) >= min {
        return text;
    }
    let target = if luminance(bg) < 0.4 { WHITE } else { BLACK };
    let mut t = 0.0;
    while t < 1.0 {
        t += 0.05;
        let c = mix(text, target, t);
        if contrast(c, bg) >= min {
            return c;
        }
    }
    target
}

impl Palette {
    /// The original look, to the digit (`theme.slint` before themes).
    #[must_use]
    pub fn dark() -> Self {
        Self {
            bg_primary: Rgb(0x1A, 0x1D, 0x21),
            bg_secondary: Rgb(0x22, 0x25, 0x29),
            bg_tertiary: Rgb(0x2B, 0x2E, 0x33),
            surface: Rgb(0x15, 0x17, 0x1A),
            hover: Rgb(0x27, 0x2A, 0x2F),
            subtle: Rgb(0x30, 0x33, 0x38),
            border: Rgb(0x3A, 0x3D, 0x42),
            scrim: Rgba(0, 0, 0, 0xAA),
            text_primary: Rgb(0xD1, 0xD2, 0xD3),
            text_secondary: Rgb(0xAB, 0xAD, 0xB0),
            text_muted: Rgb(0x92, 0x95, 0x98),
            mono_text: Rgb(0x86, 0xAA, 0x90),
            on_accent: Rgb(0xFF, 0xFF, 0xFF),
            accent: Rgb(0x12, 0x64, 0xA3),
            accent_hover: Rgb(0x0E, 0x50, 0x82),
            accent_text: Rgb(0x4A, 0x9A, 0xE0),
            success: Rgb(0x2B, 0xAC, 0x76),
            warning: Rgb(0xEC, 0xB2, 0x2E),
            gold: Rgb(0xEC, 0xB2, 0x2E),
            admin: Rgb(0x2B, 0xAC, 0x76),
            danger: Rgb(0xE0, 0x1E, 0x5A),
            danger_text: Rgb(0xFF, 0x6B, 0x8E),
            meter: Rgb(0xAB, 0xAD, 0xB0),
            thumb: Rgb(0xC8, 0xC9, 0xCA),
            row_muted_tint: Rgb(0x2E, 0x1C, 0x23),
            row_volume_tint: Rgb(0x1C, 0x23, 0x2D),
        }
    }

    /// The whole palette from three colors.
    #[must_use]
    pub fn from_triple(background: Rgb, text: Rgb, accent: Rgb) -> Self {
        let dark = luminance(background) < 0.4;
        // The panels step away from the background: lighter on a dark
        // one, darker on a light one; inputs go the other way.
        let toward = if dark { WHITE } else { BLACK };
        let step = |t: f32| mix(background, toward, t);
        let surface = if dark {
            mix(background, BLACK, 0.2)
        } else {
            mix(background, WHITE, 0.65)
        };
        let text_primary = readable_text(text, background, 4.5);
        let text_secondary = readable_text(mix(text_primary, background, 0.30), background, 4.5);
        let text_muted = readable_text(mix(text_primary, background, 0.48), background, 3.0);
        let success = if dark {
            Rgb(0x2B, 0xAC, 0x76)
        } else {
            Rgb(0x14, 0x7A, 0x50)
        };
        let warning = if dark {
            Rgb(0xEC, 0xB2, 0x2E)
        } else {
            Rgb(0xA8, 0x6B, 0x00)
        };
        let danger = if dark {
            Rgb(0xE0, 0x1E, 0x5A)
        } else {
            Rgb(0xC4, 0x12, 0x4A)
        };
        let danger_text = if dark {
            Rgb(0xFF, 0x6B, 0x8E)
        } else {
            Rgb(0xB0, 0x10, 0x40)
        };
        let accent_text = readable_text(
            if dark {
                mix(accent, WHITE, 0.35)
            } else {
                mix(accent, BLACK, 0.15)
            },
            background,
            3.0,
        );
        Self {
            bg_primary: background,
            bg_secondary: step(0.04),
            bg_tertiary: step(0.09),
            surface,
            hover: step(0.065),
            subtle: step(0.12),
            border: step(0.17),
            scrim: if dark {
                Rgba(0, 0, 0, 0xAA)
            } else {
                Rgba(0, 0, 0, 0x73)
            },
            text_primary,
            text_secondary,
            text_muted,
            mono_text: readable_text(mix(success, text_primary, 0.35), background, 3.0),
            on_accent: readable_on(accent),
            accent,
            accent_hover: mix(accent, BLACK, 0.2),
            accent_text,
            success,
            warning,
            gold: warning,
            admin: success,
            danger,
            danger_text,
            meter: text_secondary,
            thumb: mix(text_primary, text_secondary, 0.25),
            row_muted_tint: mix(background, danger, 0.10),
            row_volume_tint: mix(background, accent, 0.10),
        }
    }

    /// The palette of `preset`; `custom` is what *Custom* is made from
    /// (a color that does not parse keeps the original dark's).
    #[must_use]
    pub fn of(preset: Preset, custom: &CustomColors) -> Self {
        match preset {
            Preset::Dark => Self::dark(),
            Preset::Custom => {
                let base = CustomColors::default();
                let pick = |typed: &str, fallback: &str| {
                    parse_hex(typed)
                        .or_else(|| parse_hex(fallback))
                        .unwrap_or(BLACK)
                };
                Self::from_triple(
                    pick(&custom.background, &base.background),
                    pick(&custom.text, &base.text),
                    pick(&custom.accent, &base.accent),
                )
            }
            other => {
                let (bg, text, accent) = other.triple().unwrap_or((BLACK, WHITE, WHITE));
                Self::from_triple(bg, text, accent)
            }
        }
    }

    /// The palette the settings ask for.
    #[must_use]
    pub fn for_settings(settings: &client::config::Settings) -> Self {
        Self::of(
            Preset::from_key(&settings.ui.theme),
            &settings.ui.theme_colors,
        )
    }

    /// Set every colour of a window's `Theme` global (globals are per
    /// window instance, like its `scale`).
    pub fn apply(&self, theme: &ui::Theme<'_>) {
        let color = |rgb: Rgb| slint::Color::from_rgb_u8(rgb.0, rgb.1, rgb.2);
        let brush = |rgb: Rgb| slint::Brush::from(color(rgb));
        theme.set_bg_primary(brush(self.bg_primary));
        theme.set_bg_secondary(brush(self.bg_secondary));
        theme.set_bg_tertiary(brush(self.bg_tertiary));
        theme.set_surface(brush(self.surface));
        theme.set_hover(brush(self.hover));
        theme.set_subtle(brush(self.subtle));
        theme.set_border(brush(self.border));
        let Rgba(red, green, blue, alpha) = self.scrim;
        theme.set_scrim(slint::Brush::from(slint::Color::from_argb_u8(
            alpha, red, green, blue,
        )));
        theme.set_text_primary(color(self.text_primary));
        theme.set_text_secondary(color(self.text_secondary));
        theme.set_text_muted(color(self.text_muted));
        theme.set_mono_text(color(self.mono_text));
        theme.set_on_accent(color(self.on_accent));
        theme.set_accent(color(self.accent));
        theme.set_accent_hover(color(self.accent_hover));
        theme.set_accent_text(color(self.accent_text));
        theme.set_success(color(self.success));
        theme.set_warning(color(self.warning));
        theme.set_gold(color(self.gold));
        theme.set_admin(color(self.admin));
        theme.set_danger(color(self.danger));
        theme.set_danger_text(color(self.danger_text));
        theme.set_meter(color(self.meter));
        theme.set_thumb(color(self.thumb));
        theme.set_row_muted_tint(brush(self.row_muted_tint));
        theme.set_row_volume_tint(brush(self.row_volume_tint));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_palettes() -> Vec<(Preset, Palette)> {
        Preset::ALL
            .into_iter()
            .map(|p| (p, Palette::of(p, &CustomColors::default())))
            .collect()
    }

    #[test]
    fn hex_is_read_the_way_people_write_it() {
        assert_eq!(parse_hex("#1A1D21"), Some(Rgb(0x1A, 0x1D, 0x21)));
        assert_eq!(parse_hex("1a1d21"), Some(Rgb(0x1A, 0x1D, 0x21)));
        assert_eq!(parse_hex("  #FfFfFf "), Some(Rgb(255, 255, 255)));
        for bad in ["", "#123", "#12345G", "#1234567", "rojo", "##112233"] {
            assert_eq!(parse_hex(bad), None, "{bad:?}");
        }
        assert_eq!(hex(Rgb(0x1A, 0x0D, 0xFF)), "#1A0DFF");
    }

    #[test]
    fn dark_is_the_original_look_and_the_default() {
        let p = Palette::of(Preset::Dark, &CustomColors::default());
        assert_eq!(p, Palette::dark());
        assert_eq!(p.bg_primary, Rgb(0x1A, 0x1D, 0x21));
        assert_eq!(p.accent, Rgb(0x12, 0x64, 0xA3));
        assert_eq!(Preset::from_key("nonsense"), Preset::Dark);
        assert_eq!(Preset::from_index(99), Preset::Dark);
    }

    /// `Palette::dark` and the defaults in `theme.slint` are the same
    /// colours: the original look before any theme is applied, and
    /// after Dark is picked again.
    #[test]
    fn dark_is_what_theme_slint_starts_with() {
        let slint = include_str!("../../ui/slint/theme.slint");
        let declared = |name: &str| -> String {
            let line = slint
                .lines()
                .find(|l| l.contains(&format!("> {name}:")))
                .unwrap_or_else(|| panic!("{name} is not in theme.slint"));
            // `#RRGGBB` (a scrim's alpha after it is not looked at).
            let hash = line.find('#').expect("a colour");
            line[hash..=hash + 6].to_string()
        };
        let d = Palette::dark();
        for (name, rgb) in [
            ("bg-primary", d.bg_primary),
            ("bg-secondary", d.bg_secondary),
            ("bg-tertiary", d.bg_tertiary),
            ("surface", d.surface),
            ("hover", d.hover),
            ("subtle", d.subtle),
            ("border", d.border),
            ("text-primary", d.text_primary),
            ("text-secondary", d.text_secondary),
            ("text-muted", d.text_muted),
            ("mono-text", d.mono_text),
            ("on-accent", d.on_accent),
            ("accent", d.accent),
            ("accent-hover", d.accent_hover),
            ("accent-text", d.accent_text),
            ("success", d.success),
            ("warning", d.warning),
            ("gold", d.gold),
            ("admin", d.admin),
            ("danger", d.danger),
            ("danger-text", d.danger_text),
            ("meter", d.meter),
            ("thumb", d.thumb),
            ("row-muted-tint", d.row_muted_tint),
            ("row-volume-tint", d.row_volume_tint),
        ] {
            assert_eq!(declared(name).to_uppercase(), hex(rgb), "{name}");
        }
    }

    #[test]
    fn keys_and_indices_round_trip() {
        for p in Preset::ALL {
            assert_eq!(Preset::from_key(p.key()), p);
            assert_eq!(Preset::from_index(p.index()), p);
        }
    }

    #[test]
    fn every_theme_can_be_read() {
        for (preset, p) in all_palettes() {
            let name = preset.key();
            assert!(
                contrast(p.text_primary, p.bg_primary) >= 4.5,
                "{name}: text"
            );
            assert!(
                contrast(p.text_secondary, p.bg_primary) >= 4.5,
                "{name}: secondary"
            );
            assert!(contrast(p.text_muted, p.bg_primary) >= 3.0, "{name}: muted");
            assert!(
                contrast(p.on_accent, p.accent) >= 4.5,
                "{name}: a label on the accent"
            );
            assert!(
                contrast(p.accent_text, p.bg_primary) >= 3.0,
                "{name}: accent text"
            );
            assert!(
                contrast(p.text_primary, p.bg_secondary) >= 4.5,
                "{name}: text on panels"
            );
            assert!(
                contrast(p.text_primary, p.bg_tertiary) >= 4.0,
                "{name}: text on cards"
            );
        }
    }

    #[test]
    fn panels_step_away_from_the_background_the_right_way() {
        let dark = Palette::of(Preset::Midnight, &CustomColors::default());
        assert!(luminance(dark.bg_secondary) > luminance(dark.bg_primary));
        assert!(luminance(dark.bg_tertiary) > luminance(dark.bg_secondary));
        assert!(luminance(dark.surface) < luminance(dark.bg_primary));
        let light = Palette::of(Preset::Light, &CustomColors::default());
        assert!(luminance(light.bg_secondary) < luminance(light.bg_primary));
        assert!(luminance(light.bg_tertiary) < luminance(light.bg_secondary));
        assert!(luminance(light.surface) > luminance(light.bg_primary));
    }

    #[test]
    fn unreadable_text_is_pulled_apart_not_refused() {
        // Grey text on a nearly grey background.
        let custom = CustomColors {
            background: "#303030".into(),
            text: "#383838".into(),
            accent: "#FF0000".into(),
        };
        let p = Palette::of(Preset::Custom, &custom);
        assert!(contrast(p.text_primary, p.bg_primary) >= 4.5);
        // And an accent too light for white labels gets a dark one.
        let custom = CustomColors {
            accent: "#FFFF00".into(),
            ..CustomColors::default()
        };
        let p = Palette::of(Preset::Custom, &custom);
        assert_eq!(p.on_accent, BLACK);
    }

    #[test]
    fn a_custom_color_that_does_not_parse_keeps_the_original() {
        let custom = CustomColors {
            background: "no".into(),
            text: "#FFFFFF".into(),
            accent: "#FF00FF".into(),
        };
        let p = Palette::of(Preset::Custom, &custom);
        assert_eq!(
            p.bg_primary,
            Rgb(0x1A, 0x1D, 0x21),
            "the original background"
        );
        assert_eq!(p.accent, Rgb(0xFF, 0x00, 0xFF), "the valid ones are used");
    }

    #[test]
    fn the_original_triple_gives_almost_the_original_theme() {
        // The derivation is what makes the other themes: fed the dark's
        // own three colors it should land close to the hand-picked one
        // (within what the eye can tell apart in neighbouring tones).
        let d = Palette::dark();
        let p = Palette::from_triple(d.bg_primary, d.text_primary, d.accent);
        let near = |a: Rgb, b: Rgb| {
            (i32::from(a.0) - i32::from(b.0)).abs() <= 12
                && (i32::from(a.1) - i32::from(b.1)).abs() <= 12
                && (i32::from(a.2) - i32::from(b.2)).abs() <= 12
        };
        assert!(near(p.bg_secondary, d.bg_secondary), "{:?}", p.bg_secondary);
        assert!(near(p.bg_tertiary, d.bg_tertiary), "{:?}", p.bg_tertiary);
        assert!(near(p.surface, d.surface), "{:?}", p.surface);
        assert!(near(p.hover, d.hover), "{:?}", p.hover);
        assert!(near(p.border, d.border), "{:?}", p.border);
    }
}
