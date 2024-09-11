# Pok'em

Send a Matrix message using the CLI or an HTTP PUT/POST request.
It's intended to support easy scripting to send yourself or a team notifications over Matrix.

This runs a Matrix bot account that pings you on Matrix when it's called.
It can be run standalone, or it can run as a daemon and listen for HTTP requests.

That's it.

If you want to use my default instance at [pokem.dev](https://pokem.dev), you don't need to install anything.

It is built using [headjack](https://github.com/arcuru/headjack), a Matrix bot framework in Rust.

If this seems familiar, that's because it's a clone of the core feature of [ntfy.sh](https://ntfy.sh), but only using Matrix.
I'd encourage you to check out that project to see if it better suits your needs.

## Getting Help

There is a public Matrix room available at [#pokem:jackson.dev](https://matrix.to/#/#pokem:jackson.dev)

## Usage

### Using the public server

I run an endpoint and associated bot account accessible at [pokem.dev](https://pokem.dev), with the bot running on my homeserver on [jackson.dev](https://jackson.dev).
`pokem.dev` is usually running the git main branch, instead of the latest release.
`pokem` will default to that instance if you have not configured the server settings.

You can run your own instance (using `pokem --daemon`), but here I'll describe using pokem.dev

1. On Matrix, create a room and invite the bot account, [@pokem:jackson.dev](https://matrix.to/#/@pokem:jackson.dev).
2. Grab the Matrix Room Alias or ID from the welcome message or from your client.
3. Run `curl --fail -d "Backup successful 😀" pokem.dev/<room id>`. Or use `pokem <room id> <message>`.
4. The `curl` or `pokem` commands will block until the message is sent, and will return an error if there is a problem.

`pokem`, like `ntfy`, listens to HTTP PUT/POST requests, so it's easy to send a message.
If you'd like more examples of how to send messages, just look at the [ntfy docs](https://docs.ntfy.sh/#step-2-send-a-message) and use the Matrix Room ID instead of the ntfy "topic".

If you use the `pokem` CLI, you can set a default room in the config file, and then you don't need to specify it in commands.
`pokem Backup Successful 😀` will be all you need.

The daemon also provides a webpage that will send messages for you, e.g. [pokem.dev](https://pokem.dev).
For ease of use, you can use URLs with the Room ID so that you can generate easy links.

Try it out! Send a message to https://pokem.dev/pokem-example:jackson.dev and you can see it in [#pokem-example:jackson.dev](https://matrix.to/#/#pokem-example:jackson.dev).

You can also use the unique room id: https://pokem.dev/!JYrjsPjErpFSDdpwpI:jackson.dev, or a URI-encoded room-alias (with the Matrix standard '#') https://pokem.dev/%23pokem-example:jackson.dev.
'#' is a special URL character, you need to URI Encode it (as "%23") or just remove it from your request, as we will support "pokem-example:jackson.dev" as a room name.

#### Limitations of [@pokem:jackson.dev](https://matrix.to/#/@pokem:jackson.dev)

1. You should not rely on it to have more than 1 9 of reliability.
2. There may be usage limits in the future.

### CLI Usage

The `pokem` tool comes with several convenience features to help with sending messages.

You can configure a default room, and setup shorthand room names, to make things easier to use.

Here are some example calls:

```bash
# These three commands are all equivalent
pokem !RoomID:jackson.dev Backup failed!
pokem --room !RoomID:jackson.dev Backup failed!
curl --fail -d "Backup failed!" pokem.dev/!RoomID:jackson.dev

pokem Backup failed! # Will send to your configured default room
pokem error Backup failed! # Will send to your configured room named "error"
pokem --room error Backup failed! # Same as above

# It also accepts stdin as the room message
echo "Backup failed!" | pokem error # Sends to the room named "error"
cat README.md | pokem # Send the contents of a file to the default room
```

See the [Setup](#setup) section for config options.

### Running A Private Bot Account

If you don't want to use [@pokem:jackson.dev](https://matrix.to/#/@pokem:jackson.dev), there are 2 ways to still use Pok'em.

You can either:

1. Host your own bot using `pokem --daemon`, and access just as described above.
2. Use the Pok'em CLI with the Bot login configured. The app will login to the Matrix bot account and send the ping locally.

Running Pok'em as a daemon has several advantages, but it is perfectly usable to just use local login.

1. The daemon will be much more responsive, since it takes a while to login and sync up on Matrix.
2. It will also be continuously available to respond to Room Invites and help request messages.
3. You can skip installing `pokem` locally, which is especially useful to be pinged from a CI job.

#### Docker Setup

The Pok'em daemon is available as a Docker image on [Docker Hub](https://hub.docker.com/r/arcuru/pokem).
Here's a Docker Compose example:

```yaml
services:
  pokem:
    image: arcuru/pokem:main # Set to your desired version
    volumes:
      # Mount your config file to /config.yaml
      - ./config.yaml:/config.yaml
      # Recommended: Persist the logged in session
      # You will need to set the state directory to this location in the config file
      # e.g.
      #   matrix:
      #     state_dir: /state
      - pokem-state:/state
    network_mode: host

volumes:
  # Persists the logged in session
  pokem-state:
```

## Install

`pokem` is only packaged on crates.io, so installing it needs to be done via `cargo install pokem` or from git.

For [Nix](https://nixos.org/) users, this repo contains a Nix flake. See the [setup section](#nix) for details on configuring.

## Setup

Using your own bot account will require setup, otherwise there is no setup required.

`pokem` the CLI tool runs down this list until it finds something to do:

1. `pokem --daemon` runs it as a daemon, listening for HTTP messages.
2. If there is no server or matrix login configured, it will send the request to the `pokem.dev` instance.
3. `pokem` with a server configured will send a PUT request to the server.
4. If there is a Matrix login configured, the CLI will attempt to login to Matrix itself.

Here is the config file skeleton.
It can be placed in $XDG_CONFIG_HOME/pokem/config.yaml or passed via `pokem --config ~/path/to/config.yaml`.

```yaml
# Optional, for creating room shorthands
# These can be used in place of the raw room IDs
# e.g. `pokem error The backup failed!` will send "The backup failed!" to the room named "error"
rooms:
  # Default is a special value, and will be used if no room is specified
  # e.g. `pokem The backup failed!` will send to the default room
  default: "!RoomID:jackson.dev"
  error: "!ErrorRoom:jackson.dev"
  discord: "!RoomBridgedToDiscord:jackson.dev"
  fullteam: "!RoomWithFullTeam:jackson.dev"
  # Messages that come with the "urgent" tag sent to "fullteam" will go to "fullteam-urgent", if
  # it exists, otherwise they will be sent to "fullteam" with a "@room" ping
  fullteam-urgent: "!RoomWithFullTeamUrgent:jackson.dev"

# Optional, define the server to send messages to
# If configured, `pokem` will first try to query this server to send the message
# Will use pokem.dev by default
server:
  url: https://pokem.dev
  # Optional, customize the port if necessary
  port: 80

# Optional, if you want to login to your own Matrix account
# You will need to create the bot account manually
matrix:
  homeserver_url: https://matrix.jackson.dev
  username: "pokem"
  # Optional, will ask on first run
  #password: ""
  # Optional, but necessary for use
  #allow_list: ".*"
  # Optional, the max size of the room to join
  #room_size_limit: 5
  # Optional, customize the state directory for the Matrix login data
  # Defaults to $XDG_STATE_HOME/pokem
  #state_dir:
  # Optional, customize the default format used for messages
  # Defaults to markdown, but can also be set to plain
  #format: markdown

# Optional, to define the bindings when running as a service
daemon:
  addr: "0.0.0.0"
  port: 80
```

## Authentication

You can configure an Authentication token from the Matrix side, so that poking a Matrix room would require knowing the token.

Sending `!pokem set auth pokempassword` to the Matrix bot will set the token to "pokempassword".

Once the token is set, the room will not be pinged unless the token is given in the HTTP headers, for example:

```bash
curl --fail pokem.dev/roomid -d "poke the room" -H "Authentication: pokempassword"
pokem --auth pokempassword --room roomid poke the room
```

If the token matches the message will be sent to the room, otherwise the request will fail.

The token can be seen by anyone in the room by sending `!pokem info`, and it can be removed with `!pokem set auth off`.

## Alternative Ideas

Here are some non-standard things you could do with this:

### Alert Everywhere

1. Run the bot account on a homeserver with bridges configured.
2. Have the bot account login to anywhere else with a bridge (Discord, Slack, IRC, iMessage, etc).
3. Use this to ping the Discord/Slack/IRC room using the bridge.

### Script Your Own Messages

1. Give `pokem` _your_ login info.
2. Send any message to a room of your choice as yourself. e.g. `pokem <room> I'm running late`

## Comparison with ntfy.sh

### Cons

1. Way fewer features. `pokem` only does text pings on Matrix.
2. Fewer integrations. `pokem` is limited to only Matrix and things bridged to Matrix.

### Pros

1. Secure room topics by default. Nobody else can subscribe to the messages even with the key.
2. You don't need a separate app, just use your existing Matrix client.
3. Support for authentication tokens, so that nobody else can send messages to your room.

## Nix

Development is being done using a [Nix flake](https://nixos.wiki/wiki/Flakes).
The easiest way to install pokem is to use nix flakes.

```bash
❯ nix run github:arcuru/pokem
```

The flake contains an [overlay](https://nixos.wiki/wiki/Overlays) to make it easier to import into your own flake config.
To use, add it to your inputs:

```nix
    inputs.pokem.url = "github:arcuru/pokem";
```

And then add the overlay `inputs.pokem.overlays.default` to your pkgs.

The flake also contains a home-manager module for installing the daemon as a service.
Import the module into your home-manager config and you can configure `pokem` all from within nix:

```nix
{inputs, ... }: {
  imports = [ inputs.pokem.homeManagerModules.default ];
  services.pokem = {
    enable = true;
    settings = {
        homeserver_url = "https://matrix.jackson.dev";
        username = "pokem";
        password = "hunter2";
        allow_list = "@me:matrix.org|@myfriend:matrix.org";
    };
  };
}
```
