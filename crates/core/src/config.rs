//! Configuration types for the voice chat engine.
//!
//! Simple, flat configuration. Everything has sane defaults.
//! Users pick devices by name, we handle sample rates and channels automatically.

/// Top-level configuration for [`VoiceChat`](crate::VoiceChat).
///
/// Only expose what users actually need to decide. Sample rates, channel
/// mapping, buffer sizes, and codec settings are all handled internally.
#[derive(Debug, Clone)]
pub struct VoiceChatConfig {
    /// Input device name, or `None` for system default.
    pub input_device: Option<String>,
    /// Output device name, or `None` for system default.
    pub output_device: Option<String>,
    /// Start muted.
    pub mute_on_start: bool,

    /// Whether VAD is enabled. When disabled, all audio is sent.
    pub vad_enabled: bool,
    /// VAD detection threshold (0.01–0.50). Lower = more sensitive.
    ///
    /// - `0.01–0.08`: Very sensitive — picks up whispers, more false positives
    /// - `0.08–0.18`: High sensitivity — good for quiet environments
    /// - `0.15`:       Default — balanced for most situations
    /// - `0.18–0.30`: Medium — requires clear speech
    /// - `0.30–0.50`: Low — only loud/clear speech triggers
    ///
    /// Adjustable at runtime via [`VoiceChat::set_vad_threshold`](crate::VoiceChat::set_vad_threshold).
    pub vad_threshold: f32,

    /// Enable RNNoise-based noise suppression. Default: `true`.
    ///
    /// When enabled, background noise (fans, HVAC, keyboard, etc.) is
    /// removed before VAD and encoding. This also reduces false VAD triggers.
    /// Toggle at runtime via [`VoiceChat::set_noise_suppression`](crate::VoiceChat::set_noise_suppression).
    pub noise_suppression: bool,

    /// Master input gain multiplier (0.0–2.0). Default: `1.0`.
    ///
    /// Applied right after capture, before denoise/VAD/encode.
    /// Use this for mic boost or attenuation.
    /// Adjustable at runtime via [`VoiceChat::set_input_gain`](crate::VoiceChat::set_input_gain).
    pub input_gain: f32,

    /// Master output gain multiplier (0.0–2.0). Default: `1.0`.
    ///
    /// Applied after jitter buffer mixing, before playback.
    /// This is the "master volume" control.
    /// Adjustable at runtime via [`VoiceChat::set_output_gain`](crate::VoiceChat::set_output_gain).
    pub output_gain: f32,
}

impl Default for VoiceChatConfig {
    fn default() -> Self {
        Self {
            input_device: None,
            output_device: None,
            mute_on_start: false,
            vad_enabled: true,
            vad_threshold: 0.15,
            noise_suppression: true,
            input_gain: 1.0,
            output_gain: 0.4,
        }
    }
}

/// Per-peer audio settings (volume, mute).
///
/// Used by the jitter buffer / mixer on the output side.
#[derive(Debug, Clone)]
pub struct PeerAudioSettings {
    /// Playback volume multiplier (0.0–2.0). Default 1.0.
    pub volume: f32,
    /// Whether this peer is muted locally.
    pub is_muted: bool,
}

impl Default for PeerAudioSettings {
    fn default() -> Self {
        Self {
            volume: 1.0,
            is_muted: false,
        }
    }
}
