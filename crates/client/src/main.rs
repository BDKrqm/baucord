// main.rs
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod network;
mod ui;
mod voice;

use app::App;

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

    let mut app = App::new();
    app.run();
}