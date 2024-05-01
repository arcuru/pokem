use clap::Parser;
use headjack::*;
use lazy_static::lazy_static;

use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;

use matrix_sdk::ruma::events::tag::TagInfo;
use matrix_sdk::{Room, RoomMemberships, RoomState};

use serde::Deserialize;

use std::collections::HashMap;

use is_terminal::IsTerminal;
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
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Room ID to send the message to
    #[arg(short, long)]
    room: Option<String>,

    /// Run in daemon mode
    #[arg(short, long)]
    daemon: bool,

    /// Message to send
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
    /// IP to bind on.
    /// Defaults to 0.0.0.0
    addr: Option<String>,
    /// Port to bind on.
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
    /// Set the command prefix.
    /// Defaults to "!pokem".
    command_prefix: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
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

    /// Save different types of rooms
    /// Special value default will be used if no room is specified
    /// e.g. error/warning/info/default
    rooms: Option<HashMap<String, String>>,
}

lazy_static! {
    /// Holds the config for the bot
    static ref GLOBAL_CONFIG: Mutex<Option<Config>> = Mutex::new(None);
    /// Holds the bot
    static ref GLOBAL_BOT: Mutex<Option<Bot>> = Mutex::new(None);
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
        return daemon(&config.daemon).await;
    }

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
        info!("Sending request to pokem.jackson.dev");
        let server = ServerConfig {
            url: "https://pokem.jackson.dev".to_string(),
            port: None,
        };
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
        GLOBAL_BOT.lock().unwrap().replace(bot.clone());
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
        command_prefix: if config.command_prefix.is_none() {
            Some("!pokem".to_string())
        } else {
            config.command_prefix
        },
        room_size_limit: config.room_size_limit,
    })
    .await;

    if let Err(e) = bot.login().await {
        error!("Error logging in: {e}");
    }

    // React to invites.
    bot.join_rooms_callback(Some(|room: matrix_sdk::Room| async move {
        error!("Joined room: {}", room.room_id().as_str());
        if can_message_room(&room).await {
            room.send(RoomMessageEventContent::text_plain(
                "Welcome to Pok'em!\n\nSend \".help\" to see available commands.",
            ))
            .await
            .expect("Failed to send message");
        }
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

/// Send a message to the server.
async fn poke_server(server: &ServerConfig, room: &str, message: &str) -> anyhow::Result<()> {
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

/// Check a room to see if we should leave it.
/// It applies if we're the only ones left in the room.
#[allow(dead_code)]
async fn should_leave_room(room: &Room) -> bool {
    // Check if we are joined to the room, and there is only 1 member
    // This means we are the only member
    if let Ok(members) = room.members(RoomMemberships::ACTIVE).await {
        // We'd be the only member
        if members.len() == 1 {
            error!("Found empty room");
            true
        } else {
            false
        }
    } else {
        false
    }
}

/// Translate a provided room name into an actual Room struct.
/// This looks up by either the Room Internal ID or the Room Alias.
/// Any alias, main or alt, will be checked.
async fn get_room_from_name(bot: &Bot, name: &str) -> Option<Room> {
    if name.is_empty() {
        return None;
    }

    // Is this a room internal id?
    if let Ok(id) = matrix_sdk::ruma::RoomId::parse(name) {
        return bot.client().get_room(&id);
    }

    // Is this a user address?
    let re = regex::Regex::new(r"^@.*:.*\..*").unwrap();
    if re.is_match(name) {
        // This looks like a user name
        // unsupported at this time
        return None;
    }

    // #@patrick:jackson.dev is a valid _room_ name
    // We will be careful to not allow that, and ignore all room names that look like user names

    // If name does not start with a '#', add it
    // This is to get around oddities with specifying the '#' in the URL
    // It's annoying to reference it, so we support the room name without the '#'
    let name = if name.starts_with('#') {
        name.to_string()
    } else {
        format!("#{}", name)
    };

    // Is this a room address?
    let re = regex::Regex::new(r"^#.*:.*\..*").unwrap();
    if re.is_match(&name) {
        // We're just going to scan every room we're in to look for this room name
        // Effective? Sure.
        // Efficient? Absolutely not.
        let rooms = bot.client().joined_rooms();
        for r in &rooms {
            let room_alias = r.canonical_alias();
            if let Some(alias) = room_alias {
                if alias.as_str() == name {
                    return Some(r.clone());
                }
            }
            // Check the alt aliases
            for alias in r.alt_aliases() {
                if alias.as_str() == name {
                    return Some(r.clone());
                }
            }
        }
        return None;
    }
    error!("Failed to find room: {}", name);
    None
}

/// Send a message to a room.
async fn ping_room(bot: &Bot, room_id: &str, message: &str) -> anyhow::Result<()> {
    if let Some(r) = get_room_from_name(bot, room_id).await {
        // If we're in an invited state, we need to wait for the invite to be accepted
        let mut delay = 2;
        while r.state() == RoomState::Invited {
            if delay > 60 {
                return Err(anyhow::anyhow!("Failed to join room"));
            }
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            delay *= 2;
        }

        let mut msg = message;

        // Check for a password
        let room_config = get_room_config(&r).await;
        if room_config.password.is_some() {
            // Check if the message starts with the password
            if !msg.starts_with(&room_config.password.clone().unwrap()) {
                return Err(anyhow::anyhow!("Password required"));
            }
            // Remove the password and any leading whitespace
            msg = msg
                .trim_start_matches(&room_config.password.unwrap())
                .trim_start();
        }

        if can_message_room(&r).await {
            if let Err(e) = r.send(RoomMessageEventContent::text_plain(msg)).await {
                return Err(anyhow::anyhow!("Failed to send message: {}", e));
            }
        } else {
            error!("Failed to send message");
        }

        return Ok(());
    }
    error!("Failed to find room with name: {}", room_id);
    Err(anyhow::anyhow!(
        "Failed to find room with name: {}",
        room_id
    ))
}

/// Check if we can message the room
async fn can_message_room(room: &Room) -> bool {
    // Always send to the example room
    if room.room_id().as_str() == "!JYrjsPjErpFSDdpwpI:jackson.dev" {
        error!("Sending to example room");
        return true;
    }

    // Check if we're blocked from sending messages
    if room
        .tags()
        .await
        .unwrap_or_default()
        .is_some_and(|x| x.contains_key(&"dev.pokem.block".into()))
    {
        error!(
            "Blocked from sending messages to {}",
            room.room_id().as_str()
        );
        return false;
    }
    true
}

/// Send the help message with the room id
async fn send_help(room: &Room) {
    if can_message_room(room).await {
        if let Some(alias) = room.canonical_alias() {
            room.send(RoomMessageEventContent::text_plain(format!(
                "This Room's Alias is: {}",
                alias.as_str()
            )))
            .await
            .expect("Failed to send message");
        }
        room.send(RoomMessageEventContent::text_plain(format!(
            "This Room's ID is: {}",
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
    // The room_id may be URI encoded
    let room_id = match urlencoding::decode(&room_id) {
        Ok(room) => room.to_string(),
        Err(_) => room_id,
    };

    // If it's a GET request, we'll serve a WebUI
    if request.method() == hyper::Method::GET {
        // Create the webpage with the room id filled in
        let page = r#"
<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Pok'em</title>
<script>
  async function submitForm(event) {
    // Prevent the default form submission
    event.preventDefault();

    // Reference to feedback display elements
    const successMessage = document.getElementById('success-message');
    const errorMessage = document.getElementById('error-message');

    // Initially hide both messages
    successMessage.style.display = 'none';
    errorMessage.style.display = 'none';

    // Get the room name and message from the form inputs
    var room = document.getElementById('room').value;
    var message = document.getElementById('message').value;

    // Check if room and message are provided
    if (!room || !message) {
      errorMessage.innerHTML = 'Please fill in both fields.';
      errorMessage.style.display = 'block';
      return;
    }

    var actionURL = '/' + encodeURIComponent(room);

    try {
      const response = await fetch(actionURL, {
        method: 'POST',
        headers: {
          'Content-Type': 'text/plain',
        },
        body: message
      });

      if(response.ok) { 
        // On success, display the success message
        successMessage.innerHTML = "Message sent successfully!";
        successMessage.style.display = 'block';
      } else {
        // On failure (non-2xx status), display an error message
        errorMessage.innerHTML = "Failed to send message. Status: " + response.status;
        errorMessage.style.display = 'block';
      }
    } catch (error) {
      // On error (network issue, etc.), display an error message
      errorMessage.innerHTML = "Error sending message: " + error.message;
      errorMessage.style.display = 'block';
    }
  }

  // Decode the URL nad use that to set the Room Name
  function setInitialRoomValue() {
    const url = window.location.href;
    console.log(url);
    const roomField = document.getElementById('room');
    const roomValue = url.substring(url.lastIndexOf('/') + 1);
    console.log(roomValue);

    roomField.value = decodeURIComponent(roomValue);
  }

  // Call the function to set the initial room value when the page loads
  window.onload = setInitialRoomValue;
</script>
</head>
<body>

<h2>Pok'em!</h2>
<h3>Provide the Room and Message and we'll Poke Them for you.</h3>

<form onsubmit="submitForm(event);">
  <label for="room">Room:</label><br>
  <input type="text" id="room" size="30" maxlength="256"><br>
  <label for="message">Message:</label><br>
  <textarea id="message" rows="4" cols="50" maxlength="1024"></textarea><br><br>
  <input type="submit" value="Submit">
</form>

<!-- Feedback messages -->
<div id="success-message" style="color: green; display: none;"></div>
<div id="error-message" style="color: red; display: none;"></div>

</body>
</html>
            "#
        .to_string();
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .body(Full::new(Bytes::from(page)))
            .unwrap());
    }
    // The request body will be the message
    // Transform the body into a string
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
        None,
        Some("Print room info".to_string()),
        |_, _, room| async move {
            if can_message_room(&room).await {
                send_help(&room).await;
            }
            Ok(())
        },
    )
    .await;

    // Register a poke command that will send a poke
    bot.register_text_command(
        "poke",
        Some("<room> <message>".to_string()),
        Some("Poke the room".to_string()),
        |_, msg, room| async move {
            // Get the room and message
            let mut args = msg.split_whitespace();
            let _ = args.next(); // Ignore the command
            let room_id = args.next().unwrap_or_default();
            let message = args.collect::<Vec<&str>>().join(" ");
            error!("Room: {:?}, Message: {:?}", room_id, message);

            // Get a copy of the bot
            let bot = GLOBAL_BOT.lock().unwrap().as_ref().unwrap().clone();

            if let Err(e) = ping_room(&bot, room_id, &message).await {
                error!("Failed to send message: {:?}", e);
                if can_message_room(&room).await {
                    room.send(RoomMessageEventContent::text_plain(&format!(
                        "Failed to send message: {:?}",
                        e
                    )))
                    .await
                    .expect("Failed to send message");
                }
                return Err(());
            }
            Ok(())
        },
    )
    .await;

    // Block Pok'em from sending messages to this room
    bot.register_text_command(
        "block",
        None,
        Some("Block Pok'em from sending messages to this room".to_string()),
        |_, _, room| async move {
            if can_message_room(&room).await {
                // If we can't message the room we won't make any changes here
                if room.set_tag("dev.pokem.block".into(), TagInfo::default()).await.is_ok() {
                    room.send(RoomMessageEventContent::text_plain("Pok'em has been blocked from sending messages to this room.\nSend `.unblock` to allow messages again."))
                        .await
                        .expect("Failed to send message");
                } else {
                    room.send(RoomMessageEventContent::text_plain("ERROR: Failed to block myself."))
                        .await
                        .expect("Failed to send message");
                }
            }
            Ok(())
        },
    )
    .await;

    // Unblock Pok'em from sending messages to this room
    bot.register_text_command(
        "unblock",
        None,
        Some("Unblock Pok'em to allow notifications to this room".to_string()),
        |_, _, room| async move {
            if room.remove_tag("dev.pokem.block".into()).await.is_ok() {
                room.send(RoomMessageEventContent::text_plain(
                    "Pok'em has been unblocked from sending messages to this room.",
                ))
                .await
                .expect("Failed to send message");
            } else {
                room.send(RoomMessageEventContent::text_plain(
                    "ERROR: Failed to unblock myself.",
                ))
                .await
                .expect("Failed to send message");
            }
            Ok(())
        },
    )
    .await;

    // Register command to set variables
    bot.register_text_command(
        "set",
        Some("<block|password> <on|off|password>".to_string()),
        Some("Configure settings for Pok'em in this room".to_string()),
        set_command,
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
                    error!("Exiting daemon");
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

/// Get command prefix
fn get_command_prefix() -> String {
    GLOBAL_BOT
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .command_prefix()
}

/// Sets config options for the room
async fn set_command(_: matrix_sdk::ruma::OwnedUserId, msg: String, room: Room) -> Result<(), ()> {
    let mut room_config = get_room_config(&room).await;
    let command = msg.trim_start_matches(&get_command_prefix());
    let key = command.split_whitespace().nth(1).unwrap_or_default();
    let value = command.split_whitespace().nth(2).unwrap_or_default();
    error!("Setting room config: {} {}", key, value);

    let response = match key {
        "block" => {
            if value.is_empty() {
                format!(
                    "Block cannot be empty\n`{}set block [on|off]`",
                    get_command_prefix()
                )
            } else if value.to_lowercase() == "on" {
                room_config.block = true;
                "Blocking messages".to_string()
            } else if value.to_lowercase() == "off" {
                room_config.block = false;
                "Unblocking messages".to_string()
            } else {
                "Invalid value, use 'on' or 'off'".to_string()
            }
        }
        "password" | "pass" => {
            // Set a password necessary to send a message.
            // A password needs to be at the start of the text message
            if value.is_empty() {
                format!(
                    "Password cannot be empty\n`{}set password [off|password]`",
                    get_command_prefix()
                )
            } else if value.to_lowercase() == "on" {
                "Tried setting the password to 'on', that was probably an accident".to_string()
            } else if value.to_lowercase() == "off" {
                room_config.password = None;
                "Password removed".to_string()
            } else {
                room_config.password = Some(value.to_string());
                format!("Password set to {}", value).to_string()
            }
        }
        _ => {
            let block_status = if room_config.block { "on" } else { "off" };
            format!(
                "Usage:
`{}set [block|pass] <on|off|password>`
Current values:\n- block: {}{}",
                get_command_prefix(),
                block_status,
                if let Some(password) = room_config.password.clone() {
                    format!("\n- password: {}", password)
                } else {
                    "".to_string()
                }
            )
        }
    };
    set_room_config(&room, room_config).await;
    room.send(RoomMessageEventContent::text_markdown(&response))
        .await
        .expect("Failed to send message");
    Ok(())
}

#[derive(Debug, Default)]
struct RoomConfig {
    block: bool,
    password: Option<String>,
}

/// Write the Room config into the tags
async fn set_room_config(room: &Room, config: RoomConfig) {
    if config.block {
        room.set_tag("dev.pokem.block".into(), TagInfo::default())
            .await
            .unwrap();
    } else {
        room.remove_tag("dev.pokem.block".into()).await.unwrap();
    }
    // Grab the password from the option for ergonomics
    let password = config.password.clone().unwrap_or("".to_string());
    // Remove any existing password
    let mut placed = false;
    let tags = room.tags().await.unwrap_or_default();
    for (tag, _) in tags.unwrap_or_default() {
        if tag.to_string().starts_with("dev.pokem.pass.") {
            if config.password.is_some()
                && tag.to_string().trim_start_matches("dev.pokem.pass.") == password
            {
                // Already in place
                placed = true;
            } else {
                // If this tag doesn't match the new one, remove it
                room.remove_tag(tag).await.unwrap();
            }
        };
    }
    if config.password.is_some() && !placed {
        room.set_tag(
            format!("dev.pokem.pass.{}", password).into(),
            TagInfo::default(),
        )
        .await
        .unwrap();
    }
}

// Get all the current set room configs from the tags.
async fn get_room_config(room: &Room) -> RoomConfig {
    let mut config = RoomConfig::default();
    // the tags to only things that start with "dev.pokem."
    let tags = room.tags().await.unwrap_or_default();
    for (tag, _) in tags.unwrap_or_default() {
        if tag.to_string() == "dev.pokem.block" {
            config.block = true;
        } else if tag.to_string().starts_with("dev.pokem.pass.") {
            if config.password.is_some() {
                // We only want one password, this is a warning
                // It probably means we failed to remove a password on a password change
                error!(
                    "Multiple passwords set for room: {}",
                    room.room_id().as_str()
                );
                continue;
            }
            // Get the password
            config.password = Some(
                tag.to_string()
                    .trim_start_matches("dev.pokem.pass.")
                    .to_string(),
            );
        }
    }
    config
}
