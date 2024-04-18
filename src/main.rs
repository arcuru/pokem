use clap::Parser;
use headjack::*;
use lazy_static::lazy_static;
use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;

use serde::Deserialize;

use std::{fs::File, io::Read, path::PathBuf, sync::Mutex};
use tracing::{error, info};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct PokemArgs {
    /// path to config file
    #[arg(short, long)]
    config: PathBuf,

    /// Room info
    #[arg(short, long)]
    room: String,

    /// Message to send
    /// The rest of the arguments
    #[arg()]
    message: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    /// Homeserver for pokem
    homeserver_url: String,
    /// Username for pokem
    username: String,
    /// Optionally specify the password, if not set it will be asked for on cmd line
    password: Option<String>,
    /// Allow list of which accounts we will respond to
    allow_list: Option<String>,
    /// Room size limit to respond to
    //room_size_limit: Option<u64>,
    /// Set the state directory for pokem
    /// Defaults to $XDG_STATE_HOME/pokem
    state_dir: Option<String>,
}

lazy_static! {
    /// Holds the config for the bot
    static ref GLOBAL_CONFIG: Mutex<Option<Config>> = Mutex::new(None);
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    // Read in the config file
    let args = PokemArgs::parse();
    let mut file = File::open(args.config)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    let config: Config = serde_yaml::from_str(&contents)?;
    *GLOBAL_CONFIG.lock().unwrap() = Some(config.clone());

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
    bot.join_rooms();

    // Syncs to the current state
    if let Err(e) = bot.sync().await {
        error!("Error syncing: {e}");
    }

    info!("The client is ready! Listening to new messages…");

    // Lookup the room to see we're in it
    let r = bot
        .client()
        .get_room(&matrix_sdk::ruma::RoomId::parse(args.room).unwrap())
        .expect("Room not found");

    r.send(RoomMessageEventContent::text_plain(args.message.join(" ")))
        .await
        .expect("Failed to send message");

    Ok(())
}

