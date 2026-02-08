//! Example: how to integrate voice-chat-core into your client app.
//!
//! This shows the minimal wiring needed to get voice chat working.

use voice_chat_core::{AudioPacket, PeerAudioSettings, VoiceChat, VoiceChatConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // ── 1. Configure ─────────────────────────────────────────────────────
    let config = VoiceChatConfig {
        input_device: None,       // system default
        output_device: None,      // system default
        mute_on_start: false,
        vad_enabled: true,
        vad_threshold: 0.15,      // balanced default
        noise_suppression: true,  // RNNoise enabled
    };

    // ── 2. Create and start ──────────────────────────────────────────────
    let mut vc = VoiceChat::new(config)?;
    vc.start()?;

    // ── 3. Register peers as they join ───────────────────────────────────
    let peer_id: u64 = 12345;
    vc.add_peer(peer_id);
    vc.set_peer_settings(peer_id, PeerAudioSettings {
        volume: 1.0,
        is_muted: false,
    });

    // ── 4. Network loop (your transport layer) ──────────────────────────
    loop {
        // Send outgoing packets
        for packet in vc.take_outgoing_packets() {
            // Serialize and send over your network (renet, QUIC, WebRTC, etc.)
            // my_network.send(peer_id, &packet.opus_data, packet.header.seq, ...);
        }

        // Receive incoming packets from network
        // let (sender_id, seq, timestamp, opus_bytes) = my_network.recv();
        // vc.receive_packet(AudioPacket::new(seq, timestamp, sender_id, opus_bytes));

        // ── 5. UI state ─────────────────────────────────────────────────
        let _speaking = vc.is_speaking();     // for speaking indicator
        let _level = vc.input_level();        // for VU meter (0.0–1.0)
        let _stats = vc.stats_summary();      // for debug overlay

        // ── 6. Runtime adjustments (from UI) ────────────────────────────
        // vc.set_vad_threshold(0.20);         // user moved slider
        // vc.set_muted(true);                 // user pressed mute
        // vc.set_vad_enabled(false);          // user toggled VAD off
        // vc.set_noise_suppression(false);    // user toggled noise suppression

        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
