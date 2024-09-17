use clap::Parser;
use reqwest::header::HeaderMap;

mod config;
mod daemon;
mod utils;

use crate::config::*;
use crate::daemon::daemon;
use crate::utils::*;

use is_terminal::IsTerminal;
use std::{fs::File, io::Read, path::PathBuf};
use tracing::{error, info};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct PokemArgs {
    /// Path to config file
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Room ID to send the message to
    #[arg(short, long)]
    room: Option<String>,

    /// Run in daemon mode
    #[arg(short, long)]
    daemon: bool,

    /// Authentication token
    #[arg(long, visible_alias = "auth")]
    authentication: Option<String>,

    /// Formatting for the message. "markdown" or "plain".
    #[arg(long)]
    format: Option<String>,

    /// Message to send
    #[arg()]
    message: Option<Vec<String>>,
}

/// Get the config from the file or load the default config
fn get_config_or_default(path: &Option<PathBuf>) -> Config {
    let mut file = {
        if let Some(config) = path {
            match File::open(config) {
                Ok(file) => file,
                Err(_) => {
                    return Config::default();
                }
            }
        } else {
            let mut config = dirs::config_dir().unwrap();
            config.push("pokem");
            config.push("config.yaml");
            match File::open(config) {
                Ok(file) => file,
                Err(_) => {
                    return Config::default();
                }
            }
        }
    };
    let mut contents = String::new();
    file.read_to_string(&mut contents).unwrap();

    serde_yaml::from_str(&contents).unwrap()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    // Read in the config file
    let args = PokemArgs::parse();
    let config: Config = get_config_or_default(&args.config);
    *GLOBAL_CONFIG.lock().unwrap() = Some(config.clone());

    if args.daemon {
        // Daemon mode ignores all the other arguments
        info!("Running in daemon mode");
        return daemon(config.daemon, config.rooms).await;
    }

    let headers = {
        let mut headers = HeaderMap::new();
        if let Some(auth) = args.authentication.clone() {
            headers.insert("Authentication", auth.parse().unwrap());
        }
        if let Some(format) = args.format.clone() {
            headers.insert("Format", format.parse().unwrap());
        }
        headers
    };

    let mut messages = args.message.clone().unwrap_or_default();
    let room = {
        let rooms = config.rooms.unwrap_or_default();
        match args.room.clone() {
            Some(room) => {
                // If the room is a room name in the config, we'll transform it to the room id
                if let Some(room_id) = rooms.get(&room) {
                    room_id.clone()
                } else {
                    room
                }
            }
            None => {
                // Create a regex to see if the first argument looks like a room name
                let re = regex::Regex::new(r"^.*:.*\..*").unwrap();
                if messages.is_empty() {
                    // Check if there is a default room configured
                    // That room will be pinged with no message
                    if let Some(room_id) = rooms.get("default") {
                        room_id.clone()
                    } else {
                        return Err(anyhow::anyhow!("No room specified"));
                    }
                } else if re.is_match(&messages[0]) {
                    // Use the first arg if it's a raw room id
                    // TODO: This has surprising behavior if this isn't an intended room, we'd want to fall back to the configured default room
                    // I suppose we could fallback in this CLI? e.g. if the command fails to identify a room, then try the default room
                    messages.remove(0)
                } else if let Some(room_id) = rooms.get(&messages[0]) {
                    // Check for a room name in the config
                    messages.remove(0);
                    room_id.clone()
                } else if let Some(room_id) = rooms.get("default") {
                    // Check if a default room exists
                    room_id.clone()
                } else {
                    return Err(anyhow::anyhow!("No room specified"));
                }
            }
        }
    };
    error!("Room: {:?}, Message: {:?}", room, messages);

    // Append any stdin content to the message
    let mut input = String::new();
    if !std::io::stdin().is_terminal() {
        std::io::stdin().read_to_string(&mut input).unwrap();
        if !input.is_empty() {
            messages.push(input.trim().to_string());
        }
    }

    if config.server.is_none() && config.matrix.is_none() {
        // The user has set neither server nor matrix config
        // Assume they want to use the public instance
        info!("Sending request to pokem.dev");
        let server = ServerConfig {
            url: "https://pokem.dev".to_string(),
            port: None,
        };
        match poke_server(&server, &room, &headers, &messages.join(" ")).await {
            Ok(_) => {
                info!("Successfully sent message");
                return Ok(());
            }
            Err(e) => {
                error!("Failed to send message: {:?}", e);
            }
        }
    }

    if let Some(server) = config.server {
        info!("Sending request to server");
        match poke_server(&server, &room, &headers, &messages.join(" ")).await {
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
        GLOBAL_BOT.lock().unwrap().replace(bot.clone());
        // Ping the room
        return ping_room(&bot, &room, &headers, &messages.join(" "), false).await;
    }

    return Err(anyhow::anyhow!("Unable to send message"));
}

/// Send a message to the server.
async fn poke_server(
    server: &ServerConfig,
    room: &str,
    headers: &reqwest::header::HeaderMap,
    message: &str,
) -> anyhow::Result<()> {
    // URI encode the room
    let room = urlencoding::encode(room).to_string();

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
        // https by default, don't encourage unencrypted traffic
        format!("https://{}", url)
    };

    let client = reqwest::Client::new();
    let res = client
        .post(&url)
        .body(message.to_owned())
        .headers(headers.clone())
        .send()
        .await?;

    if res.status().is_success() {
        let body = res.text().await?;
        error!("Response: {:?}", body);
        Ok(())
    } else {
        error!("Failed to send message: {:?}", res.status());
        Err(anyhow::anyhow!("Failed to send message"))
    }
}
