// app.rs - Complete channel-based architecture with fixes
use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded};
use fltk::{prelude::*, *};
use log::{debug, info, warn};
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::network::{NetworkClient, NetworkMessage};
use crate::settings::{Settings, UserAudioSettings};
use crate::ui::chat::Chat;
use crate::ui::messages::UiMessage;
use crate::ui::sidebar::Sidebar;
use crate::ui::theme::DarkTheme;
use crate::voice::{AudioPacket, VoiceChat};

pub const VOICE_PACKET_BATCH_THRESHOLD: usize = 2;
pub const VOICE_MAX_FRAMES_PER_TICK: usize = 4; // Max 40ms per tick
pub const VOICE_MAX_BYTES_PER_TICK: usize = 4000; // Max 4KB per tick

#[derive(Clone)]
pub struct UserState {
    pub username: String,
    pub is_speaking: bool,
    pub last_activity: Instant,
    pub audio_settings: UserAudioSettings,
}

// Network control messages
#[derive(Debug, Clone)]
enum NetworkControl {
    Connect { addr: SocketAddr, nickname: String },
    Disconnect,
    SendMessage(String),
    SendVoicePacket(AudioPacket),
    SendSpeakingStatus(bool),
    Shutdown,
}

// Voice control messages
#[derive(Debug, Clone)]
enum VoiceControl {
    Start(Settings),
    Stop,
    SetMuted(bool),
    SetLoopback(bool),
    SetVadEnabled(bool),
    SetInputDevice(Option<String>),
    SetOutputDevice(Option<String>),
    SetOurClientId(renet::ClientId), // Set our client ID for voice packets
    GetDeviceList,
    GetStats,
    AddClient(renet::ClientId),
    RemoveClient(renet::ClientId),
    SetUserAudio(renet::ClientId, UserAudioSettings),
    IncomingVoicePacket(AudioPacket),
    Shutdown,
}

// Voice responses
#[derive(Debug, Clone)]
enum VoiceResponse {
    DeviceList {
        input: Vec<(String, bool)>,
        output: Vec<(String, bool)>,
    },
    Stats(String),
    DeviceInfo(String),
    Error(String),
}

pub struct App {
    app: app::App,
    sender: app::Sender<UiMessage>,
    receiver: app::Receiver<UiMessage>,

    // Channels for network thread
    network_tx: Sender<NetworkControl>,
    network_rx: Receiver<NetworkMessage>,

    // Channels for voice system
    voice_tx: Sender<VoiceControl>,
    voice_response_rx: Receiver<VoiceResponse>,
    voice_packet_rx: Receiver<AudioPacket>,

    // Application state (UI thread owns this)
    users: HashMap<renet::ClientId, UserState>,
    our_client_id: Option<renet::ClientId>,
    connected: bool,
    settings: Settings,
    is_muted: bool, // Track current mute state

    // UI components
    connection_window: Option<window::Window>,
    main_window: Option<window::Window>,
    sidebar: Option<Sidebar>,
    chat: Option<Chat>,
    error_frame: Option<frame::Frame>,

    // Track if settings
    settings_window_open: bool,

    // Thread handles
    network_thread: Option<thread::JoinHandle<()>>,
    voice_thread: Option<thread::JoinHandle<()>>,
    bridge_thread: Option<thread::JoinHandle<()>>,
    voice_drops: Option<Arc<AtomicU64>>,
    network_drops: Option<Arc<AtomicU64>>,
}

impl App {
    pub fn new() -> Self {
        let app = app::App::default();
        DarkTheme::apply_to_app();

        let (sender, receiver) = app::channel::<UiMessage>();

        // Create channels for network thread
        // Use bounded channels to prevent runaway accumulation
        let (network_tx, network_control_rx) = bounded::<NetworkControl>(256); // Control messages can queue a bit
        let (network_msg_tx, network_rx) = bounded::<NetworkMessage>(256); // UI messages

        // Create channels for voice system
        let (voice_tx, voice_control_rx) = bounded::<VoiceControl>(64); // Control is less frequent
        let (voice_response_tx, voice_response_rx) = bounded::<VoiceResponse>(64);
        let (voice_packet_tx, voice_packet_rx) = bounded::<AudioPacket>(32); // CRITICAL: Keep small for low latency

        // Add stats for dropped packets
        let voice_drops = Arc::new(AtomicU64::new(0));
        let voice_drops_handle = voice_drops.clone();

        // Start network thread (owns NetworkClient)
        let network_thread = Self::start_network_thread(network_control_rx, network_msg_tx, voice_tx.clone());

        let voice_thread = Self::start_voice_thread(
            voice_control_rx,
            voice_response_tx,
            voice_packet_tx.clone(),
            voice_drops.clone(),
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
            voice_response_rx,
            voice_packet_rx,
            users: HashMap::new(),
            our_client_id: None,
            connected: false,
            settings,
            is_muted: initial_mute,
            connection_window: None,
            main_window: None,
            sidebar: None,
            chat: None,
            error_frame: None,
            settings_window_open: false,
            network_thread: Some(network_thread),
            voice_thread: Some(voice_thread),
            bridge_thread: None,
            voice_drops: Some(Arc::new(AtomicU64::new(0))),
            network_drops: Some(Arc::new(AtomicU64::new(0))),
        }
    }

   pub fn run(&mut self) {
        self.show_connection_screen();

        // Main event loop - only handles UI messages
        while self.app.wait() {
            // Process a single UI message per tick (non-blocking)
            if let Some(msg) = self.receiver.recv() {
                self.handle_message(msg);
            }

            // Drain a bounded number of network messages per tick to avoid UI starvation
            const MAX_NET_PER_TICK: usize = 256;
            for _ in 0..MAX_NET_PER_TICK {
                match self.network_rx.try_recv() {
                    Ok(network_msg) => self.handle_network_message(network_msg),
                    Err(_) => break,
                }
            }

            // Drain a bounded number of voice responses
            const MAX_VOICE_RESP_PER_TICK: usize = 64;
            for _ in 0..MAX_VOICE_RESP_PER_TICK {
                match self.voice_response_rx.try_recv() {
                    Ok(resp) => self.handle_voice_response(resp),
                    Err(_) => break,
                }
            }
        }

        // Cleanup on exit
        self.shutdown();
    } 

fn start_network_thread(
        control_rx: Receiver<NetworkControl>,
        msg_tx: Sender<NetworkMessage>,
        voice_tx: Sender<VoiceControl>,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let mut client: Option<NetworkClient> = None;
            let mut running = true;
            let mut voice_packet_buffer = VecDeque::with_capacity(8); // Small ring, preserves order

            let mut frames_sent_this_tick = 0usize;
            let mut bytes_sent_this_tick = 0usize;
            let mut next_budget_reset = Instant::now() + Duration::from_millis(10);
            while running {
                // Time-based budget reset every 10ms
                let now_tick = Instant::now();
                if now_tick >= next_budget_reset {
                    // Advance by multiples of 10ms to avoid drift
                    let mut next = next_budget_reset;
                    while next <= now_tick {
                        next += Duration::from_millis(10);
                    }
                    next_budget_reset = next;
                }

                // Process control messages with a limit per iteration
                let mut controls_processed = 0;
                const MAX_CONTROLS_PER_TICK: usize = 10;
                frames_sent_this_tick = 0;
                bytes_sent_this_tick = 0;

                while controls_processed < MAX_CONTROLS_PER_TICK {
                    match control_rx.try_recv() {
                        Ok(control) => {
                            controls_processed += 1;
                            match control {
                                NetworkControl::Connect { addr, nickname } => {
                                    match NetworkClient::connect(addr, nickname) {
                                        Ok(c) => {
                                            msg_tx.send(NetworkMessage::Connected).ok();
                                            client = Some(c);
                                        }
                                        Err(e) => {
                                            msg_tx.send(NetworkMessage::Disconnected(e)).ok();
                                        }
                                    }
                                }
                                NetworkControl::Disconnect => {
                                    if let Some(mut c) = client.take() {
                                        c.disconnect();
                                    }
                                    msg_tx
                                        .send(NetworkMessage::Disconnected(
                                            "User requested".to_string(),
                                        ))
                                        .ok();
                                }
                                NetworkControl::SendMessage(text) => {
                                    if let Some(ref mut c) = client {
                                        c.send_message(text);
                                    }
                                }
                                NetworkControl::SendVoicePacket(packet) => {
                                    // CHECK PER-TICK LIMITS BEFORE BUFFERING
                                    if frames_sent_this_tick >= VOICE_MAX_FRAMES_PER_TICK {
                                        // Hit frame limit for this tick, drop or defer
                                        debug!("Voice frame limit hit for tick, deferring");
                                        voice_packet_buffer.insert(0, packet); // Save for next tick
                                        continue;
                                    }

                                    if bytes_sent_this_tick >= VOICE_MAX_BYTES_PER_TICK {
                                        // Hit byte limit for this tick
                                        debug!("Voice byte limit hit for tick, deferring");
                                        voice_packet_buffer.push_back(packet); // defer to next tick, keep order
                                        continue;
                                    }

                                    // Add to buffer but keep it small
                                    if voice_packet_buffer.len() >= 4 {
                                        voice_packet_buffer.pop_front(); // Drop oldest, keep order
                                    }
                                    voice_packet_buffer.push_back(packet);

                                    // FLUSH AT THRESHOLD (2 frames)
                                    if voice_packet_buffer.len() >= VOICE_PACKET_BATCH_THRESHOLD
                                        && let Some(ref mut c) = client {
                                            let mut sent_now = 0usize;
                                            while sent_now < VOICE_PACKET_BATCH_THRESHOLD {
                                                if let Some(p) = voice_packet_buffer.pop_front() {
                                                    let sz = p.opus_data.len() + 24;
                                                    if bytes_sent_this_tick + sz
                                                        > VOICE_MAX_BYTES_PER_TICK
                                                        || frames_sent_this_tick
                                                            >= VOICE_MAX_FRAMES_PER_TICK
                                                    {
                                                        // Defer for next tick, put back to front to preserve order
                                                        voice_packet_buffer.push_front(p);
                                                        break;
                                                    }
                                                    frames_sent_this_tick += 1;
                                                    bytes_sent_this_tick += sz;
                                                    c.send_voice_frame_raw(
                                                        p.header.ssrc,
                                                        p.header.timestamp,
                                                        &p.opus_data,
                                                    );
                                                    sent_now += 1;
                                                } else {
                                                    break;
                                                }
                                            }
                                            if sent_now > 0 {
                                                c.flush_voice_packet();
                                            }
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
                            }
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            running = false;
                            break;
                        }
                    }
                }

                if !voice_packet_buffer.is_empty()
                    && frames_sent_this_tick < VOICE_MAX_FRAMES_PER_TICK
                    && let Some(ref mut c) = client {
                        // Send up to remaining budget in order (FIFO)
                        let can_send = (VOICE_MAX_FRAMES_PER_TICK - frames_sent_this_tick)
                            .min(voice_packet_buffer.len());
                        let mut sent_now = 0usize;
                        while sent_now < can_send {
                            if let Some(p) = voice_packet_buffer.pop_front() {
                                let size = p.opus_data.len() + 24;
                                if bytes_sent_this_tick + size <= VOICE_MAX_BYTES_PER_TICK {
                                    bytes_sent_this_tick += size;
                                    frames_sent_this_tick += 1;
                                    c.send_voice_frame_raw(
                                        p.header.ssrc,
                                        p.header.timestamp,
                                        &p.opus_data,
                                    );
                                    sent_now += 1;
                                } else {
                                    voice_packet_buffer.push_front(p);
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                        if sent_now > 0 {
                            c.flush_voice_packet();
                        }
                    }

                // Poll network client if connected
                if let Some(ref mut c) = client {
                    c.update();

                    // Get messages and forward to UI or voice thread directly
                    for msg in c.poll_messages() {
                        match msg {
                            NetworkMessage::VoiceData { client_id, seq, timestamp, data } => {
                                let packet = AudioPacket::from_network(seq, timestamp, client_id, data);
                                // Send directly to voice thread; drop if full to keep real-time
                                let _ = voice_tx.try_send(VoiceControl::IncomingVoicePacket(packet));
                            }
                            other => {
                                if msg_tx.try_send(other).is_err() {
                                    // UI channel full, drop old messages
                                    break;
                                }
                            }
                        }
                    }

                    // Check if still connected
                    if !c.is_connected() && !c.is_connecting() {
                        client = None;
                        msg_tx
                            .send(NetworkMessage::Disconnected("Connection lost".to_string()))
                            .ok();
                    }
                }

                // Tick-boundary flush: ensure partial packet doesn't linger forever
                if let Some(ref mut c) = client {
                    c.flush_voice_packet();
                }

                // Sleep only if we have no work
                if controls_processed == 0 && voice_packet_buffer.is_empty() {
                    thread::sleep(Duration::from_millis(2));
                } else if voice_packet_buffer.len() > 2 {
                    // If voice buffer is building up, don't sleep
                    thread::yield_now();
                } else {
                    thread::sleep(Duration::from_millis(1));
                }
            }
        })
    }
    fn start_voice_thread(
        control_rx: Receiver<VoiceControl>,
        response_tx: Sender<VoiceResponse>,
        packet_tx: Sender<AudioPacket>,
        voice_drops: Arc<AtomicU64>,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let mut voice_chat: Option<VoiceChat> = None;
            let mut our_client_id: Option<renet::ClientId> = None;
            let mut running = true;
            let mut voice_started = false;

            while running {
                // Handle control messages
                while let Ok(control) = control_rx.try_recv() {
                    match control {
                        VoiceControl::Start(settings) => {
                            if !voice_started {
                                match VoiceChat::new() {
                                    Ok(mut vc) => {
                                        if let Err(e) = vc.start(&settings) {
                                            response_tx
                                                .send(VoiceResponse::Error(format!(
                                                    "Failed to start voice: {}",
                                                    e
                                                )))
                                                .ok();
                                        } else {
                                            // Apply saved settings
                                            if settings.loopback_enabled {
                                                vc.set_loopback(true);
                                            }
                                            if settings.mute_on_start {
                                                vc.set_muted(true);
                                            }
                                            voice_chat = Some(vc);
                                            voice_started = true;
                                            info!("✅ Voice system started");
                                        }
                                    }
                                    Err(e) => {
                                        response_tx
                                            .send(VoiceResponse::Error(format!(
                                                "Failed to create voice: {}",
                                                e
                                            )))
                                            .ok();
                                    }
                                }
                            }
                        }
                        VoiceControl::Stop => {
                            voice_chat = None;
                            voice_started = false;
                            info!("🛑 Voice system stopped");
                        }
                        VoiceControl::SetOurClientId(id) => {
                            our_client_id = Some(id);
                            info!("🎤 Voice thread: Set client ID to {}", id);
                        }
                        VoiceControl::SetMuted(muted) => {
                            if let Some(ref mut vc) = voice_chat {
                                vc.set_muted(muted);
                            }
                        }
                        VoiceControl::SetLoopback(enabled) => {
                            if let Some(ref mut vc) = voice_chat {
                                vc.set_loopback(enabled);
                            }
                        }
                        VoiceControl::SetVadEnabled(enabled) => {
                            // TODO: Add VAD control to VoiceChat
                        }
                        VoiceControl::SetInputDevice(device) => {
                            if let Some(ref mut vc) = voice_chat
                                && let Err(e) = vc.set_input_device(device.as_deref()) {
                                    response_tx
                                        .send(VoiceResponse::Error(format!(
                                            "Failed to set input: {}",
                                            e
                                        )))
                                        .ok();
                                }
                        }
                        VoiceControl::SetOutputDevice(device) => {
                            if let Some(ref mut vc) = voice_chat
                                && let Err(e) = vc.set_output_device(device.as_deref()) {
                                    response_tx
                                        .send(VoiceResponse::Error(format!(
                                            "Failed to set output: {}",
                                            e
                                        )))
                                        .ok();
                                }
                        }
                        VoiceControl::GetDeviceList => {
                            if let Some(ref vc) = voice_chat {
                                let input = vc.get_input_devices().unwrap_or_default();
                                let output = vc.get_output_devices().unwrap_or_default();
                                response_tx
                                    .send(VoiceResponse::DeviceList { input, output })
                                    .ok();
                            }
                        }
                        VoiceControl::GetStats => {
                            if let Some(ref vc) = voice_chat {
                                response_tx.send(VoiceResponse::Stats(vc.get_stats())).ok();
                            }
                        }
                     
                       
                        VoiceControl::AddClient(client_id) => {
                            if let Some(ref mut vc) = voice_chat {
                                vc.add_client(client_id);
                            }
                        }
                        VoiceControl::RemoveClient(client_id) => {
                            if let Some(ref mut vc) = voice_chat {
                                vc.remove_client(client_id);
                            }
                        }
                        VoiceControl::SetUserAudio(client_id, settings) => {
                            if let Some(ref mut vc) = voice_chat {
                                vc.set_user_audio_settings(client_id, settings);
                            }
                        }
                        VoiceControl::IncomingVoicePacket(packet) => {
                            if let Some(ref mut vc) = voice_chat {
                                vc.receive_packet(packet);
                            }
                        }
                        VoiceControl::Shutdown => {
                            running = false;
                        }
                    }
                }

                // Poll outgoing voice packets (only if we have a client ID)
                if let Some(ref mut vc) = voice_chat {
                    if let Some(id) = our_client_id {
                        let packets = vc.get_outgoing_packets();
                        if !packets.is_empty() {
                            for mut packet in packets {
                                // Set our client ID in outgoing packets
                                packet.header.ssrc = id;
                                // Set our client ID in outgoing packets
                                packet.header.ssrc = id;
                                // Use try_send to avoid blocking
                                match packet_tx.try_send(packet) {
                                    Ok(_) => {
                                        // Success
                                    }
                                    Err(crossbeam_channel::TrySendError::Full(_)) => {
                                        debug!("Voice packet channel full, dropping packet");
                                        voice_drops.fetch_add(1, Ordering::Relaxed); // Track the drop
                                    }
                                    Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                                        // Shutdown
                                        return;
                                    }
                                }
                            }
                        }
                    }
                    // If no client ID yet, discard packets
                    else {
                        let _ = vc.get_outgoing_packets(); // Drain but don't send
                    }

                    // Check if speaking state changed
                    // TODO: Send speaking status updates
                }

                thread::sleep(Duration::from_millis(5)); // Poll less frequently
            }
        })
    }

   fn start_message_bridge(&mut self) {
    // Don't start if already running
    if self.bridge_thread.is_some() {
        return;
    }

    // Bridge voice packets -> network with pacing + speaking state
    let network_tx = self.network_tx.clone();
    let voice_packet_rx = self.voice_packet_rx.clone();

    self.bridge_thread = Some(std::thread::spawn(move || {
        use std::collections::VecDeque;
        use std::time::{Duration, Instant};
        use crossbeam_channel::{TryRecvError, TrySendError};

        // Tiny FIFO buffer (drop-oldest) ~40ms max
        let mut packet_buffer: VecDeque<AudioPacket> = VecDeque::with_capacity(4);

        // VAD-driven speaking indicator:
        //   - Raise "speaking=true" on first packet after silence
        //   - Drop to false if we see a gap > 200ms (no packets)
        let mut currently_speaking = false;
        let mut last_audio = Instant::now().checked_sub(Duration::from_secs(10)).unwrap();

        // Stats (optional)
        let mut packet_count = 0u64;
        let mut drops = 0u64;
        let mut last_stats = Instant::now();
        let stats_interval = Duration::from_secs(10);

        loop {
            // 1) Ingest from voice thread (non-blocking)
            loop {
                match voice_packet_rx.try_recv() {
                    Ok(packet) => {
                        // buffer with drop-oldest
                        if packet_buffer.len() >= 4 {
                            packet_buffer.pop_front();
                            drops += 1;
                        }
                        packet_buffer.push_back(packet);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }

            // 2) If we have packets, mark speaking=TRUE (edge)
            if !packet_buffer.is_empty() && !currently_speaking {
                currently_speaking = true;
                let _ = network_tx.try_send(NetworkControl::SendSpeakingStatus(true));
            }

            // 3) Send up to 2 packets per tick (preserve order)
            let mut sent_now = 0usize;
            while sent_now < 2 {
                if let Some(packet) = packet_buffer.pop_front() {
                    match network_tx.try_send(NetworkControl::SendVoicePacket(packet)) {
                        Ok(_) => {
                            sent_now += 1;
                            packet_count += 1;
                            last_audio = Instant::now();
                        }
                        Err(TrySendError::Full(NetworkControl::SendVoicePacket(p))) => {
                            // Re-queue newest at the back, drop oldest behavior already applied
                            // This packet is already old; drop it.
                            let _ = p; // drop
                            drops += 1;
                            break;
                        }
                        Err(TrySendError::Disconnected(_)) => return,
                        _ => break,
                    }
                } else {
                    break;
                }
            }

            // 4) If no audio for > 200ms, mark speaking=FALSE once
            if currently_speaking && last_audio.elapsed() > Duration::from_millis(200) {
                currently_speaking = false;
                let _ = network_tx.try_send(NetworkControl::SendSpeakingStatus(false));
            }

            // 5) Light telemetry (optional)
            if last_stats.elapsed() >= stats_interval {
                last_stats = Instant::now();
                log::debug!(
                    "Bridge: sent={} dropped={} buffer={}",
                    packet_count,
                    drops,
                    packet_buffer.len()
                );
                packet_count = 0;
                drops = 0;
            }

            // 6) Sleep a bit to yield
            if sent_now == 0 && packet_buffer.is_empty() {
                std::thread::sleep(Duration::from_millis(2));
            } else {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }));
}

   fn handle_network_message(&mut self, msg: NetworkMessage) {
        use NetworkMessage::*;

        match msg {
            Connected => {
                self.sender.send(UiMessage::ConnectionEstablished);
            }
            Disconnected(reason) => {
                self.sender.send(UiMessage::ConnectionFailed(reason));
            }
            InitClient {
                users,
                server_name,
                greeting,
            } => {
                self.sender.send(UiMessage::InitClient {
                    users,
                    server_name,
                    greeting,
                });
            }
            UserJoined { client_id, username } => {
                self.sender.send(UiMessage::UserJoined { client_id, username });
            }
            UserLeft { client_id, username } => {
                self.sender.send(UiMessage::UserLeft { client_id, username });
            }
            TextMessage { client_id: _, username, text } => {
                self.sender.send(UiMessage::ReceiveChatMessage { username, text });
            }
            VoiceData { .. } => {
                // Ignored here: handled directly by voice thread in network loop
            }
            UserSpeaking { client_id, is_speaking } => {
                self.sender.send(UiMessage::UserSpeaking { client_id, is_speaking });
            }
            UiUpdate => {
                // Periodic update already handled by timeout
            }
        }
    } 

    fn handle_voice_response(&mut self, resp: VoiceResponse) {
        use VoiceResponse::*;

        match resp {
            DeviceList { input, output } => {
                // Update device lists in settings window if open
                // For now, just log it
                info!(
                    "Device list received: {} input, {} output devices",
                    input.len(),
                    output.len()
                );
            }
            Error(err) => {
                self.sender.send(UiMessage::ShowError(err));
            }
            Stats(_) => {},
            DeviceInfo(_) => {},
        }
    }

    fn handle_message(&mut self, msg: UiMessage) {
        use UiMessage::*;

        match msg {
            Connect { nickname, server } => {
                self.handle_connect(nickname, server);
            }
            ConnectionEstablished => {
                self.handle_connection_established();
            }
            ConnectionFailed(error) => {
                self.show_error(&error);
            }
            Disconnect => {
                self.handle_disconnect();
            }
            InitClient {
                users,
                server_name,
                greeting,
            } => {
                self.handle_init_client(users, server_name, greeting);
            }
            UserJoined {
                client_id,
                username,
            } => {
                self.handle_user_joined(client_id, username);
            }
            UserLeft {
                client_id,
                username,
            } => {
                self.handle_user_left(client_id, username);
            }
            UserSpeaking {
                client_id,
                is_speaking,
            } => {
                self.handle_user_speaking(client_id, is_speaking);
            }
            SendChatMessage(text) => {
                self.network_tx.send(NetworkControl::SendMessage(text)).ok();
            }
            ReceiveChatMessage { username, text } => {
                self.handle_receive_chat_message(username, text);
            }
            ToggleMute => {
                self.handle_toggle_mute();
            }
            SetLoopback(enabled) => {
                self.settings.loopback_enabled = enabled;
                self.voice_tx.send(VoiceControl::SetLoopback(enabled)).ok();
            }
            SetVadThreshold(threshold) => {
                self.settings.vad_threshold = threshold;
                // TODO: Send to voice
            }
            SetVadEnabled(enabled) => {
                self.voice_tx
                    .send(VoiceControl::SetVadEnabled(enabled))
                    .ok();
            }
            MuteUser {
                client_id,
                username,
                muted,
            } => {
                self.handle_mute_user(client_id, username, muted);
            }
            SetUserVolume {
                client_id,
                username,
                volume,
            } => {
                self.handle_set_user_volume(client_id, username, volume);
            }
            ResetUserAudio {
                client_id,
                username,
            } => {
                self.handle_reset_user_audio(client_id, username);
            }
            OpenSettings => {
                if !self.settings_window_open {
                    self.settings_window_open = true;
                    self.show_settings_window();
                }
            }
 

            SaveSettings => {
                self.settings.save();
                info!("Settings saved");
            }
            ResetSettings => {
                self.settings = Settings::default();
                self.settings.save();
                info!("Settings reset to defaults");
            }
            RefreshDevices => {
                self.voice_tx.send(VoiceControl::GetDeviceList).ok();
            }
            SetInputDevice(device) => {
                self.settings.input_device = device.clone();
                self.voice_tx
                    .send(VoiceControl::SetInputDevice(device))
                    .ok();
            }
            SetOutputDevice(device) => {
                self.settings.output_device = device.clone();
                self.voice_tx
                    .send(VoiceControl::SetOutputDevice(device))
                    .ok();
            }
            UpdateUserList => {
                self.update_user_list();
            }
            UpdateStats => {
                self.voice_tx.send(VoiceControl::GetStats).ok();

                // Log pipeline health
                if let Some(ref drops) = self.voice_drops {
                    let voice_drops = drops.load(Ordering::Relaxed);
                    if voice_drops > 0 {
                        warn!("⚠️ Pipeline health: {} voice packets dropped", voice_drops);
                        // Reset counter after logging
                        drops.store(0, Ordering::Relaxed);
                    }
                }
            }

            ShowError(error) => {
                self.show_error(&error);
            }
            ClearError => {
                if let Some(frame) = &mut self.error_frame {
                    frame.set_label("");
                }
            }
            SettingsWindowClosed => {
                self.settings_window_open = false;
            }
   
            Shutdown => {
                self.shutdown();
                self.app.quit();
            }
            _ => {}
        }
    }

    fn handle_connect(&mut self, nickname: String, server: String) {
        // Update settings
        self.settings.nickname = nickname.clone();
        self.settings.last_server = server.clone();
        self.settings.save();

        // Parse server address
        let addr = match server.parse::<SocketAddr>() {
            Ok(addr) => addr,
            Err(_) => {
                self.show_error("Invalid server address (use format: ip:port)");
                return;
            }
        };

        // Send connect message to network thread
        self.network_tx
            .send(NetworkControl::Connect { addr, nickname })
            .ok();

        // DON'T start voice here - wait until we get our client ID
    }

    fn handle_connection_established(&mut self) {
        // Only proceed if not already connected
        if self.connected {
            return;
        }

        self.connected = true;

        // Start message bridge now that we're connected
        self.start_message_bridge();

        // Hide connection window
        if let Some(mut wind) = self.connection_window.take() {
            wind.hide();
        }

        // Show main window
        self.show_main_screen();
    }

    fn handle_init_client(
        &mut self,
        users: HashMap<renet::ClientId, String>,
        server_name: String,
        greeting: String,
    ) {
        // Find our client ID
        for (id, name) in &users {
            if name == &self.settings.nickname {
                self.our_client_id = Some(*id);
                info!("📍 Found our client ID: {}", id);

                // NOW start voice with our client ID
                self.voice_tx
                    .send(VoiceControl::Start(self.settings.clone()))
                    .ok();
                self.voice_tx.send(VoiceControl::SetOurClientId(*id)).ok();

                // Apply initial mute state
                if self.settings.mute_on_start {
                    self.is_muted = true;
                    self.voice_tx.send(VoiceControl::SetMuted(true)).ok();
                    info!("🔇 Starting muted as per settings");
                } else {
                    self.is_muted = false;
                    info!("🔊 Starting unmuted");
                }

                break;
            }
        }

        // Initialize user states
        self.users.clear();
        for (id, username) in users {
            let audio_settings = self
                .settings
                .user_audio_settings
                .get(&username)
                .cloned()
                .unwrap_or_default();

            self.users.insert(
                id,
                UserState {
                    username: username.clone(),
                    is_speaking: false,
                    last_activity: Instant::now(),
                    audio_settings,
                },
            );

            // Add decoder for this user
            self.voice_tx.send(VoiceControl::AddClient(id)).ok();
        }

        // Update UI
        if let Some(chat) = &mut self.chat {
            chat.handle_init(server_name, greeting);
        }

        self.update_user_list();
    }

    fn handle_user_joined(&mut self, client_id: renet::ClientId, username: String) {
        let audio_settings = self
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
                audio_settings,
            },
        );

        self.voice_tx.send(VoiceControl::AddClient(client_id)).ok();

        if let Some(chat) = &mut self.chat {
            chat.handle_user_joined(username);
        }

        self.update_user_list();
    }

    fn handle_user_left(&mut self, client_id: renet::ClientId, username: String) {
        self.users.remove(&client_id);
        self.voice_tx
            .send(VoiceControl::RemoveClient(client_id))
            .ok();

        if let Some(chat) = &mut self.chat {
            chat.handle_user_left(username);
        }

        self.update_user_list();
    }

    fn handle_user_speaking(&mut self, client_id: renet::ClientId, is_speaking: bool) {
        if let Some(user) = self.users.get_mut(&client_id) {
            user.is_speaking = is_speaking;
            user.last_activity = Instant::now();
        }

        self.update_user_list();
    }

    fn handle_receive_chat_message(&mut self, username: String, text: String) {
        if let Some(chat) = &mut self.chat {
            chat.handle_text_message(username, text);
        }
    }

   fn handle_toggle_mute(&mut self) {
    // Toggle mute
    self.is_muted = !self.is_muted;

    // Tell voice thread
    self.voice_tx
        .send(VoiceControl::SetMuted(self.is_muted))
        .ok();

    // Reflect in UI mute button, etc.
    if let Some(sidebar) = &mut self.sidebar {
        sidebar.set_mute_state(self.is_muted);
    }

    // Speaking indicator handling:
    // - If we just MUTED: force "not speaking" on the wire immediately.
    // - If we just UNMUTED: do NOT send "speaking=true". Let VAD/packets trigger it.
    if self.is_muted {
        self.network_tx
            .send(NetworkControl::SendSpeakingStatus(false))
            .ok();
    }
} 

    fn handle_mute_user(&mut self, client_id: renet::ClientId, username: String, muted: bool) {
        let settings = self
            .settings
            .user_audio_settings
            .entry(username.clone())
            .or_default();
        settings.is_muted = muted;

        if let Some(user) = self.users.get_mut(&client_id) {
            user.audio_settings.is_muted = muted;
        }

        self.voice_tx
            .send(VoiceControl::SetUserAudio(client_id, settings.clone()))
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
            .entry(username.clone())
            .or_default();
        settings.volume = volume;

        if let Some(user) = self.users.get_mut(&client_id) {
            user.audio_settings.volume = volume;
        }

        self.voice_tx
            .send(VoiceControl::SetUserAudio(client_id, settings.clone()))
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
            .send(VoiceControl::SetUserAudio(
                client_id,
                UserAudioSettings::default(),
            ))
            .ok();

        self.settings.save();
        self.update_user_list();
    }

    fn handle_disconnect(&mut self) {
        self.connected = false;

        // Send disconnect to network
        self.network_tx.send(NetworkControl::Disconnect).ok();

        // Stop voice
        self.voice_tx.send(VoiceControl::Stop).ok();

        // Stop bridge thread if running
        // (it will naturally stop when channels are dropped)

        // Update UI
        if let Some(sidebar) = &mut self.sidebar {
            sidebar.handle_disconnected();
        }

        if let Some(chat) = &mut self.chat {
            chat.handle_disconnected("User disconnected".to_string());
        }

        // Clear users
        self.users.clear();
        self.our_client_id = None;

        // Reset mute state
        self.is_muted = false;

        // Show connection screen again
        if let Some(mut main_wind) = self.main_window.take() {
            main_wind.hide();
        }

        self.show_connection_screen();
    }

    fn update_user_list(&mut self) {
        // Check for timed-out speaking indicators
        let now = Instant::now();
        let timeout = Duration::from_millis(100);

        for user in self.users.values_mut() {
            if user.is_speaking && now.duration_since(user.last_activity) > timeout {
                user.is_speaking = false;
            }
        }

        // Update sidebar
        if let Some(sidebar) = &mut self.sidebar {
            sidebar.update_users(&self.users, self.our_client_id);
        }
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
            .with_label("We have discord at home")
            .with_align(enums::Align::Center);
        subtitle.set_label_size(12);
        subtitle.set_label_color(DarkTheme::TEXT_SECONDARY);
        col.fixed(&subtitle, 20);

        // Nickname input
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

        // Server address input
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

        // Error display
        let mut error_frame = frame::Frame::default().with_align(enums::Align::Center);
        error_frame.set_label_color(DarkTheme::DANGER);
        error_frame.set_label_size(12);
        col.fixed(&error_frame, 25);
        self.error_frame = Some(error_frame.clone());

        // Connect button
        let mut connect_btn = button::Button::default().with_label("Connect");
        connect_btn.set_color(DarkTheme::ACCENT);
        connect_btn.set_label_color(enums::Color::White);
        connect_btn.set_frame(enums::FrameType::FlatBox);
        col.fixed(&connect_btn, 40);

        col.end();
        wind.end();
        wind.show();

        // Connect button callback
        let sender = self.sender;
        connect_btn.set_callback(move |_| {
            let nickname = nick_input.value();
            let server = addr_input.value();

            if nickname.trim().is_empty() {
                sender.send(UiMessage::ShowError("Nickname cannot be empty".to_string()));
            } else {
                sender.send(UiMessage::Connect { nickname, server });
            }
        });

        self.connection_window = Some(wind);
    }

    fn show_main_screen(&mut self) {
        let mut wind = window::Window::default()
            .with_size(1000, 700)
            .with_label("baucord");
        wind.make_resizable(true);
        DarkTheme::style_window(&mut wind);

        let mut main_row = group::Flex::default_fill().row();
        main_row.set_color(DarkTheme::BG_PRIMARY);

        // Create sidebar
        let mut sidebar = Sidebar::new(self.sender);
        let sidebar_group = sidebar.create_ui();
        main_row.fixed(&sidebar_group, 250);

        // Apply current mute state
        sidebar.set_mute_state(self.is_muted);

        // Create chat
        let mut chat = Chat::new(self.sender);
        let chat_group = chat.create_ui();
        main_row.add(&chat_group);

        main_row.end();
        wind.end();
        wind.show();

        self.main_window = Some(wind);
        self.sidebar = Some(sidebar);
        self.chat = Some(chat);
    }

    fn show_settings_window(&mut self) {
        use crate::ui::settings::AudioSettingsWindow;

        let sender = self.sender;
        let settings = self.settings.clone();

        AudioSettingsWindow::show(sender, settings);
    }

    fn show_error(&mut self, error: &str) {
        if let Some(frame) = &mut self.error_frame {
            frame.set_label(error);
            frame.set_label_color(DarkTheme::DANGER);
        }

        // Auto-clear error after 5 seconds
        let sender = self.sender;
        app::add_timeout3(5.0, move |_| {
            sender.send(UiMessage::ClearError);
        });
    }

    fn shutdown(&mut self) {
        info!("Shutting down application");

        // Save settings
        self.settings.save();

        // Send shutdown to all threads
        self.network_tx.send(NetworkControl::Shutdown).ok();
        self.voice_tx.send(VoiceControl::Shutdown).ok();

        // Threads will clean up when dropped
    }
}
