//! baucord-gateway: a bot's door to a baucord server.
//!
//! On one side the server, over QUIC, as a client with a key of its
//! own (`session.rs`, `net.rs`); on the other a WebSocket on the
//! loopback where a bot in any language reads JSON events, sends JSON
//! actions and streams PCM to play in a voice channel (`ws.rs`,
//! `voice.rs`). README.md has the words on the wire and two bots.

use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use gateway::{net, voice, wire, ws};
use log::{error, info};
use transport::{ClientKey, Fingerprint};

#[derive(Parser, Debug)]
#[command(
    name = "baucord-gateway",
    version,
    about = "A bot's door to a baucord server: QUIC to the server, a local WebSocket for the bot."
)]
struct Args {
    /// The server, as `host:port`.
    #[arg(long, env = "BAUCORD_SERVER")]
    server: String,
    /// The server's identity, `SHA256:…` (Settings → Servers shows
    /// it). Without one the first connection's identity is trusted
    /// and written next to the key for every later run.
    #[arg(long, env = "BAUCORD_PIN")]
    pin: Option<String>,
    /// The bot's name on the server.
    #[arg(long, env = "BAUCORD_BOT_NAME")]
    name: String,
    /// The bot's identity key; made on the first run. Keep it like a
    /// password: whoever has it is this bot.
    #[arg(long, env = "BAUCORD_KEY", default_value = "bot.key")]
    key: PathBuf,
    /// An invite code, needed on the first run (and after `/bot reset`).
    #[arg(long, env = "BAUCORD_INVITE")]
    invite: Option<String>,
    /// Where bots connect.
    #[arg(long, env = "BAUCORD_GATEWAY_LISTEN", default_value = "127.0.0.1:8765")]
    listen: SocketAddr,
    /// A secret a bot must present (`Authorization: Bearer …` or
    /// `?token=…`); needed when `--listen` is not the loopback.
    #[arg(long, env = "BAUCORD_GATEWAY_TOKEN")]
    token: Option<String>,
    /// Opus bitrate for the voice the bot plays, in kbit/s (16–128).
    #[arg(long, env = "BAUCORD_VOICE_KBPS", default_value_t = 64)]
    voice_kbps: u32,
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();
    let args = Args::parse();
    match run(args) {
        Ok(code) => code,
        Err(e) => {
            error!("{e}");
            ExitCode::from(2)
        }
    }
}

fn run(args: Args) -> Result<ExitCode, String> {
    let name = protocol::sanitize_username(&args.name);
    if name.is_empty() {
        return Err("--name is empty once trimmed".to_string());
    }
    let server = args
        .server
        .to_socket_addrs()
        .map_err(|e| format!("--server {:?}: {e}", args.server))?
        .next()
        .ok_or_else(|| format!("--server {:?} resolves to nothing", args.server))?;
    if !args.listen.ip().is_loopback() && args.token.is_none() {
        return Err(format!(
            "--listen {} is not the loopback: set --token, or anyone who reaches it is the bot",
            args.listen
        ));
    }
    let key =
        Arc::new(ClientKey::load_or_create(&args.key).map_err(|e| format!("the bot's key: {e}"))?);
    info!(
        "bot {name}: user id {}… (key {})",
        &wire::hex(&key.user_id())[..12],
        args.key.display()
    );
    let pin_file = args.key.with_extension("pin");
    let pin = match args.pin.as_deref() {
        Some(text) => Some(
            text.parse::<Fingerprint>()
                .map_err(|()| format!("--pin {text:?} is not SHA256:<base64>"))?,
        ),
        None => std::fs::read_to_string(&pin_file)
            .ok()
            .and_then(|text| text.trim().parse::<Fingerprint>().ok())
            .inspect(|pin| info!("pinned {pin} from {}", pin_file.display())),
    };

    let (link, commands, waker) = net::link().map_err(|e| format!("the doorbell socket: {e}"))?;
    let hub = ws::Hub::new();
    let (feeder, pacer) = voice::Feeder::pair();
    {
        let link = link.clone();
        let kbps = args.voice_kbps;
        std::thread::Builder::new()
            .name("voice".to_string())
            .spawn(move || voice::run(&pacer, &link, kbps))
            .map_err(|e| format!("could not start the voice thread: {e}"))?;
    }
    {
        let context = ws::Context {
            hub: hub.clone(),
            link: link.clone(),
            feeder,
            token: args.token.clone(),
        };
        let listen = args.listen;
        std::thread::Builder::new()
            .name("websocket".to_string())
            .spawn(move || {
                if let Err(e) = ws::serve(listen, &context) {
                    error!("could not listen on {listen}: {e}");
                    std::process::exit(2);
                }
            })
            .map_err(|e| format!("could not start the listener: {e}"))?;
    }
    // The last holder of the link moves into the handler: Ctrl-C and
    // SIGTERM close the session before the process ends.
    ctrlc::set_handler(move || link.send(net::Command::Shutdown))
        .map_err(|e| format!("could not take Ctrl-C: {e}"))?;

    let mut config = net::Config {
        server,
        pin,
        pin_file: Some(pin_file),
        name,
        invite: args.invite,
        key,
    };
    let shutdown = net::run(&mut config, &commands, &waker, &hub);
    Ok(if shutdown {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}
