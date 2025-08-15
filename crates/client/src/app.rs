use fltk::{prelude::*, *};
use log::info;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::network::{NetworkClient, NetworkMessage};
use crate::settings::Settings;
use crate::ui::chat::Chat;
use crate::ui::sidebar::Sidebar;
use crate::ui::theme::DarkTheme;
use crate::voice::{AudioPacket, VoiceChat};

#[derive(Clone)]
pub struct SpeakingState {
    pub is_speaking: bool,
    pub last_activity: Instant,
}

pub struct App {
    app: app::App,
    network_client: Arc<Mutex<Option<NetworkClient>>>,
    voice_chat: Arc<Mutex<Option<VoiceChat>>>,
    settings: Arc<Mutex<Settings>>,
    speaking_indicators: Arc<Mutex<HashMap<u64, SpeakingState>>>,
    our_client_id: Arc<Mutex<Option<u64>>>,
}

impl App {
    pub fn new(
        network_client: Arc<Mutex<Option<NetworkClient>>>,
        voice_chat: Arc<Mutex<Option<VoiceChat>>>,
        settings: Arc<Mutex<Settings>>,
    ) -> Self {
        let app = app::App::default();

        DarkTheme::apply_to_app();

        App {
            app,
            network_client,
            voice_chat,
            settings,
            speaking_indicators: Arc::new(Mutex::new(HashMap::new())),
            our_client_id: Arc::new(Mutex::new(None)),
        }
    }

    pub fn run(&mut self) {
        self.show_connection_screen();
    }

    fn show_connection_screen(&mut self) {
        let mut wind = window::Window::default()
            .with_size(600, 480)
            .with_label("baucord - Connect");
        wind.make_resizable(false);
        DarkTheme::style_window(&mut wind);

        let mut col = group::Flex::default_fill().column();
        col.set_margins(20, 20, 20, 20);
        col.set_spacing(15);

        // Logo/Title
        let mut title = frame::Frame::default()
            .with_label("baucord")
            .with_align(enums::Align::Center);
        title.set_label_size(32);
        title.set_label_color(DarkTheme::TEXT_PRIMARY);
        col.fixed(&title, 50);

        let mut subtitle = frame::Frame::default()
            .with_label("Voice & Text Chat")
            .with_align(enums::Align::Center);
        subtitle.set_label_size(12);
        subtitle.set_label_color(DarkTheme::TEXT_SECONDARY);
        col.fixed(&subtitle, 20);

        let saved_nickname = self.settings.lock().unwrap().nickname.clone();
        let saved_server = self.settings.lock().unwrap().last_server.clone();

        // Nickname input
        let mut nick_frame = frame::Frame::default()
            .with_label("Nickname")
            .with_align(enums::Align::Left | enums::Align::Inside);
        nick_frame.set_label_color(DarkTheme::TEXT_SECONDARY);
        nick_frame.set_label_size(11);
        col.fixed(&nick_frame, 20);

        let mut nick_input = input::Input::default();
        nick_input.set_value(&saved_nickname);
        DarkTheme::style_input(&mut nick_input);
        col.fixed(&nick_input, 35);

        // Server address input
        let mut addr_frame = frame::Frame::default()
            .with_label("Server Address")
            .with_align(enums::Align::Left | enums::Align::Inside);
        addr_frame.set_label_color(DarkTheme::TEXT_SECONDARY);
        addr_frame.set_label_size(11);
        col.fixed(&addr_frame, 20);

        let mut addr_input = input::Input::default();
        addr_input.set_value(&saved_server);
        DarkTheme::style_input(&mut addr_input);
        col.fixed(&addr_input, 35);

        // Error display
        let mut error_frame = frame::Frame::default().with_align(enums::Align::Center);
        error_frame.set_label_color(DarkTheme::DANGER);
        error_frame.set_label_size(12);
        col.fixed(&error_frame, 25);

        // Connect button
        let mut connect_btn = button::Button::default().with_label("Connect");
        connect_btn.set_color(DarkTheme::ACCENT);
        connect_btn.set_label_color(enums::Color::White);
        connect_btn.set_frame(enums::FrameType::FlatBox);
        col.fixed(&connect_btn, 40);

        col.end();
        wind.end();
        wind.show();

        let (s, r) = app::channel::<String>();

        let network_client = self.network_client.clone();
        let voice_chat = self.voice_chat.clone();
        let settings = self.settings.clone();
        let s_error = s.clone();

        connect_btn.set_callback(move |_| {
            let nickname = nick_input.value();
            let server_addr = addr_input.value();

            if nickname.trim().is_empty() {
                s_error.send("Nickname cannot be empty".to_string());
                return;
            }

            {
                let mut settings_guard = settings.lock().unwrap();
                settings_guard.nickname = nickname.clone();
                settings_guard.last_server = server_addr.clone();
                settings_guard.save();
            }

            match server_addr.parse::<SocketAddr>() {
                Ok(addr) => match NetworkClient::connect(addr, nickname.clone()) {
                    Ok(client) => {
                        *network_client.lock().unwrap() = Some(client);

                        match VoiceChat::new() {
                            Ok(mut vc) => {
                                // Pass settings to start() method
                                let settings_ref = settings.lock().unwrap();
                                if let Err(e) = vc.start(&*settings_ref) {
                                    s_error.send(format!("Voice init failed: {}", e));
                                    return;
                                }
                                drop(settings_ref);
                                *voice_chat.lock().unwrap() = Some(vc);
                            }
                            Err(e) => {
                                s_error.send(format!("Voice setup failed: {}", e));
                                return;
                            }
                        }

                        s_error.send("CONNECTED".to_string());
                    }
                    Err(e) => {
                        s_error.send(format!("Connection failed: {}", e));
                    }
                },
                Err(_) => {
                    s_error.send("Invalid server address (use format: ip:port)".to_string());
                }
            }
        });

        while self.app.wait() {
            if let Some(msg) = r.recv() {
                if msg == "CONNECTED" {
                    wind.hide();
                    self.show_main_screen();
                    break;
                } else {
                    error_frame.set_label(&msg);
                }
            }
        }
    }

    fn show_main_screen(&mut self) {
        let mut wind = window::Window::default()
            .with_size(1000, 700)
            .with_label("baucord");
        wind.make_resizable(true);
        DarkTheme::style_window(&mut wind);

        let mut main_row = group::Flex::default_fill().row();
        main_row.set_color(DarkTheme::BG_PRIMARY);

        // Create sidebar component
        let mut sidebar = Sidebar::new(
            self.network_client.clone(),
            self.voice_chat.clone(),
            self.settings.clone(),
            self.speaking_indicators.clone(),
            self.our_client_id.clone(),
        );
        let sidebar_group = sidebar.create_ui();
        main_row.fixed(&sidebar_group, 250);

        // Create chat component
        let mut chat = Chat::new(self.network_client.clone(), self.settings.clone());
        let chat_group = chat.create_ui();
        main_row.add(&chat_group);

        main_row.end();
        wind.end();
        wind.show();

        // Setup network/voice update loop
        self.setup_network_loop(sidebar, chat);

        // Main event loop
        while self.app.wait() {}
    }

    fn setup_network_loop(&mut self, mut sidebar: Sidebar, mut chat: Chat) {
        // Use crossbeam for better performance
        let (s, r) = crossbeam_channel::unbounded::<NetworkMessage>();

        let network_client = self.network_client.clone();
        let voice_chat = self.voice_chat.clone();
        let s_net = s.clone();
        let speaking_indicators = self.speaking_indicators.clone();
        let settings = self.settings.clone();
        let our_client_id_net = self.our_client_id.clone(); // for the 100Hz network loop
        let our_client_id_ui = self.our_client_id.clone(); // for the ~60Hz UI loop

        app::add_timeout3(0.01, move |handle| {
            if let Ok(mut client_guard) = network_client.lock() {
                if let Some(client) = client_guard.as_mut() {
                    client.update();

                    let messages = client.poll_messages();

                    for msg in messages {
                        match &msg {
                            NetworkMessage::VoiceData {
                                client_id,
                                seq,
                                timestamp: ts_48k,
                                data,
                            } => {
                                if let Ok(mut vc_guard) = voice_chat.lock() {
                                    if let Some(vc) = vc_guard.as_mut() {
                                        // Create AudioPacket from network data with proper types
                                        let packet = AudioPacket::from_network(
                                            *seq,    // u32 sequence
                                            *ts_48k, // u32 48kHz timestamp
                                            *client_id,
                                            data.clone(),
                                        );
                                        vc.receive_packet(packet);
                                    }
                                }
                            }
                            NetworkMessage::UserSpeaking {
                                client_id,
                                is_speaking,
                            } => {
                                let mut indicators = speaking_indicators.lock().unwrap();
                                indicators.insert(
                                    *client_id,
                                    SpeakingState {
                                        is_speaking: *is_speaking,
                                        last_activity: Instant::now(),
                                    },
                                );
                            }
                            _ => {}
                        }
                        // Send to UI thread via crossbeam
                        let _ = s_net.try_send(msg);
                    }

                    // Send outgoing voice packets with monotonic timestamps
                    if let Ok(mut vc_guard) = voice_chat.lock() {
                        if let Some(vc) = vc_guard.as_mut() {
                            let packets = vc.get_outgoing_packets();

                            if let Some(our_id) = *our_client_id_net.lock().unwrap() {
                                for packet in packets {
                                    // The packet already has a proper monotonic ts_48k from encoder
                                    // Just pass it through
                                    client.send_voice_frame_raw(
                                        our_id,
                                        packet.header.timestamp,
                                        &packet.opus_data,
                                    );
                                }

                                // Flush once per tick (every 10ms)
                                client.flush_voice_packet();
                            }
                        }
                    }
                }
            }

            app::repeat_timeout3(0.01, handle);
        });

        // Clone these again before the second timeout
        let voice_chat2 = self.voice_chat.clone();
        let speaking_indicators2 = self.speaking_indicators.clone();

        // Handle messages in main thread
        app::add_timeout3(0.016, move |handle| {
            // ~60Hz for UI updates
            // Process all pending messages without blocking
            while let Ok(msg) = r.try_recv() {
                match msg {
                    NetworkMessage::InitClient {
                        users,
                        server_name,
                        greeting,
                    } => {
                        chat.handle_init(server_name, greeting);

                        // Store our client ID
                        if let Some(our_id) = users.keys().find(|&&id| {
                            users
                                .get(&id)
                                .map_or(false, |name| name == &settings.lock().unwrap().nickname)
                        }) {
                            *our_client_id_ui.lock().unwrap() = Some(*our_id);
                        }

                        // Add decoders for all existing users
                        if let Ok(mut vc_guard) = voice_chat2.lock() {
                            if let Some(vc) = vc_guard.as_mut() {
                                for client_id in users.keys() {
                                    vc.add_client(*client_id);
                                }
                            }
                        }

                        sidebar.refresh();
                    }
                    NetworkMessage::Connected => {
                        chat.handle_connected();
                    }
                    NetworkMessage::Disconnected(reason) => {
                        chat.handle_disconnected(reason);
                        sidebar.handle_disconnected();
                    }
                    NetworkMessage::UserJoined {
                        username,
                        client_id,
                    } => {
                        chat.handle_user_joined(username.clone());

                        if let Ok(mut vc_guard) = voice_chat2.lock() {
                            if let Some(vc) = vc_guard.as_mut() {
                                vc.add_client(client_id);
                            }
                        }

                        sidebar.update_user_list();
                    }
                    NetworkMessage::UserLeft {
                        username,
                        client_id,
                    } => {
                        chat.handle_user_left(username);

                        if let Ok(mut vc_guard) = voice_chat2.lock() {
                            if let Some(vc) = vc_guard.as_mut() {
                                vc.remove_client(client_id);
                            }
                        }

                        speaking_indicators2.lock().unwrap().remove(&client_id);
                        sidebar.update_user_list();
                    }
                    NetworkMessage::TextMessage { username, text, .. } => {
                        chat.handle_text_message(username, text);
                    }
                    NetworkMessage::UserSpeaking { .. } | NetworkMessage::UiUpdate => {
                        sidebar.update_user_list();
                    }
                    _ => {}
                }
            }

            app::repeat_timeout3(0.1, handle);
        });
    }
}
