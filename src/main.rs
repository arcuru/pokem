use clap::Parser;
use headjack::*;
use lazy_static::lazy_static;
use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;

use matrix_sdk::{Room, RoomMemberships, RoomState};
use serde::Deserialize;

use std::{fs::File, io::Read, path::PathBuf, sync::Mutex};
use tracing::{error, info};

use std::net::{IpAddr, SocketAddr};

use http_body_util::BodyExt;
use http_body_util::Full;
use hyper::body::Bytes;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::StatusCode;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct PokemArgs {
    /// Path to config file
    /// If not given we'll look in $XDG_CONFIG_HOME/pokem/config.yaml
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Room info
    #[arg(short, long)]
    room: Option<String>,

    /// Run in daemon mode
    /// This will run the bot in daemon mode, listening on a port for commands
    #[arg(short, long)]
    daemon: bool,

    /// Message to send
    /// The rest of the arguments
    #[arg()]
    message: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Clone)]
struct ServerConfig {
    /// Server URL
    url: String,
    /// Optional port
    port: Option<u16>,
}

#[derive(Debug, Deserialize, Clone)]
struct DaemonConfig {
    /// IP to bind on
    /// Defaults to 0.0.0.0
    addr: Option<String>,
    /// Port to bind on
    /// Will default to 80
    port: Option<u16>,
}

#[derive(Debug, Deserialize, Clone)]
struct MatrixConfig {
    /// Homeserver for pokem
    homeserver_url: String,
    /// Username for pokem
    username: String,
    /// Optionally specify the password, if not set it will be asked for on cmd line
    password: Option<String>,
    /// Allow list of which accounts we will respond to
    allow_list: Option<String>,
    /// Room size limit to respond to
    room_size_limit: Option<usize>,
    /// Set the state directory for pokem
    /// Defaults to $XDG_STATE_HOME/pokem
    state_dir: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    /// Configuration for logging in and messaging on Matrix
    matrix: Option<MatrixConfig>,

    /// Server config
    /// If this is setup, we will use this instead of logging in ourselves
    /// It expects the server config to point to a pokem daemon
    server: Option<ServerConfig>,

    /// Daemon config
    /// Configuration for running as a daemon
    daemon: Option<DaemonConfig>,

    /// Default room
    /// When acting as a client, this room will be the default if none is given
    default_room: Option<String>,
}

lazy_static! {
    /// Holds the config for the bot
    static ref GLOBAL_CONFIG: Mutex<Option<Config>> = Mutex::new(None);
    /// Holds the bot
    static ref GLOBAL_BOT: Mutex<Option<Bot>> = Mutex::new(None);
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    // Read in the config file
    let args = PokemArgs::parse();
    let mut file = {
        if let Some(config) = &args.config {
            File::open(config)?
        } else {
            let mut config = dirs::config_dir().unwrap();
            config.push("pokem");
            config.push("config.yaml");
            File::open(config)?
        }
    };
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    let config: Config = serde_yaml::from_str(&contents)?;
    *GLOBAL_CONFIG.lock().unwrap() = Some(config.clone());

    if args.daemon {
        // Daemon mode ignores all the other arguments
        info!("Running in daemon mode");
        return daemon(&config.daemon).await;
    }

    let mut messages = args.message.clone().unwrap_or_default();
    let room = {
        match args.room.clone() {
            Some(room) => room,
            None => {
                // We need to see if the first argument in messages looks like a room name
                // If it does, we'll use that
                let re = regex::Regex::new(r"!.*:.*").unwrap();
                // If messages length is greater than 1, we'll compare the first argument to a regex
                if messages.len() > 1 && re.is_match(&messages[0]) {
                    messages.remove(0)
                } else if let Some(default_room) = &config.default_room {
                    default_room.clone()
                } else {
                    return Err(anyhow::anyhow!("No room specified"));
                }
            }
        }
    };

    if let Some(server) = config.server {
        info!("Sending request to server");
        match poke_server(&server, &room, &messages.join(" ")).await {
            Ok(_) => {
                info!("Successfully sent message");
                return Ok(());
            }
            Err(e) => {
                error!("Failed to send message: {:?}", e);
            }
        }
    }

    if let Some(matrix) = config.matrix {
        info!("Running as a Matrix client");
        // Login to matrix
        let bot = connect(matrix).await?;
        // Ping the room
        return ping_room(&bot, &room, &messages.join(" ")).await;
    }

    return Err(anyhow::anyhow!("Unable to send message"));
}

async fn connect(config: MatrixConfig) -> anyhow::Result<Bot> {
    // The config file is read, now we can start up
    let mut bot = Bot::new(BotConfig {
        login: Login {
            homeserver_url: config.homeserver_url,
            username: config.username.clone(),
            password: config.password,
        },
        name: Some(config.username.clone()),
        allow_list: config.allow_list,
        state_dir: config.state_dir,
    })
    .await;

    if let Err(e) = bot.login().await {
        error!("Error logging in: {e}");
    }

    // React to invites.
    bot.join_rooms_callback(Some(|room: matrix_sdk::Room| async move {
        error!("Joined room");
        send_help(&room).await;
        Ok(())
    }));

    // Syncs to the current state
    if let Err(e) = bot.sync().await {
        error!("Error syncing: {e}");
    }

    info!("The client is ready! Listening to new messages…");

    Ok(bot)
}

async fn poke_server(server: &ServerConfig, room: &str, message: &str) -> anyhow::Result<()> {
    let url = {
        if server.port.is_none() {
            format!("{}/{}", server.url, room)
        } else {
            format!("{}:{}/{}", server.url, server.port.unwrap(), room)
        }
    };
    // if url doesn't start with "http://" or "https://", add "http://" to the beginning
    let url = if url.starts_with("http://") || url.starts_with("https://") {
        url
    } else {
        format!("http://{}", url)
    };

    let client = reqwest::Client::new();
    let res = client.post(&url).body(message.to_owned()).send().await?;

    if res.status().is_success() {
        let body = res.text().await?;
        error!("Response: {:?}", body);
        Ok(())
    } else {
        error!("Failed to send message: {:?}", res.status());
        Err(anyhow::anyhow!("Failed to send message"))
    }
}

/// Send a message to a room
async fn ping_room(bot: &Bot, room_id: &str, message: &str) -> anyhow::Result<()> {
    let room_id = match matrix_sdk::ruma::RoomId::parse(room_id) {
        Ok(id) => id,
        Err(e) => {
            return Err(anyhow::anyhow!("Failed to parse room id: {}", e));
        }
    };
    let r = match bot.client().get_room(&room_id) {
        Some(room) => room,
        None => {
            return Err(anyhow::anyhow!("Failed to find the room"));
        }
    };

    // If we're in an invited state, we need to wait for the invite to be accepted
    let mut delay = 2;
    while r.state() == RoomState::Invited {
        if delay > 60 {
            return Err(anyhow::anyhow!("Failed to join room"));
        }
        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        delay *= 2;
    }

    if can_message_room(&r).await {
        if let Err(e) = r.send(RoomMessageEventContent::text_plain(message)).await {
            return Err(anyhow::anyhow!("Failed to send message: {}", e));
        }
    } else {
        error!("Failed to send message");
    }

    Ok(())
}

/// Check if we can message the room
async fn can_message_room(room: &Room) -> bool {
    // Check the room size
    let room_size = room
        .members(RoomMemberships::ACTIVE)
        .await
        .unwrap_or(Vec::new())
        .len();

    // Get the room size limit
    let room_size_limit = GLOBAL_CONFIG
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .matrix
        .as_ref()
        .unwrap()
        .room_size_limit
        .unwrap_or(std::usize::MAX);

    room_size <= room_size_limit
}

/// Send the help message with the room id
async fn send_help(room: &Room) {
    if can_message_room(room).await {
        room.send(RoomMessageEventContent::text_plain(format!(
            "Welcome to pokem!\n\nThis room's name is: {}",
            room.room_id().as_str()
        )))
        .await
        .expect("Failed to send message");
    }
}

/// Poke the room from an http request
async fn daemon_poke(
    request: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, hyper::Error> {
    // The uri without the leading / will be the room id
    let room_id = request.uri().path().trim_start_matches('/').to_string();
    // The request body will be the message
    // Tranform the body into a string
    let body_bytes = request.collect().await?.to_bytes();
    let message = String::from_utf8(body_bytes.to_vec()).unwrap();
    error!("Room: {:?}, Message: {:?}", room_id, message);

    // Get a copy of the bot
    let bot = GLOBAL_BOT.lock().unwrap().as_ref().unwrap().clone();

    if let Err(e) = ping_room(&bot, &room_id, &message).await {
        error!("Failed to send message: {:?}", e);
        return Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from_static(b"Failed to send message")))
            .unwrap());
    }

    Ok(Response::builder()
        .status(StatusCode::OK)
        .body(Full::new(Bytes::from_static(b"OK")))
        .unwrap())
}

/// Run in daemon mode
/// This binds to a port and listens for incoming requests, and sends them to the Matrix room
async fn daemon(config: &Option<DaemonConfig>) -> anyhow::Result<()> {
    let addr = {
        if let Some(daemon) = config {
            let ip_addr: IpAddr = daemon
                .addr
                .clone()
                .unwrap_or("0.0.0.0".to_string())
                .parse()
                .expect("Invalid IP address");
            SocketAddr::from((ip_addr, daemon.port.unwrap_or(80)))
        } else {
            SocketAddr::from(([0, 0, 0, 0], 80))
        }
    };

    // We create a TcpListener and bind it to 127.0.0.1:3000
    let listener = TcpListener::bind(addr).await?;

    // Login to the bot and store it
    let matrix_config = GLOBAL_CONFIG
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .matrix
        .clone()
        .unwrap();
    let bot = connect(matrix_config).await?;
    GLOBAL_BOT.lock().unwrap().replace(bot.clone());

    // Register an info command to echo the room info
    bot.register_text_command(
        "info",
        "Print room info".to_string(),
        |_, _, room| async move {
            if can_message_room(&room).await {
                send_help(&room).await;
            }
            Ok(())
        },
    )
    .await;

    // Spawn a tokio task to continuously accept incoming connections
    tokio::task::spawn(async move {
        // We start a loop to continuously accept incoming connections
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(result) => result,
                Err(err) => {
                    error!("Error accepting connection: {:?}", err);
                    error!("Exitind daemon");
                    return;
                }
            };

            // Use an adapter to access something implementing `tokio::io` traits as if they implement
            // `hyper::rt` IO traits.
            let io = TokioIo::new(stream);

            // Spawn a tokio task to serve each connection concurrently
            tokio::task::spawn(async move {
                if let Err(err) = http1::Builder::new()
                    .serve_connection(io, service_fn(daemon_poke))
                    .await
                {
                    eprintln!("Error serving connection: {:?}", err);
                }
            });
        }
    });

    // Run the bot and block
    // It never exits
    bot.run().await
}
