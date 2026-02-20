// app.rs — Main application coordinator.
//
// Owns the FLTK event loop and delegates to:
//   - NetworkThreadHandle (network.rs) for renet client + protocol
//   - VoiceThreadHandle   (voice.rs)   for audio capture/playback/encoding
//
// Speaking state is driven by vc.is_speaking(), not inferred from packet flow.

use std::sync::atomic::{AtomicBool, Ordering};

use fltk::{prelude::*, *};
use log::{info, warn};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::{
    config::{user_key, Settings, UserAudioSettings},
    network::{NetworkControl, NetworkMessage, NetworkThreadHandle},
    ui::{chat::Chat, messages::UiMessage, sidebar::Sidebar},
    voice::{VoiceControl, VoiceEvent, VoiceThreadHandle},
};
use core::PeerAudioSettings;
use ui::theme;
use ui::widgets::{controls, style};

// ── Public types ────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct UserState {
    pub username: String,
    pub is_speaking: bool,
    pub last_activity: Instant,
    pub audio_settings: UserAudioSettings,
}

// ── Conversion helper ──────────────────────────────────────────────────

fn to_peer_settings(s: &UserAudioSettings) -> PeerAudioSettings {
    PeerAudioSettings {
        volume: s.volume,
        is_muted: s.is_muted,
    }
}

// ── App ────────────────────────────────────────────────────────────────

pub struct App {
    app: app::App,
    sender: app::Sender<UiMessage>,
    receiver: app::Receiver<UiMessage>,

    // Thread handles (own the threads + their channels)
    network: NetworkThreadHandle,
    voice: VoiceThreadHandle,

    // State
    users: HashMap<renet::ClientId, UserState>,
    our_client_id: Option<renet::ClientId>,
    connected: bool,
    settings: Settings,
    is_muted: bool,
    is_deafened: bool,
    is_testing_mic: bool,
    is_testing_mic_shared: Arc<AtomicBool>,

    // UI
    connection_window: Option<window::Window>,
    main_window: Option<window::Window>,
    sidebar: Option<Sidebar>,
    chat: Option<Chat>,
    error_frame: Option<frame::Frame>,
    settings_window_open: bool,
}

impl App {
    pub fn new() -> Self {
        let app = app::App::default();
        theme::apply();

        let (sender, receiver) = app::channel::<UiMessage>();

        let is_testing_mic_shared = Arc::new(AtomicBool::new(false));

        // Wake callback — worker threads call this to nudge the FLTK event loop.
        // This is the ONLY place fltk::app::awake is referenced for worker threads.
        let wake: crate::voice::WakeFn = Arc::new(|| fltk::app::awake());

        // Spawn threads via their handles.
        // Network needs voice_tx to route incoming voice packets,
        // so we use spawn_with_channels to share the sender.
        let (network_tx, network_control_rx) = crossbeam_channel::bounded::<NetworkControl>(256);
        let (network_msg_tx, network_msg_rx) = crossbeam_channel::bounded::<NetworkMessage>(256);

        let voice = VoiceThreadHandle::spawn(
            network_tx.clone(),
            is_testing_mic_shared.clone(),
            wake.clone(),
        );

        let network = NetworkThreadHandle::spawn_with_channels(
            network_tx,
            network_control_rx,
            network_msg_tx,
            network_msg_rx,
            voice.tx.clone(),
            wake,
        );

        let settings = Settings::load();

        Self {
            app,
            sender,
            receiver,
            network,
            voice,
            users: HashMap::new(),
            our_client_id: None,
            connected: false,
            settings,
            is_muted: false,
            is_deafened: false,
            is_testing_mic: false,
            is_testing_mic_shared,
            connection_window: None,
            main_window: None,
            sidebar: None,
            chat: None,
            error_frame: None,
            settings_window_open: false,
        }
    }

    pub fn run(&mut self) {
        self.show_connection_screen();

        while self.app.wait() {
            // UI messages
            while let Some(msg) = self.receiver.recv() {
                self.handle_ui_message(msg);
            }

            // Network messages (bounded drain)
            let mut needs_redraw = false;
            for _ in 0..256 {
                match self.network.msg_rx.try_recv() {
                    Ok(msg) => {
                        self.handle_network_message(msg);
                        needs_redraw = true;
                    }
                    Err(_) => break,
                }
            }

            // Voice events (bounded drain)
            for _ in 0..32 {
                match self.voice.event_rx.try_recv() {
                    Ok(evt) => {
                        self.handle_voice_event(evt);
                        needs_redraw = true;
                    }
                    Err(_) => break,
                }
            }

            if needs_redraw {
                app::redraw();
            }
        }

        self.shutdown();
    }

    // ════════════════════════════════════════════════════════════════════
    //  EVENT HANDLERS
    // ════════════════════════════════════════════════════════════════════

    fn handle_voice_event(&mut self, evt: VoiceEvent) {
        match evt {
            VoiceEvent::Started => {
                info!("Voice engine ready");
            }
            VoiceEvent::Error(e) => {
                self.show_error(&e);
            }
            VoiceEvent::SpeakingChanged(speaking) => {
                if let Some(id) = self.our_client_id
                    && let Some(user) = self.users.get_mut(&id)
                {
                    user.is_speaking = speaking;
                    user.last_activity = Instant::now();
                }
                self.update_user_list();
            }
        }
    }

    fn handle_network_message(&mut self, msg: NetworkMessage) {
        match msg {
            NetworkMessage::Connected => {
                self.handle_connection_established();
            }
            NetworkMessage::Disconnected(reason) => {
                if self.connected {
                    self.handle_disconnect_event(&reason);
                } else {
                    self.show_error(&reason);
                }
            }
            NetworkMessage::InitClient {
                users,
                server_name,
                greeting,
            } => {
                self.handle_init_client(users, server_name, greeting);
            }
            NetworkMessage::UserJoined {
                client_id,
                username,
            } => {
                self.handle_user_joined(client_id, username);
            }
            NetworkMessage::UserLeft {
                client_id,
                username,
            } => {
                self.handle_user_left(client_id, username);
            }
            NetworkMessage::TextMessage {
                client_id: _,
                username,
                text,
            } => {
                if let Some(chat) = &mut self.chat {
                    chat.handle_text_message(username, text);
                }
            }
            NetworkMessage::UserSpeaking {
                client_id,
                is_speaking,
            } => {
                if let Some(user) = self.users.get_mut(&client_id) {
                    user.is_speaking = is_speaking;
                    user.last_activity = Instant::now();
                }
                self.update_user_list();
            }
            NetworkMessage::VoiceData { .. } => {}
            NetworkMessage::UiUpdate => {}
            NetworkMessage::LatencyUpdate { rtt_ms } => {
                if let Some(sidebar) = &mut self.sidebar {
                    let audio_ms = self.voice.shared.jitter_buffer_ms.load(Ordering::Relaxed);
                    sidebar.update_latency(rtt_ms, Some(audio_ms));
                }
            }
        }
    }

    fn handle_ui_message(&mut self, msg: UiMessage) {
        match msg {
            UiMessage::Connect { nickname, server } => {
                self.handle_connect(nickname, server);
            }
            UiMessage::Disconnect => {
                self.handle_disconnect();
            }
            UiMessage::SendChatMessage(text) => {
                self.network.tx.send(NetworkControl::SendMessage(text)).ok();
            }
            UiMessage::ToggleMute => {
                self.handle_toggle_mute();
            }
            UiMessage::ToggleDeafen => {
                self.handle_toggle_deafen();
            }
            UiMessage::TestMicrophone(enabled) => {
                self.handle_test_microphone(enabled);
            }
            UiMessage::SetGateThreshold(t) => {
                self.settings.gate_threshold = t;
                self.voice.tx.send(VoiceControl::SetGateThreshold(t)).ok();
            }
            UiMessage::SetInputGain(g) => {
                self.settings.input_gain = g;
                self.voice.tx.send(VoiceControl::SetInputGain(g)).ok();
            }
            UiMessage::SetOutputGain(g) => {
                self.settings.output_gain = g;
                self.voice.tx.send(VoiceControl::SetOutputGain(g)).ok();
            }
            UiMessage::MuteUser {
                client_id,
                username,
                muted,
            } => {
                self.handle_mute_user(client_id, username, muted);
            }
            UiMessage::SetUserVolume {
                client_id,
                username,
                volume,
            } => {
                self.handle_set_user_volume(client_id, username, volume);
            }
            UiMessage::ResetUserAudio {
                client_id,
                username,
            } => {
                self.handle_reset_user_audio(client_id, username);
            }
            UiMessage::OpenSettings => {
                if !self.settings_window_open {
                    self.settings_window_open = true;
                    self.show_settings_window();
                }
            }
            UiMessage::SaveSettings => {
                self.settings.save();
            }
            UiMessage::ResetSettings => {
                self.settings.reset();
            }
            UiMessage::SetInputDevice(d) => {
                self.settings.input_device = d.clone();
                self.voice.tx.send(VoiceControl::SetInputDevice(d)).ok();
            }
            UiMessage::SetOutputDevice(d) => {
                self.settings.output_device = d.clone();
                self.voice.tx.send(VoiceControl::SetOutputDevice(d)).ok();
            }
            UiMessage::ShowError(e) => {
                self.show_error(&e);
            }
            UiMessage::ClearError => {
                if let Some(frame) = &mut self.error_frame {
                    frame.set_label("");
                }
            }
            UiMessage::SettingsWindowClosed => {
                self.settings_window_open = false;
                if self.is_testing_mic {
                    self.handle_test_microphone(false);
                }
                self.settings.save();
            }
            UiMessage::Shutdown => {
                self.shutdown();
                self.app.quit();
            }
        }
    }

    // ════════════════════════════════════════════════════════════════════
    //  CONNECTION HANDLERS
    // ════════════════════════════════════════════════════════════════════

    fn handle_connect(&mut self, nickname: String, server: String) {
        self.settings.nickname = nickname.clone();
        self.settings.last_server = server.clone();
        self.settings.save();

        let addr = match server.parse::<SocketAddr>() {
            Ok(a) => a,
            Err(_) => {
                self.show_error("Invalid address (use ip:port)");
                return;
            }
        };

        self.network
            .tx
            .send(NetworkControl::Connect { addr, nickname })
            .ok();
    }

    fn handle_connection_established(&mut self) {
        if self.connected {
            return;
        }
        self.connected = true;

        if let Some(mut wind) = self.connection_window.take() {
            wind.hide();
        }

        self.show_main_screen();
    }

    fn handle_init_client(
        &mut self,
        users: HashMap<renet::ClientId, String>,
        server_name: String,
        greeting: String,
    ) {
        let our_nick = &self.settings.nickname;
        self.our_client_id = None;

        // Exact match first, then prefix match, then fallback to highest ID.
        // The server may append a discriminator (e.g. "nick#2") if the name
        // is already taken, so exact match can fail.
        for (id, name) in &users {
            if name == our_nick {
                self.our_client_id = Some(*id);
                break;
            }
        }

        if self.our_client_id.is_none() {
            for (id, name) in &users {
                if name.starts_with(our_nick) || our_nick.starts_with(name) {
                    self.our_client_id = Some(*id);
                }
            }
        }

        if self.our_client_id.is_none() {
            if let Some((&id, _)) = users.iter().max_by_key(|(id, _)| *id) {
                self.our_client_id = Some(id);
                warn!("Could not match nickname, assuming client {id}");
            }
        }

        if let Some(id) = self.our_client_id {
            self.voice
                .tx
                .send(VoiceControl::Start {
                    settings: self.settings.clone(),
                    our_client_id: id,
                })
                .ok();

            self.is_muted = false;
        }

        self.users.clear();
        for (id, username) in &users {
            let audio = self
                .settings
                .user_audio_settings
                .get(&user_key(username))
                .cloned()
                .unwrap_or_default();

            self.users.insert(
                *id,
                UserState {
                    username: username.clone(),
                    is_speaking: false,
                    last_activity: Instant::now(),
                    audio_settings: audio.clone(),
                },
            );

            self.voice.tx.send(VoiceControl::AddPeer(*id)).ok();

            if audio.is_muted || (audio.volume - 1.0).abs() > 0.01 {
                self.voice
                    .tx
                    .send(VoiceControl::SetPeerAudio(*id, to_peer_settings(&audio)))
                    .ok();
            }
        }

        if let Some(chat) = &mut self.chat {
            chat.handle_init(server_name, greeting);
        }
        self.update_user_list();
    }

    fn handle_user_joined(&mut self, client_id: renet::ClientId, username: String) {
        let audio = self
            .settings
            .user_audio_settings
            .get(&user_key(&username))
            .cloned()
            .unwrap_or_default();

        self.users.insert(
            client_id,
            UserState {
                username: username.clone(),
                is_speaking: false,
                last_activity: Instant::now(),
                audio_settings: audio.clone(),
            },
        );

        self.voice.tx.send(VoiceControl::AddPeer(client_id)).ok();

        if audio.is_muted || (audio.volume - 1.0).abs() > 0.01 {
            self.voice
                .tx
                .send(VoiceControl::SetPeerAudio(
                    client_id,
                    to_peer_settings(&audio),
                ))
                .ok();
        }

        if let Some(chat) = &mut self.chat {
            chat.handle_user_joined(username);
        }
        self.update_user_list();
    }

    fn handle_user_left(&mut self, client_id: renet::ClientId, username: String) {
        self.users.remove(&client_id);
        self.voice.tx.send(VoiceControl::RemovePeer(client_id)).ok();

        if let Some(chat) = &mut self.chat {
            chat.handle_user_left(username);
        }
        self.update_user_list();
    }

    fn handle_toggle_mute(&mut self) {
        self.is_muted = !self.is_muted;
        self.voice
            .tx
            .send(VoiceControl::SetMuted(self.is_muted))
            .ok();

        if let Some(sidebar) = &mut self.sidebar {
            sidebar.set_mute_state(self.is_muted);
        }

        if self.is_muted {
            self.network
                .tx
                .send(NetworkControl::SendSpeakingStatus(false))
                .ok();
        }
    }

    fn handle_toggle_deafen(&mut self) {
        self.is_deafened = !self.is_deafened;
        self.voice
            .tx
            .send(VoiceControl::SetDeafened(self.is_deafened))
            .ok();

        if let Some(sidebar) = &mut self.sidebar {
            sidebar.set_deafen_state(self.is_deafened);
        }
    }

    fn handle_test_microphone(&mut self, enabled: bool) {
        self.is_testing_mic = enabled;
        self.is_testing_mic_shared.store(enabled, Ordering::Relaxed);
        self.voice.tx.send(VoiceControl::SetLoopback(enabled)).ok();
        if enabled {
            self.network
                .tx
                .send(NetworkControl::SendSpeakingStatus(false))
                .ok();
        }
    }

    fn handle_mute_user(&mut self, client_id: renet::ClientId, username: String, muted: bool) {
        let key = user_key(&username);
        let settings = self
            .settings
            .user_audio_settings
            .entry(key)
            .or_default();
        settings.is_muted = muted;

        if let Some(user) = self.users.get_mut(&client_id) {
            user.audio_settings.is_muted = muted;
        }

        self.voice
            .tx
            .send(VoiceControl::SetPeerAudio(
                client_id,
                to_peer_settings(settings),
            ))
            .ok();
        self.settings.save();
        self.update_user_list();
    }

    fn handle_set_user_volume(
        &mut self,
        client_id: renet::ClientId,
        username: String,
        volume: f32,
    ) {
        let key = user_key(&username);
        let settings = self
            .settings
            .user_audio_settings
            .entry(key)
            .or_default();
        settings.volume = volume;

        if let Some(user) = self.users.get_mut(&client_id) {
            user.audio_settings.volume = volume;
        }

        self.voice
            .tx
            .send(VoiceControl::SetPeerAudio(
                client_id,
                to_peer_settings(settings),
            ))
            .ok();
        self.settings.save();
        self.update_user_list();
    }

    fn handle_reset_user_audio(&mut self, client_id: renet::ClientId, username: String) {
        self.settings.user_audio_settings.remove(&user_key(&username));

        if let Some(user) = self.users.get_mut(&client_id) {
            user.audio_settings = UserAudioSettings::default();
        }

        self.voice
            .tx
            .send(VoiceControl::SetPeerAudio(
                client_id,
                to_peer_settings(&UserAudioSettings::default()),
            ))
            .ok();
        self.settings.save();
        self.update_user_list();
    }

    /// Shared teardown for both user-initiated and server-initiated disconnects.
    fn teardown_connection(&mut self) {
        self.connected = false;
        self.is_testing_mic = false;
        self.voice.tx.send(VoiceControl::Stop).ok();

        if let Some(sidebar) = &mut self.sidebar {
            sidebar.handle_disconnected();
        }

        self.users.clear();
        self.our_client_id = None;
        self.is_muted = false;
        self.is_deafened = false;

        if let Some(mut w) = self.main_window.take() {
            w.hide();
        }
    }

    fn handle_disconnect(&mut self) {
        self.network.tx.send(NetworkControl::Disconnect).ok();

        if let Some(chat) = &mut self.chat {
            chat.handle_disconnected("User disconnected".to_string());
        }

        self.teardown_connection();
        self.show_connection_screen();
    }

    fn handle_disconnect_event(&mut self, reason: &str) {
        if let Some(chat) = &mut self.chat {
            chat.handle_disconnected(reason.to_string());
        }

        self.teardown_connection();
        self.show_connection_screen();
        self.show_error(reason);
    }

    // ════════════════════════════════════════════════════════════════════
    //  UI
    // ════════════════════════════════════════════════════════════════════

    fn update_user_list(&mut self) {
        let timeout = Duration::from_millis(500);
        let now = Instant::now();
        for user in self.users.values_mut() {
            if user.is_speaking && now.duration_since(user.last_activity) > timeout {
                user.is_speaking = false;
            }
        }

        if let Some(sidebar) = &mut self.sidebar {
            sidebar.update_users(&self.users, self.our_client_id);
        }
    }

    fn set_window_icon(wind: &mut window::Window) {
        if let Ok(icon) = fltk::image::PngImage::from_data(include_bytes!("../assets/icon.png")) {
            wind.set_icon(Some(icon));
        }
    }

    fn show_connection_screen(&mut self) {
        let mut wind = window::Window::default()
            .with_size(600, 480)
            .with_label("baucord - Connect");
        wind.make_resizable(false);
        wind.set_color(theme::BG_PRIMARY);
        Self::set_window_icon(&mut wind);

        let mut col = group::Flex::default_fill().column();
        col.set_margins(20, 20, 20, 20);
        col.set_spacing(15);

        let mut title = frame::Frame::default()
            .with_label("baucord")
            .with_align(enums::Align::Center);
        title.set_label_size(32);
        title.set_label_color(theme::TEXT_PRIMARY);
        col.fixed(&title, 50);

        let mut subtitle = frame::Frame::default()
            .with_label("We have discord at home")
            .with_align(enums::Align::Center);
        subtitle.set_label_size(12);
        subtitle.set_label_color(theme::TEXT_SECONDARY);
        col.fixed(&subtitle, 20);

        let mut nick_frame = frame::Frame::default()
            .with_label("Nickname")
            .with_align(enums::Align::Left | enums::Align::Inside);
        nick_frame.set_label_color(theme::TEXT_SECONDARY);
        nick_frame.set_label_size(11);
        col.fixed(&nick_frame, 20);

        let mut nick_input = input::Input::default();
        nick_input.set_value(&self.settings.nickname);
        style::style_input(&mut nick_input);
        col.fixed(&nick_input, 35);

        let mut addr_frame = frame::Frame::default()
            .with_label("Server Address")
            .with_align(enums::Align::Left | enums::Align::Inside);
        addr_frame.set_label_color(theme::TEXT_SECONDARY);
        addr_frame.set_label_size(11);
        col.fixed(&addr_frame, 20);

        let mut addr_input = input::Input::default();
        addr_input.set_value(&self.settings.last_server);
        style::style_input(&mut addr_input);
        col.fixed(&addr_input, 35);

        let mut error_frame = frame::Frame::default().with_align(enums::Align::Center);
        error_frame.set_label_color(theme::DANGER);
        error_frame.set_label_size(12);
        col.fixed(&error_frame, 25);
        self.error_frame = Some(error_frame);

        let mut connect_btn = controls::accent_button("Connect", theme::ACCENT);
        col.fixed(&connect_btn, 40);

        col.end();
        wind.end();
        wind.show();

        let sender = self.sender;
        connect_btn.set_callback(move |_| {
            let nickname = nick_input.value();
            let server = addr_input.value();
            if nickname.trim().is_empty() {
                sender.send(UiMessage::ShowError("Nickname cannot be empty".into()));
            } else {
                sender.send(UiMessage::Connect { nickname, server });
            }
        });

        self.connection_window = Some(wind);
    }

    fn show_main_screen(&mut self) {
        let mut wind = window::Window::default()
            .with_size(self.settings.window_width, self.settings.window_height)
            .with_label("baucord");
        wind.make_resizable(true);
        wind.set_color(theme::BG_PRIMARY);
        Self::set_window_icon(&mut wind);

        if let (Some(x), Some(y)) = (self.settings.window_x, self.settings.window_y) {
            wind.set_pos(x, y);
        }

        let mut main_row = group::Flex::default_fill().row();
        main_row.set_color(theme::BG_PRIMARY);

        let mut sidebar = Sidebar::new(self.sender);
        let sidebar_group = sidebar.create_ui();
        main_row.fixed(&sidebar_group, 250);
        sidebar.set_mute_state(self.is_muted);

        let mut chat = Chat::new(self.sender);
        let chat_group = chat.create_ui();
        main_row.add(&chat_group);

        main_row.end();
        wind.end();

        let sender = self.sender;
        wind.set_callback(move |_w| {
            if app::event() == enums::Event::Close {
                sender.send(UiMessage::Shutdown);
            }
        });

        wind.show();

        self.main_window = Some(wind);
        self.sidebar = Some(sidebar);
        self.chat = Some(chat);
    }

    fn show_settings_window(&mut self) {
        use crate::ui::settings::SettingsWindow;
        use core::VoiceChat;

        let input_devices = VoiceChat::list_input_devices()
            .inspect_err(|e| warn!("Failed to list input devices: {e}"))
            .unwrap_or_default();
        let output_devices = VoiceChat::list_output_devices()
            .inspect_err(|e| warn!("Failed to list output devices: {e}"))
            .unwrap_or_default();

        SettingsWindow::show(
            self.sender,
            self.settings.clone(),
            input_devices,
            output_devices,
            self.voice.shared.input_level.clone(),
            self.voice.shared.output_level.clone(),
            self.voice.shared.denoised_level.clone(),
            self.voice.shared.speaking.clone(),
        );
    }

    fn show_error(&mut self, error: &str) {
        if let Some(frame) = &mut self.error_frame {
            frame.set_label(error);
            frame.set_label_color(theme::DANGER);
        }

        let sender = self.sender;
        app::add_timeout3(5.0, move |_| {
            sender.send(UiMessage::ClearError);
        });
    }

    fn shutdown(&mut self) {
        info!("Shutting down");

        if let Some(ref w) = self.main_window {
            self.settings.window_width = w.w();
            self.settings.window_height = w.h();
            self.settings.window_x = Some(w.x());
            self.settings.window_y = Some(w.y());
        }
        self.settings.save();

        self.network.tx.send(NetworkControl::Shutdown).ok();
        self.voice.tx.send(VoiceControl::Shutdown).ok();
    }
}