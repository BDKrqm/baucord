// ui/sidebar.rs
use fltk::{enums::Font, prelude::*, *};
use renet::ClientId;
use std::collections::HashMap;

use crate::app::UserState;
use crate::ui::messages::UiMessage;
use ui::theme;
use ui::widgets::controls;

pub struct Sidebar {
    sender: app::Sender<UiMessage>,

    // UI elements
    group: group::Flex,
    user_count_label: frame::Frame,
    user_list_scroll: group::Scroll,
    user_list_pack: group::Pack,
    connection_status: frame::Frame,
    latency_label: frame::Frame,
    mute_btn: button::Button,
    deafen_btn: button::Button,
}

impl Sidebar {
    pub fn new(sender: app::Sender<UiMessage>) -> Self {
        let mut sidebar = group::Flex::default().column();
        sidebar.set_color(theme::BG_SECONDARY);
        sidebar.set_frame(enums::FrameType::FlatBox);
        sidebar.set_margins(10, 10, 10, 10);
        sidebar.set_spacing(10);

        // User count label
        let mut user_count_label = frame::Frame::default()
            .with_label("Users: 0")
            .with_align(enums::Align::Left | enums::Align::Inside);
        sidebar.fixed(&user_count_label, 25);
        user_count_label.set_label_color(theme::TEXT_SECONDARY);
        user_count_label.set_label_size(13);

        // User list scroll area
        let mut user_list_scroll = group::Scroll::default();
        user_list_scroll.set_type(group::ScrollType::None);
        user_list_scroll.set_color(theme::BG_SECONDARY);
        user_list_scroll.set_frame(enums::FrameType::FlatBox);

        let mut user_list_pack = group::Pack::default();
        user_list_pack.set_type(group::PackType::Vertical);
        user_list_pack.set_spacing(5);
        user_list_pack.end();

        user_list_scroll.end();
        user_list_scroll.resizable(&user_list_pack);

        // Spacer
        sidebar.add(&frame::Frame::default());

        // ── Connection status panel ─────────────────────────────────
        let mut status_col = group::Flex::default().column();
        status_col.set_spacing(2);
        sidebar.fixed(&status_col, 50);

        let mut connection_status = frame::Frame::default().with_label("\u{25cf} Connected");
        status_col.fixed(&connection_status, 20);
        connection_status.set_label_color(theme::SUCCESS);
        connection_status.set_label_size(12);
        connection_status.set_align(enums::Align::Left | enums::Align::Inside);

        let mut latency_label =
            frame::Frame::default().with_label("  \u{1f50a} --ms  \u{1f310} --ms");
        status_col.fixed(&latency_label, 16);
        latency_label.set_label_color(theme::TEXT_MUTED);
        latency_label.set_label_size(10);
        latency_label.set_align(enums::Align::Left | enums::Align::Inside);

        status_col.end();

        // Mute / Deafen row
        let mut audio_row = group::Flex::default().row();
        audio_row.set_spacing(5);
        sidebar.fixed(&audio_row, 35);

        let mute_btn = controls::button("Mute");
        let deafen_btn = controls::button("Deafen");

        audio_row.end();

        // Settings button
        let mut settings_btn = controls::button("Settings");
        sidebar.fixed(&settings_btn, 35);

        // Disconnect button
        let mut disconnect_btn = controls::accent_button("Disconnect", theme::DANGER);
        sidebar.fixed(&disconnect_btn, 35);

        sidebar.end();

        // Callbacks
        let s = sender;
        mute_btn.clone().set_callback(move |_| {
            s.send(UiMessage::ToggleMute);
        });

        let s = sender;
        deafen_btn.clone().set_callback(move |_| {
            s.send(UiMessage::ToggleDeafen);
        });

        let s = sender;
        settings_btn.set_callback(move |_| {
            s.send(UiMessage::OpenSettings);
        });

        let s = sender;
        disconnect_btn.set_callback(move |_| {
            s.send(UiMessage::Disconnect);
        });

        Self {
            sender,
            group: sidebar,
            user_count_label,
            user_list_scroll,
            user_list_pack,
            connection_status,
            latency_label,
            mute_btn,
            deafen_btn,
        }
    }

    pub fn create_ui(&mut self) -> group::Flex {
        self.group.clone()
    }

    pub fn update_users(&mut self, users: &HashMap<ClientId, UserState>, our_id: Option<ClientId>) {
        self.user_list_pack.clear();

        self.user_count_label
            .set_label(&format!("Users: {}", users.len()));

        // Sort: ourselves first, then alphabetically
        let mut sorted_users: Vec<_> = users.iter().collect();
        sorted_users.sort_by(|(id_a, state_a), (id_b, state_b)| {
            let is_us_a = Some(**id_a) == our_id;
            let is_us_b = Some(**id_b) == our_id;

            match (is_us_a, is_us_b) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => state_a.username.cmp(&state_b.username),
            }
        });

        let item_height = 35;
        let total_height = sorted_users.len() as i32 * item_height;

        let scroll_h = self.user_list_scroll.h();
        if total_height > scroll_h {
            self.user_list_scroll.set_type(group::ScrollType::Vertical);
        } else {
            self.user_list_scroll.set_type(group::ScrollType::None);
        }

        let scroll_w = (self.user_list_scroll.w() - 16 - 10).max(100);

        self.user_list_pack.resize(
            self.user_list_scroll.x() + 5,
            self.user_list_scroll.y() + 5,
            scroll_w,
            total_height.max(10),
        );

        for (client_id, state) in &sorted_users {
            let is_us = Some(**client_id) == our_id;
            self.create_user_item(**client_id, state, is_us, scroll_w);
        }

        self.user_list_pack.redraw();
        self.user_list_scroll.redraw();
        self.group.redraw();
    }

    fn create_user_item(
        &mut self,
        client_id: ClientId,
        state: &UserState,
        is_us: bool,
        width: i32,
    ) {
        let item_height = 30;

        let mut user_item = group::Flex::default().with_size(width, item_height);
        user_item.set_type(group::FlexType::Row);
        user_item.set_margins(5, 2, 5, 2);
        user_item.set_frame(enums::FrameType::FlatBox);

        if state.audio_settings.is_muted {
            user_item.set_color(enums::Color::from_rgb(50, 30, 30));
        } else if (state.audio_settings.volume - 1.0).abs() > 0.01 {
            user_item.set_color(enums::Color::from_rgb(40, 40, 50));
        } else {
            user_item.set_color(theme::BG_SECONDARY);
        }

        // Speaking indicator
        let mut indicator = frame::Frame::default();
        user_item.fixed(&indicator, 20);

        if state.audio_settings.is_muted {
            indicator.set_label("\u{2298}");
        } else if state.is_speaking {
            indicator.set_label("\u{25cf}");
        } else {
            indicator.set_label("\u{25cb}");
        }

        indicator.set_label_size(14);
        indicator.set_label_font(Font::Helvetica);
        if state.is_speaking && !state.audio_settings.is_muted {
            indicator.set_label_color(theme::SUCCESS);
        } else {
            indicator.set_label_color(theme::TEXT_MUTED);
        }

        // Username label
        let display_name = if is_us {
            format!("{} (you)", state.username)
        } else if (state.audio_settings.volume - 1.0).abs() > 0.01 {
            format!(
                "{} ({}%)",
                state.username,
                (state.audio_settings.volume * 100.0) as i32
            )
        } else {
            state.username.clone()
        };

        let mut name_label = frame::Frame::default().with_label(&display_name);
        name_label.set_align(enums::Align::Left | enums::Align::Inside);

        if is_us {
            name_label.set_label_color(theme::ACCENT);
        } else if state.audio_settings.is_muted {
            name_label.set_label_color(theme::DANGER);
        } else {
            name_label.set_label_color(theme::TEXT_PRIMARY);
        }
        name_label.set_label_size(13);

        // Right-click → audio control popup
        if !is_us {
            let sender = self.sender;
            let username = state.username.clone();
            let client_id_copy = client_id;
            let is_muted = state.audio_settings.is_muted;
            let current_volume = state.audio_settings.volume;

            user_item.handle(move |_, ev| {
                if ev == enums::Event::Push && app::event_button() == 3 {
                    show_user_audio_popup(
                        sender,
                        client_id_copy,
                        &username,
                        current_volume,
                        is_muted,
                    );
                    true
                } else {
                    false
                }
            });
        }

        user_item.end();
        self.user_list_pack.add(&user_item);
    }

    pub fn set_connection_status(&mut self, connected: bool) {
        if connected {
            self.connection_status.set_label("\u{25cf} Connected");
            self.connection_status.set_label_color(theme::SUCCESS);
        } else {
            self.connection_status.set_label("\u{25cf} Disconnected");
            self.connection_status.set_label_color(theme::DANGER);
        }
    }

    pub fn update_latency(&mut self, rtt_ms: f64, audio_latency_ms: Option<u32>) {
        let status_label = if rtt_ms < 1.0 {
            "Disconnected".to_string()
        } else {
            "Connected".to_string()
        };
        self.connection_status.set_label(&status_label);

        if rtt_ms < 80.0 {
            self.connection_status.set_label_color(theme::SUCCESS);
        } else if rtt_ms < 150.0 {
            self.connection_status.set_label_color(theme::WARNING);
        } else {
            self.connection_status.set_label_color(theme::DANGER);
        }

        let net_str = if rtt_ms < 1.0 {
            "<1ms".to_string()
        } else {
            format!("{rtt_ms:.0}ms")
        };
        let audio_str = match audio_latency_ms {
            Some(ms) => format!("{ms}ms"),
            None => "--".to_string(),
        };
        self.latency_label
            .set_label(&format!("Audio: {audio_str} | Network: {net_str}"));
    }

    pub fn handle_disconnected(&mut self) {
        self.set_connection_status(false);
        self.latency_label.set_label("Audio: -- | Network: --");
        self.user_list_pack.clear();
        self.user_count_label.set_label("Users: 0");
    }

    pub fn set_mute_state(&mut self, muted: bool) {
        if muted {
            self.mute_btn.set_label("Unmute");
            self.mute_btn.set_color(theme::DANGER);
            self.mute_btn.set_label_color(enums::Color::White);
        } else {
            self.mute_btn.set_label("Mute");
            self.mute_btn.set_color(theme::BG_TERTIARY);
            self.mute_btn.set_label_color(theme::TEXT_PRIMARY);
        }
    }

    pub fn set_deafen_state(&mut self, deafened: bool) {
        if deafened {
            self.deafen_btn.set_label("Undeafen");
            self.deafen_btn.set_color(theme::WARNING);
            self.deafen_btn.set_label_color(enums::Color::White);
        } else {
            self.deafen_btn.set_label("Deafen");
            self.deafen_btn.set_color(theme::BG_TERTIARY);
            self.deafen_btn.set_label_color(theme::TEXT_PRIMARY);
        }
    }
}

/// Right-click popup for per-user audio controls.
///
/// Discord-style vertical layout: volume slider, mute toggle, reset.
/// All actions close the popup. Clicking outside also closes it.
fn show_user_audio_popup(
    sender: app::Sender<UiMessage>,
    client_id: ClientId,
    username: &str,
    current_volume: f32,
    is_muted: bool,
) {
    let popup_w = 200;
    let popup_h = 155;

    let x = app::event_x_root();
    let y = app::event_y_root();

    let mut popup = window::Window::default()
        .with_size(popup_w, popup_h)
        .with_pos(x, y);
    popup.set_border(false);
    popup.set_color(theme::BG_PRIMARY);
    popup.set_frame(enums::FrameType::FlatBox);

    let mut col = group::Flex::default_fill().column();
    col.set_margins(12, 8, 12, 8);
    col.set_spacing(4);

    // ── User Volume ─────────────────────────────────────────────────

    let mut vol_header = frame::Frame::default()
        .with_label("User Volume")
        .with_align(enums::Align::Left | enums::Align::Inside);
    vol_header.set_label_color(theme::TEXT_PRIMARY);
    vol_header.set_label_size(12);
    vol_header.set_label_font(enums::Font::HelveticaBold);
    col.fixed(&vol_header, 18);

    let mut slider_row = group::Flex::default().row();
    slider_row.set_spacing(6);
    col.fixed(&slider_row, 22);

    let mut slider = valuator::HorSlider::default();
    slider.set_minimum(0.0);
    slider.set_maximum(2.0);
    slider.set_value(current_volume as f64);
    slider.set_step(0.05, 1);
    slider.set_selection_color(theme::ACCENT);

    let mut pct_label = frame::Frame::default()
        .with_label(&format!("{}%", (current_volume * 100.0) as i32));
    pct_label.set_label_color(theme::TEXT_SECONDARY);
    pct_label.set_label_size(11);
    slider_row.fixed(&pct_label, 36);

    slider_row.end();

    // ── Separator ───────────────────────────────────────────────────

    let mut sep = frame::Frame::default();
    sep.set_frame(enums::FrameType::FlatBox);
    sep.set_color(theme::BG_TERTIARY);
    col.fixed(&sep, 1);

    // ── Mute button ─────────────────────────────────────────────────

    let mute_label = if is_muted { "Unmute" } else { "Mute" };
    let mut mute_btn = button::Button::default().with_label(mute_label);
    mute_btn.set_frame(enums::FrameType::FlatBox);
    mute_btn.set_label_size(12);
    mute_btn.set_align(enums::Align::Left | enums::Align::Inside);
    if is_muted {
        mute_btn.set_color(theme::BG_PRIMARY);
        mute_btn.set_label_color(theme::DANGER);
    } else {
        mute_btn.set_color(theme::BG_PRIMARY);
        mute_btn.set_label_color(theme::TEXT_PRIMARY);
    }
    col.fixed(&mute_btn, 26);

    // ── Separator ───────────────────────────────────────────────────

    let mut sep2 = frame::Frame::default();
    sep2.set_frame(enums::FrameType::FlatBox);
    sep2.set_color(theme::BG_TERTIARY);
    col.fixed(&sep2, 1);

    // ── Reset button ────────────────────────────────────────────────

    let mut reset_btn = button::Button::default().with_label("Reset Audio");
    reset_btn.set_frame(enums::FrameType::FlatBox);
    reset_btn.set_label_size(12);
    reset_btn.set_label_color(theme::TEXT_SECONDARY);
    reset_btn.set_color(theme::BG_PRIMARY);
    reset_btn.set_align(enums::Align::Left | enums::Align::Inside);
    col.fixed(&reset_btn, 26);

    col.end();
    popup.end();
    popup.show();

    // ── Callbacks ───────────────────────────────────────────────────

    // Volume slider — sends live as you drag
    let mut pct_lbl = pct_label.clone();
    let username_vol = username.to_string();
    slider.set_callback(move |s| {
        let vol = s.value() as f32;
        pct_lbl.set_label(&format!("{}%", (vol * 100.0) as i32));
        sender.send(UiMessage::SetUserVolume {
            client_id,
            username: username_vol.clone(),
            volume: vol,
        });
    });

    // Mute / Unmute — send toggled state, close popup
    let username_mute = username.to_string();
    let mut popup_mute = popup.clone();
    mute_btn.set_callback(move |_| {
        sender.send(UiMessage::MuteUser {
            client_id,
            username: username_mute.clone(),
            muted: !is_muted,
        });
        popup_mute.hide();
    });

    // Reset — unmute + reset volume, close popup
    let username_reset = username.to_string();
    let mut popup_reset = popup.clone();
    reset_btn.set_callback(move |_| {
        sender.send(UiMessage::ResetUserAudio {
            client_id,
            username: username_reset.clone(),
        });
        popup_reset.hide();
    });

    // ── Modal loop — close on outside click ─────────────────────────

    while popup.shown() {
        app::wait();

        // Close if user clicks outside the popup bounds
        if app::event() == enums::Event::Push {
            let ex = app::event_x_root();
            let ey = app::event_y_root();
            let px = popup.x();
            let py = popup.y();
            if ex < px || ex > px + popup.w() || ey < py || ey > py + popup.h() {
                popup.hide();
            }
        }
    }

    // Always send the final slider value on close — ensures the volume
    // persists even if clicking in the sidebar triggers a user list
    // rebuild before the live slider messages are processed.
    let final_vol = slider.value() as f32;
    if (final_vol - current_volume).abs() > 0.001 {
        sender.send(UiMessage::SetUserVolume {
            client_id,
            username: username.to_string(),
            volume: final_vol,
        });
    }
}