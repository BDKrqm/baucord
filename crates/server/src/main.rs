use clap::Parser;
use log::{error, info};
use std::thread;
use std::time::Duration;

mod server;
use server::{ChatServer, Config};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(short, long, default_value = "config.yaml")]
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
        eprintln!("Failed to load config from {}: {}", args.config, e);
        eprintln!("Using default configuration");
        create_default_config()
    });

    if let Some(host) = args.host {
        config.server.host = host;
    }

    if let Some(port) = args.port {
        config.server.port = port;
    }

    env_logger::Builder::new()
        .filter_level(parse_log_level(&config.logging.level))
        .init();

    info!("Starting {}", config.server.name);

    let mut server = ChatServer::new(config)?;

    info!("Server listening on:");
    for addr in server.addresses() {
        info!("  {}", addr);
    }
    info!("Press Ctrl+C to stop");

    loop {
        if let Err(e) = server.update() {
            error!("Server error: {}", e);
        }

        thread::sleep(Duration::from_millis(10));
    }
}

fn parse_log_level(level: &str) -> log::LevelFilter {
    match level.to_lowercase().as_str() {
        "trace" => log::LevelFilter::Trace,
        "debug" => log::LevelFilter::Debug,
        "info" => log::LevelFilter::Info,
        "warn" => log::LevelFilter::Warn,
        "error" => log::LevelFilter::Error,
        _ => log::LevelFilter::Info,
    }
}

fn create_default_config() -> Config {
    Config {
        server: server::ServerSettings {
            name: "Chat & Voice Server".to_string(),
            host: "0.0.0.0".to_string(),
            port: 5000,
            greeting_message: "Welcome to the server!".to_string(),
        },
        network: server::NetworkSettings {
            max_clients: 64,
            protocol_id: 27,
        },
        channels: server::ChannelSettings {
            reliable_channel_id: 2,
            voice_channel_id: 3,
            voice_buffer_size_mb: 5,
        },
        logging: server::LoggingSettings {
            level: "info".to_string(),
            stats_interval_secs: 30,
        },
        authentication: server::AuthSettings { enabled: false },
    }
}
