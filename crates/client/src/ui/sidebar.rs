// ui/sidebar.rs
use fltk::{enums::Font, prelude::*, *};
use renet::ClientId;
use std::collections::HashMap;

use crate::app::UserState;
use crate::ui::messages::UiMessage;
use crate::ui::theme::DarkTheme;

pub struct Sidebar {
    sender: app::Sender<UiMessage>,

    // UI elements
    group: group::Flex,
    user_count_label: frame::Frame,
    user_list_pack: group::Pack,
    connection_status: frame::Frame,
    mute_btn: button::Button,
}

impl Sidebar {
    pub fn new(sender: app::Sender<UiMessage>) -> Self {
        let mut sidebar = group::Flex::default().column();
        sidebar.set_color(DarkTheme::BG_SECONDARY);
        sidebar.set_frame(enums::FrameType::FlatBox);
        sidebar.set_margins(10, 10, 10, 10);
        sidebar.set_spacing(10);

        // User count label
        let mut user_count_label = frame::Frame::default()
            .with_label("Users: 0")
            .with_align(enums::Align::Left | enums::Align::Inside);
        sidebar.fixed(&user_count_label, 25);
        user_count_label.set_label_color(DarkTheme::TEXT_SECONDARY);
        user_count_label.set_label_size(13);

        // User list scroll area
        let mut user_list_scroll = group::Scroll::default();
        user_list_scroll.set_type(group::ScrollType::Vertical);
        user_list_scroll.set_color(DarkTheme::BG_SECONDARY);

        let mut user_list_pack = group::Pack::default().with_size(230, 0).with_pos(0, 0);
        user_list_pack.set_type(group::PackType::Vertical);
        user_list_pack.set_spacing(5);
        user_list_pack.end();

        user_list_scroll.end();

        // Spacer
        sidebar.add(&frame::Frame::default());

        // Connection status
        let mut connection_status = frame::Frame::default().with_label("● Connected");
        sidebar.fixed(&connection_status, 25);
        connection_status.set_label_color(DarkTheme::SUCCESS);
        connection_status.set_label_size(12);
        connection_status.set_align(enums::Align::Left | enums::Align::Inside);

        // Mute button
        let mut mute_btn = button::Button::default().with_label("Mute");
        sidebar.fixed(&mute_btn, 35);
        mute_btn.set_color(DarkTheme::BG_TERTIARY);
        mute_btn.set_label_color(DarkTheme::TEXT_PRIMARY);
        mute_btn.set_frame(enums::FrameType::FlatBox);

        // Settings button
        let mut settings_btn = button::Button::default().with_label("⚙ Settings");
        sidebar.fixed(&settings_btn, 35);
        settings_btn.set_color(DarkTheme::BG_TERTIARY);
        settings_btn.set_label_color(DarkTheme::TEXT_PRIMARY);
        settings_btn.set_frame(enums::FrameType::FlatBox);

        // Disconnect button
        let mut disconnect_btn = button::Button::default().with_label("Disconnect");
        sidebar.fixed(&disconnect_btn, 35);
        disconnect_btn.set_color(DarkTheme::DANGER);
        disconnect_btn.set_label_color(enums::Color::White);
        disconnect_btn.set_frame(enums::FrameType::FlatBox);

        sidebar.end();

        // Set up callbacks with thin message sending
        let s = sender;
        mute_btn.set_callback(move |_| {
            s.send(UiMessage::ToggleMute);
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
            user_list_pack,
            connection_status,
            mute_btn,
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

        // Add user items
        for (client_id, state) in users {
            let is_us = Some(*client_id) == our_id;
            self.create_user_item(*client_id, state, is_us);
        }

        self.user_list_pack.redraw();
    }

    fn create_user_item(&mut self, client_id: ClientId, state: &UserState, is_us: bool) {
        let mut user_item = group::Flex::default().with_size(220, 30);
        user_item.set_type(group::FlexType::Row);
        user_item.set_margins(5, 5, 5, 5);

        // Visual indicators for muted/volume adjusted users
        if state.audio_settings.is_muted {
            user_item.set_color(enums::Color::from_rgb(50, 30, 30));
        } else if (state.audio_settings.volume - 1.0).abs() > 0.01 {
            user_item.set_color(enums::Color::from_rgb(40, 40, 50));
        }

        // Speaking indicator
        let mut indicator = frame::Frame::default();
        user_item.fixed(&indicator, 20);

        if state.audio_settings.is_muted {
            indicator.set_label("⊗");
        } else if state.is_speaking {
            indicator.set_label("●");
        } else {
            indicator.set_label("◯");
        }

        indicator.set_label_size(16);
        indicator.set_label_font(Font::Helvetica);
        if state.is_speaking && !state.audio_settings.is_muted {
            indicator.set_label_color(DarkTheme::SPEAKING);
        } else {
            indicator.set_label_color(DarkTheme::TEXT_MUTED);
        }

        // Username label
        let display_name = if (state.audio_settings.volume - 1.0).abs() > 0.01 {
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
            name_label.set_label_color(DarkTheme::ACCENT);
        } else if state.audio_settings.is_muted {
            name_label.set_label_color(DarkTheme::DANGER);
        } else {
            name_label.set_label_color(DarkTheme::TEXT_PRIMARY);
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
                    // Show both mute & unmute so we don't depend on a captured is_muted
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
            self.connection_status.set_label("● Connected");
            self.connection_status.set_label_color(DarkTheme::SUCCESS);
        } else {
            self.connection_status.set_label("● Disconnected");
            self.connection_status.set_label_color(DarkTheme::DANGER);
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
            self.mute_btn.set_color(DarkTheme::DANGER);
        } else {
            self.mute_btn.set_label("Mute");
            self.mute_btn.set_color(DarkTheme::BG_TERTIARY);
        }
    }
}

fn show_volume_dialog(sender: app::Sender<UiMessage>, client_id: ClientId, username: &str) {
    let mut dialog = window::Window::default()
        .with_size(300, 200)
        .with_label(&format!("Volume - {}", username));
    dialog.set_color(DarkTheme::BG_PRIMARY);
    dialog.make_modal(true);

    let mut col = group::Flex::default_fill().column();
    col.set_margins(20, 20, 20, 20);
    col.set_spacing(10);

    // Title
    let mut title = frame::Frame::default().with_label(&format!("Adjust volume for {}", username));
    title.set_label_color(DarkTheme::TEXT_PRIMARY);
    col.fixed(&title, 30);

    // Volume label
    let volume_label = std::rc::Rc::new(std::cell::RefCell::new(
        frame::Frame::default().with_label("Volume: 100%"),
    ));
    volume_label
        .borrow_mut()
        .set_label_color(DarkTheme::TEXT_SECONDARY);
    col.fixed(&*volume_label.borrow(), 25);

    // Volume slider
    let mut slider = valuator::HorSlider::default();
    slider.set_minimum(0.0);
    slider.set_maximum(2.0);
    slider.set_value(1.0);
    slider.set_step(0.05, 1);
    slider.set_color(DarkTheme::BG_TERTIARY);
    slider.set_selection_color(DarkTheme::ACCENT);
    col.fixed(&slider, 35);

    // Buttons
    let mut button_row = group::Flex::default().row();
    col.fixed(&button_row, 35);
    button_row.set_spacing(10);

    let mut reset_btn = button::Button::default().with_label("Reset");
    reset_btn.set_color(DarkTheme::BG_TERTIARY);
    reset_btn.set_label_color(DarkTheme::TEXT_PRIMARY);

    let mut ok_btn = button::Button::default().with_label("OK");
    ok_btn.set_color(DarkTheme::ACCENT);
    ok_btn.set_label_color(enums::Color::White);

    button_row.end();
    col.end();
    dialog.end();

    // Slider callback
    let volume_label_clone = volume_label.clone();
    slider.set_callback(move |s| {
        let percent = s.value() * 100.0;
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