use clap::Parser;
use log::{error, info};
use std::thread;
use std::time::Duration;
mod config;
mod relay;
mod server;
use config::Config;
use server::ChatServer;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(short, long, default_value = "server.yaml")]
    config: String,
    /// Override host from config
    #[arg(long)]
    host: Option<String>,
    /// Override port from config
    #[arg(long)]
    port: Option<u16>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let mut config = Config::from_file(&args.config).unwrap_or_else(|e| {
        eprintln!("Failed to load config from {}: {e}", args.config);
        let config = Config::default();
        if let Err(save_err) = config.save_to_file(&args.config) {
            eprintln!(
                "Could not save default config to {}: {save_err}",
                args.config
            );
        } else {
            eprintln!("Default configuration saved to {}", args.config);
        }
        config
    });

    if let Some(host) = args.host {
        config.server.host = host;
    }
    if let Some(port) = args.port {
        config.server.port = port;
    }

    env_logger::Builder::new()
        .filter_level(
            config
                .logging
                .level
                .parse()
                .unwrap_or(log::LevelFilter::Info),
        )
        .init();

    info!("Starting {}", config.server.name);

    let mut server = ChatServer::new(config)?;

    info!("Server listening on:");
    for addr in server.addresses() {
        info!("  {addr}");
    }
    info!("Press Ctrl+C to stop");

    // ── Tight poll loop ─────────────────────────────────────────────
    //
    // Voice packets need sub-millisecond relay turnaround. At 10 ms
    // sleep the server adds 0–10 ms (avg 5 ms) of pure relay latency
    // on every voice packet — that's perceptible.
    //
    // At 1 ms sleep we add 0–1 ms (avg 0.5 ms) which is negligible.
    // CPU cost is ~0.5–1% of a single core on modern hardware, well
    // worth the latency savings for a voice server.
    //
    // The socket is set to non-blocking in ChatServer::new(), so
    // update() returns immediately when there's nothing to process.
    loop {
        if let Err(e) = server.update() {
            error!("Server error: {e}");
        }
        thread::sleep(Duration::from_millis(1));
    }
}
