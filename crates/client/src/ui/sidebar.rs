use fltk::menu;
use fltk::{prelude::*, *};
use log::info;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::app::SpeakingState;
use crate::network::NetworkClient;
use crate::settings::{Settings, UserAudioSettings};
use crate::ui::settings::AudioSettingsWindow;
use crate::ui::theme::DarkTheme;
use crate::voice::VoiceChat;

pub struct Sidebar {
    network_client: Arc<Mutex<Option<NetworkClient>>>,
    voice_chat: Arc<Mutex<Option<VoiceChat>>>,
    settings: Arc<Mutex<Settings>>,
    speaking_indicators: Arc<Mutex<HashMap<u64, SpeakingState>>>,
    our_client_id: Arc<Mutex<Option<u64>>>,

    // UI elements - using Option for safety
    sidebar_group: Option<group::Flex>,
    user_count_label: Option<frame::Frame>,
    user_list_pack: Option<group::Pack>,
    connection_status: Option<frame::Frame>,
    mute_btn: Option<button::Button>,
    menu_active: Arc<Mutex<bool>>,
}

impl Sidebar {
    pub fn new(
        network_client: Arc<Mutex<Option<NetworkClient>>>,
        voice_chat: Arc<Mutex<Option<VoiceChat>>>,
        settings: Arc<Mutex<Settings>>,
        speaking_indicators: Arc<Mutex<HashMap<u64, SpeakingState>>>,
        our_client_id: Arc<Mutex<Option<u64>>>,
    ) -> Self {
        Sidebar {
            network_client,
            voice_chat,
            settings,
            speaking_indicators,
            our_client_id,
            sidebar_group: None,
            user_count_label: None,
            user_list_pack: None,
            connection_status: None,
            mute_btn: None,
            menu_active: Arc::new(Mutex::new(false)),
        }
    }

    pub fn create_ui(&mut self) -> group::Flex {
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
        self.user_count_label = Some(user_count_label.clone());

        // User list scroll area
        let mut user_list_scroll = group::Scroll::default();
        user_list_scroll.set_type(group::ScrollType::Vertical);
        user_list_scroll.set_color(DarkTheme::BG_SECONDARY);

        let mut user_list_pack = group::Pack::default().with_size(230, 0).with_pos(0, 0);
        user_list_pack.set_type(group::PackType::Vertical);
        user_list_pack.set_spacing(5);
        user_list_pack.end();
        self.user_list_pack = Some(user_list_pack.clone());

        user_list_scroll.end();

        // Spacer
        sidebar.add(&frame::Frame::default());

        // Connection status
        let mut connection_status = frame::Frame::default().with_label("● Connected");
        sidebar.fixed(&connection_status, 25);
        connection_status.set_label_color(DarkTheme::SUCCESS);
        connection_status.set_label_size(12);
        connection_status.set_align(enums::Align::Left | enums::Align::Inside);
        self.connection_status = Some(connection_status.clone());

        // Mute button
        let mut mute_btn = button::Button::default().with_label("Mute");
        sidebar.fixed(&mute_btn, 35);
        mute_btn.set_color(DarkTheme::BG_TERTIARY);
        mute_btn.set_label_color(DarkTheme::TEXT_PRIMARY);
        mute_btn.set_frame(enums::FrameType::FlatBox);
        self.mute_btn = Some(mute_btn.clone());

        self.setup_mute_button();

        // Settings button
        let mut settings_btn = button::Button::default().with_label("⚙ Settings");
        sidebar.fixed(&settings_btn, 35);
        settings_btn.set_color(DarkTheme::BG_TERTIARY);
        settings_btn.set_label_color(DarkTheme::TEXT_PRIMARY);
        settings_btn.set_frame(enums::FrameType::FlatBox);

        self.setup_settings_button(settings_btn);

        // Disconnect button
        let mut disconnect_btn = button::Button::default().with_label("Disconnect");
        sidebar.fixed(&disconnect_btn, 35);
        disconnect_btn.set_color(DarkTheme::DANGER);
        disconnect_btn.set_label_color(enums::Color::White);
        disconnect_btn.set_frame(enums::FrameType::FlatBox);

        self.setup_disconnect_button(disconnect_btn);

        sidebar.end();

        // Store the sidebar group for later refresh
        self.sidebar_group = Some(sidebar.clone());

        sidebar
    }

    fn setup_mute_button(&self) {
        if let Some(mut mute_btn) = self.mute_btn.clone() {
            let voice_chat = self.voice_chat.clone();

            mute_btn.set_callback(move |b| {
                if let Ok(mut vc_guard) = voice_chat.lock() {
                    if let Some(vc) = vc_guard.as_mut() {
                        let muted = b.label() == "Mute";
                        //vc.set_muted(muted);
                        if muted {
                            b.set_label("Unmute");
                            b.set_color(DarkTheme::DANGER);
                        } else {
                            b.set_label("Mute");
                            b.set_color(DarkTheme::BG_TERTIARY);
                        }
                    }
                }
            });
        }
    }

    fn setup_settings_button(&self, mut settings_btn: button::Button) {
        let voice_chat = self.voice_chat.clone();
        let settings = self.settings.clone();

        settings_btn.set_callback(move |_| {
            let audio_settings = AudioSettingsWindow::new(voice_chat.clone(), settings.clone());
            audio_settings.show_window();
        });
    }

    fn setup_disconnect_button(&self, mut disconnect_btn: button::Button) {
        let network_client = self.network_client.clone();
        let voice_chat = self.voice_chat.clone();

        disconnect_btn.set_callback(move |_| {
            if let Ok(mut client_guard) = network_client.lock() {
                if let Some(client) = client_guard.as_mut() {
                    client.disconnect();
                }
                *client_guard = None;
            }
            if let Ok(mut vc_guard) = voice_chat.lock() {
                *vc_guard = None;
            }
            app::quit();
        });
    }

    pub fn update_user_list(&mut self) {
        // CHECK: Don't update if menu is active
        if *self.menu_active.lock().unwrap() {
            info!("Skipping user list update - menu is active");
            return;
        }

        if let (Some(user_list_pack), Some(user_count_label)) =
            (&mut self.user_list_pack, &mut self.user_count_label)
        {
            // Clear the pack
            user_list_pack.clear();

            if let Ok(client_guard) = self.network_client.lock() {
                if let Some(client) = client_guard.as_ref() {
                    let users = client.get_users();
                    let mut speaking = self.speaking_indicators.lock().unwrap();
                    let our_id = *self.our_client_id.lock().unwrap();
                    let settings_guard = self.settings.lock().unwrap();
                    let now = Instant::now();
                    let timeout_duration = std::time::Duration::from_millis(100);

                    // Update stale indicators
                    for (_client_id, state) in speaking.iter_mut() {
                        if state.is_speaking
                            && now.duration_since(state.last_activity) > timeout_duration
                        {
                            state.is_speaking = false;
                        }
                    }

                    // Update user count
                    user_count_label.set_label(&format!("Users: {}", users.len()));

                    // Add all users
                    for (client_id, username) in users {
                        let is_us = Some(*client_id) == our_id;
                        let is_speaking = speaking
                            .get(client_id)
                            .map(|state| state.is_speaking)
                            .unwrap_or(false);

                        let user_settings = settings_guard
                            .user_audio_settings
                            .get(username)
                            .cloned()
                            .unwrap_or_default();

                        create_user_item(
                            user_list_pack,
                            username,
                            *client_id,
                            is_us,
                            is_speaking,
                            &user_settings,
                            self.settings.clone(),
                            self.voice_chat.clone(),
                            self.menu_active.clone(),
                        );
                    }

                    user_list_pack.redraw();
                }
            }
        }
    }

    pub fn handle_disconnected(&mut self) {
        if let Some(connection_status) = &mut self.connection_status {
            connection_status.set_label("● Disconnected");
            connection_status.set_label_color(DarkTheme::DANGER);
        }
    }

    /// Force refresh the entire sidebar (useful after settings changes)
    pub fn refresh(&mut self) {
        self.update_user_list();
        if let Some(mut sidebar) = self.sidebar_group.clone() {
            sidebar.redraw();
        }
        app::awake(); // Wake the UI thread
    }
}

fn create_user_item(
    user_list_pack: &mut group::Pack,
    username: &str,
    client_id: u64,
    is_us: bool,
    is_speaking: bool,
    user_settings: &UserAudioSettings,
    settings: Arc<Mutex<Settings>>,
    voice_chat: Arc<Mutex<Option<VoiceChat>>>,
    menu_active: Arc<Mutex<bool>>,
) {
    let mut user_item = group::Flex::default().with_size(220, 30);
    user_item.set_type(group::FlexType::Row);
    user_item.set_margins(5, 5, 5, 5);

    // Apply visual indicators
    if user_settings.is_muted {
        user_item.set_color(enums::Color::from_rgb(50, 30, 30));
    } else if (user_settings.volume - 1.0).abs() > 0.01 {
        user_item.set_color(enums::Color::from_rgb(40, 40, 50));
    }

    // Speaking indicator
    let mut indicator = frame::Frame::default();
    user_item.fixed(&indicator, 20);

    if user_settings.is_muted {
        indicator.set_label("🔇");
    } else {
        indicator.set_label("●");
    }

    indicator.set_label_size(16);
    if is_speaking && !user_settings.is_muted {
        indicator.set_label_color(DarkTheme::SPEAKING);
    } else {
        indicator.set_label_color(DarkTheme::TEXT_MUTED);
    }

    // Username label
    let display_name = if (user_settings.volume - 1.0).abs() > 0.01 {
        format!("{} ({}%)", username, (user_settings.volume * 100.0) as i32)
    } else {
        username.to_string()
    };

    let mut name_label = frame::Frame::default().with_label(&display_name);
    name_label.set_align(enums::Align::Left | enums::Align::Inside);

    if is_us {
        name_label.set_label_color(DarkTheme::ACCENT);
    } else if user_settings.is_muted {
        name_label.set_label_color(DarkTheme::DANGER);
    } else {
        name_label.set_label_color(DarkTheme::TEXT_PRIMARY);
    }
    name_label.set_label_size(13);

    user_item.end();

    // Add context menu for non-self users
    if !is_us {
        setup_user_context_menu(
            user_item.clone(),
            username.to_string(),
            client_id,
            settings,
            voice_chat,
            menu_active,
        );
    }

    user_list_pack.add(&user_item);
}

// Proper context menu implementation using FLTK's built-in menu system
// Updated to use menu_active flag
fn setup_user_context_menu(
    mut user_item: group::Flex,
    username: String,
    client_id: u64,
    settings: Arc<Mutex<Settings>>,
    voice_chat: Arc<Mutex<Option<VoiceChat>>>,
    menu_active: Arc<Mutex<bool>>,
) {
    user_item.handle(move |_widget, event| {
        match event {
            enums::Event::Push => {
                if app::event_button() == 3 {
                    // SET FLAG: Menu is now active
                    *menu_active.lock().unwrap() = true;

                    show_user_context_menu(&username, client_id, &settings, &voice_chat);

                    // CLEAR FLAG: Menu is done
                    *menu_active.lock().unwrap() = false;

                    // Trigger a refresh now that it's safe
                    app::awake();

                    return true;
                }
            }
            _ => {}
        }
        false
    });
}

// Separate function to show the context menu - avoids widget lifetime issues
fn show_user_context_menu(
    username: &str,
    client_id: u64,
    settings: &Arc<Mutex<Settings>>,
    voice_chat: &Arc<Mutex<Option<VoiceChat>>>,
) {
    // Get current settings
    let current_settings = settings
        .lock()
        .unwrap()
        .user_audio_settings
        .get(username)
        .cloned()
        .unwrap_or_default();

    // Create menu items
    let mute_label = if current_settings.is_muted {
        "Unmute User"
    } else {
        "Mute User"
    };

    // Use FLTK's MenuItem for popup menu
    let choices = vec![mute_label, "Volume", "Reset Audio Settings"];

    let menu = menu::MenuItem::new(&choices);

    // Show popup menu and handle selection
    if let Some(choice) = menu.popup(app::event_x(), app::event_y()) {
        let label = choice.label().unwrap_or_default();

        // Handle the selected action
        handle_menu_action(
            &label,
            username,
            client_id,
            settings.clone(),
            voice_chat.clone(),
        );
    }
}

// Handle menu actions separately to avoid closure complexity
fn handle_menu_action(
    action: &str,
    username: &str,
    client_id: u64,
    settings: Arc<Mutex<Settings>>,
    voice_chat: Arc<Mutex<Option<VoiceChat>>>,
) {
    if action.contains("Mute") || action.contains("Unmute") {
        // Toggle mute
        let mut settings_guard = settings.lock().unwrap();
        let user_settings = settings_guard
            .user_audio_settings
            .entry(username.to_string())
            .or_insert_with(UserAudioSettings::default);

        user_settings.is_muted = !user_settings.is_muted;
        let is_muted = user_settings.is_muted;

        // Update voice chat
        if let Ok(mut vc_guard) = voice_chat.lock() {
            if let Some(vc) = vc_guard.as_mut() {
                vc.set_user_audio_settings(client_id, user_settings.clone());
            }
        }

        settings_guard.save();
        info!(
            "User {} {}",
            username,
            if is_muted { "muted" } else { "unmuted" }
        );

        // Trigger UI refresh
        app::awake();
    } else if action.contains("Volume") {
        // Show volume dialog
        show_volume_dialog(username, client_id, &settings, &voice_chat);
    } else if action.contains("Reset") {
        // Reset settings
        let mut settings_guard = settings.lock().unwrap();
        settings_guard.user_audio_settings.remove(username);

        // Reset in voice chat
        if let Ok(mut vc_guard) = voice_chat.lock() {
            if let Some(vc) = vc_guard.as_mut() {
                vc.set_user_audio_settings(client_id, UserAudioSettings::default());
            }
        }

        settings_guard.save();
        info!("Reset audio settings for {}", username);

        // Trigger UI refresh
        app::awake();
    }
}

// Simplified volume dialog
fn show_volume_dialog(
    username: &str,
    client_id: u64,
    settings: &Arc<Mutex<Settings>>,
    voice_chat: &Arc<Mutex<Option<VoiceChat>>>,
) {
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

    // Current volume
    let current_volume = settings
        .lock()
        .unwrap()
        .user_audio_settings
        .get(username)
        .map(|s| s.volume)
        .unwrap_or(1.0);

    let volume_label = std::rc::Rc::new(std::cell::RefCell::new(
        frame::Frame::default()
            .with_label(&format!("Volume: {}%", (current_volume * 100.0) as i32)),
    ));
    volume_label
        .borrow_mut()
        .set_label_color(DarkTheme::TEXT_SECONDARY);
    col.fixed(&*volume_label.borrow(), 25);

    // Volume slider
    let mut slider = valuator::HorSlider::default();
    slider.set_minimum(0.0);
    slider.set_maximum(2.0);
    slider.set_value(current_volume as f64);
    slider.set_step(0.05, 1);
    slider.set_color(DarkTheme::BG_TERTIARY);
    slider.set_selection_color(DarkTheme::ACCENT);
    col.fixed(&slider, 35);

    // Slider labels
    let mut slider_labels = group::Flex::default().row();
    col.fixed(&slider_labels, 20);

    let mut min_label = frame::Frame::default().with_label("0%");
    min_label.set_label_size(10);
    min_label.set_label_color(DarkTheme::TEXT_MUTED);

    let mut center_label = frame::Frame::default().with_label("100%");
    center_label.set_label_size(10);
    center_label.set_label_color(DarkTheme::TEXT_MUTED);
    center_label.set_align(enums::Align::Center);

    let mut max_label = frame::Frame::default().with_label("200%");
    max_label.set_label_size(10);
    max_label.set_label_color(DarkTheme::TEXT_MUTED);
    max_label.set_align(enums::Align::Right | enums::Align::Inside);

    slider_labels.end();

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
    });

    // OK button - save settings
    let settings_clone = settings.clone();
    let voice_chat_clone = voice_chat.clone();
    let username_clone = username.to_string();
    let mut dialog_clone = dialog.clone();

    ok_btn.set_callback(move |_| {
        let new_volume = slider.value() as f32;

        // Update settings
        let mut settings_guard = settings_clone.lock().unwrap();
        let user_settings = settings_guard
            .user_audio_settings
            .entry(username_clone.clone())
            .or_insert_with(UserAudioSettings::default);

        user_settings.volume = new_volume;

        // Update voice chat
        if let Ok(mut vc_guard) = voice_chat_clone.lock() {
            if let Some(vc) = vc_guard.as_mut() {
                vc.set_user_audio_settings(client_id, user_settings.clone());
            }
        }

        settings_guard.save();
        info!(
            "Set volume for {} to {}%",
            username_clone,
            (new_volume * 100.0) as i32
        );

        dialog_clone.hide();

        // Trigger UI refresh
        app::awake();
    });

    dialog.show();

    // Run modal dialog
    while dialog.shown() {
        app::wait();
    }
}

// Helper struct to pass data to menu callbacks (if needed)
struct UserMenuData {
    username: String,
    client_id: u64,
    settings: Arc<Mutex<Settings>>,
    voice_chat: Arc<Mutex<Option<VoiceChat>>>,
}

