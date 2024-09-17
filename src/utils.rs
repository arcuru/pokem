/// Common utils for pok'em
use crate::config::*;
use headjack::*;

use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;

use matrix_sdk::ruma::events::tag::TagInfo;
use matrix_sdk::ruma::events::Mentions;
use matrix_sdk::{Room, RoomMemberships, RoomState};

use tracing::{error, info};

use hyper::HeaderMap;

/// Write the Room config into the tags
pub async fn set_room_config(room: &Room, config: RoomConfig) {
    if config.block {
        room.set_tag("dev.pokem.block".into(), TagInfo::default())
            .await
            .unwrap();
    } else {
        room.remove_tag("dev.pokem.block".into()).await.unwrap();
    }
    // Grab the auth token from the option for ergonomics
    let auth_token = config.auth.clone().unwrap_or("".to_string());
    // Remove any existing auth token
    let mut placed = false;
    let tags = room.tags().await.unwrap_or_default();
    for (tag, _) in tags.unwrap_or_default() {
        if tag.to_string().starts_with("dev.pokem.pass.") {
            // Old format, remove it, we'll be replacing with the new value
            room.remove_tag(tag).await.unwrap();
        } else if tag.to_string().starts_with("dev.pokem.auth.") {
            if config.auth.is_some()
                && tag.to_string().trim_start_matches("dev.pokem.auth.") == auth_token
            {
                // Already in place
                placed = true;
            } else {
                // If this tag doesn't match the new one, remove it
                room.remove_tag(tag).await.unwrap();
            }
        };
    }
    if config.auth.is_some() && !placed {
        room.set_tag(
            format!("dev.pokem.auth.{}", auth_token).into(),
            TagInfo::default(),
        )
        .await
        .unwrap();
    }
}

// Get all the current set room configs from the tags.
pub async fn get_room_config(room: &Room) -> RoomConfig {
    let mut config = RoomConfig::default();
    let tags = room.tags().await.unwrap_or_default();
    let mut should_update = false;
    for (tag, _) in tags.unwrap_or_default() {
        if tag.to_string() == "dev.pokem.block" {
            config.block = true;
        } else if tag.to_string().starts_with("dev.pokem.auth.") {
            if config.auth.is_some() {
                // We only want one auth token, this is a warning
                // It probably means we failed to remove a token on a change
                error!(
                    "Multiple Auth Tokens set for room: {}",
                    room.room_id().as_str()
                );
                continue;
            }
            // Get the auth token
            config.auth = Some(
                tag.to_string()
                    .trim_start_matches("dev.pokem.auth.")
                    .to_string(),
            );
        } else if tag.to_string().starts_with("dev.pokem.pass.") {
            // TODO(2.0): Remove this in 2.0
            // Old format, support for now
            // It will be removed immediately and replaced
            should_update = true;
            if config.auth.is_some() {
                // We only want one password, this is a warning
                // It probably means we failed to remove a password on a password change
                error!(
                    "Multiple Auth Tokens set for room: {}",
                    room.room_id().as_str()
                );
                continue;
            }
            // Get the auth token
            config.auth = Some(
                tag.to_string()
                    .trim_start_matches("dev.pokem.pass.")
                    .to_string(),
            );
        }
    }
    // Update the settings if there are old formatted auth tokens
    if should_update {
        set_room_config(room, config.clone()).await;
    }
    config
}

/// Get command prefix
pub fn get_command_prefix() -> String {
    GLOBAL_BOT
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .command_prefix()
}

/// Check if we can message the room
pub async fn can_message_room(room: &Room) -> bool {
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
pub async fn send_help(room: &Room) {
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
        let config = get_room_config(room).await;
        if let Some(pass) = config.auth {
            room.send(RoomMessageEventContent::text_plain(format!(
                "This Room's Authentication token is: {}",
                pass
            )))
            .await
            .expect("Failed to send message");
        }
    }
}

/// Send a message to a room.
pub async fn ping_room(
    bot: &Bot,
    room_id: &str,
    headers: &HeaderMap,
    message: &str,
    mention_room: bool,
) -> anyhow::Result<()> {
    let r = get_room_from_name(bot, room_id).await;
    if r.is_none() {
        error!("Failed to find room with name: {}", room_id);
        return Err(anyhow::anyhow!(
            "Failed to find room with name: {}",
            room_id
        ));
    }
    let r = r.unwrap();

    // If we're in an invited state, we need to wait for the invite to be accepted
    let mut delay = 2;
    while r.state() == RoomState::Invited {
        if delay > 60 {
            return Err(anyhow::anyhow!("Failed to join room"));
        }
        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        delay *= 2;
    }

    let mut msg: String = message.to_string();

    let room_config = get_room_config(&r).await;

    // Validate the authentication token and remove it from the message
    if let Ok(cleaned_msg) = validate_authentication(room_config, headers, &msg) {
        msg = cleaned_msg;
    } else {
        return Err(anyhow::anyhow!("Incorrect Authentication Token"));
    }

    // Get the message formatting
    let mut msg = format_message(headers, &msg);
    if mention_room {
        msg = msg.add_mentions(Mentions::with_room_mention());
    }

    if can_message_room(&r).await {
        if let Err(e) = r.send(msg).await {
            return Err(anyhow::anyhow!("Failed to send message: {}", e));
        }
    } else {
        error!("Failed to send message");
    }
    Ok(())
}

/// Get the appropriate message formatting.
fn format_message(headers: &HeaderMap, msg: &str) -> RoomMessageEventContent {
    // Get the default format from the config
    let mut format = if let Some(default_format) = GLOBAL_CONFIG
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .matrix
        .as_ref()
        .unwrap()
        .format
        .clone()
    {
        default_format
    } else {
        "markdown".to_string()
    };
    if let Some(header_format) = headers.get("format") {
        format = header_format.to_str().unwrap_or_default().to_string();
    };
    match format.to_lowercase().as_str() {
        "markdown" => RoomMessageEventContent::text_markdown(msg),
        "plain" => RoomMessageEventContent::text_plain(msg),
        _ => {
            error!("Unknown format: {}", format);
            RoomMessageEventContent::text_markdown(msg)
        }
    }
}

/// Translate a provided room name into an actual Room struct.
/// This looks up by either the Room Internal ID or the Room Alias.
/// Any alias, main or alt, will be checked.
pub async fn get_room_from_name(bot: &Bot, name: &str) -> Option<Room> {
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

/// Validate the authentication token
///
/// Returns the message with the authentication token removed
pub fn validate_authentication(
    room_config: RoomConfig,
    headers: &HeaderMap,
    msg: &str,
) -> anyhow::Result<String> {
    if room_config.auth.is_some() {
        // Check if the authentication token is in the headers
        let token = {
            // Allow both "authentication" and "auth"
            if let Some(auth) = headers.get("authentication") {
                auth.to_str().unwrap_or_default()
            } else if let Some(auth) = headers.get("auth") {
                auth.to_str().unwrap_or_default()
            } else {
                ""
            }
        };
        if token == room_config.auth.clone().unwrap() {
            return Ok(msg.to_string());
        }

        // Allow the authentication token to be the first word in the message

        // Check if the message starts with the password
        if !msg.starts_with(&room_config.auth.clone().unwrap()) {
            return Err(anyhow::anyhow!("Incorrect Authentication Token"));
        }
        // Remove the password and any leading whitespace
        Ok(msg
            .trim_start_matches(&room_config.auth.unwrap())
            .trim_start()
            .to_string())
    } else {
        Ok(msg.to_string())
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

/// Login as a bot
pub async fn connect(config: MatrixConfig) -> anyhow::Result<Bot> {
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
            room.send(RoomMessageEventContent::text_markdown(
                "Welcome to Pok'em!\n\nSend `!pokem help` to see available commands.",
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
