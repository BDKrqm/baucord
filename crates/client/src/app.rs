// app.rs — 2 threads (network + voice), no bridge.
//
// The voice thread owns VoiceChat and talks directly to the network thread.
// Speaking state is driven by vc.is_speaking(), not inferred from packet flow.

use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded, select};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use fltk::{prelude::*, *};
use log::{info, warn};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::{
    config::{Settings, UserAudioSettings},
    network::{NetworkClient, NetworkMessage},
    ui::{chat::Chat, messages::UiMessage, sidebar::Sidebar, theme::DarkTheme},
};
use core::{AudioPacket, PeerAudioSettings, VoiceChat, VoiceChatConfig};

// ── Public types ────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct UserState {
    pub username: String,
    pub is_speaking: bool,
    pub last_activity: Instant,
    pub audio_settings: UserAudioSettings,
}

// ── Internal channel messages ───────────────────────────────────────────

#[derive(Debug)]
enum NetworkControl {
    Connect { addr: SocketAddr, nickname: String },
    Disconnect,
    SendMessage(String),
    SendVoicePacket(AudioPacket),
    SendSpeakingStatus(bool),
    Shutdown,
}

#[derive(Debug)]
enum VoiceControl {
    Start {
        settings: Settings,
        our_client_id: u64,
    },
    Stop,
    SetMuted(bool),
    SetLoopback(bool),
    SetGateThreshold(f32),
    SetInputGain(f32),
    SetOutputGain(f32),
    SetInputDevice(Option<String>),
    SetOutputDevice(Option<String>),
    AddPeer(u64),
    RemovePeer(u64),
    SetPeerAudio(u64, PeerAudioSettings),
    IncomingVoicePacket(AudioPacket),
    Shutdown,
}

#[derive(Debug, Clone)]
enum VoiceEvent {
    Started,
    Error(String),
    SpeakingChanged(bool),
}

// ── Conversion helper ──────────────────────────────────────────────────

fn to_peer_settings(s: &UserAudioSettings) -> PeerAudioSettings {
    PeerAudioSettings {
        volume: s.volume,
        is_muted: s.is_muted,
    }
}

fn settings_to_config(s: &Settings) -> VoiceChatConfig {
    VoiceChatConfig {
        input_device: s.input_device.clone(),
        output_device: s.output_device.clone(),
        mute_on_start: s.mute_on_start,
        gate_threshold: s.gate_threshold,
        noise_suppression: true, // Always on
        input_gain: s.input_gain,
        output_gain: s.output_gain,
    }
}

// ── App ────────────────────────────────────────────────────────────────

pub struct App {
    app: app::App,
    sender: app::Sender<UiMessage>,
    receiver: app::Receiver<UiMessage>,

    // Thread channels
    network_tx: Sender<NetworkControl>,
    network_rx: Receiver<NetworkMessage>,
    voice_tx: Sender<VoiceControl>,
    voice_event_rx: Receiver<VoiceEvent>,

    // State
    users: HashMap<renet::ClientId, UserState>,
    our_client_id: Option<renet::ClientId>,
    connected: bool,
    settings: Settings,
    is_muted: bool,
    is_testing_mic: bool,
    is_testing_mic_shared: Arc<AtomicBool>,

    // UI
    connection_window: Option<window::Window>,
    main_window: Option<window::Window>,
    sidebar: Option<Sidebar>,
    chat: Option<Chat>,
    error_frame: Option<frame::Frame>,
    settings_window_open: bool,

    // Shared audio levels (written by voice thread, read by settings UI)
    shared_input_level: Arc<AtomicU32>,
    shared_output_level: Arc<AtomicU32>,
    shared_denoised_level: Arc<AtomicU32>,
    shared_speaking: Arc<AtomicBool>,

    // Thread handles
    network_thread: Option<thread::JoinHandle<()>>,
    voice_thread: Option<thread::JoinHandle<()>>,
}

impl App {
    pub fn new() -> Self {
        let app = app::App::default();
        DarkTheme::apply_to_app();

        let (sender, receiver) = app::channel::<UiMessage>();

        // Network channels
        let (network_tx, network_control_rx) = bounded::<NetworkControl>(256);
        let (network_msg_tx, network_rx) = bounded::<NetworkMessage>(256);

        // Voice channels
        let (voice_tx, voice_control_rx) = bounded::<VoiceControl>(64);
        let (voice_event_tx, voice_event_rx) = bounded::<VoiceEvent>(32);

        // Shared audio levels (created before voice thread so we can pass clones)
        let shared_input_level = Arc::new(AtomicU32::new(0));
        let shared_output_level = Arc::new(AtomicU32::new(0));
        let shared_denoised_level = Arc::new(AtomicU32::new(0));
        let shared_speaking = Arc::new(AtomicBool::new(false));
        let is_testing_mic_shared = Arc::new(AtomicBool::new(false));

        // Start threads
        let network_thread =
            Self::spawn_network_thread(network_control_rx, network_msg_tx, voice_tx.clone());

        let voice_thread = Self::spawn_voice_thread(
            voice_control_rx,
            voice_event_tx,
            network_tx.clone(),
            shared_input_level.clone(),
            shared_output_level.clone(),
            shared_denoised_level.clone(),
            shared_speaking.clone(),
            is_testing_mic_shared.clone(),
        );

        let settings = Settings::load();
        let initial_mute = settings.mute_on_start;

        Self {
            app,
            sender,
            receiver,
            network_tx,
            network_rx,
            voice_tx,
            voice_event_rx,
            users: HashMap::new(),
            our_client_id: None,
            connected: false,
            settings,
            is_muted: initial_mute,
            is_testing_mic: false,
            is_testing_mic_shared,
            connection_window: None,
            main_window: None,
            sidebar: None,
            chat: None,
            error_frame: None,
            settings_window_open: false,
            shared_input_level,
            shared_output_level,
            shared_denoised_level,
            shared_speaking,
            network_thread: Some(network_thread),
            voice_thread: Some(voice_thread),
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
                match self.network_rx.try_recv() {
                    Ok(msg) => {
                        self.handle_network_message(msg);
                        needs_redraw = true;
                    }
                    Err(_) => break,
                }
            }

            // Voice events (bounded drain)
            for _ in 0..32 {
                match self.voice_event_rx.try_recv() {
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
    //  NETWORK THREAD
    // ════════════════════════════════════════════════════════════════════

    fn spawn_network_thread(
        control_rx: Receiver<NetworkControl>,
        msg_tx: Sender<NetworkMessage>,
        voice_tx: Sender<VoiceControl>,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let mut client: Option<NetworkClient> = None;
            let mut running = true;

            while running {
                // Process control messages
                for _ in 0..32 {
                    match control_rx.try_recv() {
                        Ok(ctrl) => match ctrl {
                            NetworkControl::Connect { addr, nickname } => {
                                match NetworkClient::connect(addr, nickname) {
                                    Ok(c) => {
                                        msg_tx.send(NetworkMessage::Connected).ok();
                                        app::awake();
                                        client = Some(c);
                                    }
                                    Err(e) => {
                                        msg_tx.send(NetworkMessage::Disconnected(e)).ok();
                                        app::awake();
                                    }
                                }
                            }
                            NetworkControl::Disconnect => {
                                if let Some(mut c) = client.take() {
                                    c.disconnect();
                                }
                                msg_tx
                                    .send(NetworkMessage::Disconnected("User disconnected".into()))
                                    .ok();
                                app::awake();
                            }
                            NetworkControl::SendMessage(text) => {
                                if let Some(ref mut c) = client {
                                    c.send_message(text);
                                }
                            }
                            NetworkControl::SendVoicePacket(packet) => {
                                if let Some(ref mut c) = client {
                                    c.send_voice_frame(
                                        packet.header.ssrc,
                                        packet.header.seq,
                                        packet.header.timestamp,
                                        &packet.opus_data,
                                    );
                                }
                            }
                            NetworkControl::SendSpeakingStatus(speaking) => {
                                if let Some(ref mut c) = client {
                                    c.send_speaking_status(speaking);
                                }
                            }
                            NetworkControl::Shutdown => {
                                running = false;
                            }
                        },
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            running = false;
                            break;
                        }
                    }
                }

                // Poll network
                if let Some(ref mut c) = client {
                    c.update();

                    for msg in c.poll_messages() {
                        match msg {
                            NetworkMessage::VoiceData {
                                client_id,
                                seq,
                                timestamp,
                                data,
                            } => {
                                let packet = AudioPacket::new(seq, timestamp, client_id, data);
                                let _ =
                                    voice_tx.try_send(VoiceControl::IncomingVoicePacket(packet));
                            }
                            other => {
                                let _ = msg_tx.try_send(other);
                                app::awake();
                            }
                        }
                    }

                    c.flush_voice_packet();

                    if !c.is_connected() && !c.is_connecting() {
                        client = None;
                        msg_tx
                            .send(NetworkMessage::Disconnected("Connection lost".into()))
                            .ok();
                        app::awake();
                    }
                }

                if client.is_some() {
                    thread::sleep(Duration::from_millis(1));
                } else {
                    thread::sleep(Duration::from_millis(10));
                }
            }
        })
    }

    // ════════════════════════════════════════════════════════════════════
    //  VOICE THREAD
    // ════════════════════════════════════════════════════════════════════

    fn spawn_voice_thread(
        control_rx: Receiver<VoiceControl>,
        event_tx: Sender<VoiceEvent>,
        network_tx: Sender<NetworkControl>,
        shared_input_level: Arc<AtomicU32>,
        shared_output_level: Arc<AtomicU32>,
        shared_denoised_level: Arc<AtomicU32>,
        shared_speaking: Arc<AtomicBool>,
        is_testing_mic_shared: Arc<AtomicBool>,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let mut vc: Option<VoiceChat> = None;
            let mut our_client_id: Option<u64> = None;
            let mut was_speaking = false;
            let mut running = true;

            while running {
                select! {
                    recv(control_rx) -> msg => {
                        match msg {
                            Ok(ctrl) => Self::handle_voice_control(
                                ctrl, &mut vc, &mut our_client_id, &mut running,
                                &event_tx, &mut was_speaking,
                            ),
                            Err(_) => { running = false; }
                        }
                    }
                    default(Duration::from_millis(5)) => {}
                }

                // Drain remaining control messages
                for _ in 0..32 {
                    match control_rx.try_recv() {
                        Ok(ctrl) => Self::handle_voice_control(
                            ctrl,
                            &mut vc,
                            &mut our_client_id,
                            &mut running,
                            &event_tx,
                            &mut was_speaking,
                        ),
                        Err(_) => break,
                    }
                }

                // Poll outgoing packets + update levels
                if let Some(voice) = vc.as_ref() {
                    if let Some(id) = our_client_id {
                        if !is_testing_mic_shared.load(Ordering::Relaxed) {
                            for mut packet in voice.take_outgoing_packets() {
                                packet.header.ssrc = id;
                                let _ =
                                    network_tx.try_send(NetworkControl::SendVoicePacket(packet));
                            }
                        } else {
                            // Drain packets but don't send — keeps encoder happy
                            voice.take_outgoing_packets();
                        }
                    }

                    // Update shared levels every cycle
                    shared_input_level.store(voice.input_level().to_bits(), Ordering::Relaxed);
                    shared_output_level.store(voice.output_level().to_bits(), Ordering::Relaxed);
                    shared_denoised_level
                        .store(voice.denoised_level().to_bits(), Ordering::Relaxed);
                    shared_speaking.store(voice.is_speaking(), Ordering::Relaxed);

                    // Speaking state (edge-triggered)
                    let speaking = voice.is_speaking();
                    if speaking != was_speaking {
                        was_speaking = speaking;
                        let _ = network_tx.try_send(NetworkControl::SendSpeakingStatus(speaking));
                        let _ = event_tx.try_send(VoiceEvent::SpeakingChanged(speaking));
                        app::awake();
                    }
                }
            }
        })
    }

    fn handle_voice_control(
        ctrl: VoiceControl,
        vc: &mut Option<VoiceChat>,
        our_client_id: &mut Option<u64>,
        running: &mut bool,
        event_tx: &Sender<VoiceEvent>,
        was_speaking: &mut bool,
    ) {
        match ctrl {
            VoiceControl::Start {
                settings,
                our_client_id: id,
            } => {
                let config = settings_to_config(&settings);
                match VoiceChat::new(config) {
                    Ok(mut voice) => {
                        if let Err(e) = voice.start() {
                            event_tx
                                .send(VoiceEvent::Error(format!("Start failed: {}", e)))
                                .ok();
                            app::awake();
                            return;
                        }
                        if settings.mute_on_start {
                            voice.set_muted(true);
                        }
                        *our_client_id = Some(id);
                        *vc = Some(voice);
                        *was_speaking = false;
                        event_tx.send(VoiceEvent::Started).ok();
                        app::awake();
                        info!("Voice started for client {}", id);
                    }
                    Err(e) => {
                        event_tx
                            .send(VoiceEvent::Error(format!("Init failed: {}", e)))
                            .ok();
                        app::awake();
                    }
                }
            }
            VoiceControl::Stop => {
                *vc = None;
                *our_client_id = None;
                *was_speaking = false;
                info!("Voice stopped");
            }
            VoiceControl::SetMuted(muted) => {
                if let Some(v) = vc.as_ref() {
                    v.set_muted(muted);
                }
            }
            VoiceControl::SetLoopback(enabled) => {
                if let Some(v) = vc.as_ref() {
                    v.set_loopback(enabled);
                }
            }
            VoiceControl::SetGateThreshold(t) => {
                if let Some(v) = vc.as_ref() {
                    v.set_gate_threshold(t);
                }
            }
            VoiceControl::SetInputGain(g) => {
                if let Some(v) = vc.as_ref() {
                    v.set_input_gain(g);
                }
            }
            VoiceControl::SetOutputGain(g) => {
                if let Some(v) = vc.as_ref() {
                    v.set_output_gain(g);
                }
            }
            VoiceControl::SetInputDevice(d) => {
                if let Some(v) = vc.as_mut()
                    && let Err(e) = v.set_input_device(d.as_deref())
                {
                    event_tx
                        .send(VoiceEvent::Error(format!("Input device: {}", e)))
                        .ok();
                    app::awake();
                }
            }
            VoiceControl::SetOutputDevice(d) => {
                if let Some(v) = vc.as_mut()
                    && let Err(e) = v.set_output_device(d.as_deref())
                {
                    event_tx
                        .send(VoiceEvent::Error(format!("Output device: {}", e)))
                        .ok();
                    app::awake();
                }
            }
            VoiceControl::AddPeer(id) => {
                if let Some(v) = vc.as_mut() {
                    v.add_peer(id);
                }
            }
            VoiceControl::RemovePeer(id) => {
                if let Some(v) = vc.as_mut() {
                    v.remove_peer(id);
                }
            }
            VoiceControl::SetPeerAudio(id, settings) => {
                if let Some(v) = vc.as_ref() {
                    v.set_peer_settings(id, settings);
                }
            }
            VoiceControl::IncomingVoicePacket(packet) => {
                if let Some(v) = vc.as_ref() {
                    v.receive_packet(packet);
                }
            }
            VoiceControl::Shutdown => {
                *running = false;
            }
        }
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
                self.network_tx.send(NetworkControl::SendMessage(text)).ok();
            }
            UiMessage::ToggleMute => {
                self.handle_toggle_mute();
            }
            UiMessage::TestMicrophone(enabled) => {
                self.handle_test_microphone(enabled);
            }
            UiMessage::SetGateThreshold(t) => {
                self.settings.gate_threshold = t;
                self.voice_tx.send(VoiceControl::SetGateThreshold(t)).ok();
            }
            UiMessage::SetInputGain(g) => {
                self.settings.input_gain = g;
                self.voice_tx.send(VoiceControl::SetInputGain(g)).ok();
            }
            UiMessage::SetOutputGain(g) => {
                self.settings.output_gain = g;
                self.voice_tx.send(VoiceControl::SetOutputGain(g)).ok();
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
                self.settings = Settings::default();
                self.settings.save();
            }
            UiMessage::SetInputDevice(d) => {
                self.settings.input_device = d.clone();
                self.voice_tx.send(VoiceControl::SetInputDevice(d)).ok();
            }
            UiMessage::SetOutputDevice(d) => {
                self.settings.output_device = d.clone();
                self.voice_tx.send(VoiceControl::SetOutputDevice(d)).ok();
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
                // Stop test mic if settings window closed while testing
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

        self.network_tx
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
                warn!("Could not match nickname, assuming client {}", id);
            }
        }

        if let Some(id) = self.our_client_id {
            self.voice_tx
                .send(VoiceControl::Start {
                    settings: self.settings.clone(),
                    our_client_id: id,
                })
                .ok();

            self.is_muted = self.settings.mute_on_start;
        }

        self.users.clear();
        for (id, username) in &users {
            let audio = self
                .settings
                .user_audio_settings
                .get(username)
                .cloned()
                .unwrap_or_default();

            self.users.insert(
                *id,
                UserState {
                    username: username.clone(),
                    is_speaking: false,
                    last_activity: Instant::now(),
                    audio_settings: audio,
                },
            );

            self.voice_tx.send(VoiceControl::AddPeer(*id)).ok();
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
            .get(&username)
            .cloned()
            .unwrap_or_default();

        self.users.insert(
            client_id,
            UserState {
                username: username.clone(),
                is_speaking: false,
                last_activity: Instant::now(),
                audio_settings: audio,
            },
        );

        self.voice_tx.send(VoiceControl::AddPeer(client_id)).ok();

        if let Some(chat) = &mut self.chat {
            chat.handle_user_joined(username);
        }
        self.update_user_list();
    }

    fn handle_user_left(&mut self, client_id: renet::ClientId, username: String) {
        self.users.remove(&client_id);
        self.voice_tx.send(VoiceControl::RemovePeer(client_id)).ok();

        if let Some(chat) = &mut self.chat {
            chat.handle_user_left(username);
        }
        self.update_user_list();
    }

    fn handle_toggle_mute(&mut self) {
        self.is_muted = !self.is_muted;
        self.voice_tx
            .send(VoiceControl::SetMuted(self.is_muted))
            .ok();

        if let Some(sidebar) = &mut self.sidebar {
            sidebar.set_mute_state(self.is_muted);
        }

        if self.is_muted {
            self.network_tx
                .send(NetworkControl::SendSpeakingStatus(false))
                .ok();
        }
    }

    fn handle_test_microphone(&mut self, enabled: bool) {
        self.is_testing_mic = enabled;
        self.is_testing_mic_shared.store(enabled, Ordering::Relaxed);
        self.voice_tx.send(VoiceControl::SetLoopback(enabled)).ok();
        if enabled {
            self.network_tx
                .send(NetworkControl::SendSpeakingStatus(false))
                .ok();
        }
    }

    fn handle_mute_user(&mut self, client_id: renet::ClientId, username: String, muted: bool) {
        let settings = self
            .settings
            .user_audio_settings
            .entry(username)
            .or_default();
        settings.is_muted = muted;

        if let Some(user) = self.users.get_mut(&client_id) {
            user.audio_settings.is_muted = muted;
        }

        self.voice_tx
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
        let settings = self
            .settings
            .user_audio_settings
            .entry(username)
            .or_default();
        settings.volume = volume;

        if let Some(user) = self.users.get_mut(&client_id) {
            user.audio_settings.volume = volume;
        }

        self.voice_tx
            .send(VoiceControl::SetPeerAudio(
                client_id,
                to_peer_settings(settings),
            ))
            .ok();
        self.settings.save();
        self.update_user_list();
    }

    fn handle_reset_user_audio(&mut self, client_id: renet::ClientId, username: String) {
        self.settings.user_audio_settings.remove(&username);

        if let Some(user) = self.users.get_mut(&client_id) {
            user.audio_settings = UserAudioSettings::default();
        }

        self.voice_tx
            .send(VoiceControl::SetPeerAudio(
                client_id,
                to_peer_settings(&UserAudioSettings::default()),
            ))
            .ok();
        self.settings.save();
        self.update_user_list();
    }

    fn handle_disconnect(&mut self) {
        self.connected = false;
        self.is_testing_mic = false;
        self.network_tx.send(NetworkControl::Disconnect).ok();
        self.voice_tx.send(VoiceControl::Stop).ok();

        if let Some(sidebar) = &mut self.sidebar {
            sidebar.handle_disconnected();
        }
        if let Some(chat) = &mut self.chat {
            chat.handle_disconnected("User disconnected".to_string());
        }

        self.users.clear();
        self.our_client_id = None;
        self.is_muted = false;

        if let Some(mut w) = self.main_window.take() {
            w.hide();
        }
        self.show_connection_screen();
    }

    fn handle_disconnect_event(&mut self, reason: &str) {
        self.connected = false;
        self.is_testing_mic = false;
        self.voice_tx.send(VoiceControl::Stop).ok();

        if let Some(sidebar) = &mut self.sidebar {
            sidebar.handle_disconnected();
        }
        if let Some(chat) = &mut self.chat {
            chat.handle_disconnected(reason.to_string());
        }

        self.users.clear();
        self.our_client_id = None;

        if let Some(mut w) = self.main_window.take() {
            w.hide();
        }
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
        DarkTheme::style_window(&mut wind);
        Self::set_window_icon(&mut wind);

        let mut col = group::Flex::default_fill().column();
        col.set_margins(20, 20, 20, 20);
        col.set_spacing(15);

        let mut title = frame::Frame::default()
            .with_label("baucord")
            .with_align(enums::Align::Center);
        title.set_label_size(32);
        title.set_label_color(DarkTheme::TEXT_PRIMARY);
        col.fixed(&title, 50);

        let mut subtitle = frame::Frame::default()
            .with_label("We have discord at home")
            .with_align(enums::Align::Center);
        subtitle.set_label_size(12);
        subtitle.set_label_color(DarkTheme::TEXT_SECONDARY);
        col.fixed(&subtitle, 20);

        let mut nick_frame = frame::Frame::default()
            .with_label("Nickname")
            .with_align(enums::Align::Left | enums::Align::Inside);
        nick_frame.set_label_color(DarkTheme::TEXT_SECONDARY);
        nick_frame.set_label_size(11);
        col.fixed(&nick_frame, 20);

        let mut nick_input = input::Input::default();
        nick_input.set_value(&self.settings.nickname);
        DarkTheme::style_input(&mut nick_input);
        col.fixed(&nick_input, 35);

        let mut addr_frame = frame::Frame::default()
            .with_label("Server Address")
            .with_align(enums::Align::Left | enums::Align::Inside);
        addr_frame.set_label_color(DarkTheme::TEXT_SECONDARY);
        addr_frame.set_label_size(11);
        col.fixed(&addr_frame, 20);

        let mut addr_input = input::Input::default();
        addr_input.set_value(&self.settings.last_server);
        DarkTheme::style_input(&mut addr_input);
        col.fixed(&addr_input, 35);

        let mut error_frame = frame::Frame::default().with_align(enums::Align::Center);
        error_frame.set_label_color(DarkTheme::DANGER);
        error_frame.set_label_size(12);
        col.fixed(&error_frame, 25);
        self.error_frame = Some(error_frame);

        let mut connect_btn = button::Button::default().with_label("Connect");
        connect_btn.set_color(DarkTheme::ACCENT);
        connect_btn.set_label_color(enums::Color::White);
        connect_btn.set_frame(enums::FrameType::FlatBox);
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
        DarkTheme::style_window(&mut wind);
        Self::set_window_icon(&mut wind);

        if let (Some(x), Some(y)) = (self.settings.window_x, self.settings.window_y) {
            wind.set_pos(x, y);
        }

        let mut main_row = group::Flex::default_fill().row();
        main_row.set_color(DarkTheme::BG_PRIMARY);

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

        let input_devices = VoiceChat::list_input_devices().unwrap_or_default();
        let output_devices = VoiceChat::list_output_devices().unwrap_or_default();

        SettingsWindow::show(
            self.sender,
            self.settings.clone(),
            input_devices,
            output_devices,
            self.shared_input_level.clone(),
            self.shared_output_level.clone(),
            self.shared_denoised_level.clone(),
            self.shared_speaking.clone(),
        );
    }

    fn show_error(&mut self, error: &str) {
        if let Some(frame) = &mut self.error_frame {
            frame.set_label(error);
            frame.set_label_color(DarkTheme::DANGER);
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

        self.network_tx.send(NetworkControl::Shutdown).ok();
        self.voice_tx.send(VoiceControl::Shutdown).ok();
    }
}
