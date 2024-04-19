# Pok'em

This is a (partial) [ntfy.sh](https://ntfy.sh) clone using Matrix.

This runs a Matrix bot account that pings you on Matrix when it's called.
It can be run standalone, or it can run as a daemon and listen for HTTP requests.

That's it.

If you want to use my default instance at https://pokem.jackson.dev, you don't need to install anything.

Status: Alpha

It is built using [headjack](https://github.com/arcuru/headjack), a Matrix bot framework in Rust.

## Usage

I run an endpoint and associated bot account at https://pokem.jackson.dev.
`pokem` will default to that instance if you have not configured the server settings.

You can run your own instance (using `pokem --daemon`), but here I'll describe using pokem.jackson.dev

1. On Matrix, create a room and invite the bot account, [@pokem:jackson.dev](https://matrix.to/#/@pokem:jackson.dev).
2. Grab the Matrix Room ID from the welcome message or from your client.
3. Run `curl --fail -d "Backup successful 😀" pokem.jackson.dev/<room id>`. Or use `pokem <room id> <message>`.
4. The `curl` or `pokem` commands will block until the message is sent, and will return an error if there is a problem.

`pokem`, like `ntfy`, listens to HTTP PUT/POST requests, so it's easy to send a message.
If you'd like more examples, just look at the [ntfy docs](https://docs.ntfy.sh/#step-2-send-a-message) and use the Matrix Room ID instead of the ntfy "topic".

If you use the `pokem` CLI, you can set a `default_room` in the config file, and then you don't need to specify it in commands.
`pokem Backup Successful 😀` will be all you need.

### Limitations

1. The pokem.jackson.dev instance is configured to never send messages to rooms larger than 5 people to avoid spam.
2. You should not rely on it to have more than 1 9 of reliability.
3. There may be usage limits in the future.

## Running A Private Bot Account

If you don't want to use [@pokem:jackson.dev](https://matrix.to/#/@pokem:jackson.dev), there are 2 ways to still use Pok'em.

You can either:

1. Host your own bot using `pokem --daemon`, and access just as described above.
2. Use the Pok'em CLI with the Bot login configured. The app will login to the Matrix bot account and send the ping locally.

Running Pok'em as a daemon has several advantages, but it is perfectly usable to just not bother.

1. The daemon will be much more responsive, since it takes a while to login and sync up on Matrix.
2. It will also be continuously available to respond to Room Invites and help request messages.

## Install

`pokem` is only packaged on crates.io, but it's recommended that you run from git HEAD for now.

For [Nix](https://nixos.org/) users, this repo contains a Nix flake. See the [setup section](#nix) for details on configuring.

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

1. Secure room topics by default. Nobody else can subscribe to the messages even with the key (although they could _send_ them).
2. You don't need a separate app, just use your existing Matrix client.

## Setup

Using your own bot account will require setup, otherwise there is no setup required.

`pokem` the CLI tool runs down this list until it finds something to do:

1. `pokem --daemon` runs it as a daemon, listening for HTTP messages.
2. If there is no server or matrix login configured, it will send the request to the `pokem.jackson.dev` instance.
3. `pokem` with a server configured will send a PUT request to the server. On a failure, it will fallback to trying with a a Matrix login.
4. If there is a Matrix login configured, the CLI will attempt to login to Matrix itself.

Here is the config file skeleton.
It can be placed in $XDG_CONFIG_HOME/pokem/config.yaml or passed via `pokem --config ~/path/to/config.yaml`.

```yaml
# Optional, for setting a default room
# When sending messages as a client, it will send to this room if none is given
default_room: "!RoomID:jackson.dev"

# Optional, will use pokem.jackson.dev by default
server:
  url: https://pokem.jackson.dev
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
  # Optional, the max size of the room to message
  #room_size_limit: 5
  # Optional, customize the state directory for the Matrix login data
  # Defaults to $XDG_STATE_HOME/pokem
  #state_dir:

# Optional, to define the bindings when running as a service
daemon:
  addr: "0.0.0.0"
  port: 80
```

### Nix

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
