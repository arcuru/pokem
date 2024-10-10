/// Run Pok'em as a daemon
use crate::config::*;
use crate::utils::*;

use anyhow::Context;
use clap::error::Result;
use emojis::Emoji;
use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;

use matrix_sdk::ruma::events::tag::TagInfo;
use matrix_sdk::Room;

use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::{debug, error};

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use http_body_util::BodyExt;
use http_body_util::Full;
use hyper::body::Bytes;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::StatusCode;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

#[derive(Debug, Clone, Deserialize)]
struct PokeRequest {
    topic: String,
    title: Option<String>,
    message: String,
    priority: Option<u8>,
    tags: Option<Vec<String>>,
}

impl PokeRequest {
    /// Try to deserialize the request from JSON, otherwise build it from headers and body.
    pub async fn from_request(request: Request<hyper::body::Incoming>) -> anyhow::Result<Self> {
        // Try JSON deserialization
        let headers = request.headers().clone();
        let uri = request.uri().clone();

        let body_bytes = request.collect().await?.to_bytes();
        let body_str =
            String::from_utf8(body_bytes.to_vec()).with_context(|| "error while decoding UTF-8")?;
        let Ok(poke_request) = serde_json::from_str::<PokeRequest>(&body_str) else {
            // Build from headers and body
            let query_params: HashMap<String, String> = uri
                .query()
                .map(|v| {
                    url::form_urlencoded::parse(v.as_bytes())
                        .map(|(a, b)| (a.to_lowercase(), b.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            return Ok(PokeRequest {
                // The uri without the leading / will be the room id
                topic: uri.path().trim_start_matches('/').to_string(),
                title: query_params.get("title").cloned().or_else(|| {
                    headers
                        .get("x-title")
                        .or_else(|| headers.get("title"))
                        .or_else(|| headers.get("ti"))
                        .or_else(|| headers.get("t"))
                        .and_then(|tags| tags.to_str().ok().map(String::from))
                }),
                message: query_params
                    .get("message")
                    .cloned()
                    .or_else(|| {
                        headers
                            .get("x-message")
                            .or_else(|| headers.get("message"))
                            .or_else(|| headers.get("m"))
                            .and_then(|msg| msg.to_str().ok().map(String::from))
                    })
                    .unwrap_or(body_str),
                priority: query_params
                    .get("priority")
                    .and_then(|p| p.parse().ok())
                    .or_else(|| {
                        headers
                            .get("x-priority")
                            .or_else(|| headers.get("priority"))
                            .or_else(|| headers.get("prio"))
                            .or_else(|| headers.get("p"))
                            .and_then(|priority_header| {
                                priority_header.to_str().ok().map(|header_str| {
                                    header_str.parse().unwrap_or_else(|_| {
                                        match &header_str.to_lowercase()[..] {
                                            "min" => 1,
                                            "low" => 2,
                                            "default" => 3,
                                            "high" => 4,
                                            "urgent" | "max" => 5,
                                            _ => 3,
                                        }
                                    })
                                })
                            })
                    }),
                tags: query_params
                    .get("tags")
                    .cloned()
                    .or_else(|| {
                        headers
                            .get("x-tags")
                            .or_else(|| headers.get("tags"))
                            .or_else(|| headers.get("tag"))
                            .or_else(|| headers.get("ta"))
                            .and_then(|tags| tags.to_str().ok().map(String::from))
                    })
                    .map(|tags_str| tags_str.split(',').map(String::from).collect()),
            });
        };
        Ok(poke_request)
    }
}

/// Run in daemon mode
/// This binds to a port and listens for incoming requests, and sends them to the Matrix room
pub async fn daemon(
    config: Option<DaemonConfig>,
    rooms: Option<HashMap<String, String>>,
) -> anyhow::Result<()> {
    let addr = {
        if let Some(daemon) = &config {
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
            let mut args = msg
                .trim_start_matches(&get_command_prefix())
                .split_whitespace();
            args.next(); // Ignore the "poke"
            let room_id = args.next().unwrap_or_default();
            let message = args.collect::<Vec<&str>>().join(" ");
            debug!("Room: {:?}, Message: {:?}", room_id, message);

            // Get a copy of the bot
            let bot = GLOBAL_BOT.lock().unwrap().as_ref().unwrap().clone();

            if let Err(e) = ping_room(
                &bot,
                room_id,
                &reqwest::header::HeaderMap::new(),
                &message,
                false,
            )
            .await
            {
                error!("Failed to send message: {:?}", e);
                if can_message_room(&room).await {
                    room.send(RoomMessageEventContent::text_plain(format!(
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
        Some("<block|auth> <on|off|token>".to_string()),
        Some("Configure settings for Pok'em in this room".to_string()),
        set_command,
    )
    .await;

    // Spawn a tokio task to continuously accept incoming connections
    let rooms = Arc::new(RwLock::new(rooms));
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
            let cloned_rooms = rooms.clone();
            tokio::task::spawn(async move {
                if let Err(err) = http1::Builder::new()
                    .serve_connection(io, service_fn(|req| daemon_poke(req, cloned_rooms.clone())))
                    .await
                {
                    eprintln!("Error serving connection: {:?}", err);
                }
            });
        }
    });

    // Run the bot and block
    // It never exits
    loop {
        if let Err(e) = bot.run().await {
            error!("Bot restarting after it exited with error: {e}");
        }
    }
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
        // TODO(2.0): Remove the pass/password allowances in 2.0, it's here for backwards compatibility
        "auth" | "authentication" | "password" | "pass" => {
            // Set an auth token necessary to send a message.
            if value.is_empty() {
                format!(
                    "Token cannot be empty\n`{}set auth [off|token]`",
                    get_command_prefix()
                )
            } else if value.to_lowercase() == "on" {
                "Tried setting the Auth Token to 'on', that was probably an accident".to_string()
            } else if value.to_lowercase() == "off" {
                room_config.auth = None;
                "Auth Token removed".to_string()
            } else {
                room_config.auth = Some(value.to_string());
                format!("Auth Token set to {}", value).to_string()
            }
        }
        _ => {
            let block_status = if room_config.block { "on" } else { "off" };
            format!(
                "Usage:
`{}set [block|auth] <on|off|token>`
Current values:\n- block: {}{}",
                get_command_prefix(),
                block_status,
                if let Some(token) = room_config.auth.clone() {
                    format!("\n- Authentication Token: {}", token)
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

/// Poke the room from an http request
async fn daemon_poke(
    request: Request<hyper::body::Incoming>,
    rooms: Arc<RwLock<Option<HashMap<String, String>>>>,
) -> anyhow::Result<Response<Full<Bytes>>> {
    let headers = request.headers().clone();
    let is_get = request.method() == hyper::Method::GET;
    let mut poke_request = PokeRequest::from_request(request).await?;

    // The room_id may be URI encoded
    let mut room_id = match urlencoding::decode(&poke_request.topic) {
        Ok(room) => room.to_string(),
        Err(_) => poke_request.topic,
    };

    let urgent = poke_request.priority.is_some_and(|p| p > 3);

    // Add title
    if let Some(title) = poke_request.title {
        poke_request.message = format!("**{title}**\n\n{}", poke_request.message);
    }

    // Add emojis
    if let Some(tags) = poke_request.tags {
        let emojis_vec: Vec<&'static Emoji> = tags
            .iter()
            .filter_map(|shortcode| emojis::get_by_shortcode(shortcode.as_str()))
            .collect();
        let emojis_str = emojis_vec
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<String>>()
            .join("");
        if !emojis_str.is_empty() {
            poke_request.message = format!("{emojis_str} {}", poke_request.message);
        }
    }

    // If the room is a room name in the config, we'll transform it to the room id.
    // If the message is urgent and <room_name>-urgent exists, it will got there, otherwise
    // we mention the entire @room.
    let mut mention_room = false;
    room_id = match &rooms.read().await.as_ref().and_then(|r| {
        if urgent {
            r.get(&format!("{}-urgent", room_id)).or_else(|| {
                // No urgent room found, pinging @room
                mention_room = true;
                r.get(&room_id)
            })
        } else {
            r.get(&room_id)
        }
    }) {
        Some(room_id) => room_id.to_string(),
        _ => {
            // No urgent room found, pinging @room
            if urgent {
                mention_room = true;
            }
            room_id
        }
    };

    // If it's a GET request, we'll serve a WebUI
    if is_get {
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

    // Get a copy of the bot
    let bot = GLOBAL_BOT.lock().unwrap().as_ref().unwrap().clone();

    if let Err(e) = ping_room(
        &bot,
        &room_id,
        &headers,
        &poke_request.message,
        mention_room,
    )
    .await
    {
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
