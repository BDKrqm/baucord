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
        use env_logger::Target;
        use std::fs::File;

        let log_file = File::create("baucord.log").unwrap();
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
            .target(Target::Pipe(Box::new(log_file)))
            .init();
    }
    let settings = Arc::new(Mutex::new(Settings::load()));

    let network_client = Arc::new(Mutex::new(None::<NetworkClient>));
    let voice_chat = Arc::new(Mutex::new(None::<VoiceChat>));

    let mut app = App::new(network_client.clone(), voice_chat.clone(), settings.clone());

    app.run();

    if let Ok(settings_guard) = settings.lock() {
        settings_guard.save();
    }
}
