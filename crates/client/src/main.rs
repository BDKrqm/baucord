#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{Arc, Mutex};

mod app;
mod network;
mod settings;
mod ui;
mod voice;

use app::App;
use network::NetworkClient;
use settings::Settings;
use voice::VoiceChat;

fn main() {
    #[cfg(debug_assertions)]
    {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    }

    #[cfg(not(debug_assertions))]
    {
        // For release builds, log to a file
        use env_logger::Target;
        use std::fs::File;

        let log_file = File::create("baucord.log").unwrap();
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
            .target(Target::Pipe(Box::new(log_file)))
            .init();
    }
    // Load settings from disk
    let settings = Arc::new(Mutex::new(Settings::load()));

    // Create shared state for network client and voice chat
    let network_client = Arc::new(Mutex::new(None::<NetworkClient>));
    let voice_chat = Arc::new(Mutex::new(None::<VoiceChat>));

    // Create UI with network, voice, and settings references
    let mut chat_ui = App::new(network_client.clone(), voice_chat.clone(), settings.clone());

    // Run UI (blocking)
    chat_ui.run();

    // Save settings on exit
    if let Ok(settings_guard) = settings.lock() {
        settings_guard.save();
    }
}
