// ui/sidebar.rs
use fltk::{enums::Font, prelude::*, *};
use renet::ClientId;
use std::collections::HashMap;

use crate::app::UserState;
use crate::ui::messages::UiMessage;
use ui::{helpers, theme};

pub struct Sidebar {
    sender: app::Sender<UiMessage>,

    // UI elements
    group: group::Flex,
    user_count_label: frame::Frame,
    user_list_scroll: group::Scroll,
    user_list_pack: group::Pack,
    connection_status: frame::Frame,
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

        // User list scroll area — scrollbar hidden by default, shown when needed
        let mut user_list_scroll = group::Scroll::default();
        user_list_scroll.set_type(group::ScrollType::None);
        user_list_scroll.set_color(theme::BG_SECONDARY);
        user_list_scroll.set_frame(enums::FrameType::FlatBox);

        // Pack inside scroll - don't set fixed size, let it grow
        let mut user_list_pack = group::Pack::default();
        user_list_pack.set_type(group::PackType::Vertical);
        user_list_pack.set_spacing(5);
        user_list_pack.end();

        user_list_scroll.end();
        user_list_scroll.resizable(&user_list_pack);

        // Spacer
        sidebar.add(&frame::Frame::default());

        // Connection status
        let mut connection_status = frame::Frame::default().with_label("\u{25cf} Connected");
        sidebar.fixed(&connection_status, 25);
        connection_status.set_label_color(theme::SUCCESS);
        connection_status.set_label_size(12);
        connection_status.set_align(enums::Align::Left | enums::Align::Inside);

        // Mute / Deafen row
        let mut audio_row = group::Flex::default().row();
        audio_row.set_spacing(5);
        sidebar.fixed(&audio_row, 35);

        let mut mute_btn = helpers::button("Mute");
        let mut deafen_btn = helpers::button("Deafen");

        audio_row.end();

        // Settings button
        let mut settings_btn = helpers::button("Settings");
        sidebar.fixed(&settings_btn, 35);

        // Disconnect button
        let mut disconnect_btn = helpers::accent_button("Disconnect", theme::DANGER);
        sidebar.fixed(&disconnect_btn, 35);

        sidebar.end();

        // Set up callbacks with thin message sending
        let s = sender;
        mute_btn.set_callback(move |_| {
            s.send(UiMessage::ToggleMute);
        });

        let s = sender;
        deafen_btn.set_callback(move |_| {
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
            mute_btn,
            deafen_btn,
        }
    }

    pub fn create_ui(&mut self) -> group::Flex {
        self.group.clone()
    }

    pub fn update_users(&mut self, users: &HashMap<ClientId, UserState>, our_id: Option<ClientId>) {
        // Clear existing users
        self.user_list_pack.clear();

        // Update count
        self.user_count_label
            .set_label(&format!("Users: {}", users.len()));

        // Sort users: put ourselves first, then alphabetically
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

        // Calculate total height needed
        let item_height = 35; // 30 item + 5 spacing
        let total_height = sorted_users.len() as i32 * item_height;

        // Show scrollbar only when content overflows the scroll area
        let scroll_h = self.user_list_scroll.h();
        if total_height > scroll_h {
            self.user_list_scroll.set_type(group::ScrollType::Vertical);
        } else {
            self.user_list_scroll.set_type(group::ScrollType::None);
        }

        // Available width — always subtract scrollbar (16px) to prevent
        // layout jumps when the scrollbar appears/disappears.
        let scroll_w = (self.user_list_scroll.w() - 16 - 10).max(100);

        // Resize pack to fit content BEFORE adding items.
        // On macOS Cocoa, the pack must be correctly sized before
        // children are added via .add(), otherwise child labels
        // get zero-size bounding boxes and don't render.
        self.user_list_pack.resize(
            self.user_list_scroll.x() + 5,
            self.user_list_scroll.y() + 5,
            scroll_w,
            total_height.max(10),
        );

        // Add user items — each calls user_list_pack.add() internally.
        // Do NOT wrap in begin()/end() — that causes double-parenting
        // on macOS where Flex::default() auto-parents into the active
        // group AND .add() tries to parent again, corrupting the
        // widget tree and making labels invisible.
        for (client_id, state) in &sorted_users {
            let is_us = Some(**client_id) == our_id;
            self.create_user_item(**client_id, state, is_us, scroll_w);
        }

        // Force redraw
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

        // Visual indicators for muted/volume adjusted users
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
            indicator.set_label_color(theme::SPEAKING);
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

        // Add context menu for non-self users
        if !is_us {
            let sender = self.sender;
            let username = state.username.clone();
            let client_id_copy = client_id;

            user_item.handle(move |_, ev| {
                if ev == enums::Event::Push && app::event_button() == 3 {
                    // Right click - show context menu
                    let menu = menu::MenuItem::new(&[
                        "Mute User",
                        "Unmute User",
                        "Set Volume",
                        "Reset Audio",
                    ]);

                    if let Some(choice) = menu.popup(app::event_x(), app::event_y()) {
                        match choice.label().unwrap_or_default().as_str() {
                            "Mute User" => {
                                sender.send(UiMessage::MuteUser {
                                    client_id: client_id_copy,
                                    username: username.clone(),
                                    muted: true,
                                });
                            }
                            "Unmute User" => {
                                sender.send(UiMessage::MuteUser {
                                    client_id: client_id_copy,
                                    username: username.clone(),
                                    muted: false,
                                });
                            }
                            "Set Volume" => {
                                show_volume_dialog(sender, client_id_copy, &username);
                            }
                            "Reset Audio" => {
                                sender.send(UiMessage::ResetUserAudio {
                                    client_id: client_id_copy,
                                    username: username.clone(),
                                });
                            }
                            _ => {}
                        }
                    }
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

    pub fn update_latency(&mut self, rtt_ms: f64) {
        // RTT is round-trip; show half for one-way feel, but label as RTT
        let label = if rtt_ms < 1.0 {
            "\u{25cf} Connected - <1ms".to_string()
        } else {
            format!("\u{25cf} Connected - {:.0}ms", rtt_ms)
        };
        self.connection_status.set_label(&label);
        // Color-code: green <80ms, yellow <150ms, red >150ms
        if rtt_ms < 80.0 {
            self.connection_status.set_label_color(theme::SUCCESS);
        } else if rtt_ms < 150.0 {
            self.connection_status.set_label_color(theme::WARNING);
        } else {
            self.connection_status.set_label_color(theme::DANGER);
        }
    }

    pub fn handle_disconnected(&mut self) {
        self.set_connection_status(false);
        self.user_list_pack.clear();
        self.user_count_label.set_label("Users: 0");
    }

    pub fn set_mute_state(&mut self, muted: bool) {
        if muted {
            self.mute_btn.set_label("Unmute");
            self.mute_btn.set_color(theme::DANGER);
        } else {
            self.mute_btn.set_label("Mute");
            self.mute_btn.set_color(theme::BG_TERTIARY);
        }
    }

    pub fn set_deafen_state(&mut self, deafened: bool) {
        if deafened {
            self.deafen_btn.set_label("Undeafen");
            self.deafen_btn.set_color(theme::WARNING);
        } else {
            self.deafen_btn.set_label("Deafen");
            self.deafen_btn.set_color(theme::BG_TERTIARY);
        }
    }
}

fn show_volume_dialog(sender: app::Sender<UiMessage>, client_id: ClientId, username: &str) {
    let mut dialog = window::Window::default()
        .with_size(300, 200)
        .with_label(&format!("Volume - {}", username));
    dialog.set_color(theme::BG_PRIMARY);
    dialog.make_modal(true);

    let mut col = group::Flex::default_fill().column();
    col.set_margins(20, 20, 20, 20);
    col.set_spacing(10);

    // Title
    let mut title = frame::Frame::default().with_label(&format!("Adjust volume for {}", username));
    title.set_label_color(theme::TEXT_PRIMARY);
    col.fixed(&title, 30);

    // Volume label
    let volume_label = std::rc::Rc::new(std::cell::RefCell::new(
        frame::Frame::default().with_label("Volume: 100%"),
    ));
    volume_label
        .borrow_mut()
        .set_label_color(theme::TEXT_SECONDARY);
    col.fixed(&*volume_label.borrow(), 25);

    // Volume slider
    let mut slider = valuator::HorSlider::default();
    slider.set_minimum(0.0);
    slider.set_maximum(2.0);
    slider.set_value(1.0);
    slider.set_step(0.05, 1);
    slider.set_color(theme::BG_TERTIARY);
    slider.set_selection_color(theme::ACCENT);
    col.fixed(&slider, 35);

    // Buttons
    let mut button_row = group::Flex::default().row();
    col.fixed(&button_row, 35);
    button_row.set_spacing(10);

    let mut reset_btn = helpers::button("Reset");
    let mut ok_btn = helpers::accent_button("OK", theme::ACCENT);

    button_row.end();
    col.end();
    dialog.end();

    // Slider callback
    let volume_label_clone = volume_label.clone();
    slider.set_callback(move |s| {
        let percent = (s.value() * 100.0) as i32;
        volume_label_clone
            .borrow_mut()
            .set_label(&format!("Volume: {}%", percent));
    });

    // Reset button
    let mut slider_clone = slider.clone();
    reset_btn.set_callback(move |_| {
        slider_clone.set_value(1.0);
        slider_clone.do_callback();
    });

    // OK button
    let username_clone = username.to_string();
    let mut dialog_clone = dialog.clone();
    ok_btn.set_callback(move |_| {
        let volume = slider.value() as f32;
        sender.send(UiMessage::SetUserVolume {
            client_id,
            username: username_clone.clone(),
            volume,
        });
        dialog_clone.hide();
    });

    dialog.show();

    // Run modal
    while dialog.shown() {
        app::wait();
    }
}
