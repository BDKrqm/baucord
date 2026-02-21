// ui/settings/about.rs — About panel: version, paths, credits, tech links.

use fltk::{prelude::*, *};
use ui::theme;
use ui::widgets::labels;

pub fn build(x: i32, y: i32, w: i32) -> group::Flex {
    let mut col = group::Flex::default()
        .with_pos(x, y)
        .with_size(w, 800)
        .column();
    col.set_spacing(4);

    labels::section_header(&mut col, "\u{2139}", "ABOUT");

    // Version
    let version = env!("CARGO_PKG_VERSION");
    let mut ver_label = frame::Frame::default()
        .with_label(&format!("baucord v{}", version))
        .with_align(enums::Align::Left | enums::Align::Inside);
    col.fixed(&ver_label, 24);
    ver_label.set_label_color(theme::ACCENT);
    ver_label.set_label_size(16);

    // Website
    link_button(&mut col, "https://baucord.mpw.sh", "https://baucord.mpw.sh");

    spacer(&mut col, 4);

    // ── Paths ───────────────────────────────────────────────────

    let exe_path = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "Unknown".to_string());

    let config_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("settings.json").display().to_string()))
        .unwrap_or_else(|| "Unknown".to_string());

    path_label(&mut col, "Running from:", &exe_path);
    path_label(&mut col, "Config file:", &config_path);

    // Single "Open folder" button
    let mut open_btn = button::Button::default().with_label("Open install folder");
    col.fixed(&open_btn, 26);
    open_btn.set_frame(enums::FrameType::FlatBox);
    open_btn.set_color(theme::BG_TERTIARY);
    open_btn.set_label_color(theme::ACCENT);
    open_btn.set_label_size(11);

    let exe_for_btn = exe_path.clone();
    open_btn.set_callback(move |_| {
        if let Some(parent) = std::path::Path::new(&exe_for_btn).parent() {
            let _ = open::that(parent);
        }
    });

    separator(&mut col);

    // ── Beta testers (2 columns) ────────────────────────────────

    labels::section_header(&mut col, "\u{1f465}", "BETA TESTERS");

    let mut testers = vec![
        "Bartrix",
        "BDK",
        "ChichoV3",
        "Cholo",
        "Empe",
        "HóRs3d1ç´k",
        "Nachovsky",
        "Naqel",
        "Nicky21",
        "Oveer",
        "Patitutitu",
        "Pin0",
        "Scryer",
    ];
    testers.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));

    let mut i = 0;
    while i < testers.len() {
        let mut row = group::Flex::default().row();
        col.fixed(&row, 20);
        row.set_spacing(10);

        let mut left = frame::Frame::default()
            .with_label(testers[i])
            .with_align(enums::Align::Left | enums::Align::Inside);
        left.set_label_color(theme::TEXT_PRIMARY);
        left.set_label_size(12);

        if i + 1 < testers.len() {
            let mut right = frame::Frame::default()
                .with_label(testers[i + 1])
                .with_align(enums::Align::Left | enums::Align::Inside);
            right.set_label_color(theme::TEXT_PRIMARY);
            right.set_label_size(12);
        } else {
            frame::Frame::default();
        }

        row.end();
        i += 2;
    }

    separator(&mut col);

    // ── Built with ──────────────────────────────────────────────

    labels::section_header(&mut col, "\u{1f6e0}", "BUILT WITH");

    let techs: &[(&str, &str)] = &[
        ("Rust — Programming Language",  "https://rust-lang.org"),
        ("FLTK — User Interface",        "https://www.fltk.org/"),
        ("Renet — Networking",           "https://github.com/lucaspoffo/renet"),
        ("Opus — Audio Codec",           "https://opus-codec.org/"),
        ("RNNoise — Noise Suppression",  "https://jmvalin.ca/demo/rnnoise/"),
        ("Rubato — Audio Resampling",    "https://github.com/HEnquist/rubato"),
    ];

    for &(name, url) in techs {
        // Name as plain label
        let mut name_lbl = frame::Frame::default()
            .with_label(name)
            .with_align(enums::Align::Left | enums::Align::Inside);
        col.fixed(&name_lbl, 18);
        name_lbl.set_label_color(theme::TEXT_PRIMARY);
        name_lbl.set_label_size(12);

        // URL as clickable link
        link_button(&mut col, url, url);
    }

    // Bottom spacer
    frame::Frame::default();

    col.end();
    col
}

/// A path display row: label + path text (not clickable).
fn path_label(col: &mut group::Flex, label: &str, path: &str) {
    let mut lbl = frame::Frame::default()
        .with_label(label)
        .with_align(enums::Align::Left | enums::Align::Inside);
    col.fixed(&lbl, 16);
    lbl.set_label_color(theme::TEXT_SECONDARY);
    lbl.set_label_size(11);

    let mut path_frame = frame::Frame::default()
        .with_label(path)
        .with_align(enums::Align::Left | enums::Align::Inside);
    col.fixed(&path_frame, 18);
    path_frame.set_label_color(theme::TEXT_PRIMARY);
    path_frame.set_label_size(11);
}

/// A clickable URL button styled as a link.
fn link_button(col: &mut group::Flex, label: &str, url: &str) {
    let mut btn = button::Button::default()
        .with_label(label);
    col.fixed(&btn, 18);
    btn.set_frame(enums::FrameType::NoBox);
    btn.set_label_color(theme::ACCENT);
    btn.set_label_size(11);
    btn.set_align(enums::Align::Left | enums::Align::Inside);
    let url = url.to_string();
    btn.set_callback(move |_| {
        let _ = open::that(&url);
    });
}

fn separator(col: &mut group::Flex) {
    let mut sep = frame::Frame::default();
    col.fixed(&sep, 1);
    sep.set_frame(enums::FrameType::FlatBox);
    sep.set_color(theme::BG_TERTIARY);
    spacer(col, 4);
}

fn spacer(col: &mut group::Flex, h: i32) {
    let s = frame::Frame::default();
    col.fixed(&s, h);
}