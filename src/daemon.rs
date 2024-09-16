/// Run Pok'em as a daemon
use crate::config::*;
use crate::utils::*;

use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;

use matrix_sdk::ruma::events::tag::TagInfo;
use matrix_sdk::Room;

use tokio::sync::RwLock;
use tracing::error;

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
            error!("Room: {:?}, Message: {:?}", room_id, message);

            // Get a copy of the bot
            let bot = GLOBAL_BOT.lock().unwrap().as_ref().unwrap().clone();

            if let Err(e) =
                ping_room(&bot, room_id, &reqwest::header::HeaderMap::new(), &message).await
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
) -> Result<Response<Full<Bytes>>, hyper::Error> {
    // The uri without the leading / will be the room id
    let room_id = request.uri().path().trim_start_matches('/').to_string();
    // The room_id may be URI encoded
    let mut room_id = match urlencoding::decode(&room_id) {
        Ok(room) => room.to_string(),
        Err(_) => room_id,
    };

    // If the room is a room name in the config, we'll transform it to the room id
    room_id = if let Some(room_id) = &rooms.read().await.as_ref().and_then(|r| r.get(&room_id)) {
        room_id.to_string()
    } else {
        room_id
    };

    let headers = request.headers().clone();

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

    if let Err(e) = ping_room(&bot, &room_id, &headers, &message).await {
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
