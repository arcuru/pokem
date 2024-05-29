use headjack::Bot;
/// Common config options for pok'em
use lazy_static::lazy_static;

use serde::Deserialize;

use std::collections::HashMap;

use std::sync::Mutex;

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    /// Server URL
    pub url: String,
    /// Optional port
    pub port: Option<u16>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DaemonConfig {
    /// IP to bind on.
    /// Defaults to 0.0.0.0
    pub addr: Option<String>,
    /// Port to bind on.
    /// Will default to 80
    pub port: Option<u16>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MatrixConfig {
    /// Homeserver for pokem
    pub homeserver_url: String,
    /// Username for pokem
    pub username: String,
    /// Optionally specify the password, if not set it will be asked for on cmd line
    pub password: Option<String>,
    /// Allow list of which accounts we will respond to
    pub allow_list: Option<String>,
    /// Room size limit to respond to
    pub room_size_limit: Option<usize>,
    /// Set the state directory for pokem
    /// Defaults to $XDG_STATE_HOME/pokem
    pub state_dir: Option<String>,
    /// Set the command prefix.
    /// Defaults to "!pokem".
    pub command_prefix: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct Config {
    /// Configuration for logging in and messaging on Matrix
    pub matrix: Option<MatrixConfig>,

    /// Server config
    /// If this is setup, we will use this instead of logging in ourselves
    /// It expects the server config to point to a pokem daemon
    pub server: Option<ServerConfig>,

    /// Daemon config
    /// Configuration for running as a daemon
    pub daemon: Option<DaemonConfig>,

    /// Save different types of rooms
    /// Special value default will be used if no room is specified
    /// e.g. error/warning/info/default
    pub rooms: Option<HashMap<String, String>>,
}

lazy_static! {
    /// Holds the config for the bot
    pub static ref GLOBAL_CONFIG: Mutex<Option<Config>> = Mutex::new(None);
    /// Holds the bot
    pub static ref GLOBAL_BOT: Mutex<Option<Bot>> = Mutex::new(None);
}

/// Config settings in a single room
#[derive(Clone, Debug, Default)]
pub struct RoomConfig {
    pub block: bool,
    pub auth: Option<String>,
}
